// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Ephemeral, checked segment-Voronoi proposals. Original boundaries, not the
//! integer sites or this graph, remain manufacturing material/void authority.
//! Independently authored adapter to the BSL-1.0 boostvoronoi 0.12.1 API.

use super::super::{boolean, native};
use crate::planner::MAX_RETAINED_GEOMETRY_UNITS;
use crate::{check_cancel, error};
use boostvoronoi::{
    builder::Builder,
    diagram::{Cell, Diagram, SourceCategory},
};
use geo::{
    Coord, MultiPolygon,
    kernels::{Kernel, Orientation, RobustKernel},
};
use spiling_contracts::manufacturing::{
    MAX_MANUFACTURING_RETAINED_BYTES, ManufacturingError, ManufacturingErrorCode,
};
use std::{mem::size_of, sync::atomic::AtomicBool};

// A power-of-two scale makes conversion multiplication exact. Half a grid
// diagonal is 0.67435 nanometres; the remaining allowance encloses f64 origin,
// subtraction and reconstruction arithmetic. Large/ill-conditioned worlds refuse.
const SCALE: f64 = 1_048_576.0;
const MAX_POINT_PERTURBATION_MM: f64 = 0.000001;

pub(super) struct Skeleton {
    pub edges: Vec<MedialEdge>,
    /// Dense retained-vertex table, in original diagram vertex order. Includes
    /// degree one and two; degree is in the retained undirected interior graph.
    pub junctions: Vec<MedialJunction>,
    /// Number of dense retained finite vertices, excluding caller inventory.
    /// Allocation capacities are admitted only in bytes, never as point units.
    pub retained_points: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct BoundarySupport {
    /// Exterior then holes, in polygon iteration order; never a diagram index.
    pub boundary_index: u32,
    /// Original closed-ring window index, including any redundant collinear edge.
    pub segment_index: u32,
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// Original endpoint when this cell belongs to an endpoint site.
    pub point: Option<[f64; 2]>,
}

impl BoundarySupport {
    pub fn foot(&self, p: [f64; 2]) -> [f64; 2] {
        if let Some(point) = self.point {
            return point;
        }
        let d = sub(self.b, self.a);
        let length = norm(d);
        let u = mul(d, 1.0 / length);
        add(self.a, mul(u, dot(sub(p, self.a), u).clamp(0.0, length)))
    }

    pub fn radius(&self, p: [f64; 2]) -> f64 {
        norm(sub(p, self.foot(p)))
    }
}

pub(super) struct MedialEdge {
    pub supports: [BoundarySupport; 2],
    pub endpoints: [[f64; 2]; 2],
    /// Indices into `Skeleton::junctions`, in endpoint order. No coordinate
    /// snapping, approximate-key merging, or inferred junction correspondence.
    pub vertex_ids: [usize; 2],
    curve: Curve,
    arithmetic: f64,
}

#[derive(Clone, Copy)]
enum Curve {
    Line,
    // Quantized computational focus/directrix in world-mm local coordinates.
    // x is linear in t. y = ((x-fx)^2+fy^2)/(2*fy).
    Parabola {
        origin: [f64; 2],
        a: [f64; 2],
        u: [f64; 2],
        n: [f64; 2],
        focus: [f64; 2],
        x: [f64; 2],
        endpoint_error: f64,
    },
}

impl MedialEdge {
    pub fn at(&self, t: f64) -> [f64; 2] {
        // Preserve shared graph endpoints exactly. Their discrepancy from the
        // analytic parabola is explicitly included in the chord enclosure.
        if t == 0.0 {
            return self.endpoints[0];
        }
        if t == 1.0 {
            return self.endpoints[1];
        }
        match self.curve {
            Curve::Line => lerp(self.endpoints[0], self.endpoints[1], t),
            Curve::Parabola {
                origin,
                a,
                u,
                n,
                focus,
                x,
                ..
            } => parabola_point(origin, a, u, n, focus, x[0] + (x[1] - x[0]) * t),
        }
    }

