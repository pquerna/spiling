# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0

"""Tooling-only independent original STEP sources; never a runtime CAD backend."""
import argparse
import hashlib
import json
from pathlib import Path
import tempfile

import OCP
from OCP.APIHeaderSection import APIHeaderSection_MakeHeader
from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut
from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox, BRepPrimAPI_MakeCylinder
from OCP.gp import gp_Ax2, gp_Dir, gp_Pnt
from OCP.IFSelect import IFSelect_RetDone
from OCP.Interface import Interface_Static
from OCP.STEPControl import STEPControl_AsIs, STEPControl_Writer
from OCP.TCollection import TCollection_HAsciiString

NOTICE = "SPDX-FileCopyrightText: 2026 Spiling contributors\nSPDX-License-Identifier: OSL-3.0\nLicensed under the Open Software License version 3.0\n\nOriginal Spiling analytic inputs exported by independent OCCT tooling; no third-party CAD source.\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if OCP.__version__ != "7.9.3.1":
        raise RuntimeError("recipe requires cadquery-ocp==7.9.3.1")
    root = Path(__file__).resolve().parent
    Interface_Static.SetCVal_s("write.step.schema", "AP214IS")
    Interface_Static.SetCVal_s("write.step.unit", "MM")
    Interface_Static.SetIVal_s("write.step.assembly", 0)
    hole_box = BRepPrimAPI_MakeBox(20.0, 20.0, 8.0).Shape()
    hole_cutter = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(10.0, 10.0, -1.0), gp_Dir(0.0, 0.0, 1.0)), 3.0, 10.0).Shape()
    cut = BRepAlgoAPI_Cut(hole_box, hole_cutter)
    if not cut.IsDone():
        raise RuntimeError("original OCCT through-hole construction failed")
    shapes = [
        ("occt-box-mm.step", BRepPrimAPI_MakeBox(20.0, 10.0, 8.0).Shape()),
        ("occt-cylinder-mm.step", BRepPrimAPI_MakeCylinder(5.0, 8.0).Shape()),
        ("occt-through-hole-mm.step", cut.Shape()),
    ]
    files = {}
    fixtures = []
    with tempfile.TemporaryDirectory(prefix="spiling-independent-step-") as temporary:
        for name, shape in shapes:
            if not BRepCheck_Analyzer(shape).IsValid():
                raise RuntimeError("independent OCCT source solid is invalid: " + name)
            writer = STEPControl_Writer()
            if writer.Transfer(shape, STEPControl_AsIs) != IFSelect_RetDone:
                raise RuntimeError("independent STEP transfer failed: " + name)
            header = APIHeaderSection_MakeHeader(writer.Model())
            header.SetName(TCollection_HAsciiString(name))
            header.SetTimeStamp(TCollection_HAsciiString("2026-01-01T00:00:00"))
            header.Apply(writer.Model())
            path = Path(temporary) / name
            if writer.Write(str(path)) != IFSelect_RetDone:
                raise RuntimeError("independent STEP write failed: " + name)
            data = path.read_bytes()
            files[name] = data
            files[name + ".license"] = NOTICE.encode()
            fixtures.append({"path": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    manifest = {"schema_version": 1, "exporter": {"bindings": "cadquery-ocp", "version": OCP.__version__, "schema": "AP214IS", "unit": "millimetre"}, "purpose": "Original independent-exporter admission diagnostics; outcomes recorded in geometry evidence.", "fixtures": fixtures}
    files["manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    files["manifest.json.license"] = NOTICE.encode()
    for name, data in files.items():
        target = root / name
        if args.check:
            if target.read_bytes() != data:
                raise RuntimeError("independent original corpus drift: " + name)
        elif target.exists() and name.endswith(".step"):
            if target.read_bytes() != data:
                raise RuntimeError("frozen independent STEP changed; introduce a new fixture version: " + name)
        else:
            target.write_bytes(data)
    print("three original independently exported STEP fixtures " + ("match" if args.check else "generated"))


if __name__ == "__main__":
    main()
