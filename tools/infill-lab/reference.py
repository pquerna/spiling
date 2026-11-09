# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Independent, isolated CalculiX element qualification over unchanged geometry.

Research only: prescribed fixtures, perfectly bonded nominal-footprint material,
linear elasticity; no strength, buckling, physical interfaces or Core authority.
"""

from __future__ import annotations

import json
import math
import os
from pathlib import Path
import re
import resource
import subprocess
import time

import numpy as np

from geometry import connected_meshes, prism_mesh, mesh_from_prisms, mesh_from_voxels
from mechanics import elastic_tensor
from model import Material, file_hash

JUDGE = (
    "reference.py",
    "geometry.py",
    "mechanics.py",
    "model.py",
    "coupon.py",
    "requirements.lock",
)
WEDGE_EDGES = ((0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (0, 3), (1, 4), (2, 5))


def element_mesh(mesh, element):
    """Elevate existing edges by node identity, never weld coincident components."""
    if mesh.cell_kind == "tet" and element == "C3D10":
        cells = mesh.cells.T.copy()
        points = mesh.points.T
        determinant = np.linalg.det(points[cells[:, 1:]] - points[cells[:, :1]])
        if np.any(~np.isfinite(determinant)) or np.any(determinant <= 0):
            raise ValueError("positive finite affine tetrahedra required")
        edges = ((0, 1), (1, 2), (2, 0), (0, 3), (1, 3), (2, 3))
        return elevated_edges(points, cells, edges)
    if mesh.cell_kind == "wedge" and element in ("C3D6", "C3D15", "C3D10"):
        cells = mesh.cells.T.copy()
        p = mesh.points.T
        determinant = np.einsum(
            "ij,ij->i",
            np.cross(p[cells[:, 1]] - p[cells[:, 0]], p[cells[:, 2]] - p[cells[:, 0]]),
            p[cells[:, 3]] - p[cells[:, 0]],
        )
        if np.any(abs(determinant) < 1e-20):
            raise ValueError("degenerate reference prism")
        reverse = determinant < 0
        cells[reverse] = cells[reverse][:, [0, 2, 1, 3, 5, 4]]
        if element == "C3D6":
            return p, cells
        edges = WEDGE_EDGES
        if element == "C3D10":
            # A shared vertical face always uses the smaller lower vertex to
            # larger upper vertex diagonal. Neighboring prisms therefore agree.
            base_order = np.argsort(cells[:, :3], axis=1)
            bottom = np.take_along_axis(cells[:, :3], base_order, axis=1)
            top = np.take_along_axis(cells[:, 3:], base_order, axis=1)
            a, b, c = bottom.T
            aa, bb, cc = top.T
            cells = np.stack(
                [
                    np.stack(t, axis=1)
                    for t in ((a, b, c, cc), (a, b, bb, cc), (a, aa, bb, cc))
                ],
                axis=1,
            ).reshape(-1, 4)
            determinants = np.einsum(
                "ij,ij->i",
                np.cross(
                    p[cells[:, 1]] - p[cells[:, 0]], p[cells[:, 2]] - p[cells[:, 0]]
                ),
                p[cells[:, 3]] - p[cells[:, 0]],
            )
            reverse = determinants < 0
            cells[reverse] = cells[reverse][:, [0, 2, 1, 3]]
            edges = ((0, 1), (1, 2), (2, 0), (0, 3), (1, 3), (2, 3))
        return elevated_edges(p, cells, edges)
    if mesh.cell_kind == "hex" and element == "C3D8I":
        from skfem import ElementHex1

        corners = [
            (0, 0, 0),
            (1, 0, 0),
            (1, 1, 0),
            (0, 1, 0),
            (0, 0, 1),
            (1, 0, 1),
            (1, 1, 1),
            (0, 1, 1),
        ]
        order = [
            int(np.flatnonzero(np.all(ElementHex1().doflocs == c, axis=1))[0])
            for c in corners
        ]
        return mesh.points.T.copy(), mesh.cells[order].T.copy()
    raise ValueError("element does not match preserved mesh geometry")


def elevated_edges(points, cells, edges):
    pairs = np.sort(np.stack([cells[:, edge] for edge in edges], axis=1), axis=2)
    unique, inverse = np.unique(pairs.reshape(-1, 2), axis=0, return_inverse=True)
    mids = points[unique].mean(axis=1)
    return np.vstack([points, mids]), np.hstack(
        [cells, len(points) + inverse.reshape(-1, len(edges))]
    )


def beam_mesh(nx, ny=1, nz=1, *, element="C3D15", size=(8.0, 0.45, 0.2)):
    if any(type(n) is not int or not 1 <= n <= 256 for n in (nx, ny, nz)):
        raise ValueError("invalid bounded beam refinement")
    length, width, height = size
    if not all(math.isfinite(v) and v > 0 for v in size):
        raise ValueError("invalid beam dimensions")
    x, y, z = (
        np.linspace(0, length, nx + 1),
        np.linspace(-width / 2, width / 2, ny + 1),
        np.linspace(-height / 2, height / 2, nz + 1),
    )
    if element == "C3D8I":
        return mesh_from_voxels(np.ones((nx, ny, nz), bool), (x, y, z))
    xy = np.array([[a, b] for a in x for b in y])
    triangles = []
    for i in range(nx):
        for j in range(ny):
            a = i * (ny + 1) + j
            b = a + ny + 1
            triangles.extend(((a, b, b + 1), (a, b + 1, a + 1)))
    return mesh_from_prisms(
        xy, np.asarray(triangles), z, np.ones((len(triangles), nz), bool)
    )


def fixture(points, case, bounds, material):
    bounds = np.asarray(bounds)
    lo, hi = bounds
    conditions, loaded = {}, []

    def at(axis, value):
        return np.flatnonzero(np.isclose(points[:, axis], value, rtol=0, atol=1e-8))

    if case == "pure_bending":
        kappa = 0.001
        for node in np.r_[at(0, lo[0]), at(0, hi[0])]:
            x, y, z = points[node]
            values = (
                -kappa * x * z,
                material.poisson * kappa * y * z,
                0.5 * kappa * (x * x + material.poisson * (z * z - y * y)),
            )
            for axis, value in enumerate(values):
                conditions[(int(node), axis)] = value
        return conditions, [], kappa
    if case in {"cantilever_x_y", "cantilever_x_z", "cantilever_z_x", "cantilever_z_y"}:
        a = "xyz".index(case.split("_")[1])
        t = "xyz".index(case.split("_")[2])
        left = at(a, lo[a])
        right = at(a, hi[a])
        if len(left) == 0 or len(right) == 0:
            raise ValueError("reference cantilever does not span both end patches")
        delta = 0.001 * (hi[t] - lo[t])
        conditions.update({(int(n), component): 0. for n in left for component in range(3)})
        conditions.update({(int(n), t): -delta for n in right})
        return conditions, [(int(n), t) for n in right], delta
    if case not in ("compression_x", "compression_y", "compression_z"):
        raise ValueError("unknown independent reference fixture")
    axis = "xyz".index(case[-1])
    lower, upper = at(axis, lo[axis]), at(axis, hi[axis])
    if not len(lower) or not len(upper):
        raise ValueError("reference component does not span datum faces")
    delta = 0.001 * (hi[axis] - lo[axis])
    conditions.update({(int(i), axis): 0.0 for i in lower})
    conditions.update({(int(i), axis): -delta for i in upper})
    loaded = [(int(i), axis) for i in upper]
    tangent = [a for a in range(3) if a != axis]
    a = int(lower[np.argmin(points[lower, tangent[0]] + points[lower, tangent[1]])])
    b = int(lower[np.argmax(points[lower, tangent[0]])])
    for t in tangent:
        conditions[(a, t)] = 0.0
    conditions[(b, tangent[1])] = 0.0
    return conditions, loaded, delta


def deck(mesh, element, material, case, bounds):
    points, cells = element_mesh(mesh, element)
    conditions, loaded, amplitude = fixture(points, case, bounds, material)
    tensor = elastic_tensor(material)
    constants = [
        tensor[0, 0],
        tensor[0, 1],
        tensor[1, 1],
        tensor[0, 2],
        tensor[1, 2],
        tensor[2, 2],
        tensor[5, 5],
        tensor[4, 4],
        tensor[3, 3],
    ]
    nodes = sorted({n for n, _ in conditions})
    lines = ["*NODE"] + [
        f"{i + 1}," + ",".join(format(float(v), ".12e") for v in p)
        for i, p in enumerate(points)
    ]
    lines += [f"*ELEMENT,TYPE={element},ELSET=BODY"] + [
        f"{i + 1}," + ",".join(str(int(v) + 1) for v in c) for i, c in enumerate(cells)
    ]
    lines += [
        "*MATERIAL,NAME=ASSUMED",
        "*ELASTIC,TYPE=ORTHO",
        ",".join(format(v, ".12e") for v in constants[:8]),
        format(constants[8], ".12e"),
        "*SOLID SECTION,ELSET=BODY,MATERIAL=ASSUMED",
        "*NSET,NSET=FIXTURE",
    ]
    lines += [
        ",".join(str(n + 1) for n in nodes[i : i + 16])
        for i in range(0, len(nodes), 16)
    ]
    lines += ["*STEP", "*STATIC", "*BOUNDARY"]
    lines += [
        f"{n + 1},{a + 1},{a + 1},{v:.12e}" for (n, a), v in sorted(conditions.items())
    ]
    lines += [
        "*NODE PRINT,NSET=FIXTURE",
        "RF",
        "*EL PRINT,ELSET=BODY,TOTALS=ONLY",
        "ELSE,EVOL",
        "*END STEP",
    ]
    return (
        "\n".join(lines) + "\n",
        conditions,
        loaded,
        amplitude,
        len(points),
        len(cells),
    )


def parse_dat(text, conditions):
    match = re.search(
        r"forces \(fx,fy,fz\) for set FIXTURE.*?\n\s*\n(.*?)(?:\n\s*\n|\Z)",
        text,
        re.S | re.I,
    )
    if not match:
        raise ValueError("missing independent fixture force output")
    forces = {}
    for line in match.group(1).splitlines():
        fields = line.split()
        if len(fields) != 4 or not fields[0].isdigit():
            raise ValueError("malformed independent fixture force")
        node = int(fields[0]) - 1
        if node in forces:
            raise ValueError("duplicate independent fixture node")
        forces[node] = np.array([float(v.replace("D", "E")) for v in fields[1:]])
    if set(forces) != {n for n, _ in conditions} or not all(
        np.all(np.isfinite(v)) for v in forces.values()
    ):
        raise ValueError("incomplete/nonfinite independent reactions")

    def total(label):
        m = re.search(
            r"total " + label + r".*?\n\s*\n\s*([-+0-9.EeDd]+)", text, re.S | re.I
        )
        if not m:
            raise ValueError("missing independent total " + label)
        v = float(m.group(1).replace("D", "E"))
        if not math.isfinite(v) or v <= 0:
            raise ValueError("invalid independent total " + label)
        return v

    return forces, total("internal energy"), total("volume")


def solve_reference(
    mesh, folder, *, element, material, case, bounds, executable="ccx", seconds=600
):
    if mesh.components != 1:
        raise ValueError("reference solve requires one existing connected component")
    folder.mkdir(parents=True, exist_ok=False)
    began = time.monotonic()
    text, conditions, loaded, amplitude, nodes, cells = deck(
        mesh, element, material, case, bounds
    )
    path = folder / "reference.inp"
    path.write_text(text)
    with (folder / "solver.log").open("w") as log:
        completed = subprocess.run(
            [str(executable), "-i", "reference"],
            cwd=folder,
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout=seconds,
            env={
                **os.environ,
                "OMP_NUM_THREADS": os.environ.get("OMP_NUM_THREADS", "1"),
            },
        )
    log = (folder / "solver.log").read_text()
    if completed.returncode or "*ERROR" in log or "Job finished" not in log:
        raise RuntimeError("independent solver did not complete; retain solver.log")
    forces, energy, volume = parse_dat(
        (folder / "reference.dat").read_text(), conditions
    )
    work = sum(forces[n][a] * v for (n, a), v in conditions.items())
    norm = sum(np.linalg.norm(v) for v in forces.values())
    balance = float(np.linalg.norm(sum(forces.values())) / max(norm, 1e-30))
    energy_error = abs(2 * energy - work) / max(abs(work), 1e-30)
    volume_error = abs(volume / mesh.volume - 1)
    # DAT writes ~7 significant digits. This is an explicit independent output
    # precision gate, not a relaxation of the existing FP64 coupon judge.
    if balance > 1e-5 or energy_error > 1e-5 or volume_error > 1e-5 or work <= 0:
        raise ValueError(
            f"reference physical gates failed: force={balance}, energy={energy_error}, volume={volume_error}"
        )
    result = dict(
        case=case,
        element=element,
        nodes=nodes,
        elements=cells,
        internal_energy_n_mm=energy,
        boundary_work_n_mm=work,
        force_balance=balance,
        energy_error=energy_error,
        volume_relative_error=volume_error,
        mesh_volume_mm3=mesh.volume,
        input_sha256=file_hash(path),
        wall_s=time.monotonic() - began,
        cumulative_children_peak_rss_kib=resource.getrusage(
            resource.RUSAGE_CHILDREN
        ).ru_maxrss,
    )
    if loaded:
        reaction = -sum(forces[n][a] for n, a in loaded)
        if reaction <= 0:
            raise ValueError("nonpositive independent reaction")
        result.update(reaction_n=reaction, stiffness_n_mm=reaction / amplitude)
    else:
        lo, hi = np.asarray(bounds)
        length, width, height = hi - lo
        expected = (
            0.5 * material.young_mpa * amplitude**2 * width * height**3 * length / 12
        )
        result.update(
            analytical_energy_n_mm=expected,
            analytical_relative_error=abs(energy / expected - 1),
        )
    return result


def evaluate(folder, output):
    spec = json.loads((folder / "case.json").read_text())
    if spec.get("schema") != 1 or spec.get("kind") != "element_reference":
        raise ValueError("unknown independent element input")
    here = Path(__file__).resolve().parent
    if spec["judge"] != {name: file_hash(here / name) for name in JUDGE}:
        raise ValueError("independent element judge differs")
    if spec["kind_source"] == "actual_roads":
        if file_hash(folder / "roads.json") != spec["roads_sha256"]:
            raise ValueError("independent replay road bytes differ")
        data = json.loads((folder / "roads.json").read_text())
        if not data["report"]["verified"] or not data["software_only"]:
            raise ValueError("actual reference lacks native replay")
        if (
            data["source_sha256"] != spec["source_sha256"]
            or data["candidate"] != spec["candidate"]
        ):
            raise ValueError(
                "reference candidate/source differs from frozen native input"
            )
        if spec["scope"] not in ("interior", "whole_object"):
            raise ValueError("unknown reference deposition scope")
        from coupon import interior_roads

        roads = interior_roads(data) if spec["scope"] == "interior" else data["roads"]
    elif spec["kind_source"] != "analytical_beam":
        raise ValueError("unknown independent geometry source")
    rows = []
    for i, scenario in enumerate(spec["scenarios"]):
        row = dict(
            scenario=scenario,
            input_sha256=file_hash(folder / "case.json"),
            judge=spec["judge"],
        )
        try:
            material = Material(**scenario["material"])
            if spec["kind_source"] == "analytical_beam":
                mesh = beam_mesh(**scenario["refinement"], element=scenario["element"])
                if not np.allclose(mesh.bounds, scenario["bounds"], rtol=0, atol=1e-10):
                    raise ValueError("analytical beam datums differ from geometry")
                parts = [mesh]
            else:
                mesh, info = prism_mesh(
                    roads, window=scenario["bounds"], **scenario["refinement"]
                )
                parts = [
                    part
                    for part in connected_meshes(mesh)
                    if np.allclose(
                        part.bounds[:, "xyz".index(scenario["case"][-1])],
                        np.asarray(scenario["bounds"])[
                            :, "xyz".index(scenario["case"][-1])
                        ],
                        rtol=0,
                        atol=1e-8,
                    )
                ]
                row["mesh"] = info
            results = [
                solve_reference(
                    part,
                    output / f"case-{i}-component-{j}",
                    element=scenario["element"],
                    material=material,
                    case=scenario["case"],
                    bounds=scenario["bounds"],
                    seconds=scenario.get("seconds", 600),
                )
                for j, part in enumerate(parts)
            ]
            row.update(status="ok", components=results, spanning_components=len(parts))
            if scenario["case"] != "pure_bending":
                row["stiffness_n_mm"] = sum(r["stiffness_n_mm"] for r in results)
                row["mechanism"] = (
                    "elastic_load_path" if parts else "no_component_spans_fixture_faces"
                )
        except Exception as error:
            row.update(
                status="failed", error_type=type(error).__name__, error=str(error)
            )
        rows.append(row)
        with (output / "trials.jsonl").open("a") as stream:
            stream.write(json.dumps(row, allow_nan=False) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        print(json.dumps(row, allow_nan=False), flush=True)
    return dict(
        status="ok" if rows and all(r["status"] == "ok" for r in rows) else "failed",
        scope="independent_nominal_linear_elastic_element_reference_not_strength",
        trials=len(rows),
        accepted=sum(r["status"] == "ok" for r in rows),
    )
