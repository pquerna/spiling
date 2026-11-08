# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""P1 auxiliary-space preconditioning; the physical operator remains TetP2."""

from __future__ import annotations
import numpy as np
from scipy import sparse


def auxiliary_preconditioner(body, element_order):
    """Nested P1 auxiliary space; the physical operator remains TetP2."""
    import pyamg
    from pyamg.multilevel import MultilevelSolver
    from pyamg.relaxation.smoothing import setup_block_jacobi

    if body.mesh.cell_kind != "tet_p2":
        raise ValueError("auxiliary space requires the existing TetP2 topology")
    points = body.mesh.points.T
    cells = body.mesh.cells.T
    order = element_order
    ccx = cells[:, np.argsort(order)]
    count = int(ccx[:, :4].max()) + 1
    parents = np.full((len(points) - count, 2), -1, dtype=np.int64)
    for mid, (a, b) in enumerate(((0, 1), (1, 2), (2, 0), (0, 3), (1, 3), (2, 3)), 4):
        parents[ccx[:, mid] - count] = ccx[:, [a, b]]
    if np.any(parents < 0):
        raise ValueError("quadratic edge-node inventory is incomplete")
    rows = np.r_[np.arange(count), np.repeat(np.arange(count, len(points)), 2)]
    cols = np.r_[np.arange(count), parents.ravel()]
    data = np.r_[np.ones(count), np.full(2 * len(parents), 0.5)]
    point_p = sparse.coo_matrix(
        (data, (rows, cols)), shape=(len(points), count)
    ).tocsr()
    del rows, cols, data, parents

    def build(a, scaling, fixed):
        # Map physical coarse displacements into the symmetrically scaled
        # fine system; constrain prescribed rows/columns, retaining pin modes.
        p = sparse.kron(point_p, sparse.eye(3), format="csr").tobsr(blocksize=(3, 3))
        free = np.ones(a.shape[0], dtype=bool)
        free[fixed] = False
        row_nodes = np.repeat(np.arange(len(points)), np.diff(p.indptr))
        for start in range(0, len(p.indices), 65536):
            stop = start + 65536
            r = row_nodes[start:stop]
            c = p.indices[start:stop]
            p.data[start:stop] *= (
                free[3 * r[:, None] + np.arange(3)]
                / scaling[3 * r[:, None] + np.arange(3)]
            )[:, :, None]
            p.data[start:stop] *= free[3 * c[:, None] + np.arange(3)][:, None, :]
        coarse_fixed = fixed[fixed < 3 * count]
        pins = sparse.coo_matrix(
            (np.ones(len(coarse_fixed)), (coarse_fixed, coarse_fixed)), shape=p.shape
        ).tobsr(blocksize=(3, 3))
        p = p + pins
        del row_nodes, pins
        # Galerkin projection changes only the preconditioner, never the
        # fine operator used by CG and independent physical residual checks.
        ac = (p.T @ a @ p).tobsr(blocksize=(3, 3))
        candidates = np.zeros((3 * count, 6))
        for axis in range(3):
            candidates[axis::3, axis] = 1
            rotation = np.cross(
                np.eye(3)[axis],
                points[:count]
                - (points[:count].min(axis=0) + points[:count].max(axis=0)) / 2,
            )
            candidates[:, axis + 3] = rotation.ravel()
        candidates[coarse_fixed] = 0
        coarse = pyamg.smoothed_aggregation_solver(
            ac,
            B=candidates,
            symmetry="symmetric",
            strength=("symmetric", {"theta": 0.05}),
            smooth=("energy", {"degree": 2}),
            max_coarse=80,
        )
        fine = MultilevelSolver.Level()
        fine.A = a
        fine.P = p
        fine.R = p.T
        fine.presmoother = setup_block_jacobi(fine, iterations=2)
        fine.postsmoother = setup_block_jacobi(fine, iterations=2)
        hierarchy = MultilevelSolver([fine] + coarse.levels)
        return hierarchy.aspreconditioner()

    return build
