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
class AnalysisMesh:
    points: np.ndarray
    cells: np.ndarray
    volume: float
    count: int
    components: int
    bounds: np.ndarray
    spacing: tuple
    grid_shape: tuple
    cell_kind: str = "hex"


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
    return AnalysisMesh(
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


def footprint_slabs(roads):
    import shapely
    from shapely.geometry import LineString

    width = min(r["width"] for r in roads)
    if not math.isfinite(width) or width <= 0:
        raise ValueError("invalid road width")
    starts, ends = {}, {}
    for i, r in enumerate(roads):
        low, high = round(r["lower"], 10), round(r["upper"], 10)
        if not low < high:
            raise ValueError("invalid physical interval")
        starts.setdefault(low, []).append(i)
        ends.setdefault(high, []).append(i)
    levels = sorted(set(starts) | set(ends))
    if len(levels) > 4097:
        raise ValueError("physical slab budget exceeded")
    active, domains, cache = set(), [], {}
    for low, high in zip(levels, levels[1:]):
        active.difference_update(ends.get(low, ()))
        active.update(starts.get(low, ()))
        signature = tuple(
            sorted(
                (
                    tuple(
                        sorted(
                            (tuple(roads[i]["start"][:2]), tuple(roads[i]["end"][:2]))
                        )
                    ),
                    roads[i]["width"],
                )
                for i in active
            )
        )
        if signature not in cache:
            polygons = [
                LineString(pair).buffer(w / 2, quad_segs=32) for pair, w in signature
            ]
            cache[signature] = shapely.union_all(polygons, grid_size=1e-9)
        domains.append(cache[signature])
    return width, levels, domains, list(cache.values())


def refine_triangles(points, triangles, max_edge, *, uniform=False, owners=None):
    """Conforming marked-edge bisection; no independent hanging-side refinement."""
    owners = np.arange(len(triangles)) if owners is None else owners
    for iteration in range(20):
        edges = np.concatenate(
            [triangles[:, [0, 1]], triangles[:, [1, 2]], triangles[:, [2, 0]]]
        )
        unique, inverse = np.unique(np.sort(edges, axis=1), axis=0, return_inverse=True)
        marked = (
            np.ones(len(unique), dtype=bool)
            if uniform
            else np.linalg.norm(points[unique[:, 1]] - points[unique[:, 0]], axis=1)
            > max_edge * (1 + 1e-10)
        )
        if not np.any(marked):
            return points, triangles, owners
        mids = np.full(len(unique), -1, dtype=int)
        mids[marked] = len(points) + np.arange(marked.sum())
        points = np.vstack([points, points[unique[marked]].mean(axis=1)])
        local = mids[inverse].reshape(3, -1).T
        out, new_owners = [], []
        for index, ((a, b, c), (ab, bc, ca)) in enumerate(zip(triangles, local)):
            before = len(out)
            count = int(ab >= 0) + int(bc >= 0) + int(ca >= 0)
            if count == 0:
                out.append((a, b, c))
            elif count == 3:
                out.extend([(a, ab, ca), (ab, b, bc), (ca, bc, c), (ab, bc, ca)])
            elif count == 1:
                if ab >= 0:
                    out.extend([(a, ab, c), (ab, b, c)])
                elif bc >= 0:
                    out.extend([(b, bc, a), (bc, c, a)])
                else:
                    out.extend([(c, ca, b), (ca, a, b)])
            else:
                if ab < 0:
                    out.extend([(c, ca, bc), (ca, a, b), (ca, b, bc)])
                elif bc < 0:
                    out.extend([(a, ab, ca), (ab, b, c), (ab, c, ca)])
                else:
                    out.extend([(b, bc, ab), (bc, c, a), (bc, a, ab)])
            new_owners.extend([owners[index]] * (len(out) - before))
        triangles = np.asarray(out, dtype=int)
        owners = np.asarray(new_owners)
        if len(triangles) > 200000:
            raise ValueError("planar triangle budget exceeded")
        if uniform:
            return points, triangles, owners
    raise ValueError("planar refinement did not terminate")


def mesh_from_prisms(xy, triangles, z, occupied):
    """Share node DOFs through identical full faces only, never point contacts."""
    indices = np.column_stack(np.nonzero(occupied))
    n = len(indices)
    if not n:
        raise ValueError("no deposited prisms")
    nxy = len(xy)
    base = triangles[indices[:, 0]]
    lower = base + nxy * indices[:, 1, None]
    upper = base + nxy * (indices[:, 1, None] + 1)
    raw = np.hstack([lower, upper])
    faces = [(0, 1, 2), (3, 4, 5), (0, 1, 4, 3), (1, 2, 5, 4), (2, 0, 3, 5)]
    keys, corners = [], []
    for face in faces:
        ids = raw[:, face]
        perm = np.argsort(ids, axis=1)
        sorted_ids = np.take_along_axis(ids, perm, axis=1)
        sorted_corners = np.take_along_axis(
            np.broadcast_to(face, ids.shape), perm, axis=1
        )
        if len(face) == 3:
            sorted_ids = np.column_stack([sorted_ids, np.full(n, -1)])
            sorted_corners = np.column_stack([sorted_corners, np.full(n, -1)])
        keys.append(sorted_ids)
        corners.append(
            np.where(
                sorted_corners >= 0, np.arange(n)[:, None] * 6 + sorted_corners, -1
            )
        )
    keys = np.concatenate(keys)
    corners = np.concatenate(corners)
    _, labels, counts = np.unique(keys, axis=0, return_inverse=True, return_counts=True)
    if counts.max() > 2:
        raise ValueError("nonmanifold prism face")
    order = np.argsort(labels, kind="stable")
    starts = np.r_[0, np.cumsum(counts)[:-1]]
    paired = starts[counts == 2]
    a, b = order[paired], order[paired + 1]
    valid = corners[a] >= 0
    src, dst = corners[a][valid], corners[b][valid]
    _, node_labels = connected_components(
        sparse.coo_matrix(
            (np.ones(len(src), np.int8), (src, dst)), shape=(n * 6, n * 6)
        ).tocsr(),
        directed=False,
    )
    _, representatives = np.unique(node_labels, return_index=True)
    node_xy = xy[raw.reshape(-1) % nxy]
    node_z = np.asarray(z)[raw.reshape(-1) // nxy]
    points = np.column_stack([node_xy, node_z])[representatives].T
    cells = node_labels.reshape(n, 6).T
    cell_a, cell_b = a % n, b % n
    components = connected_components(
        sparse.coo_matrix((np.ones(len(a)), (cell_a, cell_b)), shape=(n, n)).tocsr(),
        directed=False,
        return_labels=False,
    )
    tri_xy = xy[base]
    u, v = tri_xy[:, 1] - tri_xy[:, 0], tri_xy[:, 2] - tri_xy[:, 0]
    volume = np.sum(
        abs(u[:, 0] * v[:, 1] - u[:, 1] * v[:, 0]) / 2 * np.diff(z)[indices[:, 1]]
    )
    return AnalysisMesh(
        points,
        cells,
        float(volume),
        n,
        int(components),
        np.array([points.min(axis=1), points.max(axis=1)]),
        (float(np.ptp(xy[:, 0])), float(np.ptp(xy[:, 1])), float(max(np.diff(z)))),
        occupied.shape,
        "wedge",
    )


def prism_mesh(
    roads, *, max_xy=4.0, max_z=1.0, divisions=1, window=None, max_cells=2_000_000
):
    """GEOS constrained planar arrangement, extruded over actual physical slabs."""
    import shapely

    if not roads or max_xy <= 0 or max_z <= 0 or divisions not in (1, 2):
        raise ValueError("invalid conforming prism request")
    _, levels, domains, unique = footprint_slabs(roads)
    if window is not None:
        window = np.asarray(window, dtype=float)
        if (
            window.shape != (2, 3)
            or not np.all(np.isfinite(window))
            or np.any(window[1] <= window[0])
        ):
            raise ValueError("invalid source-frame coupon bounds")
        selected = [
            k
            for k in range(len(domains))
            if levels[k + 1] > window[0, 2] + 1e-9 and levels[k] < window[1, 2] - 1e-9
        ]
        if not selected:
            raise ValueError("coupon has no deposition intervals")
        xy = shapely.box(window[0, 0], window[0, 1], window[1, 0], window[1, 1])
        clipped = {id(d): shapely.intersection(d, xy, grid_size=1e-9) for d in unique}
        domains = [clipped[id(domains[k])] for k in selected]
        levels = [max(levels[selected[0]], window[0, 2])] + [
            min(levels[k + 1], window[1, 2]) for k in selected
        ]
        unique = list({id(d): d for d in domains}.values())
    linework = shapely.union_all([d.boundary for d in unique], grid_size=1e-9)
    faces = shapely.get_parts(shapely.polygonize(shapely.get_parts(linework)))
    triangles, face_ids = shapely.get_parts(
        shapely.constrained_delaunay_triangles(faces), return_index=True
    )
    coords = np.asarray([np.asarray(t.exterior.coords)[:3] for t in triangles])
    xy, inverse = np.unique(coords.reshape(-1, 2), axis=0, return_inverse=True)
    tri = inverse.reshape(-1, 3)
    xy, tri, face_ids = refine_triangles(xy, tri, max_xy, owners=face_ids)
    if divisions == 2:
        xy, tri, face_ids = refine_triangles(
            xy, tri, max_xy, uniform=True, owners=face_ids
        )
    # Every internal edge must have an exact mate. A T-junction is not licensed
    # by globally merging equal-coordinate vertices elsewhere in the mesh.
    edges = np.concatenate([tri[:, [0, 1]], tri[:, [1, 2]], tri[:, [2, 0]]])
    edges, counts = np.unique(np.sort(edges, axis=1), axis=0, return_counts=True)
    if counts.max() > 2:
        raise ValueError("nonmanifold planar triangulation")
    whole = shapely.union_all(unique, grid_size=1e-9)
    free_edges = shapely.linestrings(xy[edges[counts == 1]])
    # Overlay intersections are represented on the declared 1e-9 mm grid.
    # This check admits only outer-boundary roundoff, never a material tie.
    covered = shapely.covers(whole.boundary.buffer(2e-9), free_edges)
    if not np.all(covered):
        midpoints = shapely.line_interpolate_point(
            free_edges[~covered], 0.5, normalized=True
        )
        distances = shapely.distance(midpoints, whole.boundary)
        raise ValueError(
            f"hanging internal triangulation edge: count={sum(~covered)}, max_midpoint_distance={max(distances):.9g}, example={shapely.get_coordinates(free_edges[~covered][:2]).tolist()}"
        )
    # Classify the original noded faces, then inherit ownership through exact
    # subdivision. Rounding new midpoint vertices and reclassifying triangles
    # can erase their connecting neighbors and manufacture numerical islands.
    masks = {}
    for domain in unique:
        outside = shapely.area(shapely.difference(faces, domain, grid_size=1e-9))
        inside = shapely.area(shapely.intersection(faces, domain, grid_size=1e-9))
        if np.any((outside > 0) & (inside > 0)):
            bad = np.flatnonzero((outside > 0) & (inside > 0))
            raise ValueError(
                f"arrangement face straddles deposition: {len(bad)} faces; outside={outside[bad[0]]:.9g}, inside={inside[bad[0]]:.9g}"
            )
        masks[id(domain)] = (outside == 0)[face_ids]
    runs = []
    nominal = 0.0
    for k, d in enumerate(domains):
        low, high = levels[k : k + 2]
        nominal += d.area * (high - low)
        mask = masks[id(d)]
        if runs and np.array_equal(runs[-1][2], mask):
            runs[-1][1] = high
        else:
            runs.append([low, high, mask])
    z = [levels[0]]
    slabs = []
    for low, high, mask in runs:
        n = max(1, math.ceil((high - low - 1e-9) / max_z))
        z.extend(np.linspace(low, high, n + 1)[1:])
        slabs.extend([mask] * n)
    occupied = np.stack(slabs, axis=1)
    if occupied.sum() > max_cells:
        raise ValueError("prism element budget exceeded")
    mesh = mesh_from_prisms(xy, tri, np.asarray(z), occupied)
    if mesh.volume > (1 + 1e-8) * nominal:
        raise ValueError("analysis mesh exceeds nominal deposited volume")
    if mesh.volume < 0.97 * nominal:
        raise ValueError(
            f"prism mesh retains only {mesh.volume / nominal:.2%} of footprint volume; refine"
        )
    return mesh, {
        "method": "conforming_footprint_prisms",
        "max_xy_mm": max_xy,
        "max_z_mm": max_z,
        "divisions": divisions,
        "physical_slabs": len(domains),
        "mesh_slabs": len(slabs),
        "planar_triangles": len(tri),
        "nominal_footprint_volume_mm3": nominal,
        "retained_volume_fraction": mesh.volume / nominal,
    }


def connected_meshes(mesh):
    """Separate existing DOF-connected bodies; never join coincident vertices."""
    if mesh.components == 1:
        return [mesh]
    a = np.tile(mesh.cells[0], mesh.cells.shape[0] - 1)
    b = mesh.cells[1:].reshape(-1)
    _, labels = connected_components(
        sparse.coo_matrix(
            (np.ones(len(a), np.int8), (a, b)), shape=(mesh.points.shape[1],) * 2
        ).tocsr(),
        directed=False,
    )
    cell_labels = labels[mesh.cells[0]]
    out = []
    for label in np.unique(cell_labels):
        raw = mesh.cells[:, cell_labels == label]
        used, inverse = np.unique(raw, return_inverse=True)
        cells = inverse.reshape(raw.shape)
        points = mesh.points[:, used]
        p = points[:, cells]
        if mesh.cell_kind == "wedge":
            u, v = p[:, 1] - p[:, 0], p[:, 2] - p[:, 0]
            volume = np.sum(abs(u[0] * v[1] - u[1] * v[0]) / 2 * (p[2, 3] - p[2, 0]))
        else:
            volume = np.prod(p.max(axis=1) - p.min(axis=1), axis=0).sum()
        out.append(
            AnalysisMesh(
                points,
                cells,
                float(volume),
                raw.shape[1],
                1,
                np.array([points.min(axis=1), points.max(axis=1)]),
                mesh.spacing,
                mesh.grid_shape,
                mesh.cell_kind,
            )
        )
    return out
