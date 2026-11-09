# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Original OCCT pressure shapes and actual saved-return Web + roof journeys.

Tooling only; requires cadquery-ocp==7.9.3.1 in an isolated Python prefix.
Run with prebuilt coherent binaries inside an external memory/task/time bound.
No evaluator, mesh fallback or printer/strength qualification is supplied here.
"""

from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import time
from native import ROOT, run


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def shapes():
    from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeFace, BRepBuilderAPI_MakePolygon
    from OCP.BRepPrimAPI import (
        BRepPrimAPI_MakeBox,
        BRepPrimAPI_MakeCylinder,
        BRepPrimAPI_MakePrism,
        BRepPrimAPI_MakeCone,
        BRepPrimAPI_MakeSphere,
    )
    from OCP.gp import gp_Ax2, gp_Dir, gp_Pnt, gp_Vec

    def prism(points):
        wire = BRepBuilderAPI_MakePolygon()
        for x, y in points:
            wire.Add(gp_Pnt(x, y, 0))
        wire.Close()
        return BRepPrimAPI_MakePrism(
            BRepBuilderAPI_MakeFace(wire.Wire()).Face(), gp_Vec(0, 0, 12)
        ).Shape()

    def cylinder(x, y, radius, height=12, z=0):
        return BRepPrimAPI_MakeCylinder(
            gp_Ax2(gp_Pnt(x, y, z), gp_Dir(0, 0, 1)), radius, height
        ).Shape()

    def cut(a, b):
        operation = BRepAlgoAPI_Cut(a, b)
        if not operation.IsDone():
            raise RuntimeError("original pressure-shape subtraction failed")
        return operation.Shape()

    round_body = cylinder(12, 12, 12)
    perforated = BRepPrimAPI_MakeBox(32, 24, 12).Shape()
    for x, y in [(8, 8), (24, 8), (8, 17), (24, 17)]:
        perforated = cut(perforated, cylinder(x, y, 2, 14, -1))
    return {
        "round": (round_body, "candidate"),
        "annulus": (cut(round_body, cylinder(12, 12, 5, 14, -1)), "candidate"),
        "l-shape": (
            prism([(0, 0), (28, 0), (28, 10), (10, 10), (10, 28), (0, 28)]),
            "candidate",
        ),
        "u-shape": (
            prism(
                [
                    (0, 0),
                    (28, 0),
                    (28, 28),
                    (18, 28),
                    (18, 10),
                    (10, 10),
                    (10, 28),
                    (0, 28),
                ]
            ),
            "candidate",
        ),
        "narrow-neck": (
            prism(
                [
                    (0, 0),
                    (12, 0),
                    (12, 10),
                    (22, 10),
                    (22, 0),
                    (34, 0),
                    (34, 24),
                    (22, 24),
                    (22, 14),
                    (12, 14),
                    (12, 24),
                    (0, 24),
                ]
            ),
            "candidate",
        ),
        "four-holes": (perforated, "candidate"),
        "thin": (BRepPrimAPI_MakeBox(28, 4, 12).Shape(), "candidate"),
        "diamond": (prism([(12, 0), (24, 12), (12, 24), (0, 12)]), "candidate"),
        "cone": (BRepPrimAPI_MakeCone(12, 6, 12).Shape(), "local-roof-probe"),
        "sphere": (
            BRepPrimAPI_MakeSphere(gp_Pnt(12, 12, 12), 12).Shape(),
            "local-roof-probe",
        ),
        "shelf": (
            cut(
                BRepPrimAPI_MakeBox(28, 24, 12).Shape(),
                BRepPrimAPI_MakeBox(gp_Pnt(14, -1, 6), 15, 26, 7).Shape(),
            ),
            "local-roof-probe",
        ),
    }


def generate(folder):
    from OCP.BRepCheck import BRepCheck_Analyzer
    from OCP.IFSelect import IFSelect_RetDone
    from OCP.Interface import Interface_Static
    from OCP.STEPControl import STEPControl_Writer, STEPControl_AsIs

    folder.mkdir(parents=True, exist_ok=False)
    Interface_Static.SetCVal_s("write.step.schema", "AP214IS")
    Interface_Static.SetCVal_s("write.step.unit", "MM")
    result = {}
    for name, (shape, scope) in shapes().items():
        if not BRepCheck_Analyzer(shape).IsValid():
            raise RuntimeError(f"invalid original pressure shape: {name}")
        path = folder / f"{name}.step"
        writer = STEPControl_Writer()
        writer.Transfer(shape, STEPControl_AsIs)
        if writer.Write(str(path)) != IFSelect_RetDone:
            raise RuntimeError(f"original STEP export failed: {name}")
        path.write_text(
            re.sub(
                r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}",
                "2026-01-01T00:00:00",
                path.read_text(),
            )
        )
        result[name] = {"source_sha256": sha(path), "scope": scope}
    (folder / "manifest.json").write_text(
        json.dumps({"generator_sha256": sha(Path(__file__)), "cases": result}, indent=2)
    )
    return result


def exercise(args, name, source, fraction, spacing, metadata):
    folder = (
        args.output / f"{name}-{fraction:g}-{'auto' if spacing is None else spacing}"
    )
    folder.mkdir()
    owned_source = folder / "source.step"
    shutil.copyfile(source, owned_source)
    intent = json.loads(
        (ROOT / "fixtures/manufacturing/web-roof.intent.json").read_text()
    )
    intent["recipe"].update(
        infill_fraction=fraction,
        sparse_infill=(
            {"kind": "web_roof", "roof_spacing_mm": spacing, "architecture": None}
            if args.pattern == "web_roof"
            else {"kind": args.pattern}
        ),
        bead_width_mm=args.width,
        perimeter_count=args.walls,
        layer_height_mm=args.layer_height,
    )
    # All examples fit the captured bed. Raise only final park above the tallest
    # 24 mm probe; native admission still owns source/prime/park clearance.
    intent["printer"]["workflow"]["job"]["park_position_mm"][2] = 30
    intent_file = folder / "intent.json"
    intent_file.write_text(json.dumps(intent))
    project = folder / "Web and roof.spil"
    started = time.monotonic()
    result = {
        "case": name,
        "pattern": args.pattern,
        "fraction": fraction,
        "roof_spacing_mm": spacing,
        "width_mm": args.width,
        "wall_count": args.walls,
        "layer_height_mm": args.layer_height,
        **metadata,
    }
    command = [args.bin_dir / "spiling-cli", "project"]
    try:
        _, compile_s = run(
            command
            + [
                "create",
                project,
                "--import",
                owned_source,
                "--pose",
                "1",
                json.dumps(
                    {"translation_mm": [30, 30, 0], "rotation_xyzw": [0, 0, 0, 1]}
                ),
                "--intent",
                intent_file,
                "--compile",
                "--verify",
                "--save",
                "--manufacturing-out",
                folder / "export",
                "--engine",
                args.bin_dir / "spiling-engine",
            ],
            timeout=args.seconds,
            output=folder / "create",
        )
        owned_source.unlink()
        intent_file.unlink()
        _, return_s = run(
            command
            + [
                "inspect",
                project,
                "--verify",
                "--manufacturing-out",
                folder / "returned-export",
                "--replay-out",
                folder / "actual.splr",
                "--engine",
                args.bin_dir / "spiling-engine",
            ],
            timeout=args.seconds,
            output=folder / "return",
        )
        returned = json.loads((folder / "return.stdout.json").read_text())
        retained_recipe = returned["manufacturing"]["intent"]["recipe"]
        for field in (
            "sparse_infill",
            "infill_fraction",
            "bead_width_mm",
            "perimeter_count",
            "layer_height_mm",
        ):
            if retained_recipe[field] != intent["recipe"][field]:
                raise AssertionError(f"saved return changed recipe {field}")
        for artifact in [
            "program.gcode",
            "plan.json",
            "provenance.json",
            "verification.json",
        ]:
            if (folder / "export" / artifact).read_bytes() != (
                folder / "returned-export" / artifact
            ).read_bytes():
                raise AssertionError(f"saved return changed {artifact}")
        run(
            [
                "node",
                ROOT / "tools/infill-lab/export-replay.mjs",
                folder / "return.stdout.json",
                folder / "actual.splr",
                folder / "roads.json",
            ],
            timeout=args.seconds,
            output=folder / "decode",
        )
        plan = json.loads((folder / "returned-export/plan.json").read_text())
        result.update(
            status="accepted",
            compile_seconds=compile_s,
            return_seconds=return_s,
            nominal_material_mm3=sum(
                layer["region_coverage"]["nominal_deposition_volume_mm3"]
                for layer in plan["layers"]
            ),
            layers=len(plan["layers"]),
            sparse_roads=sum(
                p["kind"] == "sparse_fill"
                for layer in plan["layers"]
                for p in layer["paths"]
            ),
        )
    except (RuntimeError, TimeoutError, AssertionError) as error:
        result.update(status="refused-or-failed", diagnostic=str(error))
    result["elapsed_seconds"] = time.monotonic() - started
    (folder / "result.json").write_text(json.dumps(result, indent=2))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--bin-dir", type=Path, default=ROOT / "target/debug")
    parser.add_argument(
        "--cases",
        default="round,annulus,l-shape,u-shape,narrow-neck,four-holes,thin,diamond,cone,sphere,shelf",
    )
    parser.add_argument("--fractions", default="0.08,0.16")
    parser.add_argument("--roof-spacing", type=float)
    parser.add_argument(
        "--pattern",
        choices=("web_roof", "fine", "grid"),
        default="web_roof",
        help="ordinary-pattern controls locate shared construction failures",
    )
    parser.add_argument("--seconds", type=int, default=120)
    parser.add_argument("--width", type=float, default=0.45)
    parser.add_argument("--walls", type=int, default=2)
    parser.add_argument("--layer-height", type=float, default=0.2)
    parser.add_argument("--generate-only", action="store_true")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.bin_dir = args.bin_dir.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    metadata = generate(args.output / "sources")
    if args.generate_only:
        return
    producers = {
        name: sha(args.bin_dir / name) for name in ["spiling-cli", "spiling-engine"]
    }
    (args.output / "producer.json").write_text(json.dumps(producers, indent=2))
    results = []
    for name in args.cases.split(","):
        for fraction in map(float, args.fractions.split(",")):
            if any(
                sha(args.bin_dir / binary) != digest
                for binary, digest in producers.items()
            ):
                raise RuntimeError("coherent producer binaries changed during campaign")
            result = exercise(
                args,
                name,
                args.output / "sources" / f"{name}.step",
                fraction,
                args.roof_spacing,
                metadata[name],
            )
            results.append(result)
            (args.output / "results.json").write_text(json.dumps(results, indent=2))
            print(json.dumps(result), flush=True)
    if any(r["scope"] == "candidate" and r["status"] != "accepted" for r in results):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
