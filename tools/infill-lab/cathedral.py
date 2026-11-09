# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Native window/pier ablations and finite web-bay mechanics, not strength.

Capture with campaign.py using the emitted candidates. Pack only after complete
saved-return capture; evaluate with the frozen shared quadratic mechanics entrypoint
in a frozen context. Each refinement is a separate explicit judge cohort.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict
import json
import math
from pathlib import Path
import shutil

from model import Candidate, Material, digest, file_hash
from vault_mechanics import JUDGE
from vault_search import frontier, geometric_mean, native_rows, write


CASES = ("compression_x", "compression_z", "cantilever_z_y")


def seeds():
    base = dict(pattern="web_roof", fraction=0.05)
    opening = (2., 10., 4., 4., 1.8)
    for name, windows, ribs in (
        ("flat", False, False), ("windows", True, False),
        ("pier_ribs", False, True), ("windows_and_ribs", True, True),
    ):
        yield dict(name=name, parent="flat" if windows or ribs else None,
                   hypothesis="panel material removal and independently charged pier stiffening",
                   candidate=Candidate(**base, web_opening=opening if windows else None,
                                       pier_rib_depth_mm=0.9 if ribs else None).data())


def fixture(data):
    # Freeze from the unchanged control's actual first sparse layer. No proposed
    # geometry or candidate-dependent crop; include two intersection piers, the
    # complete 2..12 mm window and real material above and below it.
    roads = [r for r in data["roads"] if r["kind"] == 5 and isinstance(r["owner"], int)]
    if not roads:
        raise ValueError("baseline has no actual sparse web")
    z = min(r["upper"] for r in roads)
    first = [r for r in roads if r["upper"] == z]
    roots = []
    for fixed in range(2):
        roots.append(sorted({r["start"][fixed] for r in first
                             if r["start"][fixed] == r["end"][fixed]}))
    if min(map(len, roots)) < 2:
        raise ValueError("baseline does not contain a complete two-pier web bay")
    x0, x1 = roots[0][:2]
    y = roots[1][len(roots[1]) // 2]
    width = first[0]["width"]
    return [[x0 - 3*width, y - 3*width, 1.2],
            [x1 + 3*width, y + 3*width, 14.]]


def pack(capture, context, output, divisions, solver="p_amg", cap_segments=32):
    spec, rows = native_rows(capture)
    admitted = [r for r in rows if r["status"] == "ok"]
    controls = [r for r in admitted if r["candidate"] == next(seeds())["candidate"]]
    if len(controls) != 1 or len({(r["specimen"], r["source_sha256"]) for r in admitted}) != 1:
        raise ValueError("one unchanged source and one flat control required")
    control = json.loads((capture / controls[0]["native"] / "roads.json").read_text())
    bounds = fixture(control)
    judge = {name: file_hash(context / name) for name in JUDGE}
    output.mkdir(parents=True, exist_ok=False)
    jobs = []
    for row in admitted:
        folder = capture / row["native"]
        data = json.loads((folder / "roads.json").read_text())
        opening = row["candidate"].get("web_opening")
        if opening is not None and not (bounds[0][2] < opening[0] and opening[0]+opening[1] < bounds[1][2]):
            raise ValueError("web-bay crop must retain the complete window and cap")
        manifest = dict(schema=1, kind="cathedral_web_bay", judge=judge,
                        specimen=row["specimen"], candidate=row["candidate"],
                        source_sha256=row["source_sha256"], roads_sha256=file_hash(folder / "roads.json"),
                        binaries=spec["identity"]["binaries"], native_capture_id=row["id"],
                        scenario=dict(cases=list(CASES), bounds=bounds, solver=solver,
                            material=asdict(Material(z_ratio=0.25)),
                            refinement=dict(max_xy=2., max_z=0.4, divisions=divisions, cap_segments=cap_segments)))
        key = digest(manifest)
        dest = output / key
        dest.mkdir()
        write(dest / "case.json", manifest)
        shutil.copyfile(folder / "roads.json", dest / "roads.json")
        jobs.append(dict(id=key, candidate=row["candidate"]))
    write(output / "jobs.json", dict(schema=1, jobs=jobs))
    write(output / "cohort.json", dict(native_identity=spec["identity"], bounds=bounds,
                                       cases=CASES, divisions=divisions, judge=judge, solver=solver,
                                       scope="finite_cut_web_bay_assumed_weak_z_elasticity_not_whole_part_strength"))


def collect(inputs, results, image, output):
    cohort = json.loads((inputs / "cohort.json").read_text())
    jobs = json.loads((inputs / "jobs.json").read_text())["jobs"]
    if len({j["id"] for j in jobs}) != len(jobs):
        raise ValueError("duplicate web-bay job")
    expected_image = json.loads(image.read_text())["id"]
    names = {digest(p["candidate"]): p for p in seeds()}
    rows = []
    expected_scenario = None
    for job in jobs:
        key = job["id"]
        case_path = inputs / key / "case.json"
        case = json.loads(case_path.read_text())
        if (digest(case) != key or case["judge"] != cohort["judge"]
                or case["kind"] != "cathedral_web_bay"
                or case["candidate"] != job["candidate"]
                or case["scenario"]["bounds"] != cohort["bounds"]
                or case["scenario"]["cases"] != list(CASES)
                or case["scenario"]["refinement"]["divisions"] != cohort["divisions"]):
            raise ValueError("changed web-bay input or judge")
        if expected_scenario is not None and case["scenario"] != expected_scenario:
            raise ValueError("cannot pool changed physical or solver scenarios")
        expected_scenario = case["scenario"]
        folder = results / key
        launch = json.loads((folder / "launch.json").read_text())
        dispatch = json.loads((folder / "dispatch.json").read_text())
        if dispatch["job"] != key:
            raise ValueError("different dispatched job")
        if not launch["removed"] or launch["image"]["id"] != expected_image:
            raise ValueError("unremoved container or different evaluator image")
        proposal = names.get(digest(job["candidate"]), {})
        lines = (folder / "trials.jsonl").read_text().splitlines() if (folder / "trials.jsonl").exists() else []
        if len(lines) > 1:
            raise ValueError("duplicate web-bay result")
        row = json.loads(lines[0]) if lines else dict(status="failed", error="no complete numerical result")
        if lines and any(row[k] != case[k] for k in ("judge", "scenario", "candidate", "roads_sha256", "source_sha256")):
            raise ValueError("returned web-bay identity differs")
        if lines and row["input_sha256"] != file_hash(case_path):
            raise ValueError("returned input hash differs")
        if row["status"] == "ok":
            if dispatch["exit_code"] != 0 or launch["docker_exit"] != 0:
                raise ValueError("failed launch cannot supply a successful score")
            if set(row["stiffness_n_mm"]) != set(CASES):
                raise ValueError("incomplete response inventory")
            for r in row["components"]:
                if r["case"] not in CASES or any(not math.isfinite(r[k]) or not 0 <= r[k] <= 1e-5
                        for k in ("relative_residual", "energy_error", "force_balance")):
                    raise ValueError("failed physical numerical gate")
            for c in CASES:
                value = row["stiffness_n_mm"][c]
                components = [r for r in row["components"] if r["case"] == c]
                if not math.isfinite(value) or value < 0 or not math.isclose(value, sum(r["stiffness_n_mm"] for r in components), rel_tol=1e-10, abs_tol=1e-12):
                    raise ValueError("response disagrees with solved component inventory")
                if (not components) != (c in row["nonspanning_cases"]):
                    raise ValueError("unwitnessed zero response")
        data = json.loads((inputs / key / "roads.json").read_text())
        if file_hash(inputs / key / "roads.json") != case["roads_sha256"]:
            raise ValueError("changed native roads")
        rows.append(dict(row, candidate=job["candidate"], job=key,
                         capture_id=case["native_capture_id"],
                         name=proposal.get("name", Candidate(**job["candidate"]).identity),
                         parent=proposal.get("parent"), image=expected_image,
                         volume_mm3=data["report"]["deposited_volume_mm3"],
                         motion_s=data["motion_seconds_lower_bound"]))
    baseline = next((r for r in rows if r["name"] == "flat" and r["status"] == "ok"), None)
    if baseline and all(baseline["stiffness_n_mm"][c] > 0 for c in CASES):
        for r in rows:
            if r["status"] == "ok":
                r["response_ratios"] = {c: r["stiffness_n_mm"][c] / baseline["stiffness_n_mm"][c] for c in CASES}
                r["geometric_mean_ratio"] = geometric_mean(list(r["response_ratios"].values()))
    write(output, dict(cohort=cohort, image=expected_image,
                       scope="provisional_finite_web_bay_elasticity_not_strength_or_global_infill_frontier",
                       minimum_effect_threshold=None, rows=rows,
                       frontier=[r["capture_id"] for r in frontier(rows)]))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)
    q = sub.add_parser("seeds")
    q.add_argument("--output", type=Path, required=True)
    q = sub.add_parser("pack")
    for name in ("capture", "context", "output"):
        q.add_argument("--"+name, type=Path, required=True)
    q.add_argument("--divisions", type=int, choices=(1,2,4,8), default=1)
    q.add_argument("--solver", choices=("p_amg", "superlu_mmd", "superlu"), default="p_amg")
    q.add_argument("--cap-segments", type=int, choices=(4,8,16,32), default=32)
    q = sub.add_parser("collect")
    for name in ("inputs", "results", "image", "output"):
        q.add_argument("--"+name, type=Path, required=True)
    a = p.parse_args()
    if a.command == "seeds":
        a.output.mkdir(parents=True, exist_ok=False)
        write(a.output / "proposals.json", dict(proposals=list(seeds())))
        (a.output / "candidates.jsonl").write_text("".join(json.dumps(r["candidate"])+"\n" for r in seeds()))
    elif a.command == "pack":
        pack(a.capture, a.context, a.output, a.divisions, a.solver, a.cap_segments)
    else:
        collect(a.inputs, a.results, a.image, a.output)


if __name__ == "__main__":
    main()
