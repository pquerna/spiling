# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Bounded TetP2 assembly over the unchanged C3D10 reference topology.

Research elasticity only. scikit-fem owns basis/quadrature, SciPy/PyAMG own the
solve; no ideal lattice, new contact, physical strength or project authority.
"""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import resource
import time

import numpy as np
from scipy import sparse
from scipy.sparse.linalg import spsolve
from skfem import ElementTetP2
from skfem.quadrature import get_quadrature_tet

from geometry import AnalysisMesh, connected_meshes, prism_mesh
from mechanics import ElasticBody, elastic_tensor, solve
from model import Material, file_hash
from reference import beam_mesh, element_mesh, fixture

JUDGE = (
    "quadratic.py",
    "quadratic_preconditioner.py",
    "reference.py",
    "geometry.py",
    "mechanics.py",
    "model.py",
    "coupon.py",
    "requirements.lock",
)


@dataclass(frozen=True)
class NodeDofs:
    """Explicit displacement DOFs for every corner and elevated edge node."""

    nodal_dofs: np.ndarray
    N: int


def kernels(material):
    element = ElementTetP2()
    x, weights = get_quadrature_tet(2)
    grad = np.array([element.lbasis(x, i)[1] for i in range(10)])
    moments = np.einsum("irq,jsq,q->rsij", grad, grad, weights)
    strain = np.zeros((6, 3, 3))
    for a in range(3):
        strain[a, a, a] = 1
    for v, (a, b) in enumerate(((1, 2), (0, 2), (0, 1)), 3):
        strain[v, a, b] = strain[v, b, a] = 1
    tensor = np.einsum("vda,vw,wfb->dafb", strain, elastic_tensor(material), strain)
    integrated = np.einsum("rsij,dafb->rasbidjf", moments, tensor).reshape(81, 900)
    # Match library basis to C3D10's existing node identities, never coordinates
    # of different physical elements. Only reference-element coordinates are used.
    corners = np.array([[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]])
    edges = ((0, 1), (1, 2), (2, 0), (0, 3), (1, 3), (2, 3))
    locations = np.vstack([corners, [corners[list(e)].mean(axis=0) for e in edges]])
    order = np.array(
        [np.flatnonzero(np.all(locations == p, axis=1)).item() for p in element.doflocs]
    )
    return integrated, order


def prepare(mesh, material=Material(), *, chunk=2048):
    if mesh.components != 1 or type(chunk) is not int or not 1 <= chunk <= 8192:
        raise ValueError(
            "one connected reference body and bounded assembly chunk required"
        )
    began = time.monotonic()
    points, cells = element_mesh(mesh, "C3D10")
    template, order = kernels(material)
    cells = cells[:, order]
    dofs = NodeDofs(np.arange(3 * len(points)).reshape(-1, 3).T, 3 * len(points))
    shape = (dofs.N, dofs.N)
    partials = []
    volume = 0.0
    for start in range(0, len(cells), chunk):
        block_cells = cells[start : start + chunk]
        corners = points[block_cells[:, :4]]
        jacobian = (corners[:, 1:] - corners[:, :1]).transpose(0, 2, 1)
        determinants = np.linalg.det(jacobian)
        if np.any(~np.isfinite(determinants)) or np.any(determinants <= 0):
            raise ValueError("nonpositive affine tetrahedron")
        volume += determinants.sum() / 6
        inverse = np.linalg.inv(jacobian)
        scales = np.einsum("era,esb,e->erasb", inverse, inverse, determinants).reshape(
            -1, 81
        )
        values = (scales @ template).reshape(-1, 30, 30)
        indices = (3 * block_cells[:, :, None] + np.arange(3)).reshape(-1, 30)
        rows = np.broadcast_to(indices[:, :, None], values.shape).ravel()
        cols = np.broadcast_to(indices[:, None, :], values.shape).ravel()
        block = sparse.coo_matrix((values.ravel(), (rows, cols)), shape=shape).tocsr()
        # Binary reduction bounds retained sparse partials and avoids repeatedly
        # adding tiny chunks to an already complete global matrix.
        level = 0
        while level < len(partials) and partials[level] is not None:
            block = partials[level] + block
            partials[level] = None
            level += 1
        if level == len(partials):
            partials.append(block)
        else:
            partials[level] = block
    if not np.isclose(volume, mesh.volume, rtol=1e-10, atol=1e-12):
        raise ValueError("quadratic topology changed footprint volume")
    matrix = sparse.csr_matrix(shape)
    for block in partials:
        if block is not None:
            matrix += block
    matrix = matrix.tobsr(blocksize=(3, 3))
    lifted = AnalysisMesh(
        points.T,
        cells.T,
        float(volume),
        len(cells),
        1,
        mesh.bounds.copy(),
        mesh.spacing,
        mesh.grid_shape,
        "tet_p2",
    )
    return ElasticBody(lifted, material, dofs, matrix, time.monotonic() - began)


def solve_body(body, case, bounds, *, solver="amg"):
    if case != "pure_bending":
        if solver == "p_amg":
            from quadratic_preconditioner import auxiliary_preconditioner

            factory = auxiliary_preconditioner(body, kernels(body.material)[1])
        else:
            factory = None
        result = solve(
            body.mesh,
            case,
            body.material,
            fixture_bounds=bounds,
            prepared=body,
            solver="amg" if solver == "p_amg" else solver,
            preconditioner_factory=factory,
        )
        result.update(
            linear_solver=solver,
            model="effective_nominal_footprint_TetP2_elastic_not_strength",
            assembly_wall_s=body.assembly_s,
        )
        return result
    # Tiny solved analytical control only, independent of the AMG path. Solve
    # free DOFs; prescribing the analytical field everywhere would hide locking.
    began = time.monotonic()
    conditions, _, kappa = fixture(body.mesh.points.T, case, bounds, body.material)
    fixed = np.array(sorted(3 * n + a for n, a in conditions))
    values = np.array([conditions[(int(i // 3), int(i % 3))] for i in fixed])
    free = np.setdiff1d(np.arange(body.basis.N), fixed)
    matrix = body.stiffness.tocsr()
    u = np.zeros(body.basis.N)
    u[fixed] = values
    rhs = -(matrix @ u)
    u[free] = spsolve(matrix[free][:, free], rhs[free])
    force = matrix @ u
    residual = float(
        np.linalg.norm(force[free]) / max(np.linalg.norm(rhs[free]), 1e-30)
    )
    energy = float(0.5 * u @ force)
    work = float(u[fixed] @ force[fixed])
    balance = float(
        np.linalg.norm(force.reshape(-1, 3).sum(axis=0)) / np.linalg.norm(force)
    )
    energy_error = abs(2 * energy / work - 1)
    length, width, height = np.diff(np.asarray(bounds), axis=0)[0]
    expected = body.material.young_mpa * kappa**2 * width * height**3 * length / 24
    if residual > 2e-8 or balance > 1e-6 or energy_error > 1e-6 or energy <= 0:
        raise ValueError("unconverged solved quadratic beam control")
    return dict(
        case=case,
        strain_energy_n_mm=energy,
        analytical_relative_error=abs(energy / expected - 1),
        relative_residual=residual,
        force_balance=balance,
        energy_error=energy_error,
        assembly_wall_s=body.assembly_s,
        wall_s=time.monotonic() - began,
        dof=body.basis.N,
        elements=body.mesh.count,
        linear_solver="analytical_control_direct",
    )


def evaluate(folder, output):
    spec = json.loads((folder / "case.json").read_text())
    here = Path(__file__).resolve().parent
    if (
        spec["schema"] != 1
        or spec["kind"] != "quadratic_reference"
        or spec["judge"] != {n: file_hash(here / n) for n in JUDGE}
    ):
        raise ValueError("unknown or mismatched quadratic judge")
    target_path = folder / "reference-case.json"
    if file_hash(target_path) != spec["reference_case_sha256"]:
        raise ValueError("reference input differs")
    target = json.loads(target_path.read_text())
    scenario = target["scenarios"][0]
    if len(target["scenarios"]) != 1 or scenario["element"] != "C3D10":
        raise ValueError("one unchanged C3D10 case required")
    began = time.monotonic()
    row = dict(
        input_sha256=file_hash(folder / "case.json"),
        judge=spec["judge"],
        scenario=scenario,
        role=spec["role"],
        solver=spec["solver"],
    )
    try:
        material = Material(**scenario["material"])
        if target["kind_source"] == "analytical_beam":
            mesh = beam_mesh(**scenario["refinement"], element="C3D10")
            if not np.allclose(mesh.bounds, scenario["bounds"], rtol=0, atol=1e-10):
                raise ValueError("analytical fixture differs")
            parts = [mesh]
        elif target["kind_source"] == "actual_roads":
            if file_hash(folder / "roads.json") != target["roads_sha256"]:
                raise ValueError("native road input differs")
            data = json.loads((folder / "roads.json").read_text())
            if (
                not data["report"]["verified"]
                or not data["software_only"]
                or data["source_sha256"] != target["source_sha256"]
                or data["candidate"] != target["candidate"]
                or target["scope"] != "interior"
            ):
                raise ValueError("unknown native source/replay scope")
            from coupon import interior_roads

            mesh, info = prism_mesh(
                interior_roads(data),
                window=scenario["bounds"],
                **scenario["refinement"],
            )
            axis = "xyz".index(scenario["case"][-1])
            parts = [
                p
                for p in connected_meshes(mesh)
                if np.allclose(
                    p.bounds[:, axis],
                    np.asarray(scenario["bounds"])[:, axis],
                    rtol=0,
                    atol=1e-8,
                )
            ]
            row["mesh"] = info
        else:
            raise ValueError("unknown quadratic source")
        results = []
        for part in parts:
            body = prepare(part, material)
            print(
                json.dumps(
                    dict(
                        phase="assembled_TetP2",
                        dof=body.basis.N,
                        assembly_s=body.assembly_s,
                    )
                ),
                flush=True,
            )
            results.append(
                solve_body(
                    body, scenario["case"], scenario["bounds"], solver=spec["solver"]
                )
            )
            del body
        if not results:
            raise ValueError("qualification requires a spanning component")
        row.update(status="ok", components=results)
        if scenario["case"] != "pure_bending":
            row["stiffness_n_mm"] = sum(r["stiffness_n_mm"] for r in results)
        if spec["role"] == "parity":
            path = folder / "reference-result.json"
            if file_hash(path) != spec["reference_result_sha256"]:
                raise ValueError("reference result differs")
            ref = json.loads(path.read_text())
            if (
                ref["status"] != "ok"
                or ref["scenario"] != scenario
                or ref["input_sha256"] != spec["reference_case_sha256"]
                or ref["judge"] != target["judge"]
            ):
                raise ValueError("reference result does not bind input")
            if len(ref["components"]) != len(results):
                raise ValueError("reference component inventory differs")
            for component, expected in zip(results, ref["components"]):
                if (
                    component["dof"] != 3 * expected["nodes"]
                    or component["elements"] != expected["elements"]
                    or any(
                        not 0 <= expected[k] <= 1e-5
                        for k in (
                            "force_balance",
                            "energy_error",
                            "volume_relative_error",
                        )
                    )
                ):
                    raise ValueError("reference topology/numerical gates differ")
            if "stiffness_n_mm" in row:
                error = abs(row["stiffness_n_mm"] / ref["stiffness_n_mm"] - 1)
            else:
                error = max(r["analytical_relative_error"] for r in results)
            row["reference_relative_error"] = error
            if error > 1e-5:
                raise ValueError(f"quadratic reference mismatch: {error}")
        elif spec["role"] != "refinement":
            raise ValueError("unknown quadratic evaluation role")
    except Exception as error:
        row.update(status="failed", error_type=type(error).__name__, error=str(error))
    row.update(
        wall_s=time.monotonic() - began,
        peak_rss_kib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
    )
    (output / "trials.jsonl").write_text(json.dumps(row, allow_nan=False) + "\n")
    return dict(
        status=row["status"],
        scope="quadratic_nominal_elastic_reference_not_strength",
        trials=1,
    )