    /// Conservative distance to the same-parameter chord on any subinterval
    /// 0 <= t0 <= t1 <= 1. For a quadratic the maximum deviation is |p''|dt²/8.
    /// Includes evaluated-coordinate and library-endpoint discrepancy enclosures;
    /// it is not a source/section error or permission to enter original material.
    pub fn chord_error_bound(&self, t0: f64, t1: f64) -> f64 {
        if !t0.is_finite() || !t1.is_finite() || t0 < 0.0 || t1 > 1.0 || t0 > t1 {
            return f64::INFINITY;
        }
        if t0 == t1 {
            return 0.0;
        }
        match self.curve {
            Curve::Line => self.arithmetic,
            Curve::Parabola {
                focus,
                x,
                endpoint_error,
                ..
            } => {
                let dx = (x[1] - x[0]).abs();
                let dt = t1 - t0;
                (dx * dx / (8.0 * focus[1].abs()) * dt * dt * (1.0 + 32.0 * f64::EPSILON)
                    + 2.0 * endpoint_error
                    + self.arithmetic)
                    .next_up()
            }
        }
    }
}

pub(super) struct MedialJunction {
    pub point: [f64; 2],
    pub valence: usize,
}

struct Site {
    support: BoundarySupport,
    q: [[i32; 2]; 2],
    ring_len: usize,
    // Both representations: a single sweep admits raw AND quantized geometry.
    min: [f64; 2],
    max: [f64; 2],
}

struct Ring {
    first: usize,
    len: usize,
    polygon: usize,
    hole: bool,
    min: [f64; 2],
    max: [f64; 2],
}

#[derive(Clone, Copy)]
struct Admission {
    retained_points: usize,
    external_owned_bytes: usize,
}

/// `external_owned_bytes` is the TOTAL already-live caller ownership in this
/// allocation budget, not an addition to an implicit point-to-byte conversion.
/// Semantic point inventory and owned-byte admission are independent.
pub(super) fn construct(
    material: &MultiPolygon<f64>,
    retained_points: usize,
    external_owned_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Skeleton, ManufacturingError> {
    check_cancel(cancel)?;
    let admission = Admission {
        retained_points,
        external_owned_bytes,
    };
    let mut count = 0usize;
    let mut ring_count = 0usize;
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for polygon in &material.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            check_cancel(cancel)?;
            boolean::charge(work, ring.0.len().max(1))?;
            if ring.0.len() < 4 || ring.0.first() != ring.0.last() {
                return Err(condition(
                    "skeleton requires explicitly closed positive rings",
                ));
            }
            count = count.checked_add(ring.0.len() - 1).ok_or_else(storage)?;
            ring_count = ring_count.checked_add(1).ok_or_else(storage)?;
            for p in &ring.0 {
                for (axis, value) in [p.x, p.y].into_iter().enumerate() {
                    if !value.is_finite() || !native::merge_predicate_coordinate(value) {
                        return Err(condition(
                            "skeleton input is outside the robust predicate envelope",
                        ));
                    }
                    min[axis] = min[axis].min(value);
                    max[axis] = max[axis].max(value);
                }
            }
        }
    }
    if count == 0 {
        return Ok(Skeleton {
            edges: Vec::new(),
            junctions: Vec::new(),
            retained_points: 0,
        });
    }
    if count > u32::MAX as usize / 24 || ring_count > u32::MAX as usize {
        return Err(storage());
    }
    // Constructor is O(n log n), with <=3n cells, <6n vertices and <18n
    // half-edges. Keep a conservative 8 KiB/site estimate for the live external
    // constructor/diagram throughout extraction, even after its events are freed.
    // Add our known site/ring/sweep-index/degree-remap scratch separately. Graph
    // buffers and old+new capacities during growth are admitted in addition.
    // These are bounded staging estimates, not a hard external allocator RSS
    // limit or a promise of internal cancellation.
    let staging = constructor_staging(count, ring_count)?;
    admit_bytes(admission, staging)?;
    let log = usize::BITS as usize - count.leading_zeros() as usize;
    boolean::charge(
        work,
        count.checked_mul(128 * (log + 1)).ok_or_else(storage)?,
    )?;
    let origin = [min[0] * 0.5 + max[0] * 0.5, min[1] * 0.5 + max[1] * 0.5];
    let mut sites = Vec::with_capacity(count);
    let mut rings = Vec::with_capacity(ring_count);
    for (polygon_index, polygon) in material.0.iter().enumerate() {
        for (local_ring, ring) in std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .enumerate()
        {
            check_cancel(cancel)?;
            let boundary_index = rings.len() as u32;
            let first = sites.len();
            let len = ring.0.len() - 1;
            let mut bounds_min = [f64::INFINITY; 2];
            let mut bounds_max = [f64::NEG_INFINITY; 2];
            for (segment_index, edge) in ring.0.windows(2).enumerate() {
                boolean::charge(work, 1)?;
                let a = [edge[0].x, edge[0].y];
                let b = [edge[1].x, edge[1].y];
                if a == b {
                    return Err(condition("skeleton boundary has a zero-length edge"));
                }
                let q = [quantize(a, origin)?, quantize(b, origin)?];
                if q[0] == q[1] {
                    return Err(condition(
                        "positive boundary edge collapses on the bounded skeleton grid",
                    ));
                }
                let mut lo = [0.0; 2];
                let mut hi = [0.0; 2];
                for axis in 0..2 {
                    lo[axis] = a[axis]
                        .min(b[axis])
                        .min(world(q[0], origin)[axis])
                        .min(world(q[1], origin)[axis]);
                    hi[axis] = a[axis]
                        .max(b[axis])
                        .max(world(q[0], origin)[axis])
                        .max(world(q[1], origin)[axis]);
                    bounds_min[axis] = bounds_min[axis].min(lo[axis]);
                    bounds_max[axis] = bounds_max[axis].max(hi[axis]);
                }
                sites.push(Site {
                    support: BoundarySupport {
                        boundary_index,
                        segment_index: segment_index as u32,
                        a,
                        b,
                        point: None,
                    },
                    q,
                    ring_len: len,
                    min: lo,
                    max: hi,
                });
            }
            rings.push(Ring {
                first,
                len,
                polygon: polygon_index,
                hole: local_ring != 0,
                min: bounds_min,
                max: bounds_max,
            });
        }
    }
    validate_sites(&sites, work, cancel)?;
    validate_rings(&sites, &rings, false, work, cancel)?;
    validate_rings(&sites, &rings, true, work, cancel)?;
    check_cancel(cancel)?;
    // The library call cannot be interrupted; all input/storage/work admission
    // above precedes it, with coarse cancellation immediately on either side.
    let diagram = Builder::<i32>::default()
        .with_segments(
            sites
                .iter()
                .map(|s| [s.q[0][0], s.q[0][1], s.q[1][0], s.q[1][1]]),
        )
        .and_then(|builder| builder.build())
        .map_err(|e| condition(&format!("segment-Voronoi construction refused: {e}")))?;
    check_cancel(cancel)?;
    validate_diagram(&diagram, count, work, cancel)?;
    // Only retained interior edges allocate graph storage. Degrees also serve
    // as the dense remap table; their <=6n entries are included in staging.
    let mut edges = Vec::new();
    let mut degrees = vec![0usize; diagram.num_vertices()];
    let mut retained_vertices = 0usize;
    for edge in diagram.edges() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let twin = &diagram.edges()[edge.twin().map_err(diagram_error)?.usize()];
        if !edge.is_primary() || edge.id().usize() >= twin.id().usize() {
            continue;
        }
        let (Some(v0), Some(v1)) = (edge.vertex0(), twin.vertex0()) else {
            continue;
        };
        let ids = [v0.usize(), v1.usize()];
        let local = ids.map(|id| {
            let v = &diagram.vertices()[id];
            [v.x() / SCALE, v.y() / SCALE]
        });
        if ids[0] == ids[1] || local[0] == local[1] {
            return Err(condition(
                "segment-Voronoi retained a collapsed finite primary edge",
            ));
        }
        let cells = [
            edge.cell().map_err(diagram_error)?,
            twin.cell().map_err(diagram_error)?,
        ];
        let supports = [
            support(&diagram.cells()[cells[0].usize()], &sites)?,
            support(&diagram.cells()[cells[1].usize()], &sites)?,
        ];
        let endpoints = local.map(|p| add(origin, p));
        let magnitude = endpoints
            .into_iter()
            .flatten()
            .fold(1.0f64, |a, b| a.max(b.abs()));
        let arithmetic = (4096.0 * f64::EPSILON * magnitude).next_up();
        let (curve, arithmetic) = if edge.is_linear() {
            (Curve::Line, arithmetic)
        } else {
            make_parabola(
                &diagram.cells()[cells[0].usize()],
                &diagram.cells()[cells[1].usize()],
                &sites,
                local,
                endpoints,
                origin,
                arithmetic,
            )?
        };
        let proposal = MedialEdge {
            supports,
            endpoints,
            vertex_ids: ids,
            curve,
            arithmetic,
        };
        let middle = proposal.at(0.5);
        let qmiddle = mul(sub(middle, origin), SCALE);
        if !qmiddle
            .into_iter()
            .all(|v| v.is_finite() && native::merge_predicate_coordinate(v))
        {
            return Err(condition(
                "skeleton midpoint is outside the predicate envelope",
            ));
        }
        // A finite Voronoi arc cannot cross an input segment away from its
        // zero-radius endpoint. Its strict interior point labels the entire
        // open arc's component in the validated integer arrangement. This is
        // computational graph filtering ONLY, never commanded-bead authority.
        if !inside_material(qmiddle, &sites, work, cancel)? {
            continue;
        }
        let next_vertices = retained_vertices
            .checked_add(ids.iter().filter(|&&id| degrees[id] == 0).count())
            .ok_or_else(storage)?;
        reserve_retained_edge(&mut edges, next_vertices, admission, staging)?;
        for id in ids {
            degrees[id] = degrees[id].checked_add(1).ok_or_else(storage)?;
        }
        retained_vertices = next_vertices;
        edges.push(proposal);
    }
    let graph_bytes = edges
        .capacity()
        .checked_mul(size_of::<MedialEdge>())
        .and_then(|b| b.checked_add(retained_vertices.checked_mul(size_of::<MedialJunction>())?))
        .ok_or_else(storage)?;
    admit_points(admission, retained_vertices)?;
    admit_bytes(
        admission,
        staging.checked_add(graph_bytes).ok_or_else(storage)?,
    )?;
    let mut junctions = Vec::with_capacity(retained_vertices);
    for (id, degree) in degrees.iter_mut().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        if *degree == 0 {
            *degree = usize::MAX;
            continue;
        }
        let v = &diagram.vertices()[id];
        let point = add(origin, [v.x() / SCALE, v.y() / SCALE]);
        let valence = *degree;
        *degree = junctions.len();
        junctions.push(MedialJunction { point, valence });
    }
    for edge in &mut edges {
        edge.vertex_ids = edge.vertex_ids.map(|id| degrees[id]);
    }
    let owned = edges
        .capacity()
        .checked_mul(size_of::<MedialEdge>())
        .and_then(|b| {
            b.checked_add(
                junctions
                    .capacity()
                    .checked_mul(size_of::<MedialJunction>())?,
            )
        })
        .ok_or_else(storage)?;
    admit_bytes(admission, owned)?;
    Ok(Skeleton {
        edges,
        junctions,
        retained_points: retained_vertices,
    })
}

