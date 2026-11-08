# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Research meshes from actual replay roads; never native geometry authority.

The main model uses local unions of nominal footprint prisms. A separate
inscribed-stadium diagnostic exposes sensitivity to idealized contact shape.
Only full-face neighbors share nodes; edge/corner contact never invents a bond.
Neither continuum volume is equated to commanded extrusion dose.
"""

from __future__ import annotations
from dataclasses import dataclass
import math
import numpy as np
from scipy import sparse
from scipy.sparse.csgraph import connected_components


@dataclass
class VoxelMesh:
    points: np.ndarray
    cells: np.ndarray
    volume: float
    count: int
    components: int
    bounds: np.ndarray
    spacing: tuple
    grid_shape: tuple


def road_contains(road, x, y, z, *, inset=1e-10):
    a, b = np.asarray(road["start"][:2]), np.asarray(road["end"][:2])
    direction = b - a
    den = float(direction @ direction)
    if den <= 0:
        raise ValueError("zero-length nominal road")
    t = np.clip(((x - a[0]) * direction[0] + (y - a[1]) * direction[1]) / den, 0.0, 1.0)
    d2 = (x - a[0] - t * direction[0]) ** 2 + (y - a[1] - t * direction[1]) ** 2
    lo, hi, w = road["lower"], road["upper"], road["width"]
    h = hi - lo
    if not (0 < h <= w and all(math.isfinite(v) for v in (lo, hi, w))):
        raise ValueError("invalid nominal stadium")
    radial = (w - h) / 2 + np.sqrt(
        np.maximum(0.0, (h / 2) ** 2 - (z - (lo + hi) / 2) ** 2)
    )
    return (
        (z >= lo - 1e-10)
        & (z <= hi + 1e-10)
        & (d2 <= np.maximum(0, radial - inset) ** 2)
    )


def voxelize_stadium(
    roads, *, dx=0.1125, dz=0.1, phase=(0.0, 0.0), max_cells=2_000_000
):
    if not roads or not (0 < dx <= 1 and 0 < dz <= 0.3):
        raise ValueError("empty roads or invalid mesh resolution")
    endpoints = np.array([r[key] for r in roads for key in ("start", "end")])
    radius = max(r["width"] for r in roads) / 2
    lo = endpoints[:, :2].min(axis=0) - radius
    hi = endpoints[:, :2].max(axis=0) + radius
    grids = []
    for axis in range(2):
        first = math.floor((lo[axis] - phase[axis]) / dx) * dx + phase[axis]
        last = math.ceil((hi[axis] - first) / dx)
        grids.append(first + np.arange(last + 1) * dx)
    levels = sorted({round(r[k], 10) for r in roads for k in ("lower", "upper")})
    z = [levels[0]]
    for a, b in zip(levels, levels[1:]):
        z.extend(np.linspace(a, b, max(1, math.ceil((b - a - 1e-9) / dz)) + 1)[1:])
    grids.append(np.asarray(z))
    shape = tuple(len(g) - 1 for g in grids)
    if math.prod(shape) > max_cells:
        raise ValueError(f"mesh cell budget exceeded: {shape}")
    occupied = np.zeros(shape, dtype=bool)
    for r in roads:
        a, b = np.asarray(r["start"][:2]), np.asarray(r["end"][:2])
        lower = [*np.minimum(a, b) - r["width"] / 2, r["lower"]]
        upper = [*np.maximum(a, b) + r["width"] / 2, r["upper"]]
        starts = [
            max(0, int(np.searchsorted(g, value - 1e-9)) - 1)
            for g, value in zip(grids, lower)
        ]
        ends = [
            min(len(g) - 1, int(np.searchsorted(g, value + 1e-9)) + 1)
            for g, value in zip(grids, upper)
        ]
        if any(b <= a for a, b in zip(starts, ends)):
            continue
        vectors = [g[a : b + 1] for g, a, b in zip(grids, starts, ends)]
        xx, yy, zz = np.meshgrid(*vectors, indexing="ij", sparse=True)
        vertices = road_contains(r, xx, yy, zz)
        solid = np.ones(tuple(b - a for a, b in zip(starts, ends)), dtype=bool)
        for i in (0, 1):
            for j in (0, 1):
                for k in (0, 1):
                    solid &= vertices[
                        i : i + solid.shape[0],
                        j : j + solid.shape[1],
                        k : k + solid.shape[2],
                    ]
        occupied[tuple(slice(a, b) for a, b in zip(starts, ends))] |= solid
    return mesh_from_voxels(occupied, grids)


def mesh_from_voxels(occupied, grids):
    from skfem import ElementHex1

    indices = np.column_stack(np.nonzero(occupied))
    n = len(indices)
    if not n:
        raise ValueError("no inscribed elements at this resolution")
    corners = np.asarray(ElementHex1().doflocs, dtype=int)
    cell_ids = np.full(occupied.shape, -1, dtype=np.int32)
    cell_ids[occupied] = np.arange(n)
    edge_a, edge_b, comp_a, comp_b = [], [], [], []
    for axis in range(3):
        left = [slice(None)] * 3
        right = left.copy()
        left[axis] = slice(None, -1)
        right[axis] = slice(1, None)
        a, b = cell_ids[tuple(left)], cell_ids[tuple(right)]
        ok = (a >= 0) & (b >= 0)
        a, b = a[ok], b[ok]
        comp_a.append(a)
        comp_b.append(b)
        for i, c in enumerate(corners):
            if c[axis] != 1:
                continue
            other = c.copy()
            other[axis] = 0
            j = int(np.flatnonzero(np.all(corners == other, axis=1))[0])
            edge_a.append(a * 8 + i)
            edge_b.append(b * 8 + j)
    src, dst = np.concatenate(edge_a), np.concatenate(edge_b)
    matrix = sparse.coo_matrix(
        (np.ones(len(src), dtype=np.int8), (src, dst)), shape=(n * 8, n * 8)
    ).tocsr()
    _, labels = connected_components(matrix, directed=False)
    _, representatives = np.unique(labels, return_index=True)
    allcoords = np.stack(
        [
            g[(indices[:, axis, None] + corners[None, :, axis]).reshape(-1)]
            for axis, g in enumerate(grids)
        ]
    )
    points = allcoords[:, representatives]
    cells = labels.reshape(n, 8).T
    ca, cb = np.concatenate(comp_a), np.concatenate(comp_b)
    comps = connected_components(
        sparse.coo_matrix((np.ones(len(ca)), (ca, cb)), shape=(n, n)).tocsr(),
        directed=False,
        return_labels=False,
    )
    volumes = np.ones(n)
    for axis, g in enumerate(grids):
        volumes *= np.diff(g)[indices[:, axis]]
    return VoxelMesh(
        points,
        cells,
        float(volumes.sum()),
        n,
        int(comps),
        np.array([points.min(axis=1), points.max(axis=1)]),
        tuple(float(np.max(np.diff(g))) for g in grids),
        occupied.shape,
    )


def voxelize(roads, *, dx=0.1125, dz=0.2, phase=(0.0, 0.0), max_cells=2_000_000):
    """Effective footprint-prism model, separate from measured material/dose.

    GEOS is tooling-only, operating on decoded roads, never BREP admission.
    A cell is retained only when its entire XY square is covered by the local
    footprint union in every crossed physical slab. No global bead-pair tie.
    """
    import shapely
    from shapely.geometry import LineString

    if not roads or dx <= 0 or dz <= 0:
        raise ValueError("invalid voxel request")
    endpoints = np.array([r[key] for r in roads for key in ("start", "end")])
    radius = max(r["width"] for r in roads) / 2
    lo = endpoints[:, :2].min(axis=0) - radius
    hi = endpoints[:, :2].max(axis=0) + radius
    grids = []
    for axis in range(2):
        first = math.floor((lo[axis] - phase[axis]) / dx) * dx + phase[axis]
        grids.append(first + np.arange(math.ceil((hi[axis] - first) / dx) + 1) * dx)
    levels = sorted({round(r[key], 10) for r in roads for key in ("lower", "upper")})
    z = [levels[0]]
    for a, b in zip(levels, levels[1:]):
        z.extend(np.linspace(a, b, max(1, math.ceil((b - a - 1e-9) / dz)) + 1)[1:])
    grids.append(np.asarray(z))
    shape = tuple(len(g) - 1 for g in grids)
    if math.prod(shape) > max_cells:
        raise ValueError(f"mesh cell budget exceeded: {shape}")
    x, y = np.meshgrid(grids[0][:-1], grids[1][:-1], indexing="ij")
    squares = shapely.box(x, y, x + dx, y + dx)
    # Grid quantization is invariant across physical slabs; do this once.
    tiles = shapely.set_precision(squares, 1e-9)
    occupied = np.zeros(shape, dtype=bool)
    cache = {}
    for k in range(shape[2]):
        low, high = grids[2][k : k + 2]
        ids = tuple(
            i
            for i, r in enumerate(roads)
            if r["lower"] <= low + 1e-9 and r["upper"] >= high - 1e-9
        )
        # Exact local footprint identity only. Z membership has already been
        # checked; no rounded coordinate keys and no global contact inference.
        signature = tuple(
            sorted(
                (
                    tuple(roads[i]["start"][:2]),
                    tuple(roads[i]["end"][:2]),
                    roads[i]["width"],
                )
                for i in ids
            )
        )
        if signature not in cache:
            polygons = [
                LineString([roads[i]["start"][:2], roads[i]["end"][:2]]).buffer(
                    roads[i]["width"] / 2, quad_segs=32
                )
                for i in ids
            ]
            domain = shapely.union_all(polygons, grid_size=1e-9)
            # Quantize both operands on the same declared numerical grid, not a weld dilation.
            cache[signature] = shapely.covers(domain, tiles)
        occupied[:, :, k] = cache[signature]
    return mesh_from_voxels(occupied, grids)
