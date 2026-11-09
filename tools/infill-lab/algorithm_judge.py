# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Frozen research admission and scoring, separate from editable construction.

Derived rows are NOT native replay evidence. Unchanged native roof and shell
inventories, nominal layer support and complete costs are checked independently.
Native production promotion requires engine construction, fresh replay and return.
"""
from __future__ import annotations
import argparse
import copy
import json
import math
from pathlib import Path
import resource
import time

from shapely.geometry import LineString
from shapely.ops import unary_union
from model import file_hash

MAX_ROADS = 120_000
UPPER_EDIT_Z = 12.0
WIDTH = 0.45


def key(r):
    return tuple(
        json.dumps(r[k], sort_keys=True, separators=(",", ":"))
        for k in ("start", "end", "width", "lower", "upper", "kind", "owner", "layer")
    )


def context(data):
    sparse = [copy.deepcopy(r) for r in data["roads"] if r["kind"] == 5]
    first = min(r["layer"] for r in sparse)
    roots = [
        sorted(
            {
                r["start"][fixed]
                for r in sparse
                if r["layer"] == first and r["start"][fixed] == r["end"][fixed]
            }
        )
        for fixed in (0, 1)
    ]
    return dict(sparse=sparse, roots=roots, width=WIDTH, edit_top=UPPER_EDIT_Z)


def footprint(rows):
    return unary_union(
        [
            LineString([r["start"][:2], r["end"][:2]]).buffer(
                r["width"] / 2, quad_segs=8
            )
            for r in rows
        ]
    )


def admit(baseline, sparse):
    if not isinstance(sparse, list) or len(sparse) > MAX_ROADS:
        raise ValueError("unbounded sparse output")
    control = context(baseline)
    reference = {key(r) for r in control["sparse"] if r["upper"] >= UPPER_EDIT_Z}
    protected = {key(r) for r in sparse if r["upper"] >= UPPER_EDIT_Z}
    if reference != protected:
        raise ValueError("changed protected roof/root corridor")
    layers = {}
    for r in baseline["roads"]:
        layers.setdefault(r["layer"], [[], []])[0].append(r)
        if r["kind"] != 5:
            layers[r["layer"]][1].append(r)
    seen = set()
    for r in sparse:
        if (
            set(r) != set(control["sparse"][0])
            or r["kind"] != 5
            or type(r["owner"]) is not int
        ):
            raise ValueError("changed schema or deposition role")
        if r["layer"] not in layers or not all(
            math.isfinite(v) for k in ("start", "end") for v in r[k]
        ):
            raise ValueError("unknown layer or nonfinite coordinate")
        ref = layers[r["layer"]][0][0]
        if abs(r["width"] - WIDTH) > 1e-12 or any(
            abs(r[k] - ref[k]) > 1e-9 for k in ("lower", "upper")
        ):
            raise ValueError("changed physical interval/width")
        if any(abs(p[2] - r["upper"]) > 1e-9 for p in (r["start"], r["end"])):
            raise ValueError("changed layer plane")
        if math.dist(r["start"], r["end"]) < 0.001 or not any(
            r["start"][a] == r["end"][a] for a in (0, 1)
        ):
            raise ValueError("non-axis or short road")
        if not all(
            20.9 - 1e-6 <= p[a] <= 67.1 + 1e-6
            for p in (r["start"], r["end"])
            for a in (0, 1)
        ):
            raise ValueError("road outside frozen rectangular interior")
        k = key(r)
        if k in seen:
            raise ValueError("duplicate road")
        seen.add(k)
        layers[r["layer"]][1].append(r)
    # Every web intersection and its finite ligaments remain. Added material
    # must grow from earlier deposited material, never assumed solid occupancy.
    protected_nodes = 0
    max_unsupported_area = 0.0
    previous = None
    for index, (original, current) in sorted(layers.items()):
        z = original[0]["upper"]
        if z < UPPER_EDIT_Z and any(r["kind"] == 5 for r in original):
            sparse_layer = [r for r in current if r["kind"] == 5]
            for fixed in (0, 1):
                for root in control["roots"][fixed]:
                    intervals = sorted(
                        (
                            min(r["start"][1 - fixed], r["end"][1 - fixed]),
                            max(r["start"][1 - fixed], r["end"][1 - fixed]),
                        )
                        for r in sparse_layer
                        if r["start"][fixed] == r["end"][fixed] == root
                    )
                    for node in control["roots"][1 - fixed]:
                        if not any(
                            lo <= node - 1.8 + 1e-6 and hi >= node + 1.8 - 1e-6
                            for lo, hi in intervals
                        ):
                            raise ValueError("removed intersection pier/ligament")
                        protected_nodes += 1
        if z <= UPPER_EDIT_Z + 0.2 + 1e-8:
            current_footprint = footprint(current)
            if previous is not None and any(r["kind"] == 5 for r in original):
                # Frozen nominal 2:1 corbel envelope. This is a research
                # screening condition, not physical/native anchor qualification.
                permitted = previous.buffer(
                    0.5 * (z - original[0]["lower"]) + 2e-6, quad_segs=8
                )
                area = current_footprint.difference(permitted).area
                max_unsupported_area = max(max_unsupported_area, area)
                if area > 1e-5:
                    raise ValueError(
                        f"unsupported nominal layer growth: {area:.8g} mm2 at {z}"
                    )
            previous = current_footprint
    return dict(
        protected_roof_roads=len(reference),
        protected_nodes=protected_nodes,
        max_unsupported_area_mm2=max_unsupported_area,
        scope="nominal_2_to_1_layer_screen_not_native_or_physical_qualification",
    )


def accounting(baseline, sparse):
    def dose(r):
        h = r["upper"] - r["lower"]
        w = r["width"]
        return ((w - h) * h + math.pi * h * h / 4) * math.dist(r["start"], r["end"])

    old = [r for r in baseline["roads"] if r["kind"] == 5]
    volume = (
        baseline["report"]["deposited_volume_mm3"]
        - sum(map(dose, old))
        + sum(map(dose, sparse))
    )

    # Explicit comparable proxy; not the native motion lower bound. Includes
    # deposited lengths plus deterministic nearest-endpoint travel and per-road
    # start overhead, with the full original fixed-role length/time charged.
    def motion(rows):
        total = 0.0
        last = None
        for r in sorted(rows, key=lambda r: r["layer"]):
            a, b = r["start"], r["end"]
            if last is not None and math.dist(last, b) < math.dist(last, a):
                a, b = b, a
            if last is not None:
                total += math.dist(last, a) / 150.0
            total += math.dist(a, b) / 40.0 + 0.025
            last = b
        return total

    fixed = [r for r in baseline["roads"] if r["kind"] != 5]
    return dict(
        volume_mm3=volume,
        material_ratio=volume / baseline["report"]["deposited_volume_mm3"],
        motion_proxy_s=motion(fixed) + motion(sparse),
        sparse_roads=len(sparse),
        sparse_length_mm=sum(math.dist(r["start"], r["end"]) for r in sparse),
    )


def evaluate(folder, output):
    from vault_mechanics import web_response

    spec = json.loads((folder / "case.json").read_text())
    source = Path(__file__).resolve().parent
    if any(
        file_hash(
            (source / name)
            if (source / name).exists()
            else Path("/opt/spiling-eval") / name
        )
        != sha
        for name, sha in spec["judge"].items()
    ):
        raise ValueError("frozen judge changed")
    baseline = json.loads((folder / "baseline.json").read_text())
    if (
        file_hash(folder / "baseline.json") != spec["baseline_sha256"]
        or file_hash(folder / "algorithm.py") != spec["algorithm_sha256"]
    ):
        raise ValueError("trial code/input changed")
    # Generator code executes only during preparation, not inside the judge.
    # Remote evaluator consumes its inert rows and the exact archived source.
    sparse = json.loads((folder / "sparse.json").read_text())
    if file_hash(folder / "sparse.json") != spec["sparse_sha256"]:
        raise ValueError("inert construction changed")
    began = time.monotonic()
    row = dict(
        name=spec["name"],
        parent=spec["parent"],
        hypothesis=spec["hypothesis"],
        algorithm_sha256=spec["algorithm_sha256"],
        judge=spec["judge"],
        native_replayed=False,
        scope="derived_nominal_deposition_local_elastic_stiffness_not_strength",
    )
    try:
        row["admission"] = admit(baseline, sparse)
        row.update(accounting(baseline, sparse))
        derived = copy.deepcopy(baseline)
        derived["roads"] = [r for r in baseline["roads"] if r["kind"] != 5] + sparse
        # Never copy native replay status onto derived construction.
        derived["report"]["verified"] = False
        scenario = spec["scenario"]
        row.update(web_response(derived, scenario, workdir=output), status="ok")
    except Exception as error:
        row.update(status="failed", error_type=type(error).__name__, error=str(error))
    row.update(
        wall_s=time.monotonic() - began,
        peak_rss_kib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
    )
    (output / "trial.json").write_text(
        json.dumps(row, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {k: v for k, v in row.items() if k not in ("components", "mesh", "judge")}
        ),
        flush=True,
    )
    return row


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--input", type=Path, default=Path("/input"))
    p.add_argument("--output", type=Path, default=Path("/output"))
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=True)
    evaluate(a.input, a.output)
