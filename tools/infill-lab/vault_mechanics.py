# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Paired-search roof coupons from verified actual roads, including deposited skins.

Perfectly bonded nominal-footprint TetP2 elasticity, not strength or a complete
part. Geometry/fixtures/material stay frozen within each search cohort.
"""

from __future__ import annotations

import json
from pathlib import Path
import resource
import sys
import time

import numpy as np

from geometry import connected_meshes, volume_mesh
from model import Candidate, Material, file_hash
from quadratic import JUDGE as QUADRATIC_JUDGE, prepare, solve_body

JUDGE = (*QUADRATIC_JUDGE, "vault_mechanics.py")


def load(folder):
    from coupon import costs

    spec = json.loads((folder / "case.json").read_text())
    here = Path(__file__).resolve().parent
    if (
        spec["schema"] != 1
        or spec["kind"] not in {"vault_roof_patch", "cathedral_web_bay"}
        or spec["judge"] != {p: file_hash(here / p) for p in JUDGE}
    ):
        raise ValueError("unknown or changed roof-patch judge")
    if file_hash(folder / "roads.json") != spec["roads_sha256"]:
        raise ValueError("actual road input changed")
    data = json.loads((folder / "roads.json").read_text())
    Candidate(**spec["candidate"])
    if (
        not data["software_only"]
        or not data["report"]["verified"]
        or data["source_sha256"] != spec["source_sha256"]
        or data["candidate"] != spec["candidate"]
        or data["bin_sha256"] != spec["binaries"]
    ):
        raise ValueError("unverified or mismatched native construction")
    costs(data)  # All deposited object roles must reconcile with replay dose.
    return spec, data



def patch_response(data, scenario, *, workdir=None):
    """One assembly/component, reused across XYZ; disconnected paths earn zero."""
    bounds = np.asarray(scenario["bounds"], dtype=float)
    if (
        bounds.shape != (2, 3)
        or not np.all(np.isfinite(bounds))
        or np.any(bounds[1] <= bounds[0])
    ):
        raise ValueError("invalid patch bounds")
    if not all(
        20 < bounds[0, i] < bounds[1, i] < 20 + data["size_mm"][i] for i in (0, 1)
    ):
        raise ValueError("roof patch must exclude exterior side walls")
    if not np.isclose(bounds[1, 2], data["size_mm"][2], rtol=0, atol=1e-8):
        raise ValueError("roof patch must retain the actual top skin")
    material = Material(**scenario["material"])
    # Keep deposited skins and roof growth; cropping happens in the union mesher.
    # No fabricated lid, centrelines, geometry repair or missing-road fallback.
    mesh, info = volume_mesh(data["roads"], scenario, workdir=workdir)
    responses = {axis: 0.0 for axis in "xyz"}
    records = []
    for part in connected_meshes(mesh):
        axes = [
            axis
            for i, axis in enumerate("xyz")
            if np.allclose(part.bounds[:, i], bounds[:, i], rtol=0, atol=1e-8)
        ]
        if not axes:
            continue
        body = prepare(part, material)
        for axis in axes:
            result = solve_body(
                body, "compression_" + axis, bounds, solver=scenario["solver"]
            )
            responses[axis] += result["stiffness_n_mm"]
            records.append(dict(axis=axis, **result))
        del body
    return dict(stiffness_n_mm=responses, components=records, mesh=info)


def web_response(data, scenario, *, workdir=None):
    """Finite cut web, including its complete window and two real piers.

    Reuse each component's unchanged assembly across the explicit fixture set.
    This is a local mechanism study, not roof capacity or whole-part strength.
    """
    from coupon import interior_roads

    bounds = np.asarray(scenario["bounds"], dtype=float)
    cases = scenario["cases"]
    if cases != ["compression_x", "compression_z", "cantilever_z_y"]:
        raise ValueError("unknown web-bay fixture inventory")
    material = Material(**scenario["material"])
    mesh, info = volume_mesh(interior_roads(data), scenario, workdir=workdir)
    responses = dict.fromkeys(cases, 0.0)
    records = []
    for part in connected_meshes(mesh):
        admitted = [
            c
            for c in cases
            if np.allclose(
                part.bounds[:, 0 if c == "compression_x" else 2],
                bounds[:, 0 if c == "compression_x" else 2],
                rtol=0,
                atol=1e-8,
            )
        ]
        if not admitted:
            continue
        body = prepare(part, material)
        for case in admitted:
            result = solve_body(body, case, bounds, solver=scenario["solver"])
            responses[case] += result["stiffness_n_mm"]
            records.append(result)
        del body
    return dict(
        stiffness_n_mm=responses,
        components=records,
        mesh=info,
        nonspanning_cases=[
            c for c in cases if not any(r["case"] == c for r in records)
        ],
    )


def evaluate(folder, output):
    spec, data = load(folder)
    began = time.monotonic()
    row = dict(
        input_sha256=file_hash(folder / "case.json"),
        judge=spec["judge"],
        candidate=spec["candidate"],
        specimen=spec["specimen"],
        roads_sha256=spec["roads_sha256"],
        source_sha256=spec["source_sha256"],
        scenario=spec["scenario"],
        volume_mm3=data["report"]["deposited_volume_mm3"],
        motion_s=data["motion_seconds_lower_bound"],
        scope=(
            "finite_web_bay_nominal_elasticity_not_whole_part_strength"
            if spec["kind"] == "cathedral_web_bay"
            else "local_roof_patch_nominal_elasticity_not_whole_part_strength"
        ),
    )
    try:
        response = (
            web_response(data, spec["scenario"], workdir=output)
            if spec["kind"] == "cathedral_web_bay"
            else patch_response(data, spec["scenario"], workdir=output)
        )
        row.update(response, status="ok")
    except Exception as error:
        row.update(status="failed", error_type=type(error).__name__, error=str(error))
    row.update(
        wall_s=time.monotonic() - began,
        peak_rss_kib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
    )
    (output / "trials.jsonl").write_text(json.dumps(row, allow_nan=False) + "\n")
    return dict(status=row["status"], scope=row["scope"], trials=1)
