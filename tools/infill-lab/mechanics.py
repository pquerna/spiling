# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Research elasticity through scikit-fem/PETSc-independent sparse libraries.

No strength/failure, physical calibration or project authority. Full-integration
Q1 hexahedra; compare with an external C3D8 solver and convergence controls.
"""

from __future__ import annotations
import time
import numpy as np
from scipy.sparse.linalg import cg
import pyamg
from skfem import MeshHex, Basis, ElementHex1, ElementVector, asm
from skfem.models.elasticity import linear_elasticity, lame_parameters
from model import Material
from geometry import VoxelMesh


def solve(
    mesh: VoxelMesh,
    case="compression_z",
    material=Material(),
    *,
    rtol=1e-8,
    maxiter=2000,
):
    if mesh.components != 1:
        raise ValueError(
            f"inscribed mesh has {mesh.components} disconnected components; refine, do not invent bonds"
        )
    np.random.seed(0)  # Freeze AMG randomized setup; this is a research solve.
    began = time.monotonic()
    m = MeshHex(mesh.points, mesh.cells, sort_t=False)
    basis = Basis(m, ElementVector(ElementHex1()), intorder=2)
    lam, mu = lame_parameters(material.young_mpa, material.poisson)
    stiffness = asm(linear_elasticity(lam, mu), basis).tocsr()
    p = mesh.points
    lo, hi = mesh.bounds
    extent = hi - lo

    def dofs(nodes, axis):
        return basis.nodal_dofs[axis, np.asarray(nodes, dtype=int)]

    def at(axis, value):
        return np.flatnonzero(np.isclose(p[axis], value, rtol=0, atol=1e-8))

    delta = 0.001 * float(extent[2])
    conditions = {}
    loaded = []
    if case.startswith("compression_"):
        axis = {"x": 0, "y": 1, "z": 2}[case[-1]]
        delta = 0.001 * float(extent[axis])
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
        span = 0.75 * extent[0]
        center = (lo[0] + hi[0]) / 2
        width = max(mesh.spacing[0] * 1.1, 0.05 * extent[0])
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
    free = np.setdiff1d(np.arange(basis.N), fixed, assume_unique=True)
    kff = stiffness[free][:, free].tocsr()
    rhs = -stiffness[free][:, fixed] @ values
    null = np.zeros((basis.N, 6))
    for axis in range(3):
        null[basis.nodal_dofs[axis], axis] = 1.0
    for axis in range(3):
        rotation = np.cross(np.eye(3)[axis], p.T - (lo + hi) / 2).T
        for dim in range(3):
            null[basis.nodal_dofs[dim], axis + 3] = rotation[dim]
    ml = pyamg.smoothed_aggregation_solver(
        kff,
        B=null[free],
        symmetry="symmetric",
        smooth=("energy", {"degree": 2}),
        max_coarse=80,
    )
    iterations = [0]

    def callback(_):
        iterations[0] += 1

    interior, info = cg(
        kff,
        rhs,
        M=ml.aspreconditioner(),
        rtol=rtol,
        atol=0.0,
        maxiter=maxiter,
        callback=callback,
    )
    residual = float(
        np.linalg.norm(kff @ interior - rhs) / max(np.linalg.norm(rhs), 1e-30)
    )
    if info != 0 or residual > rtol * 2:
        raise ValueError(f"unconverged elasticity: code={info}, residual={residual}")
    u = np.zeros(basis.N)
    u[fixed] = values
    u[free] = interior
    force = stiffness @ u
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
        "relative_residual": residual,
        "energy_error": energy_error,
        "force_balance": balance,
        "iterations": iterations[0],
        "dof": int(basis.N),
        "elements": int(mesh.count),
        "mesh_volume_mm3": mesh.volume,
        "wall_s": time.monotonic() - began,
        "model": "effective_nominal_footprint_prisms_isotropic_Q1_not_strength",
    }
