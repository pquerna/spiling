# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0

"""Tooling-only independent original STEP sources; never a runtime CAD backend."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import tempfile

import OCP
from OCP.APIHeaderSection import APIHeaderSection_MakeHeader
from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut
from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepBuilderAPI import BRepBuilderAPI_NurbsConvert
from OCP.BRepFilletAPI import BRepFilletAPI_MakeFillet
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox, BRepPrimAPI_MakeCone, BRepPrimAPI_MakeCylinder
from OCP.gp import gp_Ax2, gp_Dir, gp_Pnt
from OCP.IFSelect import IFSelect_RetDone
from OCP.Interface import Interface_Static
from OCP.STEPControl import STEPControl_AsIs, STEPControl_Writer
from OCP.TCollection import TCollection_HAsciiString
from OCP.TopAbs import TopAbs_EDGE
from OCP.TopExp import TopExp
from OCP.TopoDS import TopoDS
from OCP.TopTools import TopTools_IndexedMapOfShape

NOTICE = "SPDX-FileCopyrightText: 2026 Spiling contributors\nSPDX-License-Identifier: OSL-3.0\nLicensed under the Open Software License version 3.0\n\nOriginal Spiling analytic inputs exported by independent OCCT tooling; no third-party CAD source.\n"


def mechanical_shapes():
    box = BRepPrimAPI_MakeBox(24.0, 18.0, 10.0).Shape()
    edges = TopTools_IndexedMapOfShape()
    TopExp.MapShapes_s(box, TopAbs_EDGE, edges)
    fillet = BRepFilletAPI_MakeFillet(box)
    for index in range(1, edges.Extent() + 1):
        fillet.Add(2.0, TopoDS.Edge_s(edges.FindKey(index)))
    fillet.Build()
    if not fillet.IsDone():
        raise RuntimeError("original rounded housing fillets failed")
    pocket = BRepPrimAPI_MakeCylinder(
        gp_Ax2(gp_Pnt(12.0, 9.0, 4.13), gp_Dir(0.0, 0.0, 1.0)), 3.0, 7.0
    ).Shape()
    housing = BRepAlgoAPI_Cut(fillet.Shape(), pocket)
    fitting = BRepAlgoAPI_Cut(
        BRepPrimAPI_MakeCone(6.0, 4.0, 12.0).Shape(),
        BRepPrimAPI_MakeCylinder(
            gp_Ax2(gp_Pnt(0.0, 0.0, -1.0), gp_Dir(0.0, 0.0, 1.0)), 2.0, 14.0
        ).Shape(),
    )
    if not housing.IsDone() or not fitting.IsDone():
        raise RuntimeError("original mechanical-part subtraction failed")
    spline_housing = BRepBuilderAPI_NurbsConvert(housing.Shape(), True)
    narrow_pocket = BRepAlgoAPI_Cut(
        BRepPrimAPI_MakeBox(12.0, 10.0, 6.0).Shape(),
        BRepPrimAPI_MakeBox(gp_Pnt(2.0, 4.7, 1.07), 8.0, 0.6, 5.0).Shape(),
    )
    if not narrow_pocket.IsDone():
        raise RuntimeError("original narrow pocket subtraction failed")
    return [
        ("occt-tapered-fitting-mm.step", fitting.Shape()),
        ("occt-rounded-housing-mm.step", housing.Shape()),
        ("occt-spline-housing-mm.step", spline_housing.Shape()),
        ("occt-narrow-pocket-mm.step", narrow_pocket.Shape()),
    ]


def mechanical_oracle(name):
    if name == "occt-tapered-fitting-mm.step":
        return {
            "nominal_bounds_mm": [[-6.0, -6.0, 0.0], [6.0, 6.0, 12.0]],
            "sections": [
                {"z_mm": z, "material_area_mm2": math.pi * ((6.0 - z / 6.0) ** 2 - 4.0), "loop_count": 2}
                for z in [1.0, 6.0, 11.0]
            ],
        }
    if name == "occt-narrow-pocket-mm.step":
        return {
            "nominal_bounds_mm": [[0.0, 0.0, 0.0], [12.0, 10.0, 6.0]],
            "pocket_floor_z_mm": 1.07,
            "pocket_width_mm": 0.6,
            "sections": [
                {"z_mm": 1.0, "material_area_mm2": 120.0, "loop_count": 1},
                {"z_mm": 1.1, "material_area_mm2": 115.2, "loop_count": 2},
                {"z_mm": 5.0, "material_area_mm2": 115.2, "loop_count": 2},
            ],
        }
    if name in {"occt-rounded-housing-mm.step", "occt-spline-housing-mm.step"}:
        sections = []
        for z in [0.5, 4.1, 4.16, 9.5]:
            q = math.sqrt(4.0 - (2.0 - min(z, 10.0 - z, 2.0)) ** 2)
            pocket_area = 9.0 * math.pi if z > 4.13 else 0.0
            sections.append({
                "z_mm": z,
                "material_area_mm2": 280.0 + 68.0 * q + math.pi * q * q - pocket_area,
                "loop_count": 2 if pocket_area else 1,
            })
        return {"nominal_bounds_mm": [[0.0, 0.0, 0.0], [24.0, 18.0, 10.0]], "pocket_floor_z_mm": 4.13, "sections": sections}
    return None


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
    shapes.extend(mechanical_shapes())
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
            oracle = mechanical_oracle(name)
            if oracle is not None:
                fixtures[-1]["analytic_oracle"] = oracle
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
    print(str(len(fixtures)) + " original independently exported STEP fixtures " + ("match" if args.check else "generated"))


if __name__ == "__main__":
    main()
