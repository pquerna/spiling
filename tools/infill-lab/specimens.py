# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Original tooling-only OCCT sources, imported by the ordinary SolidCore engine."""

from pathlib import Path
import json
from model import file_hash

SIZES = {"beam": (12.0, 5.0, 4.0), "cube": (6.0, 6.0, 6.0), "holdout": (16.0, 6.0, 4.0)}


def generate(folder: Path):
    from OCP.BRepCheck import BRepCheck_Analyzer
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox
    from OCP.IFSelect import IFSelect_RetDone
    from OCP.Interface import Interface_Static
    from OCP.STEPControl import STEPControl_Writer, STEPControl_AsIs

    folder.mkdir(parents=True, exist_ok=True)
    Interface_Static.SetCVal_s("write.step.schema", "AP214IS")
    Interface_Static.SetCVal_s("write.step.unit", "MM")
    for name, size in SIZES.items():
        destination = folder / f"{name}.step"
        manifest = folder / f"{name}.json"
        if destination.exists():
            metadata = json.loads(manifest.read_text())
            if metadata != {
                "size_mm": list(size),
                "generator_sha256": file_hash(Path(__file__)),
                "source_sha256": file_hash(destination),
            }:
                raise ValueError(
                    "generated specimen changed; use a new study directory"
                )
            continue
        shape = BRepPrimAPI_MakeBox(*size).Shape()
        if not BRepCheck_Analyzer(shape).IsValid():
            raise RuntimeError("original study solid is invalid")
        writer = STEPControl_Writer()
        writer.Transfer(shape, STEPControl_AsIs)
        if writer.Write(str(destination)) != IFSelect_RetDone:
            raise RuntimeError("STEP export failed")
        # Deterministic STEP header: original generated data, never rewriting an imported fixture.
        text = destination.read_text()
        import re

        text = re.sub(
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}", "2026-01-01T00:00:00", text
        )
        destination.write_text(text)
        with manifest.open("x") as stream:
            json.dump(
                {
                    "size_mm": size,
                    "generator_sha256": file_hash(Path(__file__)),
                    "source_sha256": file_hash(destination),
                },
                stream,
            )
