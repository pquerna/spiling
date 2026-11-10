# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Frozen native reinforcement pairs, per-input qualification and complete costs.

No road generation or mutation. Responses are finite cut assumed-law elasticity,
not whole-part strength, buckling or a universal infill recommendation.
"""
from __future__ import annotations

import argparse
from dataclasses import asdict
import json
import math
from pathlib import Path
import shutil

import numpy as np
import shapely

from campaign import write
from cathedral import CASES, fixture
from geometry import footprint_slabs
from model import Material, digest, file_hash
from reference import JUDGE as REFERENCE_JUDGE
from quadratic import JUDGE as QUADRATIC_JUDGE
from vault_search import native_rows


def axial_reference(data, bounds, material):
    """Exact free axial response only when the actual cut is a Z extrusion."""
    from remote.coupon import interior_roads

    box = np.asarray(bounds, dtype=float)
    _, levels, domains, _ = footprint_slabs(interior_roads(data))
    if levels[0] > box[0, 2] + 1e-9 or levels[-1] < box[1, 2] - 1e-9:
        raise ValueError("axial reference lacks complete physical intervals")
    crop = shapely.box(*box[:, :2].reshape(-1))
    active = [
        shapely.intersection(domain, crop, grid_size=1e-9)
        for k, domain in enumerate(domains)
        if levels[k+1] > box[0, 2] + 1e-9 and levels[k] < box[1, 2] - 1e-9
    ]
    if not active or any(not domain.equals(active[0]) for domain in active[1:]):
        return dict(admitted=False, reason="changing_material_section")
    area = active[0].area
    if area <= 0:
        raise ValueError("axial reference has no positive material")
    return dict(
        admitted=True,
        area_mm2=area,
        stiffness_n_mm=material.young_mpa * material.z_ratio * area / (box[1, 2]-box[0, 2]),
        scope="exact_assumed_law_free_axial_extrusion_not_strength",
    )


def selected(captures, fraction):
    result = {}
    for capture in captures:
        capture_spec, rows = native_rows(capture)
        for row in rows:
            c = row["candidate"]
            if (row["status"] != "ok" or c["pattern"] != "web_roof"
                    or c["fraction"] != fraction or c.get("web_opening")
                    or c.get("pier_rib_depth_mm")):
                continue
            name = row["specimen"]
            if name in result:
                raise ValueError("duplicate specimen/control; select one native capture")
            folder = capture / row["native"]
            data = json.loads((folder / "roads.json").read_text())
            if data["process_edits"]:
                raise ValueError("uniform reinforcement study excludes regional edits")
            result[name] = (row, folder, data, capture_spec["identity"])
    return result


def pack(args):
    if (len(set(args.max_xy)) != 2 or not all(math.isfinite(h) and h > 0 for h in args.max_xy)
            or not math.isfinite(args.max_z) or args.max_z <= 0
            or not math.isfinite(args.shift_mm) or not math.isfinite(args.half_width)
            or not 0 < args.control_fraction < 1 or not 0 < args.candidate_fraction < 1
            or not 0 < args.seconds <= 86400):
        raise ValueError("finite positive frozen study parameters required")
    controls = selected(args.control, args.control_fraction)
    candidates = selected(args.candidate, args.candidate_fraction)
    args.output.mkdir(parents=True, exist_ok=False)
    material = Material(z_ratio=.25)
    judge_files = REFERENCE_JUDGE if args.backend == "reference" else QUADRATIC_JUDGE
    judge = {n: file_hash(args.context / n) for n in judge_files}
    jobs, costs, analytical = [], [], []
    for name in args.specimens:
        if name not in controls or name not in candidates:
            raise ValueError(f"missing complete native pair: {name}")
        control, candidate = controls[name], candidates[name]
        if control[0]["source_sha256"] != candidate[0]["source_sha256"]:
            raise ValueError("paired source bytes differ")
        if control[2]["size_mm"] != candidate[2]["size_mm"]:
            raise ValueError("paired specimen dimensions differ")
        settings = [{k: v for k, v in item[0]["candidate"].items() if k != "fraction"}
                    for item in (control, candidate)]
        if settings[0] != settings[1]:
            raise ValueError("paired construction settings differ beyond density")
        if control[3]["intent"] != candidate[3]["intent"]:
            raise ValueError("paired base recipes differ")
        bounds = fixture(candidate[2], args.half_width)
        # A shifted crop is a separate physical cohort, never pooled silently.
        width = next(r["width"] for r in candidate[2]["roads"] if r["kind"] == 5)
        if abs(args.shift_mm) > args.half_width - 3.5 * width:
            raise ValueError("shift clips the full automatic flange envelope")
        bounds = [[v + args.shift_mm if a < 2 else v for a, v in enumerate(p)] for p in bounds]
        for variant, (row, folder, data, identity) in (("control", control), ("candidate", candidate)):
            costs.append(dict(
                specimen=name, variant=variant, source_sha256=row["source_sha256"],
                roads_sha256=row["roads_sha256"], replay_sha256=row["replay_sha256"],
                native_capture_id=row["id"], binaries=data["bin_sha256"],
                engine_build=data["engine_build"], candidate=row["candidate"],
                base_intent_sha256=identity["intent"],
                source_independent_reopen=data.get("source_independent_reopen"),
                volume_mm3=data["report"]["deposited_volume_mm3"],
                motion_s=data["motion_seconds_lower_bound"],
            ))
            analytical.append(dict(specimen=name, variant=variant, **axial_reference(data, bounds, material)))
            for max_xy in args.max_xy:
                for case in CASES:
                    scenario = dict(
                        element=args.element, case=case, material=asdict(material), bounds=bounds,
                        mesh_method=args.mesh_method, seconds=args.seconds,
                        refinement=dict(divisions=1, max_xy=max_xy, max_z=args.max_z),
                    )
                    target = dict(
                        schema=1, kind="element_reference", judge={n: file_hash(args.context/n) for n in REFERENCE_JUDGE},
                        kind_source="actual_roads", scope="interior", specimen=name,
                        **{k: row[k] for k in ("candidate", "source_sha256", "roads_sha256", "replay_sha256")},
                        binaries=data["bin_sha256"], scenarios=[scenario],
                    )
                    if args.backend == "reference":
                        spec = target
                        key = digest(spec)
                        dest = args.output/key
                        dest.mkdir()
                    else:
                        if args.element != "C3D10":
                            raise ValueError("in-process quadratic study requires C3D10")
                        temporary = args.output / f"pending-{len(jobs)}"
                        temporary.mkdir()
                        write(temporary / "reference-case.json", target)
                        spec = dict(schema=1, kind="quadratic_reference", judge=judge, role="refinement", solver="p_amg", specimen=name,
                                    reference_case_sha256=file_hash(temporary/"reference-case.json"))
                        key = digest(spec)
                        dest = args.output/key
                        temporary.rename(dest)
                    write(dest/"case.json", spec)
                    shutil.copyfile(folder/"roads.json", dest/"roads.json")
                    jobs.append(dict(id=key, specimen=name, variant=variant, max_xy=max_xy, case=case, scenario=scenario))
    write(args.output/"jobs.json", dict(
        schema=1, jobs=jobs, judge=judge, costs=costs, analytical=analytical,
        backend=args.backend, specimens=args.specimens, max_xy=args.max_xy,
        shift_mm=args.shift_mm, scope="finite_cut_weak_z_elasticity_not_whole_part_strength",
    ))


def read_row(inputs, outputs, job, backend, expected_image):
    key = job["id"]
    spec = json.loads((inputs/key/"case.json").read_text())
    if digest(spec) != key:
        raise ValueError("changed frozen input")
    folder = outputs/key
    if not (folder/"launch.json").exists() or not (folder/"trials.jsonl").exists():
        return dict(status="unknown", reason="missing_result", **job)
    launch = json.loads((folder/"launch.json").read_text())
    if not launch.get("removed") or launch["backend"] != backend or launch["image"]["id"] != expected_image:
        raise ValueError("changed image/backend or missing container cleanup")
    entries = [json.loads(s) for s in (folder/"trials.jsonl").read_text().splitlines()]
    if len(entries) != 1:
        raise ValueError("incomplete/duplicated response")
    row = entries[0]
    target = spec
    if backend != "reference":
        reference = inputs/key/"reference-case.json"
        if file_hash(reference) != spec["reference_case_sha256"]:
            raise ValueError("changed reference geometry/fixture")
        target = json.loads(reference.read_text())
    if file_hash(inputs/key/"roads.json") != target["roads_sha256"]:
        raise ValueError("changed native road bytes")
    scenario = target["scenarios"][0]
    if (row["input_sha256"] != file_hash(inputs/key/"case.json") or row["judge"] != spec["judge"]
            or row["scenario"] != scenario or scenario != job["scenario"]):
        raise ValueError("response does not bind exact scenario/judge")
    if row["status"] != "ok" or launch.get("failure") or launch.get("docker_exit") != 0:
        return dict(status="unknown", reason=row.get("error", launch.get("failure", "failed solve")), **job)
    if not row["components"]:
        return dict(status="unknown", reason="missing_spanning_component", **job)
    for component in row["components"]:
        fields = (("force_balance", 1e-5), ("energy_error", 1e-5), ("volume_relative_error", 1e-5)) if backend == "reference" else (("relative_residual", 2e-8), ("force_balance", 1e-6), ("energy_error", 1e-6))
        if any(not math.isfinite(component[k]) or not 0 <= component[k] <= limit for k, limit in fields):
            raise ValueError("failed physical numerical gate")
    value = row["stiffness_n_mm"]
    if not math.isfinite(value) or value <= 0:
        raise ValueError("invalid response")
    return dict(**job, status="ok", stiffness_n_mm=value, mesh=row.get("mesh"), components=row["components"], image=expected_image)


def collect(args):
    panel = json.loads((args.inputs/"jobs.json").read_text())
    image = json.loads(args.image.read_text())["id"]
    rows = [read_row(args.inputs, args.results, j, panel["backend"], image) for j in panel["jobs"]]
    if len({j["id"] for j in panel["jobs"]}) != len(panel["jobs"]):
        raise ValueError("duplicated expected job")
    checks, comparisons = [], []
    coarse, fine = sorted(panel["max_xy"])[-2:][::-1]
    if len(set(panel["max_xy"])) != 2:
        raise ValueError("declare exactly two refinement levels")
    for name in panel["specimens"]:
        response = {}
        for variant in ("control", "candidate"):
            response[variant] = {}
            for case in CASES:
                pair = [next(r for r in rows if (r["specimen"], r["variant"], r["case"], r["max_xy"]) == (name, variant, case, h)) for h in (coarse, fine)]
                passed = all(r["status"] == "ok" for r in pair)
                change = abs(pair[1]["stiffness_n_mm"]/pair[0]["stiffness_n_mm"]-1) if passed else None
                checks.append(dict(specimen=name, variant=variant, case=case, relative_change=change, passed=passed and change <= .05))
                if passed:
                    response[variant][case] = pair[1]["stiffness_n_mm"]
            exact = next(r for r in panel["analytical"] if r["specimen"] == name and r["variant"] == variant)
            if exact["admitted"]:
                k = response[variant].get("compression_z")
                error = abs(k/exact["stiffness_n_mm"]-1) if k is not None else None
                checks.append(dict(specimen=name, variant=variant, case="exact_extrusion_axial", relative_error=error, passed=error is not None and error <= .005))
        complete = all(len(v) == len(CASES) for v in response.values())
        comparisons.append(dict(specimen=name, response=response, provisional=True,
                                ratios={case: response["candidate"][case]/response["control"][case] for case in CASES} if complete else None))
    passed = all(c["passed"] for c in checks) and all(r["status"] == "ok" for r in rows)
    write(args.output, dict(scope=panel["scope"], numerical_gate_passed=passed, checks=checks,
                            comparisons=comparisons, costs=panel["costs"], rows=rows,
                            independent_formulation_qualified=False))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="action", required=True)
    q = sub.add_parser("pack")
    q.add_argument("--control", type=Path, nargs="+", required=True)
    q.add_argument("--candidate", type=Path, nargs="+", required=True)
    q.add_argument("--control-fraction", type=float, default=.05)
    q.add_argument("--candidate-fraction", type=float, default=.05)
    q.add_argument("--context", type=Path, required=True)
    q.add_argument("--output", type=Path, required=True)
    q.add_argument("--specimens", nargs="+", default=["cube48", "beam80", "panel48", "tube32"])
    q.add_argument("--max-xy", type=float, nargs=2, default=[2., 1.])
    q.add_argument("--max-z", type=float, default=.4)
    q.add_argument("--half-width", type=float, default=2.25)
    q.add_argument("--shift-mm", type=float, default=0.)
    q.add_argument("--element", choices=["C3D10", "C3D15"], default="C3D10")
    q.add_argument("--mesh-method", choices=["prisms", "tetgen"], default="tetgen")
    q.add_argument("--backend", choices=["reference", "quadratic"], default="reference")
    q.add_argument("--seconds", type=int, default=600)
    q = sub.add_parser("collect")
    for name in ("inputs", "results", "image", "output"):
        q.add_argument("--"+name, type=Path, required=True)
    args = p.parse_args()
    (pack if args.action == "pack" else collect)(args)


if __name__ == "__main__":
    main()