fn quantize(p: [f64; 2], origin: [f64; 2]) -> Result<[i32; 2], ManufacturingError> {
    let mut q = [0; 2];
    for axis in 0..2 {
        let v = ((p[axis] - origin[axis]) * SCALE).round();
        if !v.is_finite() || v.abs() > (i32::MAX - 8) as f64 {
            return Err(condition("skeleton local-origin i32 extent exceeded"));
        }
        q[axis] = v as i32;
    }
    let reconstructed = world(q, origin);
    let magnitude = p
        .into_iter()
        .chain(origin)
        .fold(1.0f64, |a, b| a.max(b.abs()));
    let bound = (norm(sub(reconstructed, p)) + 32.0 * f64::EPSILON * magnitude).next_up();
    if !bound.is_finite() || bound > MAX_POINT_PERTURBATION_MM {
        return Err(condition(
            "skeleton point perturbation cannot be enclosed within 1e-6 mm",
        ));
    }
    Ok(q)
}

fn validate_sites(
    sites: &[Site],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut order: Vec<usize> = (0..sites.len()).collect();
    boolean::sort_by(&mut order, |&i| sites[i].min[0], work, cancel)?;
    for (at, &i) in order.iter().enumerate() {
        check_cancel(cancel)?;
        let a = &sites[i];
        for &j in &order[at + 1..] {
            boolean::charge(work, 1)?;
            let b = &sites[j];
            if b.min[0] > a.max[0] {
                break;
            }
            if b.min[1] > a.max[1] || a.min[1] > b.max[1] {
                continue;
            }
            check_cancel(cancel)?;
            let adjacent = a.support.boundary_index == b.support.boundary_index
                && ((a.support.segment_index as usize + 1) % a.ring_len
                    == b.support.segment_index as usize
                    || (b.support.segment_index as usize + 1) % b.ring_len
                        == a.support.segment_index as usize);
            validate_pair(
                [a.support.a, a.support.b],
                [b.support.a, b.support.b],
                adjacent,
            )?;
            validate_pair(
                a.q.map(|p| p.map(f64::from)),
                b.q.map(|p| p.map(f64::from)),
                adjacent,
            )?;
        }
    }
    Ok(())
}

fn validate_pair(
    a: [[f64; 2]; 2],
    b: [[f64; 2]; 2],
    adjacent: bool,
) -> Result<(), ManufacturingError> {
    let ab = [orient(a[0], a[1], b[0]), orient(a[0], a[1], b[1])];
    let ba = [orient(b[0], b[1], a[0]), orient(b[0], b[1], a[1])];
    let shared = a.iter().filter(|p| b.contains(p)).count();
    if adjacent && shared == 1 {
        // Collinear adjacent edges may continue monotonically, never reverse
        // over a positive subsegment. Keep original edges and source indices.
        for p in a {
            if on_open(p, b) {
                return Err(condition("collinear boundary backtracking/overlap"));
            }
        }
        for p in b {
            if on_open(p, a) {
                return Err(condition("collinear boundary backtracking/overlap"));
            }
        }
        return Ok(());
    }
    let opposite = |x: Orientation, y: Orientation| {
        x != Orientation::Collinear && y != Orientation::Collinear && x != y
    };
    let intersects = (opposite(ab[0], ab[1]) && opposite(ba[0], ba[1]))
        || b.into_iter().any(|p| on_closed(p, a))
        || a.into_iter().any(|p| on_closed(p, b));
    if intersects {
        return Err(condition(
            "skeleton boundaries cross, touch, or positively overlap",
        ));
    }
    Ok(())
}

