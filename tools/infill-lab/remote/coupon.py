# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Finite-interior mechanics over inert, independently replayed deposition.

This worker never starts the engine or imports geometry authority. Hashes bind
its frozen input/judge; the retained native capture owns compilation/replay.
"""

from __future__ import annotations
import json
import math
from pathlib import Path
import platform
import resource
import time

from geometry import prism_mesh
from mechanics import solve_coupon
from model import Candidate, Material, digest, file_hash

JUDGE = ("model.py", "geometry.py", "mechanics.py", "coupon.py", "requirements.lock")


def interior_roads(data):
    return [
        r
        for r in data["roads"]
        if r["kind"] in (2, 5, 13) or (r["kind"] == 8 and type(r["owner"]) is int)
    ]


def costs(data):
    roles = {"walls_mm3": 0.0, "skins_mm3": 0.0, "interior_mm3": 0.0}
    for r in data["roads"]:
        h, w = r["upper"] - r["lower"], r["width"]
        if not (math.isfinite(h) and math.isfinite(w) and 0 < h <= w):
            raise ValueError("invalid nominal bead interval")
        volume = ((w - h) * h + math.pi * h * h / 4) * math.dist(r["start"], r["end"])
        if r["kind"] == 1:
            role = "walls_mm3"
        elif r["kind"] in (2, 5, 13) or (r["kind"] == 8 and type(r["owner"]) is int):
            role = "interior_mm3"
        elif r["kind"] in (3, 4) or (r["kind"] == 8 and r["owner"] == "mandatory_skin"):
            role = "skins_mm3"
        else:
            raise ValueError("unclassified deposition role")
        roles[role] += volume
    if not math.isclose(
        sum(roles.values()),
        data["report"]["deposited_volume_mm3"],
        rel_tol=1e-6,
        abs_tol=1e-5,
    ):
        raise ValueError("role dose does not reconcile with verified object dose")
    return roles


def load(folder):
    spec = json.loads((folder / "case.json").read_text())
    if spec["schema"] != 2 or spec["kind"] != "actual_interior_coupon":
        raise ValueError("unknown coupon input schema")
    here = Path(__file__).resolve().parent
    if spec["judge"] != {p: file_hash(here / p) for p in JUDGE}:
        raise ValueError("frozen coupon judge differs from container")
    if file_hash(folder / "roads.json") != spec["roads_sha256"]:
        raise ValueError("deposition input hash differs")
    data = json.loads((folder / "roads.json").read_text())
    Candidate(**data["candidate"])
    if not data["report"]["verified"] or not data["software_only"]:
        raise ValueError("input lacks software-only native replay receipt")
    if (
        data["candidate"] != spec["candidate"]
        or data["source_sha256"] != spec["source_sha256"]
    ):
        raise ValueError("candidate/source differs from frozen deposition")
    return spec, data


def evaluate(folder, output):
    spec, data = load(folder)
    roles = costs(data)
    roads = interior_roads(data)
    started = time.monotonic()
    rows = []
    for scenario in spec["scenarios"]:
        before = time.monotonic()
        print(json.dumps({"phase": "mesh", "scenario": scenario}), flush=True)
        material = Material(**scenario["material"])
        row = {
            "candidate": spec["candidate"],
            "specimen": spec["specimen"],
            "input_sha256": file_hash(folder / "case.json"),
            "source_sha256": spec["source_sha256"],
            "replay_sha256": spec["replay_sha256"],
            "engine_build": data["engine_build"],
            "bin_sha256": data["bin_sha256"],
            "judge": spec["judge"],
            "scenario": scenario,
            "volume_mm3": data["report"]["deposited_volume_mm3"],
            "mass_g": data["report"]["deposited_volume_mm3"] * material.density_g_mm3,
            "motion_s": data["motion_seconds_lower_bound"],
            "roles": roles,
            "native": spec["native"],
            "compile_s": data["compile_wall_s"],
        }
        try:
            mesh, info = prism_mesh(
                roads,
                window=scenario["bounds"],
                max_xy=scenario["max_xy"],
                max_z=scenario["max_z"],
                divisions=scenario["divisions"],
            )
            row["mesh_s"] = time.monotonic() - before
            print(
                json.dumps(
                    {
                        "phase": "mechanics",
                        "nodes": int(mesh.points.shape[1]),
                        "mesh_s": row["mesh_s"],
                    }
                ),
                flush=True,
            )
            row["mesh"] = info
            row["mechanics"] = solve_coupon(
                mesh, material, scenario["bounds"], solver=scenario["solver"]
            )
            row["status"] = "ok"
        except Exception as error:
            row.update(
                status="refused_or_failed",
                error_type=type(error).__name__,
                error=str(error),
            )
        row.update(
            wall_s=time.monotonic() - before,
            peak_rss_kib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
            architecture=platform.machine(),
        )
        row["id"] = digest({"input": row["input_sha256"], "scenario": scenario})
        with (output / "trials.jsonl").open("a") as stream:
            stream.write(json.dumps(row, allow_nan=False) + "\n")
            stream.flush()
            import os

            os.fsync(stream.fileno())
        rows.append(row)
        print(
            json.dumps({k: row[k] for k in ("status", "id", "wall_s", "peak_rss_kib")}),
            flush=True,
        )
    return {
        "status": "ok" if all(r["status"] == "ok" for r in rows) and rows else "failed",
        "scope": "finite_actual_interior_coupon_not_whole_part_strength",
        "trials": len(rows),
        "accepted": sum(r["status"] == "ok" for r in rows),
        "wall_s": time.monotonic() - started,
        "materials": [s["material"] for s in spec["scenarios"]],
    }
