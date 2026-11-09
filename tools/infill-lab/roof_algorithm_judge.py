# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Separate frozen subset-only roof experiment; not native print authority.

Preserve every continuous source web. Removed corbels must leave all retained
corbels supported and the first required skin within the declared bridge span.
"""
from __future__ import annotations
import argparse
import copy
import json
from pathlib import Path
import resource
import time
from shapely.geometry import LineString
from shapely.ops import unary_union
from algorithm_judge import accounting, context, key, footprint
from model import file_hash


def admit_roof(baseline, sparse):
    c = context(baseline)
    known = {key(r) for r in c["sparse"]}
    selected = {key(r) for r in sparse}
    if len(selected) != len(sparse) or not selected <= known:
        raise ValueError("roof trial must be a unique subset of native sparse roads")

    def is_web(r):
        fixed = int(r["start"][0] != r["end"][0])
        return r["start"][fixed] in c["roots"][fixed]

    required = {key(r) for r in c["sparse"] if is_web(r)}
    if not required <= selected:
        raise ValueError("removed a continuous web or its root")
    layers = {}
    for r in baseline["roads"]:
        layers.setdefault(r["layer"], [[], []])[0].append(r)
        if r["kind"] != 5:
            layers[r["layer"]][1].append(r)
    for r in sparse:
        layers[r["layer"]][1].append(r)
    first_skin = min(r["layer"] for r in baseline["roads"] if r["kind"] == 4)
    previous = None
    worst = 0.0
    max_gap = 0.0
    contacts = 0
    for layer, (original, current) in sorted(layers.items()):
        # The baseline top skin includes actual Bridge roles. Its original
        # native admission remains provenance, not admission of this subset.
        z = original[0]["upper"]
        height = z - original[0]["lower"]
        active = [r for r in current if r["kind"] == 5]
        if previous is not None and active:
            permitted = previous.buffer(0.5 * height + 2e-6, quad_segs=32)
            # Examine new current deposition centerlines against nominal
            # contact radius; this is a screening test, never a full-bead proof.
            for r in active:
                unsupported = LineString([r["start"][:2], r["end"][:2]]).difference(
                    permitted
                )
                worst = max(worst, unsupported.length)
                if unsupported.length > 1e-5:
                    raise ValueError(
                        f"unsupported retained corbel at layer {layer}: {unsupported.length}"
                    )
        if layer == first_skin:
            for r in current:
                if r["kind"] not in (4, 8) or r["owner"] not in ("mandatory_skin",):
                    continue
                line = LineString([r["start"][:2], r["end"][:2]])
                gaps = line.difference(previous)
                pieces = list(gaps.geoms) if hasattr(gaps, "geoms") else [gaps]
                for gap in pieces:
                    max_gap = max(max_gap, gap.length)
                    if gap.length > 10.0 + 1e-5:
                        raise ValueError(
                            f"first roof skin exceeds captured bridge span: {gap.length}"
                        )
                contacts += 1
        previous = footprint(current)
    if not contacts:
        raise ValueError("missing required roof contact inventory")
    return dict(
        continuous_web_roads=len(required),
        roof_rows_checked=contacts,
        max_skin_gap_mm=max_gap,
        unsupported_centerline_mm=worst,
        scope="nominal_centerline_contact_screen_not_native_full_bead_or_physical_qualification",
    )


def evaluate(folder, output):
    from vault_mechanics import patch_response

    spec = json.loads((folder / "case.json").read_text())
    here = Path(__file__).resolve().parent
    for name, h in spec["judge"].items():
        source = (
            here / name if (here / name).exists() else Path("/opt/spiling-eval") / name
        )
        if file_hash(source) != h:
            raise ValueError("frozen roof judge differs")
    if any(
        file_hash(folder / name) != spec[field]
        for name, field in [
            ("baseline.json", "baseline_sha256"),
            ("sparse.json", "sparse_sha256"),
            ("algorithm.py", "algorithm_sha256"),
        ]
    ):
        raise ValueError("trial input changed")
    baseline = json.loads((folder / "baseline.json").read_text())
    sparse = json.loads((folder / "sparse.json").read_text())
    began = time.monotonic()
    row = {
        k: spec[k]
        for k in ("name", "parent", "hypothesis", "algorithm_sha256", "judge")
    }
    row.update(
        native_replayed=False,
        scope="derived_nominal_roof_patch_elasticity_not_strength",
    )
    try:
        row["admission"] = admit_roof(baseline, sparse)
        row.update(accounting(baseline, sparse))
        derived = copy.deepcopy(baseline)
        derived["roads"] = [r for r in baseline["roads"] if r["kind"] != 5] + sparse
        derived["report"]["verified"] = False
        row.update(
            patch_response(derived, spec["roof_scenario"], workdir=output), status="ok"
        )
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
            {k: v for k, v in row.items() if k not in ("mesh", "components", "judge")}
        ),
        flush=True,
    )


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--input", type=Path, default=Path("/input"))
    p.add_argument("--output", type=Path, default=Path("/output"))
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=True)
    evaluate(a.input, a.output)