fn validate_rings(
    sites: &[Site],
    rings: &[Ring],
    quantized: bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for ring in rings {
        check_cancel(cancel)?;
        let witness = coordinates(&sites[ring.first], quantized)[0];
        let mut own_outer = false;
        let mut own_holes = 0usize;
        // No intersections exist after the sweep, so one witness determines
        // containment. Charge all broad-phase scans as well as narrow predicates.
        for other in rings {
            boolean::charge(work, 1)?;
            if std::ptr::eq(ring, other) {
                continue;
            }
            if !quantized && (0..2).any(|a| witness[a] < other.min[a] || witness[a] > other.max[a])
            {
                continue;
            }
            if inside_ring(witness, sites, other, quantized, work, cancel)?
                && other.polygon == ring.polygon
            {
                if other.hole {
                    own_holes += 1;
                } else {
                    own_outer = true;
                }
            }
        }
        if (ring.hole && (!own_outer || own_holes != 0)) || (!ring.hole && own_holes != 0) {
            return Err(condition(
                "skeleton hole/exterior association is not a disjoint filled polygon",
            ));
        }
        if !ring.hole {
            for other in rings
                .iter()
                .filter(|r| !r.hole && r.polygon != ring.polygon)
            {
                boolean::charge(work, 1)?;
                if !inside_ring(witness, sites, other, quantized, work, cancel)? {
                    continue;
                }
                let mut in_hole = false;
                for hole in rings
                    .iter()
                    .filter(|r| r.hole && r.polygon == other.polygon)
                {
                    boolean::charge(work, 1)?;
                    in_hole |= inside_ring(witness, sites, hole, quantized, work, cancel)?;
                }
                if !in_hole {
                    return Err(condition("skeleton polygon interiors overlap"));
                }
            }
        }
    }
    Ok(())
}

fn inside_ring(
    p: [f64; 2],
    sites: &[Site],
    ring: &Ring,
    quantized: bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let mut inside = false;
    for site in &sites[ring.first..ring.first + ring.len] {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        inside ^= ray_crossing(p, coordinates(site, quantized))?;
    }
    Ok(inside)
}

fn inside_material(
    p: [f64; 2],
    sites: &[Site],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let mut inside = false;
    for site in sites {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        inside ^= ray_crossing(p, coordinates(site, true))?;
    }
    Ok(inside)
}

fn ray_crossing(p: [f64; 2], [a, b]: [[f64; 2]; 2]) -> Result<bool, ManufacturingError> {
    let orientation = orient(a, b, p);
    if orientation == Orientation::Collinear && on_closed(p, [a, b]) {
        return Err(condition(
            "skeleton classification touches a computational boundary",
        ));
    }
    Ok(if a[1] <= p[1] && p[1] < b[1] {
        orientation == Orientation::CounterClockwise
    } else if b[1] <= p[1] && p[1] < a[1] {
        orientation == Orientation::Clockwise
    } else {
        false
    })
}

fn validate_diagram(
    d: &Diagram,
    sites: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if d.num_cells() > sites * 3 || d.num_edges() > sites * 24 || d.num_vertices() > sites * 6 {
        return Err(storage());
    }
    for (index, cell) in d.cells().iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        if cell.id().usize() != index || cell.source_index().usize() >= sites {
            return Err(condition(
                "segment-Voronoi returned invalid cell/source indices",
            ));
        }
        if let Some(incident) = cell.get_incident_edge() {
            let edge = d.edges().get(incident.usize()).ok_or_else(bad_diagram)?;
            if edge.cell().map_err(diagram_error)? != cell.id() {
                return Err(bad_diagram());
            }
        }
        if cell.contains_point() == cell.contains_segment() {
            return Err(bad_diagram());
        }
    }
    for (index, vertex) in d.vertices().iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        if vertex.get_id().usize() != index
            || ![vertex.x(), vertex.y()]
                .into_iter()
                .all(|v| v.is_finite() && native::merge_predicate_coordinate(v))
        {
            return Err(bad_diagram());
        }
        let incident = vertex.get_incident_edge().map_err(diagram_error)?;
        let edge = d.edges().get(incident.usize()).ok_or_else(bad_diagram)?;
        if edge.vertex0() != Some(vertex.get_id()) {
            return Err(bad_diagram());
        }
    }
    for (index, edge) in d.edges().iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let twin_id = edge.twin().map_err(diagram_error)?;
        let next_id = edge.next().map_err(diagram_error)?;
        let prev_id = edge.prev().map_err(diagram_error)?;
        let twin = d.edges().get(twin_id.usize()).ok_or_else(bad_diagram)?;
        let next = d.edges().get(next_id.usize()).ok_or_else(bad_diagram)?;
        let prev = d.edges().get(prev_id.usize()).ok_or_else(bad_diagram)?;
        let cell = edge.cell().map_err(diagram_error)?;
        if edge.id().usize() != index
            || twin_id == edge.id()
            || twin.twin().map_err(diagram_error)? != edge.id()
            || next.prev().map_err(diagram_error)? != edge.id()
            || prev.next().map_err(diagram_error)? != edge.id()
            || next.cell().map_err(diagram_error)? != cell
            || prev.cell().map_err(diagram_error)? != cell
            || cell.usize() >= d.num_cells()
            || twin.is_primary() != edge.is_primary()
            || twin.is_linear() != edge.is_linear()
            || edge
                .vertex0()
                .is_some_and(|v| v.usize() >= d.num_vertices())
            || next.vertex0() != twin.vertex0()
        {
            return Err(bad_diagram());
        }
    }
    Ok(())
}

fn support(cell: &Cell, sites: &[Site]) -> Result<BoundarySupport, ManufacturingError> {
    let mut result = sites
        .get(cell.source_index().usize())
        .ok_or_else(bad_diagram)?
        .support;
    result.point = match cell.source_category() {
        SourceCategory::Segment => None,
        SourceCategory::SegmentStart => Some(result.a),
        SourceCategory::SegmentEnd => Some(result.b),
        SourceCategory::SinglePoint => return Err(bad_diagram()),
    };
    Ok(result)
}

