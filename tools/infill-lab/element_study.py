# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Pack frozen element qualification before any room/web fitness-driven search.

Uses retained verified native constructions. No synthetic road is presented as a
native candidate; analytical beams have a distinct, explicit source kind.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict
import json
from pathlib import Path
import shutil

from campaign import bounds, write
from model import Material, digest, file_hash
from reference import JUDGE


def qualification_beams():
    for element, refinements in {
        "C3D6": [(2, 1, 1), (8, 1, 1), (32, 1, 1), (8, 2, 2), (8, 1, 4)],
        "C3D15": [(2, 1, 1), (8, 1, 1), (2, 2, 2)],
        "C3D10": [(2, 1, 1), (8, 1, 1)],
        "C3D8I": [(2, 1, 1), (8, 1, 1)],
    }.items():
        for nx, ny, nz in refinements:
            yield dict(
                element=element,
                case="pure_bending",
                material=asdict(Material(z_ratio=1)),
                refinement=dict(nx=nx, ny=ny, nz=nz),
                bounds=[[0, -0.225, -0.1], [8, 0.225, 0.1]],
            )
        for axis in "xyz":
            yield dict(
                element=element,
                case="compression_" + axis,
                material=asdict(Material(z_ratio=0.25)),
                refinement=dict(nx=2, ny=1, nz=1),
                bounds=[[0, -0.225, -0.1], [8, 0.225, 0.1]],
            )


def coupon_scenarios(name, extra_divisions=()):
    for element, division, dz in (
        ("C3D6", 1, 1.0),
        ("C3D6", 1, 0.1),
        ("C3D6", 1, 0.05),
        ("C3D15", 1, 1.0),
        ("C3D15", 2, 1.0),
        ("C3D15", 1, 0.1),
        ("C3D10", 1, 1.0),
        ("C3D10", 2, 1.0),
    ):
        yield dict(
            element=element,
            case="compression_z",
            material=asdict(Material(z_ratio=0.25)),
            bounds=bounds(name),
            refinement=dict(divisions=division, max_xy=4.0, max_z=dz),
        )

    for division in extra_divisions:
        if type(division) is not int or division not in (4, 8):
            raise ValueError("extra planar refinement must be 4 or 8")
        for element in ("C3D15", "C3D10"):
            yield dict(
                element=element,
                case="compression_z",
                material=asdict(Material(z_ratio=0.25)),
                bounds=bounds(name),
                refinement=dict(divisions=division, max_xy=4.0, max_z=1.0),
                seconds=1800,
            )


def pack(capture, context, output, extra_divisions=()):
    judge = {name: file_hash(context / name) for name in JUDGE}
    output.mkdir(parents=True, exist_ok=False)
    jobs = []

    def add(spec, source=None):
        spec = {"schema": 1, "kind": "element_reference", "judge": judge, **spec}
        identity = digest(spec)
        folder = output / identity
        folder.mkdir()
        write(folder / "case.json", spec)
        if source is not None:
            shutil.copyfile(source, folder / "roads.json")
        jobs.append(
            dict(
                id=identity,
                specimen=spec.get("specimen", "analytical_beam"),
                candidate=spec.get("candidate"),
                case=spec["scenarios"][0],
            )
        )

    for scenario in qualification_beams():
        add(dict(kind_source="analytical_beam", scenarios=[scenario]))
    rows = [
        json.loads(line)
        for line in (capture / "captures.jsonl").read_text().splitlines()
    ]
    selected = [
        r
        for r in rows
        if r["status"] == "ok"
        and r["specimen"] in ("cube32", "beam80")
        and r["candidate"]["family"] == "uniform"
        and r["candidate"]["fraction"] == 0.1
        and r["candidate"]["axis"] == "alternating_xy"
        and r["candidate"]["pattern"] == "rectilinear"
    ]
    if {r["specimen"] for r in selected} != {"cube32", "beam80"} or len(selected) != 2:
        raise ValueError("exact frozen cube/beam alternating controls required")
    for row in selected:
        native = capture / row["native"]
        manifest = json.loads((native / "manifest.json").read_text())
        if any(file_hash(native / name) != value for name, value in manifest.items()):
            raise ValueError("retained native manifest differs")
        if (
            file_hash(native / "roads.json") != row["roads_sha256"]
            or file_hash(native / "actual.splr") != row["replay_sha256"]
        ):
            raise ValueError("retained source/replay capture differs")
        for scenario in coupon_scenarios(row["specimen"], extra_divisions):
            add(
                dict(
                    kind_source="actual_roads",
                    scope="interior",
                    candidate=row["candidate"],
                    specimen=row["specimen"],
                    roads_sha256=row["roads_sha256"],
                    source_sha256=row["source_sha256"],
                    replay_sha256=row["replay_sha256"],
                    scenarios=[scenario],
                ),
                native / "roads.json",
            )
    write(
        output / "jobs.json",
        dict(
            schema=1,
            judge=judge,
            jobs=jobs,
            failed_captures=[],
            capture_sha256=file_hash(capture / "capture.json"),
            scope="element_qualification_no_candidate_search",
            required_response_refinement=0.05,
        ),
    )
    print(json.dumps(dict(jobs=len(jobs), output=str(output))))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--context", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--extra-divisions", type=int, nargs="*", default=[])
    args = parser.parse_args()
    if len(set(args.extra_divisions)) != len(args.extra_divisions):
        parser.error("duplicate extra refinements")
    pack(
        args.capture.resolve(),
        args.context.resolve(),
        args.output.resolve(),
        args.extra_divisions,
    )


if __name__ == "__main__":
    main()
