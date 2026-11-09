# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0

"""Original box with coplanar side-face ownership changing at Z=2 mm."""
from pathlib import Path
import re
from OCP.BRepAlgoAPI import BRepAlgoAPI_Fuse
from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox
from OCP.gp import gp_Pnt
from OCP.IFSelect import IFSelect_RetDone
from OCP.Interface import Interface_Static
from OCP.STEPControl import STEPControl_Writer, STEPControl_AsIs


def generate(destination):
    if destination.exists():
        raise ValueError("frozen original source already exists")
    shape = BRepAlgoAPI_Fuse(
        BRepPrimAPI_MakeBox(20.0, 10.0, 2.0).Shape(),
        BRepPrimAPI_MakeBox(gp_Pnt(0.0, 0.0, 2.0), 20.0, 10.0, 4.0).Shape(),
    ).Shape()
    if not BRepCheck_Analyzer(shape).IsValid():
        raise RuntimeError("invalid original split-height box")
    Interface_Static.SetCVal_s("write.step.schema", "AP214IS")
    Interface_Static.SetCVal_s("write.step.unit", "MM")
    writer = STEPControl_Writer()
    writer.Transfer(shape, STEPControl_AsIs)
    if writer.Write(str(destination)) != IFSelect_RetDone:
        raise RuntimeError("fixture export failed")
    destination.write_text(re.sub(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}", "2026-11-09T00:00:00", destination.read_text()))


if __name__ == "__main__":
    generate(Path(__file__).resolve().parents[1] / "fixtures/geometry/analysis-height-split-box.step")