fn make_parabola(
    c0: &Cell,
    c1: &Cell,
    sites: &[Site],
    endpoints_local: [[f64; 2]; 2],
    endpoints: [[f64; 2]; 2],
    origin: [f64; 2],
    arithmetic: f64,
) -> Result<(Curve, f64), ManufacturingError> {
    let (point, segment) = if c0.contains_point() && c1.contains_segment() {
        (c0, c1)
    } else if c1.contains_point() && c0.contains_segment() {
        (c1, c0)
    } else {
        return Err(bad_diagram());
    };
    let ps = sites
        .get(point.source_index().usize())
        .ok_or_else(bad_diagram)?;
    let ss = sites
        .get(segment.source_index().usize())
        .ok_or_else(bad_diagram)?;
    let fq = match point.source_category() {
        SourceCategory::SegmentStart => ps.q[0],
        SourceCategory::SegmentEnd => ps.q[1],
        _ => return Err(bad_diagram()),
    };
    let a = ss.q[0].map(|v| v as f64 / SCALE);
    let dq = [
        i64::from(ss.q[1][0]) - i64::from(ss.q[0][0]),
        i64::from(ss.q[1][1]) - i64::from(ss.q[0][1]),
    ];
    let f = [
        i64::from(fq[0]) - i64::from(ss.q[0][0]),
        i64::from(fq[1]) - i64::from(ss.q[0][1]),
    ];
    let length = (dq[0] as f64).hypot(dq[1] as f64);
    let u = [dq[0] as f64 / length, dq[1] as f64 / length];
    let n = [-u[1], u[0]];
    // i32 differences and their products fit i128. Exact dot/cross avoid
    // cancellation in near-collinear integer focus/directrix configurations.
    let cross = i128::from(dq[0]) * i128::from(f[1]) - i128::from(dq[1]) * i128::from(f[0]);
    let projection = i128::from(dq[0]) * i128::from(f[0]) + i128::from(dq[1]) * i128::from(f[1]);
    if cross == 0 {
        return Err(condition("zero-height point/segment parabola"));
    }
    let focus = [
        projection as f64 / (length * SCALE),
        cross as f64 / (length * SCALE),
    ];
    let x = endpoints_local.map(|p| dot(sub(p, a), u));
    let xmax = x[0].abs().max(x[1].abs());
    let delta = (x[0] - focus[0]).abs().max((x[1] - focus[0]).abs());
    let ymax = (delta * delta + focus[1] * focus[1]) / (2.0 * focus[1].abs());
    let curvature = (x[1] - x[0]).powi(2) / (8.0 * focus[1].abs());
    // Absolute forward-error enclosure for evaluating the stored f64
    // coefficients, including x interpolation and y's derivative amplification.
    // All integer-site scales here stay far from f64 underflow.
    let evaluation = (1024.0
        * f64::EPSILON
        * (1.0
            + norm(origin)
            + norm(a)
            + xmax
            + ymax
            + curvature
            + delta / focus[1].abs() * (xmax + focus[0].abs())))
    .next_up()
    .max(arithmetic);
    if !evaluation.is_finite() || !curvature.is_finite() {
        return Err(condition("segment-Voronoi parabolic arithmetic overflow"));
    }
    let mut endpoint_error = 0.0f64;
    for i in 0..2 {
        let computed = parabola_point(origin, a, u, n, focus, x[i]);
        let discrepancy = norm(sub(computed, endpoints[i]));
        if !discrepancy.is_finite() {
            return Err(condition(&format!(
                "segment-Voronoi parabola endpoint is not numerically consistent with its sites: endpoint={i}, point_site={:?}/{:?}, segment_site={:?}, focus={focus:?}, x={x:?}, local_endpoints={endpoints_local:?}, computed={computed:?}, endpoint_world={:?}, discrepancy={discrepancy:.17e}, evaluation={evaluation:.17e}, origin={origin:?}",
                ps.q,
                point.source_category(),
                ss.q,
                endpoints[i]
            )));
        }
        // `evaluation` bounds OUR coefficient arithmetic, not the backend's
        // construction of a shared finite vertex. Retain their measured
        // discrepancy separately; never weld that vertex onto the parabola.
        // The resulting curve/chord proposal still needs full D qualification.
        endpoint_error = endpoint_error.max(discrepancy);
    }
    let endpoint_error = (endpoint_error + evaluation).next_up();
    if !endpoint_error.is_finite() {
        return Err(condition("segment-Voronoi endpoint discrepancy overflow"));
    }
    Ok((
        Curve::Parabola {
            origin,
            a,
            u,
            n,
            focus,
            x,
            endpoint_error,
        },
        evaluation,
    ))
}

