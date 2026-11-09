# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0

"""Original full-part native-analysis fixture, not a runtime geometry fallback."""
from pathlib import Path
import re
from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut
from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox
from OCP.gp import gp_Pnt
from OCP.IFSelect import IFSelect_RetDone
from OCP.Interface import Interface_Static
from OCP.ShapeUpgrade import ShapeUpgrade_UnifySameDomain
from OCP.STEPControl import STEPControl_Writer, STEPControl_AsIs


def generate(destination):
    if destination.exists():
        raise ValueError("frozen original source already exists")
    shape = BRepPrimAPI_MakeBox(40.0, 48.0, 6.0).Shape()
    for origin, size in [((10, 12, -1), (22, 37, 8)), ((32, 24, -1), (9, 25, 8)), ((3, 34, -1), (4, 6, 8))]:
        shape = BRepAlgoAPI_Cut(shape, BRepPrimAPI_MakeBox(gp_Pnt(*origin), *size).Shape()).Shape()
    unify = ShapeUpgrade_UnifySameDomain(shape, True, True, False)
    unify.Build()
    shape = unify.Shape()
    if not BRepCheck_Analyzer(shape).IsValid():
        raise RuntimeError("invalid original hook")
    Interface_Static.SetCVal_s("write.step.schema", "AP214IS")
    Interface_Static.SetCVal_s("write.step.unit", "MM")
    writer = STEPControl_Writer()
    writer.Transfer(shape, STEPControl_AsIs)
    if writer.Write(str(destination)) != IFSelect_RetDone:
        raise RuntimeError("fixture export failed")
    destination.write_text(re.sub(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}", "2026-11-08T00:00:00", destination.read_text()))


if __name__ == "__main__":
    generate(Path(__file__).resolve().parents[1] / "fixtures/geometry/analysis-hook.step")
