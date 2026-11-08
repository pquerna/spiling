# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Research elasticity through scikit-fem/PETSc-independent sparse libraries.

No strength/failure, physical calibration or project authority. Full-integration
Q1 hexahedra; compare with an external C3D8 solver and convergence controls.
"""

from __future__ import annotations
from dataclasses import dataclass
from functools import cached_property
import time
import os
import sys
import json
import numpy as np
from scipy.sparse.linalg import cg, splu
from scipy import sparse
from skfem.assembly import Dofs
from skfem import MeshHex, MeshWedge1, Basis, ElementHex1, ElementWedge1, ElementVector
from model import Material
from geometry import AnalysisMesh


@dataclass(frozen=True)
class ElasticBody:
    mesh: AnalysisMesh
    material: Material
    basis: object
    stiffness: object
    assembly_s: float

    @cached_property
    def direct_order(self):
        """One METIS physical-node ordering shared by loads, not by candidates."""
        import pymetis

        started = time.monotonic()
        matrix = self.stiffness
        graph = sparse.csr_matrix(
            (
                np.ones(len(matrix.indices), dtype=np.int8),
                matrix.indices.copy(),
                matrix.indptr.copy(),
            ),
            shape=(self.mesh.points.shape[1],) * 2,
        )
        graph.setdiag(0)
        graph.eliminate_zeros()
        graph.sort_indices()
        nodes, inverse = pymetis.nested_dissection(
            adjacency=pymetis.CSRAdjacency(graph.indptr, graph.indices),
            options=pymetis.Options(seed=0),
        )
        nodes = np.asarray(nodes, dtype=int)
        inverse = np.asarray(inverse, dtype=int)
        if not np.array_equal(nodes[inverse], np.arange(len(nodes))):
            raise ValueError("invalid physical-node factor ordering")
        return (3 * nodes[:, None] + np.arange(3)).reshape(
            -1
        ), time.monotonic() - started


def elastic_tensor(material):
    """Symmetric compliance; weak-Z scenarios are assumptions, not calibration.

    Engineering Voigt order xx, yy, zz, yz, xz, xy. Reciprocal Poisson ratios
    follow from symmetric compliance rather than guessing independent ratios.
    """
    e, nu, beta = material.young_mpa, material.poisson, material.z_ratio
    compliance = np.diag(
        [
            1 / e,
            1 / e,
            1 / (beta * e),
            2 * (1 + nu) / (beta * e),
            2 * (1 + nu) / (beta * e),
            2 * (1 + nu) / e,
        ]
    )
    for i, j in ((0, 1), (0, 2), (1, 2)):
        compliance[i, j] = compliance[j, i] = -nu / e
    if np.linalg.eigvalsh(compliance).min() <= 0:
        raise ValueError("material compliance is not positive definite")
    return np.linalg.inv(compliance)


def element_kernels(material):
    """scikit-fem full-integration unit-element kernels, scaled by cell metrics.

    Reuse quadrature, not a homogenized material or candidate stiffness. Each
    actual conforming cell retains its own metric and ordinary Q1 assembly.
    """
    corners = ElementHex1().doflocs.T
    mesh = MeshHex(corners, np.arange(8)[:, None], sort_t=False)
    basis = Basis(mesh, ElementVector(ElementHex1()), intorder=2)
    gradients = np.asarray([field[0].grad[:, :, 0, :] for field in basis.basis])
    tensor = elastic_tensor(material)
    strains = np.zeros((3, 24, 6, gradients.shape[-1]))
    for axis in range(3):
        strains[axis, :, axis] = gradients[:, axis, axis]
    # Engineering shear strain order yz, xz, xy.
    for row, (a, b) in enumerate(((1, 2), (0, 2), (0, 1)), start=3):
        strains[b, :, row] = gradients[:, a, b]
        strains[a, :, row] = gradients[:, b, a]
    return np.einsum("adiq,ij,bejq,q->abde", strains, tensor, strains, basis.W)


def assemble_rectilinear(mesh, material, dofs, *, chunk=32768):
    """Bounded sparse assembly avoids per-cell stored quadrature/gradient fields."""
    kernels = element_kernels(material)
    coords = mesh.points[:, mesh.cells]
    low, high = coords.min(axis=1), coords.max(axis=1)
    sizes = (high - low).T
    if np.any(sizes <= 0):
        raise ValueError("degenerate rectilinear cell")
    expected = (
        low[:, None, :] + ElementHex1().doflocs.T[:, :, None] * (high - low)[:, None, :]
    )
    if not np.allclose(coords, expected, rtol=0, atol=1e-9):
        raise ValueError("template assembly requires axis-aligned conforming cells")
    shape = (dofs.N, dofs.N)
    partials = []
    for start in range(0, mesh.count, chunk):
        stop = min(start + chunk, mesh.count)
        dims = sizes[start:stop]
        scales = np.prod(dims, axis=1)[:, None, None] / (
            dims[:, :, None] * dims[:, None, :]
        )
        values = np.einsum("eab,abij->eij", scales, kernels, optimize=True)
        indices = (
            dofs.nodal_dofs[:, mesh.cells[:, start:stop]]
            .transpose(2, 1, 0)
            .reshape(-1, 24)
        )
        rows = np.broadcast_to(indices[:, :, None], values.shape).reshape(-1)
        cols = np.broadcast_to(indices[:, None, :], values.shape).reshape(-1)
        block = sparse.coo_matrix(
            (values.reshape(-1), (rows, cols)), shape=shape
        ).tocsr()
        # Balanced reduction avoids rescanning the entire growing matrix for
        # every fixed-size chunk (quadratic assembly traffic on large bodies).
        level = 0
        while level < len(partials) and partials[level] is not None:
            block = partials[level] + block
            partials[level] = None
            level += 1
        if level == len(partials):
            partials.append(block)
        else:
            partials[level] = block
    matrix = sparse.csr_matrix(shape)
    for block in partials:
        if block is not None:
            matrix += block
    return matrix


def prism_quadrature(order=2):
    """Triangle × line rule on (x,y,z), integral of unity = 1/2.

    scikit-fem 12.0.2's RefWedge rule tensors the two triangle coordinates
    independently, giving points outside the reference triangle and weight
    1/4. Supply the correct product explicitly; never rescale a wrong rule.
    """
    from skfem.quadrature import get_quadrature
    from skfem.refdom import RefTri, RefLine

    tri, tw = get_quadrature(RefTri, order)
    line, lw = get_quadrature(RefLine, order)
    points = np.vstack([np.repeat(tri, len(lw), axis=1), np.tile(line, (1, len(tw)))])
    weights = (tw[:, None] * lw[None, :]).reshape(-1)
    return points, weights


def wedge_kernels(material):
    element = ElementWedge1()
    reference = MeshWedge1(element.doflocs.T, np.arange(6)[:, None], sort_t=False)
    basis = Basis(reference, ElementVector(element), quadrature=prism_quadrature())
    gradients = np.asarray([field[0].grad[:, :, 0, :] for field in basis.basis])
    strains = np.zeros((3, 3, 18, 6, gradients.shape[-1]))
    for r in range(3):
        for a in range(3):
            strains[r, a, :, a] = gradients[:, a, r]
        for row, (a, b) in enumerate(((1, 2), (0, 2), (0, 1)), start=3):
            strains[r, b, :, row] = gradients[:, a, r]
            strains[r, a, :, row] = gradients[:, b, r]
    return np.einsum(
        "radiq,ij,sbfjq,q->rasbdf", strains, elastic_tensor(material), strains, basis.W
    ).reshape(81, 324)


def assemble_prisms(mesh, material, dofs, *, chunk=16384):
    kernels = wedge_kernels(material)
    coords = mesh.points[:, mesh.cells]
    jac = np.stack(
        [
            coords[:, 1] - coords[:, 0],
            coords[:, 2] - coords[:, 0],
            coords[:, 3] - coords[:, 0],
        ],
        axis=-1,
    ).transpose(1, 0, 2)
    if not np.allclose(
        coords[:, 4] - coords[:, 1], coords[:, 3] - coords[:, 0], atol=1e-9, rtol=0
    ) or not np.allclose(
        coords[:, 5] - coords[:, 2], coords[:, 3] - coords[:, 0], atol=1e-9, rtol=0
    ):
        raise ValueError("only affine triangular prisms are admitted")
    determinants = abs(np.linalg.det(jac))
    if np.any(determinants <= 0):
        raise ValueError("degenerate analysis prism")
    inverse = np.linalg.inv(jac)
    shape = (dofs.N, dofs.N)
    partials = []
    for start in range(0, mesh.count, chunk):
        stop = min(start + chunk, mesh.count)
        inv = inverse[start:stop]
        scales = np.einsum(
            "era,esb,e->erasb", inv, inv, determinants[start:stop]
        ).reshape(-1, 81)
        values = (scales @ kernels).reshape(-1, 18, 18)
        indices = (
            dofs.nodal_dofs[:, mesh.cells[:, start:stop]]
            .transpose(2, 1, 0)
            .reshape(-1, 18)
        )
        rows = np.broadcast_to(indices[:, :, None], values.shape).reshape(-1)
        cols = np.broadcast_to(indices[:, None, :], values.shape).reshape(-1)
        block = sparse.coo_matrix(
            (values.reshape(-1), (rows, cols)), shape=shape
        ).tocsr()
        level = 0
        while level < len(partials) and partials[level] is not None:
            block = partials[level] + block
            partials[level] = None
            level += 1
        if level == len(partials):
            partials.append(block)
        else:
            partials[level] = block
    matrix = sparse.csr_matrix(shape)
    for block in partials:
        if block is not None:
            matrix += block
    return matrix


def prepare(mesh: AnalysisMesh, material=Material()):
    """One material/mesh matrix shared by independent loads, never by candidates."""
    if mesh.components != 1:
        raise ValueError(
            f"inscribed mesh has {mesh.components} disconnected components; refine, do not invent bonds"
        )
    began = time.monotonic()
    if mesh.cell_kind == "wedge":
        m = MeshWedge1(mesh.points, mesh.cells, sort_t=False)
        basis = Dofs(m, ElementVector(ElementWedge1()))
        stiffness = assemble_prisms(mesh, material, basis).tobsr(blocksize=(3, 3))
    else:
        m = MeshHex(mesh.points, mesh.cells, sort_t=False)
        basis = Dofs(m, ElementVector(ElementHex1()))
        stiffness = assemble_rectilinear(mesh, material, basis).tobsr(blocksize=(3, 3))
    return ElasticBody(mesh, material, basis, stiffness, time.monotonic() - began)


def solve(
    mesh: AnalysisMesh,
    case="compression_z",
    material=Material(),
    *,
    rtol=1e-8,
    maxiter=2000,
    fixture_bounds=None,
    prepared: ElasticBody | None = None,
    solver="amg",
    preconditioner_factory=None,
):
    if preconditioner_factory is not None and solver != "amg":
        raise ValueError("custom preconditioners require the iterative solver")
    np.random.seed(0)  # Freeze AMG randomized setup; this is a research solve.
    began = time.monotonic()
    reused = prepared is not None
    body = prepared if prepared is not None else prepare(mesh, material)
    if body.mesh is not mesh or body.material != material:
        raise ValueError("prepared elasticity belongs to another mesh/material")
    basis, stiffness = body.basis, body.stiffness
    p = mesh.points
    lo, hi = mesh.bounds
    datums = (
        mesh.bounds
        if fixture_bounds is None
        else np.asarray(fixture_bounds, dtype=float)
    )
    if datums.shape != (2, 3):
        raise ValueError("fixture bounds require two three-dimensional points")
    datum_lo, datum_hi = datums
    datum_extent = datum_hi - datum_lo
    if np.any(~np.isfinite(datum_extent)) or np.any(datum_extent <= 0):
        raise ValueError("invalid source fixture bounds")

    def dofs(nodes, axis):
        return basis.nodal_dofs[axis, np.asarray(nodes, dtype=int)]

    def at(axis, value):
        return np.flatnonzero(np.isclose(p[axis], value, rtol=0, atol=1e-8))

    delta = 0.001 * float(datum_extent[2])
    conditions = {}
    loaded = []
    if case.startswith("compression_"):
        axis = {"x": 0, "y": 1, "z": 2}[case[-1]]
        delta = 0.001 * float(datum_extent[axis])
        lower, upper = at(axis, lo[axis]), at(axis, hi[axis])
        conditions.update({int(i): 0.0 for i in dofs(lower, axis)})
        loaded = dofs(upper, axis)
        conditions.update({int(i): -delta for i in loaded})
        # Remove the three remaining rigid modes at two distant lower-face nodes.
        tangent = [a for a in range(3) if a != axis]
        a = int(lower[np.argmin(p[tangent[0], lower] + p[tangent[1], lower])])
        b = int(lower[np.argmax(p[tangent[0], lower])])
        for t in tangent:
            conditions[int(basis.nodal_dofs[t, a])] = 0.0
        conditions[int(basis.nodal_dofs[tangent[1], b])] = 0.0
    elif case == "bend_z":
        # Two finite support strips; design-independent specimen-frame positions.
        bottom = at(2, lo[2])
        top = at(2, hi[2])
        span = 0.75 * datum_extent[0]
        center = (datum_lo[0] + datum_hi[0]) / 2
        width = 0.05 * datum_extent[0]
        left = bottom[np.abs(p[0, bottom] - (center - span / 2)) <= width]
        right = bottom[np.abs(p[0, bottom] - (center + span / 2)) <= width]
        load = top[np.abs(p[0, top] - center) <= width]
        if not all(len(x) for x in (left, right, load)):
            raise ValueError("missing finite fixture patch")
        for ns in (left, right):
            conditions.update({int(i): 0.0 for i in dofs(ns, 2)})
        # One axial pin, one lateral pin; the rest of each support strip is a roller.
        anchor = int(left[np.argmin(p[1, left])])
        conditions[int(basis.nodal_dofs[0, anchor])] = 0.0
        conditions[int(basis.nodal_dofs[1, anchor])] = 0.0
        other = int(right[np.argmin(p[1, right])])
        conditions[int(basis.nodal_dofs[1, other])] = 0.0
        loaded = dofs(load, 2)
        conditions.update({int(i): -delta for i in loaded})
    else:
        raise ValueError("unknown load case")
    fixed = np.array(sorted(conditions), dtype=int)
    values = np.array([conditions[int(i)] for i in fixed])
    # Symmetric full-system Dirichlet projection preserves physical node blocks.
    # Scalar elimination destroys that structure and duplicates huge CSR arrays.
    kff = stiffness.copy()
    free_mask = np.ones(basis.N, dtype=bool)
    free_mask[fixed] = False
    row_nodes = np.repeat(np.arange(mesh.points.shape[1]), np.diff(kff.indptr))
    for start in range(0, len(kff.indices), 65536):
        stop = start + 65536
        row = row_nodes[start:stop]
        col = kff.indices[start:stop]
        kff.data[start:stop] *= free_mask[3 * row[:, None] + np.arange(3)][:, :, None]
        kff.data[start:stop] *= free_mask[3 * col[:, None] + np.arange(3)][:, None, :]
    diagonal_blocks = np.flatnonzero(row_nodes == kff.indices)
    if len(diagonal_blocks) != mesh.points.shape[1]:
        raise ValueError("missing physical-node diagonal block")
    kff.data[diagonal_blocks[fixed // 3], fixed % 3, fixed % 3] = stiffness.diagonal()[
        fixed
    ]
    boundary = np.zeros(basis.N)
    boundary[fixed] = values
    rhs = -(stiffness @ boundary)
    rhs[fixed] = 0.0
    physical_rhs_norm = max(float(np.linalg.norm(rhs)), 1e-30)
    diagonal = kff.diagonal()
    if np.any(diagonal <= 0):
        raise ValueError("nonpositive elasticity diagonal")
    scaling = 1 / np.sqrt(diagonal)
    for start in range(0, len(kff.indices), 65536):
        stop = start + 65536
        row, col = row_nodes[start:stop], kff.indices[start:stop]
        kff.data[start:stop] *= scaling[3 * row[:, None] + np.arange(3)][:, :, None]
        kff.data[start:stop] *= scaling[3 * col[:, None] + np.arange(3)][:, None, :]
    rhs *= scaling
    del row_nodes
    if solver not in ("amg", "superlu", "superlu_mmd"):
        raise ValueError("unknown elasticity solver")
    if solver in ("superlu", "superlu_mmd"):
        setup_started = time.monotonic()
        if os.environ.get("SPILING_INFILL_PROGRESS") == "1":
            print(
                json.dumps(
                    {"phase": "factorization", "case": case, "dof": int(basis.N)}
                ),
                file=sys.stderr,
                flush=True,
            )
        permutation = (
            body.direct_order[0] if solver == "superlu" else np.arange(basis.N)
        )
        ordered = kff.tocsr()[permutation][:, permutation].tocsc()
        ordered.eliminate_zeros()
        factor = splu(
            ordered,
            permc_spec="NATURAL" if solver == "superlu" else "MMD_AT_PLUS_A",
            diag_pivot_thresh=0.0,
            options={"SymmetricMode": True},
        )
        setup_s = time.monotonic() - setup_started
        solve_started = time.monotonic()

        def direct_apply(vector):
            result = np.empty_like(vector)
            result[permutation] = factor.solve(vector[permutation])
            return result

        interior = direct_apply(rhs)
        for _ in range(3):
            defect = rhs - kff @ interior
            if np.linalg.norm(defect) <= rtol * 0.01 * max(np.linalg.norm(rhs), 1e-30):
                break
            interior += direct_apply(defect)
        info = 0
        iterations = [0]
    else:
        if preconditioner_factory is None:
            null = np.zeros((basis.N, 6))
            for axis in range(3):
                null[basis.nodal_dofs[axis], axis] = 1.0
            for axis in range(3):
                rotation = np.cross(np.eye(3)[axis], p.T - (lo + hi) / 2).T
                for dim in range(3):
                    null[basis.nodal_dofs[dim], axis + 3] = rotation[dim]
            null[fixed] = 0.0
            null /= scaling[:, None]
        setup_started = time.monotonic()
        progress = os.environ.get("SPILING_INFILL_PROGRESS") == "1"
        if progress:
            print(
                json.dumps(
                    {"phase": "preconditioner", "case": case, "dof": int(basis.N)}
                ),
                file=sys.stderr,
                flush=True,
            )
        if preconditioner_factory is None:
            import pyamg

            ml = pyamg.smoothed_aggregation_solver(
                kff,
                B=null,
                symmetry="symmetric",
                strength=("symmetric", {"theta": 0.05}),
                smooth=("energy", {"degree": 2}),
                max_coarse=80,
            )
            preconditioner = ml.aspreconditioner()
        else:
            preconditioner = preconditioner_factory(kff, scaling, fixed)
        setup_s = time.monotonic() - setup_started
        solve_started = time.monotonic()
        iterations = [0]

        def callback(_):
            iterations[0] += 1
            if progress and iterations[0] % 25 == 0:
                print(
                    json.dumps(
                        {
                            "phase": "cg",
                            "case": case,
                            "iterations": iterations[0],
                            "wall_s": time.monotonic() - solve_started,
                            "scaled_residual": float(
                                np.linalg.norm(kff @ _ - rhs)
                                / max(np.linalg.norm(rhs), 1e-30)
                            ),
                        }
                    ),
                    file=sys.stderr,
                    flush=True,
                )

        interior, info = cg(
            kff,
            rhs,
            M=preconditioner,
            rtol=rtol * 0.01,
            atol=0.0,
            maxiter=maxiter,
            callback=callback,
        )
    residual = float(
        np.linalg.norm(kff @ interior - rhs) / max(np.linalg.norm(rhs), 1e-30)
    )
    if info != 0 or residual > rtol * 2:
        raise ValueError(f"unconverged elasticity: code={info}, residual={residual}")
    interior *= scaling
    u = boundary + interior
    u[fixed] = values
    force = stiffness @ u
    physical_residual = float(np.linalg.norm(force[free_mask]) / physical_rhs_norm)
    if physical_residual > rtol * 2:
        raise ValueError(
            f"unconverged physical elasticity residual={physical_residual}; scaled={residual}"
        )
    reaction = -float(force[loaded].sum())
    energy = float(0.5 * u @ force)
    if reaction <= 0:
        raise ValueError("nonpositive fixture reaction")
    energy_error = abs(energy - 0.5 * reaction * delta) / max(energy, 1e-30)
    if energy_error > 1e-6:
        raise ValueError(f"energy mismatch {energy_error}")
    balance = float(
        np.linalg.norm([force[basis.nodal_dofs[a]].sum() for a in range(3)]) / reaction
    )
    if balance > 1e-6:
        raise ValueError(f"reaction imbalance {balance}")
    return {
        "case": case,
        "stiffness_n_mm": reaction / delta,
        "reaction_n": reaction,
        "delta_mm": delta,
        "strain_energy_n_mm": energy,
        "relative_residual": physical_residual,
        "scaled_relative_residual": residual,
        "energy_error": energy_error,
        "force_balance": balance,
        "iterations": iterations[0],
        "linear_solver": solver,
        "preconditioner_s": setup_s,
        "cg_s": time.monotonic() - solve_started,
        "dof": int(basis.N),
        "elements": int(mesh.count),
        "fixture_bounds_mm": [datum_lo.tolist(), datum_hi.tolist()],
        "mesh_volume_mm3": mesh.volume,
        "wall_s": time.monotonic() - began,
        "matrix_reused": reused,
        "assembly_wall_s": 0.0 if reused else body.assembly_s,
        "model": "effective_nominal_footprint_prisms_elastic_"
        + ("Wedge1" if mesh.cell_kind == "wedge" else "Q1")
        + "_not_strength",
    }


def solve_coupon(mesh, material, bounds, *, solver="superlu"):
    """Independent compression of a finite cut interior, not whole-part strength.

    A free component touching at most one frictionless fixture has a zero-energy
    rigid translation. It cannot carry force between plates. Components joining
    both plates are solved independently with ordinary elastic gauges.
    """
    from geometry import connected_meshes

    parts = connected_meshes(mesh)
    prepared = {}
    cases = []
    bounds = np.asarray(bounds, dtype=float)
    for axis in range(3):
        case = "compression_" + "xyz"[axis]
        responses = []
        for index, part in enumerate(parts):
            if not np.allclose(
                part.bounds[:, axis], bounds[:, axis], rtol=0, atol=1e-8
            ):
                continue
            if index not in prepared:
                prepared[index] = prepare(part, material)
            responses.append(
                solve(
                    part,
                    case,
                    material,
                    fixture_bounds=bounds,
                    prepared=prepared[index],
                    solver=solver,
                )
            )
        cases.append(
            {
                "case": case,
                "stiffness_n_mm": sum(r["stiffness_n_mm"] for r in responses),
                "spanning_components": len(responses),
                "components": responses,
                "mechanism": "elastic_load_path"
                if responses
                else "no_component_spans_fixture_faces",
            }
        )
    values = [r["stiffness_n_mm"] for r in cases]
    import math

    return {
        "case": "compression_envelope",
        "scope": "actual_interior_coupon",
        "stiffness_n_mm": math.exp(sum(math.log(v) for v in values) / 3)
        if all(v > 0 for v in values)
        else 0.0,
        "worst_stiffness_n_mm": min(values),
        "cases": cases,
        "elements": mesh.count,
        "dof": int(mesh.points.shape[1] * 3),
        "mesh_volume_mm3": mesh.volume,
        "fixture_bounds_mm": bounds.tolist(),
        "connected_components": len(parts),
        "model": "nominal_actual_interior_coupon_elastic_not_whole_part_strength",
    }
