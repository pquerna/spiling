# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Independent, optional CalculiX C3D8 reference on the analytic axial coupon."""

from pathlib import Path
import re
import numpy as np
from native import run


def axial_ccx(executable: Path, folder: Path):
    folder.mkdir(parents=True, exist_ok=False)
    # CalculiX node order, independent of scikit-fem's ordering.
    nodes = [
        (0, 0, 0),
        (10, 0, 0),
        (10, 10, 0),
        (0, 10, 0),
        (0, 0, 20),
        (10, 0, 20),
        (10, 10, 20),
        (0, 10, 20),
    ]
    lines = ["*NODE"] + [f"{i + 1}, {x}, {y}, {z}" for i, (x, y, z) in enumerate(nodes)]
    lines += [
        "*ELEMENT, TYPE=C3D8, ELSET=BODY",
        "1, 1,2,3,4,5,6,7,8",
        "*MATERIAL, NAME=ASSUMED_PLA",
        "*ELASTIC",
        "2300, .35",
        "*SOLID SECTION, ELSET=BODY, MATERIAL=ASSUMED_PLA",
        "*NSET, NSET=TOP",
        "5,6,7,8",
        "*NSET, NSET=BOTTOM",
        "1,2,3,4",
        "*BOUNDARY",
        "BOTTOM, 3,3,0",
        "1,1,2,0",
        "2,2,2,0",
        "*STEP",
        "*STATIC",
        "*BOUNDARY",
        "TOP,3,3,-.02",
        "*NODE PRINT, NSET=TOP, TOTALS=YES",
        "RF",
        "*END STEP",
    ]
    (folder / "axial.inp").write_text("\n".join(lines) + "\n")
    run([executable, "-i", "axial"], cwd=folder, timeout=60, output=folder / "solver")
    text = (folder / "axial.dat").read_text()
    # Sum the four top-node reactions, rather than trusting a status string.
    match = re.search(r"forces.*?\n\s*\n(.*?)(?:\n\s*\n)", text, re.S | re.I)
    if not match:
        raise ValueError("missing independent reaction output")
    values = []
    for line in match.group(1).splitlines():
        fields = line.split()
        if len(fields) == 4 and fields[0].isdigit():
            values.append(float(fields[3]))
    if len(values) != 4:
        raise ValueError(f"missing top reactions: {text}")
    reaction = -sum(values)
    if not np.isclose(reaction, 230.0, rtol=1e-7):
        raise ValueError(f"CalculiX axial oracle failed: {reaction}")
    return {
        "solver": "CalculiX C3D8",
        "reaction_n": reaction,
        "expected_n": 230.0,
        "relative_error": abs(reaction - 230) / 230,
    }


def bending_ccx(executable: Path, folder: Path, mesh):
    """Same nontrivial replay-derived Q1 mesh, independently assembled by ccx."""
    from skfem import ElementHex1

    folder.mkdir(parents=True, exist_ok=False)
    p = mesh.points
    lo, hi = mesh.bounds
    extent = hi - lo
    standard = np.array(
        [
            (0, 0, 0),
            (1, 0, 0),
            (1, 1, 0),
            (0, 1, 0),
            (0, 0, 1),
            (1, 0, 1),
            (1, 1, 1),
            (0, 1, 1),
        ]
    )
    order = [
        int(np.flatnonzero(np.all(ElementHex1().doflocs == c, axis=1))[0])
        for c in standard
    ]
    lines = ["*NODE"] + [
        f"{i + 1}, {x:.12g}, {y:.12g}, {z:.12g}" for i, (x, y, z) in enumerate(p.T)
    ]
    lines += ["*ELEMENT, TYPE=C3D8, ELSET=BODY"]
    lines += [
        str(i + 1) + ", " + ", ".join(str(int(j) + 1) for j in cell[order])
        for i, cell in enumerate(mesh.cells.T)
    ]
    lines += [
        "*MATERIAL, NAME=ASSUMED_PLA",
        "*ELASTIC",
        "2300, .35",
        "*SOLID SECTION, ELSET=BODY, MATERIAL=ASSUMED_PLA",
    ]
    bottom = np.flatnonzero(np.isclose(p[2], lo[2], atol=1e-8, rtol=0))
    top = np.flatnonzero(np.isclose(p[2], hi[2], atol=1e-8, rtol=0))
    center = (lo[0] + hi[0]) / 2
    span = 0.75 * extent[0]
    width = max(mesh.spacing[0] * 1.1, 0.05 * extent[0])
    left = bottom[abs(p[0, bottom] - (center - span / 2)) <= width]
    right = bottom[abs(p[0, bottom] - (center + span / 2)) <= width]
    loaded = top[abs(p[0, top] - center) <= width]
    lines += ["*NSET, NSET=LOAD"]
    for start in range(0, len(loaded), 16):
        lines.append(", ".join(str(int(i) + 1) for i in loaded[start : start + 16]))
    lines += ["*BOUNDARY"] + [f"{int(i) + 1},3,3,0" for i in np.r_[left, right]]
    a = int(left[np.argmin(p[1, left])])
    b = int(right[np.argmin(p[1, right])])
    lines += [
        f"{a + 1},1,2,0",
        f"{b + 1},2,2,0",
        "*STEP",
        "*STATIC",
        "*BOUNDARY",
        f"LOAD,3,3,{-float(extent[2]) * 0.001:.12g}",
        "*NODE PRINT, NSET=LOAD, TOTALS=YES",
        "RF",
        "*END STEP",
    ]
    (folder / "bending.inp").write_text("\n".join(lines) + "\n")
    run(
        [executable, "-i", "bending"], cwd=folder, timeout=120, output=folder / "solver"
    )
    text = (folder / "bending.dat").read_text()
    match = re.search(r"forces.*?\n\s*\n(.*?)(?:\n\s*\n)", text, re.S | re.I)
    if not match:
        raise ValueError("missing independent reaction output")
    values = []
    for line in match.group(1).splitlines():
        fields = line.split()
        if len(fields) == 4 and fields[0].isdigit():
            values.append(float(fields[3]))
    if len(values) != len(loaded):
        raise ValueError("incomplete independent fixture reactions")
    return {
        "solver": "CalculiX C3D8",
        "stiffness_n_mm": -sum(values) / (0.001 * extent[2]),
        "elements": mesh.count,
        "loaded_nodes": len(loaded),
    }
