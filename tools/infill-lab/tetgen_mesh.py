# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Bounded external volume remeshing of the retained footprint boundary.

This is offline assumed-law research, not native geometry authority. Conserve
the accepted affine source mesh through exact oriented boundary identity and
oppositely cancelling internal faces. Share displacement nodes only through
identical positive-area faces. No footprint re-rounding or geometric dilation.
"""

from __future__ import annotations

import hashlib
from collections import Counter
from fractions import Fraction
import math
from pathlib import Path
import shutil
import subprocess
import time

import numpy as np
from scipy import sparse
from scipy.sparse.csgraph import connected_components
import shapely

from geometry import AnalysisMesh, footprint_slabs
from reference import element_mesh

FACES = np.array(((0,2,1),(0,1,3),(0,3,2),(1,2,3)))
EDGES = np.array(((0,1),(0,2),(0,3),(1,2),(1,3),(2,3)))


def oriented_cells(points, cells):
    if (points.ndim != 2 or points.shape[1] != 3 or cells.ndim != 2 or cells.shape[1] != 4
            or not len(cells) or not np.issubdtype(cells.dtype,np.integer)
            or not np.all(np.isfinite(points))
            or cells.min() < 0 or cells.max() >= len(points)):
        raise ValueError("invalid tetrahedral geometry")
    if len(np.unique(np.sort(cells,axis=1),axis=0)) != len(cells):
        raise ValueError("duplicate tetrahedron")
    determinant = np.linalg.det(points[cells[:,1:]]-points[cells[:,:1]])
    if np.any(~np.isfinite(determinant)) or np.any(determinant == 0):
        raise ValueError("degenerate tetrahedron")
    cells = cells.copy()
    negative = determinant < 0
    cells[negative] = cells[negative][:,[0,2,1,3]]
    return cells, float(np.abs(determinant).sum()/6)


def boundary_inventory(cells):
    """Return outward faces; reject same-side/overlapping face neighbors."""
    oriented = cells[:,FACES].reshape(-1,3)
    ordering = np.argsort(oriented,axis=1)
    faces = np.take_along_axis(oriented,ordering,axis=1)
    _,inverse,count = np.unique(faces,axis=0,return_inverse=True,return_counts=True)
    if count.max() > 2:
        raise ValueError("nonmanifold tetrahedral face")
    # Parity of permutation to sorted vertex IDs retains cyclic orientation.
    parity = ((ordering[:,0]>ordering[:,1]).astype(int)
              +(ordering[:,0]>ordering[:,2])+(ordering[:,1]>ordering[:,2]))%2
    order = np.argsort(inverse)
    begin = np.r_[0,np.cumsum(count)[:-1]][count==2]
    if np.any(parity[order[begin]] == parity[order[begin+1]]):
        raise ValueError("internal faces do not cancel with opposite orientation")
    return oriented[count[inverse]==1]


def exact_plane(points):
    """Canonical homogeneous plane of binary64 coordinates, with no epsilon."""
    a, b, c = [[Fraction(float(v)) for v in p] for p in points]
    u, v = [b[i]-a[i] for i in range(3)], [c[i]-a[i] for i in range(3)]
    normal = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]]
    if not any(normal):
        raise ValueError("degenerate oriented source boundary")
    coefficients = [*normal, -sum(normal[i]*a[i] for i in range(3))]
    denominator = math.lcm(*(v.denominator for v in coefficients))
    integers = [v.numerator*(denominator//v.denominator) for v in coefficients]
    divisor = math.gcd(*integers)
    sign = 1 if next(v for v in integers[:3] if v) > 0 else -1
    return tuple(sign*v//divisor for v in integers)


def conserve_boundary(source_points, source_boundary, points, cells):
    """Conserve the source's oriented surface chain, not its diagonal choices.

    Cancelling internal cell faces and an identical oriented material boundary
    preserve source winding/multiplicity. Changed boundary triangles may only
    retessellate exact planes with the same oriented edge chains. A compact
    planar 2-chain with zero boundary is zero, so this preserves the surface
    itself, including orientation and multiplicity. Point/edge identities and
    original coordinates remain strict; no tolerance or coordinate welding.
    This consumes the admitted affine mesh, never a CAD proof.
    """
    if len(points)<len(source_points) or not np.array_equal(points[:len(source_points)],source_points):
        raise ValueError("remeshing changed original boundary coordinates or IDs")
    def inventory(faces):
        if not np.issubdtype(faces.dtype, np.integer):
            raise ValueError("invalid oriented source boundary indices")
        # Rotate each oriented cycle to its minimum ID; never reverse it.
        cycles = np.take_along_axis(faces,(np.argmin(faces,axis=1)[:,None]+np.arange(3))%3,axis=1)
        return np.ascontiguousarray(cycles[np.lexsort(cycles[:,::-1].T)], dtype=np.int64)
    before, after = inventory(source_boundary), inventory(boundary_inventory(cells))
    if np.array_equal(before, after):
        return "identical_oriented_triangles"
    # Exact common faces cancel before the more expensive rational predicate.
    # Facet/cell limits and the external container budget bound this work.
    dtype = np.dtype((np.void, before.dtype.itemsize*3))
    old_keys, new_keys = before.view(dtype).ravel(), after.view(dtype).ravel()
    changed = ((before[~np.isin(old_keys,new_keys)], 1),
               (after[~np.isin(new_keys,old_keys)], -1))
    edges = Counter()
    for faces, direction in changed:
        for face in faces:
            plane = exact_plane(points[face])
            for a, b in zip(face, np.roll(face,-1)):
                a, b = int(a), int(b)
                edges[(plane,min(a,b),max(a,b))] += direction*(1 if a < b else -1)
    if any(edges.values()):
        raise ValueError("remeshing changed oriented source boundary")
    return "exact_oriented_planar_chains"


def face_mesh(points, cells, *, expected_volume=None):
    """Explode coordinate-coincident point/edge contacts; join full faces only."""
    cells,volume = oriented_cells(points,cells)
    boundary_inventory(cells)
    if expected_volume is not None and not np.isclose(volume,expected_volume,rtol=1e-9,atol=1e-10):
        raise ValueError("remeshing changed retained footprint volume")
    ordering = np.argsort(cells[:,FACES],axis=2)
    faces = np.take_along_axis(cells[:,FACES],ordering,axis=2).reshape(-1,3)
    local = np.arange(len(cells))[:,None,None]*4+FACES[None,:,:]
    local = np.take_along_axis(local,ordering,axis=2).reshape(-1,3)
    _,inverse,count = np.unique(faces,axis=0,return_inverse=True,return_counts=True)
    if count.max() > 2:
        raise ValueError("nonmanifold tetrahedral face")
    order = np.argsort(inverse)
    begin = np.r_[0,np.cumsum(count)[:-1]][count==2]
    a,b = order[begin],order[begin+1]
    source,target = local[a].reshape(-1),local[b].reshape(-1)
    _,labels = connected_components(sparse.coo_matrix(
        (np.ones(len(source),np.int8),(source,target)),shape=(4*len(cells),)*2).tocsr(),directed=False)
    _,representatives = np.unique(labels,return_index=True)
    components,_ = connected_components(sparse.coo_matrix(
        (np.ones(len(a),np.int8),(a//4,b//4)),shape=(len(cells),)*2).tocsr(),directed=False)
    vertices = points[cells].reshape(-1,3)[representatives].T
    return AnalysisMesh(vertices,labels.reshape(-1,4).T,volume,len(cells),int(components),
                        np.array([vertices.min(axis=1),vertices.max(axis=1)]),
                        (1.,1.,1.),(len(cells),),"tet")


def material_containment(points, cells, roads, bounds, *, cap_segments=32, max_relations=8_000_000):
    """Project each clipped convex tet/slab piece, never just its centroid."""
    bounds = np.asarray(bounds,dtype=float)
    if (bounds.shape != (2,3) or not np.all(np.isfinite(bounds))
            or np.any(bounds[1] <= bounds[0]) or not np.all(np.isfinite(points))
            or np.any(points < bounds[0]) or np.any(points > bounds[1])):
        raise ValueError("tetrahedra leave the declared finite crop")
    coords = points[cells]
    zmin,zmax = coords[:,:,2].min(axis=1),coords[:,:,2].max(axis=1)
    _,levels,domains,_ = footprint_slabs(roads,cap_segments=cap_segments)
    window = shapely.box(*bounds[:,:2].reshape(-1))
    checked = 0
    for k,domain in enumerate(domains):
        low,high = max(levels[k],bounds[0,2]),min(levels[k+1],bounds[1,2])
        if low >= high:
            continue
        active = np.flatnonzero((zmin < high)&(zmax > low))
        checked += len(active)
        if checked > max_relations:
            raise ValueError("tet/slab containment work budget exceeded")
        if not len(active):
            continue
        q = coords[active]
        inside = (q[:,:,2]>=low)&(q[:,:,2]<=high)
        hullpoints = np.full((len(active),16,2),np.nan)
        hullpoints[:,:4] = np.where(inside[:,:,None],q[:,:,:2],np.nan)
        a,b = q[:,EDGES[:,0]],q[:,EDGES[:,1]]
        delta = b-a
        for index,level in enumerate((low,high)):
            crosses = ((a[:,:,2]<level)&(level<b[:,:,2]))|((b[:,:,2]<level)&(level<a[:,:,2]))
            ratio = np.divide(level-a[:,:,2],delta[:,:,2],out=np.zeros_like(a[:,:,2]),where=delta[:,:,2]!=0)
            hit = a[:,:,:2]+ratio[:,:,None]*delta[:,:,:2]
            hullpoints[:,4+6*index:10+6*index] = np.where(crosses[:,:,None],hit,np.nan)
        flat = hullpoints.reshape(-1,2)
        valid = np.all(np.isfinite(flat),axis=1)
        groups = np.repeat(np.arange(len(active)),16)[valid]
        hulls = shapely.convex_hull(shapely.multipoints(flat[valid],indices=groups))
        if len(hulls) != len(active) or np.any(shapely.is_empty(hulls)):
            raise ValueError("empty clipped tetrahedron")
        material = shapely.intersection(domain,window,grid_size=1e-9)
        # Same declared footprint lattice on both operands; no weld dilation,
        # missing-area threshold or centroid-based coverage surrogate.
        outside = shapely.difference(hulls,material,grid_size=1e-9)
        bad = ~shapely.is_empty(outside)
        if bad.any():
            raise ValueError(f"tetrahedron leaves physical slab {k}: tet={active[bad][0]}, outside={shapely.to_wkt(outside[bad][0])}")
    return checked


def remesh(mesh, roads, bounds, workdir, *, cap_segments=32, seconds=90, max_cells=2_000_000, max_source_prisms=250_000):
    started = time.monotonic()
    executable = shutil.which("tetgen")
    if executable is None:
        raise ValueError("explicit TetGen research executable is unavailable")
    if type(max_source_prisms) is not int or not 1 <= max_source_prisms <= 1_000_000:
        raise ValueError("invalid explicit source prism resource bound")
    if mesh.cell_kind != "wedge" or mesh.components != 1 or mesh.count > max_source_prisms:
        raise ValueError(f"volume remeshing requires one bounded connected source prism mesh: count={mesh.count}, components={mesh.components}, limit={max_source_prisms}")
    workdir = Path(workdir)
    workdir.mkdir(parents=True,exist_ok=False)
    points,elevated = element_mesh(mesh,"C3D10")
    cells,_ = oriented_cells(points,elevated[:,:4])
    boundary = boundary_inventory(cells)
    if len(boundary) > 500_000:
        raise ValueError("source boundary face budget exceeded")
    ids = np.unique(boundary)
    index = np.full(len(points),-1,dtype=int)
    index[ids] = np.arange(len(ids))
    boundary,points = index[boundary],points[ids]
    edges = np.concatenate([boundary[:,e] for e in ((0,1),(1,2),(0,2))])
    _,edge_count = np.unique(np.sort(edges,axis=1),axis=0,return_counts=True)
    if np.any(edge_count != 2):
        raise ValueError("source boundary is not closed")
    poly = workdir/"body.poly"
    with poly.open("x") as stream:
        stream.write(f"{len(points)} 3 0 0\n")
        for i,p in enumerate(points):
            stream.write(f"{i+1} "+" ".join(format(float(x),".17g") for x in p)+"\n")
        stream.write(f"{len(boundary)} 0\n")
        for face in boundary:
            stream.write("1 0\n3 "+" ".join(str(int(x)+1) for x in face)+"\n")
        stream.write("0\n0\n")
    with (workdir/"tetgen.log").open("x") as log:
        # TetGen 1.6: -M disables facet/vertex merging, -Y preserves surface
        # triangles, -J preserves original node IDs. Exact conservation below
        # remains mandatory regardless of what the executable promises.
        result = subprocess.run([executable,"-pQMYJ",str(poly.resolve())],stdout=log,stderr=subprocess.STDOUT,timeout=seconds)
    if result.returncode:
        raise ValueError(f"external TetGen failed with {result.returncode}")
    node,ele = workdir/"body.1.node",workdir/"body.1.ele"
    sizes = []
    for path,limit,width in ((node,max_cells*4,3),(ele,max_cells,4)):
        with path.open() as stream:
            header = [int(x) for x in stream.readline().split()]
        if len(header) < 3 or not 0 < header[0] <= limit or header[1] != width or header[2] != 0 or path.stat().st_size > limit*256:
            raise ValueError("bounded linear TetGen output required")
        sizes.append(header[0])
    vertices = np.atleast_2d(np.loadtxt(node,skiprows=1,usecols=(0,1,2,3)))
    elements = np.atleast_2d(np.loadtxt(ele,skiprows=1,usecols=(0,1,2,3,4)))
    if len(vertices) != sizes[0] or len(elements) != sizes[1] or not np.all(np.isfinite(elements)):
        raise ValueError("TetGen output counts or finite indices differ")
    if not np.array_equal(vertices[:,0],np.arange(1,len(vertices)+1)) or not np.array_equal(elements[:,0],np.arange(1,len(elements)+1)):
        raise ValueError("noncanonical TetGen node/element indices")
    if np.any(elements[:,1:] != np.floor(elements[:,1:])):
        raise ValueError("nonintegral TetGen node index")
    vertices,cells = vertices[:,1:],elements[:,1:].astype(int)-1
    cells,_ = oriented_cells(vertices,cells)
    conservation = conserve_boundary(points,boundary,vertices,cells)
    result = face_mesh(vertices,cells,expected_volume=mesh.volume)
    if result.components != mesh.components:
        raise ValueError("remeshing changed face-connected component count")
    return result,dict(method="retained_boundary_tetgen",source_prisms=mesh.count,
                       source_tets=len(elevated),boundary_faces=len(boundary),tets=result.count,
                       oriented_boundary_preserved=True,internal_faces_cancel=True,
                       boundary_conservation=conservation,
                       switches="-pQMYJ",volume_mm3=result.volume,components=result.components,
                       executable_sha256=hashlib.sha256(Path(executable).read_bytes()).hexdigest(),
                       wall_s=time.monotonic()-started)