fn parabola_point(
    origin: [f64; 2],
    a: [f64; 2],
    u: [f64; 2],
    n: [f64; 2],
    f: [f64; 2],
    x: f64,
) -> [f64; 2] {
    let y = ((x - f[0]).powi(2) + f[1] * f[1]) / (2.0 * f[1]);
    add(origin, add(a, add(mul(u, x), mul(n, y))))
}
fn coordinates(s: &Site, quantized: bool) -> [[f64; 2]; 2] {
    if quantized {
        s.q.map(|p| p.map(f64::from))
    } else {
        [s.support.a, s.support.b]
    }
}
fn orient(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> Orientation {
    RobustKernel::orient2d(Coord::from(a), Coord::from(b), Coord::from(p))
}
fn on_closed(p: [f64; 2], e: [[f64; 2]; 2]) -> bool {
    (0..2).all(|a| p[a] >= e[0][a].min(e[1][a]) && p[a] <= e[0][a].max(e[1][a]))
        && orient(e[0], e[1], p) == Orientation::Collinear
}
fn on_open(p: [f64; 2], e: [[f64; 2]; 2]) -> bool {
    p != e[0] && p != e[1] && on_closed(p, e)
}
fn world(q: [i32; 2], origin: [f64; 2]) -> [f64; 2] {
    add(origin, q.map(|v| v as f64 / SCALE))
}
fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn mul(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn norm(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}
fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    add(a, mul(sub(b, a), t))
}
fn constructor_staging(count: usize, rings: usize) -> Result<usize, ManufacturingError> {
    count
        .checked_mul(8192 + size_of::<Site>() + 7 * size_of::<usize>())
        .and_then(|b| b.checked_add(rings.checked_mul(size_of::<Ring>())?))
        .ok_or_else(storage)
}
// Geometric growth gives amortized linear copying, while clamping each requested
// capacity to the remaining retained graph allowance. No reserve is issued for
// a discarded exterior/hole/secondary edge. Account a possible moving realloc's
// old AND new buffers against staging before requesting the allocation.
fn reserve_retained_edge(
    edges: &mut Vec<MedialEdge>,
    vertices: usize,
    retained: Admission,
    staging: usize,
) -> Result<(), ManufacturingError> {
    admit_points(retained, vertices)?;
    let vertex_bytes = vertices
        .checked_mul(size_of::<MedialJunction>())
        .ok_or_else(storage)?;
    let old_bytes = edges
        .capacity()
        .checked_mul(size_of::<MedialEdge>())
        .ok_or_else(storage)?;
    if edges.len() == edges.capacity() {
        let remaining = (MAX_MANUFACTURING_RETAINED_BYTES as usize)
            .checked_sub(retained.external_owned_bytes)
            .and_then(|b| b.checked_sub(staging))
            .and_then(|b| b.checked_sub(vertex_bytes))
            .and_then(|b| b.checked_sub(old_bytes))
            .ok_or_else(storage)?;
        let max_capacity = remaining / size_of::<MedialEdge>();
        if edges.len().checked_add(1).ok_or_else(storage)? > max_capacity {
            return Err(storage());
        }
        let desired = if edges.capacity() == 0 {
            8
        } else {
            edges.capacity().checked_mul(2).ok_or_else(storage)?
        };
        let capacity = desired.min(max_capacity);
        let new_bytes = capacity
            .checked_mul(size_of::<MedialEdge>())
            .ok_or_else(storage)?;
        let peak = staging
            .checked_add(new_bytes)
            .and_then(|b| b.checked_add(old_bytes))
            .and_then(|b| b.checked_add(vertex_bytes))
            .ok_or_else(storage)?;
        admit_bytes(retained, peak)?;
        edges
            .try_reserve_exact(capacity - edges.len())
            .map_err(|_| storage())?;
    }
    let owned = edges
        .capacity()
        .checked_mul(size_of::<MedialEdge>())
        .and_then(|b| b.checked_add(vertex_bytes))
        .ok_or_else(storage)?;
    admit_bytes(retained, staging.checked_add(owned).ok_or_else(storage)?)
}

fn admit_points(retained: Admission, points: usize) -> Result<(), ManufacturingError> {
    if retained
        .retained_points
        .checked_add(points)
        .is_none_or(|n| n > MAX_RETAINED_GEOMETRY_UNITS)
    {
        return Err(storage());
    }
    Ok(())
}
fn admit_bytes(retained: Admission, bytes: usize) -> Result<(), ManufacturingError> {
    if retained.retained_points > MAX_RETAINED_GEOMETRY_UNITS
        || retained
            .external_owned_bytes
            .checked_add(bytes)
            .is_none_or(|b| b as u64 > MAX_MANUFACTURING_RETAINED_BYTES)
    {
        return Err(storage());
    }
    Ok(())
}
fn storage() -> ManufacturingError {
    error(
        ManufacturingErrorCode::ResourceLimit,
        "bounded segment-skeleton storage/work admission exceeded",
    )
}
fn condition(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::NativeConditioningFailure, message)
}
fn bad_diagram() -> ManufacturingError {
    condition("segment-Voronoi returned inconsistent finite geometry or half-edge structure")
}
fn diagram_error(e: boostvoronoi::BvError) -> ManufacturingError {
    condition(&format!("invalid segment-Voronoi structure: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Contains, LineString, Point, Polygon};
    use std::collections::VecDeque;

    fn ring(points: &[[f64; 2]]) -> LineString<f64> {
        LineString::from(points.iter().map(|p| (p[0], p[1])).collect::<Vec<_>>())
    }

    fn polygon(points: &[[f64; 2]]) -> MultiPolygon<f64> {
        MultiPolygon(vec![Polygon::new(ring(points), vec![])])
    }

    fn build(material: &MultiPolygon<f64>) -> Skeleton {
        construct(material, 0, 0, &mut 0, &AtomicBool::new(false))
            .expect("independent polygon has an admitted medial construction")
    }

    fn incidence(graph: &Skeleton) -> Vec<Vec<usize>> {
        let mut neighbors = vec![Vec::new(); graph.junctions.len()];
        for edge in &graph.edges {
            let [a, b] = edge.vertex_ids;
            assert_eq!(
                edge.endpoints,
                [graph.junctions[a].point, graph.junctions[b].point]
            );
            neighbors[a].push(b);
            neighbors[b].push(a);
        }
        for (vertex, links) in graph.junctions.iter().zip(&neighbors) {
            assert_eq!(
                vertex.valence,
                links.len(),
                "degree is retained undirected incidence"
            );
        }
        neighbors
    }

    fn assert_arm_connections(graph: &Skeleton, start: usize, arms: &[fn([f64; 2]) -> bool]) {
        let neighbors = incidence(graph);
        let mut reached = vec![false; neighbors.len()];
        let mut queue = VecDeque::from([start]);
        reached[start] = true;
        while let Some(index) = queue.pop_front() {
            for &next in &neighbors[index] {
                if !reached[next] {
                    reached[next] = true;
                    queue.push_back(next);
                }
            }
        }
        for arm in arms {
            assert!(
                graph
                    .junctions
                    .iter()
                    .enumerate()
                    .any(|(i, v)| reached[i] && arm(v.point)),
                "one junction component must reach every required material arm"
            );
        }
    }

    #[test]
    fn skeleton_x_and_t_keep_true_junction_arm_incidence() {
        let cross = polygon(&[
            [-1., -6.],
            [1., -6.],
            [1., -1.],
            [6., -1.],
            [6., 1.],
            [1., 1.],
            [1., 6.],
            [-1., 6.],
            [-1., 1.],
            [-6., 1.],
            [-6., -1.],
            [-1., -1.],
            [-1., -6.],
        ]);
        let graph = build(&cross);
        let center = graph
            .junctions
            .iter()
            .position(|v| norm(v.point) < 1e-8 && v.valence == 4)
            .expect("symmetric cross has one true four-way medial vertex");
        assert_arm_connections(
            &graph,
            center,
            &[|p| p[0] < -4., |p| p[0] > 4., |p| p[1] < -4., |p| p[1] > 4.],
        );
        let tee = polygon(&[
            [-1., -6.],
            [1., -6.],
            [1., -1.],
            [6., -1.],
            [6., 1.],
            [-6., 1.],
            [-6., -1.],
            [-1., -1.],
            [-1., -6.],
        ]);
        let graph = build(&tee);
        let center = graph
            .junctions
            .iter()
            .position(|v| {
                v.point[0].abs() < 1e-8 && v.point[1] > -2. && v.point[1] < 1. && v.valence >= 3
            })
            .expect("T has a true central medial branch, not three disconnected strokes");
        assert_arm_connections(
            &graph,
            center,
            &[|p| p[0] < -4., |p| p[0] > 4., |p| p[1] < -4.],
        );
    }

    #[test]
    fn skeleton_hole_preserves_cycle_and_original_boundary_supports() {
        let material = MultiPolygon(vec![Polygon::new(
            ring(&[[-8., -8.], [8., -8.], [8., 8.], [-8., 8.], [-8., -8.]]),
            vec![ring(&[
                [-2., -2.],
                [-2., 2.],
                [2., 2.],
                [2., -2.],
                [-2., -2.],
            ])],
        )]);
        let graph = build(&material);
        let neighbors = incidence(&graph);
        let mut degree: Vec<_> = neighbors.iter().map(Vec::len).collect();
        let mut queue: VecDeque<_> = degree
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| (d < 2).then_some(i))
            .collect();
        while let Some(i) = queue.pop_front() {
            for &j in &neighbors[i] {
                if degree[j] >= 2 {
                    degree[j] -= 1;
                    if degree[j] == 1 {
                        queue.push_back(j);
                    }
                }
            }
            degree[i] = 0;
        }
        assert!(
            degree.iter().any(|&d| d >= 2),
            "protected hole requires an actual graph cycle"
        );
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.supports[0].boundary_index != e.supports[1].boundary_index),
            "inner and outer boundary must both support the material skeleton"
        );
        for edge in &graph.edges {
            for support in edge.supports {
                let source = if support.boundary_index == 0 {
                    material.0[0].exterior()
                } else {
                    &material.0[0].interiors()[0]
                };
                let i = support.segment_index as usize;
                assert_eq!(support.a, [source.0[i].x, source.0[i].y]);
                assert_eq!(support.b, [source.0[i + 1].x, source.0[i + 1].y]);
            }
            for t in [0.1, 0.3, 0.5, 0.7, 0.9] {
                let p = edge.at(t);
                assert!(
                    material.contains(&Point::from(p)),
                    "no retained arc in the hole or exterior"
                );
                assert!((edge.supports[0].radius(p) - edge.supports[1].radius(p)).abs() < 2e-6);
            }
        }
    }

    #[test]
    fn near_collinear_supports_keep_shared_vertices_inside_curve_chord_enclosures() {
        // Isolate a real finite diagram event's two supports. This exercises
        // the curve consumer, not a material-authority claim for the support pair.
        let origin = [110.0, 109.99999999999977];
        let sites = [
            [[-10492882, 9436516], [-10493128, 9436516]],
            [[-10493373, 9436515], [-10493619, 9436514]],
        ]
        .into_iter()
        .enumerate()
        .map(|(index, q)| {
            let [a, b] = q.map(|p| add(origin, [p[0] as f64 / SCALE, p[1] as f64 / SCALE]));
            Site {
                support: BoundarySupport {
                    boundary_index: 0,
                    segment_index: index as u32,
                    a,
                    b,
                    point: None,
                },
                q,
                ring_len: 2,
                min: [a[0].min(b[0]), a[1].min(b[1])],
                max: [a[0].max(b[0]), a[1].max(b[1])],
            }
        })
        .collect::<Vec<_>>();
        let diagram = Builder::<i32>::default()
            .with_segments(
                sites
                    .iter()
                    .map(|s| [s.q[0][0], s.q[0][1], s.q[1][0], s.q[1][1]]),
            )
            .and_then(|builder| builder.build())
            .unwrap();
        let point = diagram
            .cells()
            .iter()
            .find(|cell| {
                cell.source_index().usize() == 0
                    && cell.source_category() == SourceCategory::SegmentEnd
            })
            .unwrap();
        let segment = diagram
            .cells()
            .iter()
            .find(|cell| cell.source_index().usize() == 1 && cell.contains_segment())
            .unwrap();
        let local = [
            [-10.122453212738037, 37.279099464416504],
            [-10.121984481819434, 37.16402626231431],
        ];
        let endpoints = local.map(|p| add(origin, p));
        let magnitude = endpoints
            .iter()
            .flatten()
            .fold(1.0_f64, |m, p| m.max(p.abs()));
        let arithmetic = (4096.0 * f64::EPSILON * magnitude).next_up();
        let (curve, arithmetic) = make_parabola(
            point, segment, &sites, local, endpoints, origin, arithmetic,
        )
        .expect(
            "finite shared vertex error is enclosed, not compared to a different arithmetic budget",
        );
        let edge = MedialEdge {
            supports: [
                support(point, &sites).unwrap(),
                support(segment, &sites).unwrap(),
            ],
            endpoints,
            vertex_ids: [0, 1],
            curve,
            arithmetic,
        };
        assert_eq!(edge.at(0.0), endpoints[0]);
        assert_eq!(edge.at(1.0), endpoints[1]);
        for [lo, hi] in [[0.0, 1.0], [0.9, 1.0], [1.0 - 2.0_f64.powi(-20), 1.0]] {
            let bound = edge.chord_error_bound(lo, hi);
            assert!(bound.is_finite());
            for i in 0..=64 {
                let t = i as f64 / 64.0;
                let p = edge.at(lo + (hi - lo) * t);
                assert!(norm(sub(p, lerp(edge.at(lo), edge.at(hi), t))) <= bound);
                assert!(
                    (edge.supports[0].radius(p) - edge.supports[1].radius(p)).abs()
                        <= 4.0 * arithmetic,
                    "the declared endpoint focus and finite segment must remain the actual supporting sites"
                );
            }
        }
    }

    #[test]
    fn skeleton_parabolic_subinterval_chords_enclose_curvature() {
        let material = polygon(&[
            [0., 0.],
            [6., 0.],
            [6., 2.],
            [2., 2.],
            [2., 6.],
            [0., 6.],
            [0., 0.],
        ]);
        let graph = build(&material);
        let mut curved = 0usize;
        for edge in &graph.edges {
            if !matches!(edge.curve, Curve::Parabola { .. }) {
                continue;
            }
            curved += 1;
            let whole = edge.chord_error_bound(0., 1.);
            assert!(
                whole > 0.0001,
                "real concavity curvature, not a linear library echo"
            );
            for [lo, hi] in [[0., 1.], [0., 0.5], [0.25, 0.75], [0.4, 0.6], [0.5, 1.]] {
                let bound = edge.chord_error_bound(lo, hi);
                for i in 0..=64 {
                    let u = i as f64 / 64.;
                    let actual = edge.at(lo + (hi - lo) * u);
                    let chord = lerp(edge.at(lo), edge.at(hi), u);
                    assert!(
                        norm(sub(actual, chord)) <= bound,
                        "analytic curve escaped its commanded-chord enclosure"
                    );
                }
            }
            assert!(
                edge.chord_error_bound(0.25, 0.75) < whole * 0.26,
                "quadratic subdivision must tighten deviation rather than keep a global heuristic"
            );
        }
        assert!(
            curved >= 2,
            "L-shaped concavity exposes both point/segment parabolas"
        );
    }

    #[test]
    fn skeleton_refuses_cross_touch_and_collinear_backtracking_before_builder() {
        let invalid = [
            polygon(&[[0., 0.], [4., 4.], [0., 4.], [4., 0.], [0., 0.]]),
            polygon(&[[0., 0.], [4., 0.], [2., 0.], [2., 2.], [0., 2.], [0., 0.]]),
            MultiPolygon(vec![
                Polygon::new(
                    ring(&[[0., 0.], [2., 0.], [2., 2.], [0., 2.], [0., 0.]]),
                    vec![],
                ),
                Polygon::new(
                    ring(&[[2., 2.], [4., 2.], [4., 4.], [2., 4.], [2., 2.]]),
                    vec![],
                ),
            ]),
            MultiPolygon(vec![Polygon::new(
                ring(&[[0., 0.], [4., 0.], [4., 4.], [0., 4.], [0., 0.]]),
                vec![ring(&[[0., 1.], [1., 1.], [1., 2.], [0., 2.], [0., 1.]])],
            )]),
        ];
        for material in invalid {
            let failure = construct(&material, 0, 0, &mut 0, &AtomicBool::new(false))
                .err()
                .expect("invalid constructor premises must produce a typed refusal");
            assert_eq!(
                failure.code,
                ManufacturingErrorCode::NativeConditioningFailure
            );
        }
        let collinear = polygon(&[[0., 0.], [2., 0.], [4., 0.], [4., 3.], [0., 3.], [0., 0.]]);
        let graph = build(&collinear);
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.supports.iter().any(|s| s.segment_index == 1)),
            "monotone original collinear sampling retains its original support index"
        );
    }

    #[test]
    fn skeleton_quantization_and_resource_boundaries_are_explicit() {
        let ordinary = polygon(&[
            [0.123456789, 0.],
            [4.123456789, 0.],
            [4.123456789, 3.],
            [0.123456789, 3.],
            [0.123456789, 0.],
        ]);
        let graph = build(&ordinary);
        assert!(graph.retained_points > 0);
        let origin = [2.123456789, 1.5];
        for p in &ordinary.0[0].exterior().0 {
            let p = [p.x, p.y];
            assert!(
                norm(sub(
                    world(quantize(p, origin).expect("admitted grid"), origin),
                    p
                )) <= MAX_POINT_PERTURBATION_MM
            );
        }
        let collapsed = polygon(&[[0., 0.], [1e-8, 0.], [4., 0.], [4., 3.], [0., 3.], [0., 0.]]);
        assert_eq!(
            construct(&collapsed, 0, 0, &mut 0, &AtomicBool::new(false))
                .err()
                .expect("positive edge collapse refuses")
                .code,
            ManufacturingErrorCode::NativeConditioningFailure
        );
        assert_eq!(
            construct(
                &ordinary,
                MAX_RETAINED_GEOMETRY_UNITS,
                0,
                &mut 0,
                &AtomicBool::new(false)
            )
            .err()
            .expect("live graph geometry reservation refuses before allocation")
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
        let mut exhausted_work = super::super::super::MAX_BOOLEAN_PAIRS;
        assert_eq!(
            construct(
                &ordinary,
                0,
                0,
                &mut exhausted_work,
                &AtomicBool::new(false)
            )
            .err()
            .expect("shared work limit refuses")
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
        assert!(construct(&ordinary, 0, 0, &mut 0, &AtomicBool::new(true)).is_err());
        assert_eq!(
            construct(
                &ordinary,
                0,
                MAX_MANUFACTURING_RETAINED_BYTES as usize,
                &mut 0,
                &AtomicBool::new(false)
            )
            .err()
            .expect("total external byte ownership is independently admitted")
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::*;
    use geo::{LineString, Polygon};

    #[test]
    fn skeleton_retained_capacity_excludes_discarded_diagram_edges() {
        let mut points = Vec::with_capacity(629);
        for i in 0..628 {
            let angle = std::f64::consts::TAU * i as f64 / 628.;
            points.push((10. * angle.cos(), 10. * angle.sin()));
        }
        points.push(points[0]);
        let material = MultiPolygon(vec![Polygon::new(LineString::from(points), vec![])]);
        let graph = construct(&material, 0, 0, &mut 0, &AtomicBool::new(false))
            .expect("sampled round material has an admitted compact interior graph");
        let retained = MAX_RETAINED_GEOMETRY_UNITS - graph.retained_points;
        let edge_bytes = graph.edges.capacity() * size_of::<MedialEdge>();
        let graph_peak =
            edge_bytes + edge_bytes / 2 + graph.junctions.capacity() * size_of::<MedialJunction>();
        let external = MAX_MANUFACTURING_RETAINED_BYTES as usize
            - constructor_staging(628, 1).unwrap()
            - graph_peak;
        let pressured = construct(
            &material,
            retained,
            external,
            &mut 0,
            &AtomicBool::new(false),
        )
        .expect("byte allowance for the compact retained graph excludes discarded diagram edges");
        assert_eq!(graph.edges.len(), pressured.edges.len());
        assert_eq!(graph.junctions.len(), pressured.junctions.len());
        assert!(pressured.retained_points <= graph.retained_points);
        for (a, b) in graph.edges.iter().zip(&pressured.edges) {
            assert_eq!(a.endpoints, b.endpoints);
            assert_eq!(a.vertex_ids, b.vertex_ids);
        }
    }
}
