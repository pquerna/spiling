// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Process-continuing boundary cells transport contours, not false thickness.
//! Every value here is a construction proposal; original-D occupancy and actual
//! command qualification remain independent authorities.
use super::*;
use geo::{
    Coord, Distance, Euclidean, Line, Point,
    kernels::{Kernel, Orientation, RobustKernel},
};
use std::{cmp::Ordering, collections::BinaryHeap};

#[derive(Clone, Copy)]
pub(super) struct VertexField {
    pub width: f64,
    pub pairs: usize,
    pub central: bool,
}

/// One outer-lane field, shared by birth, sampling and completion. Endpoint
/// cones recover the preferred width locally, not across an entire long cell.
#[derive(Clone, Copy, Debug)]
pub(super) struct OuterWidth {
    pub endpoints: [f64; 2],
    preferred: f64,
    travel: Option<f64>,
}
impl OuterWidth {
    pub fn new(endpoints: [f64; 2], preferred: f64, distance: Option<f64>) -> Self {
        Self {
            endpoints,
            preferred,
            travel: distance.map(|distance| {
                (distance * program_coordinates::MAX_WALL_WIDTH_GRADIENT * 0.9)
                    .max((endpoints[1] - endpoints[0]).abs())
            }),
        }
    }
    pub fn at(self, t: f64) -> f64 {
        if t == 0.0 {
            return self.endpoints[0];
        }
        if t == 1.0 {
            return self.endpoints[1];
        }
        let Some(travel) = self.travel else {
            return self.endpoints[0] + (self.endpoints[1] - self.endpoints[0]) * t;
        };
        let lower = (self.endpoints[0] - travel * t).max(self.endpoints[1] - travel * (1.0 - t));
        let upper = (self.endpoints[0] + travel * t).min(self.endpoints[1] + travel * (1.0 - t));
        self.preferred.max(lower).min(upper)
    }
    /// All affine switches, including lower cones for wider incoming ports.
    /// Sorting this fixed stack buffer is charged by the caller.
    pub fn knots(self) -> [f64; 8] {
        let Some(travel) = self.travel.filter(|travel| *travel > 0.0) else {
            return [0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        };
        let [a, b] = self.endpoints;
        [
            0.0,
            1.0,
            (self.preferred - a) / travel,
            1.0 + (b - self.preferred) / travel,
            (a - self.preferred) / travel,
            1.0 + (self.preferred - b) / travel,
            (b + travel - a) / (2.0 * travel),
            (a + travel - b) / (2.0 * travel),
        ]
        .map(|t| t.clamp(0.0, 1.0))
    }
}

/// Retained literal convex-corner incidence, never a coordinate-created join.
pub(super) fn source_corner(
    curve: &MedialEdge,
    graph: &skeleton::Skeleton,
) -> Option<([f64; 2], usize, [[f64; 2]; 2])> {
    let [a, b] = curve.supports;
    if a.point.is_some() || b.point.is_some() || a.boundary_index != b.boundary_index {
        return None;
    }
    let vertex = [a.a, a.b].into_iter().find(|p| *p == b.a || *p == b.b)?;
    let rays = [
        if a.a == vertex {
            sub(a.b, vertex)
        } else {
            sub(a.a, vertex)
        },
        if b.a == vertex {
            sub(b.b, vertex)
        } else {
            sub(b.a, vertex)
        },
    ];
    let end = (0..2).find(|&end| {
        graph.junctions[curve.vertex_ids[end]].valence == 1
            && norm(sub(curve.endpoints[end], vertex))
                <= 0.000001 + curve.chord_error_bound(0.0, 1.0)
    })?;
    let direction = sub(curve.endpoints[1 - end], vertex);
    if rays.into_iter().any(|ray| dot(direction, ray) <= 0.0) {
        return None;
    }
    Some((vertex, end, rays))
}

/// Only a retained terminal cell with two original segments sharing a literal
/// convex vertex supplies this proposal. No coordinate match creates incidence.
pub(super) fn corner_seed(
    curve: &MedialEdge,
    graph: &skeleton::Skeleton,
    widths: &[f64],
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<(usize, f64)>, ManufacturingError> {
    let Some((vertex, end, rays)) = source_corner(curve, graph) else {
        return Ok(None);
    };
    let cosine = dot(rays[0], rays[1]) / (norm(rays[0]) * norm(rays[1]));
    if !cosine.is_finite() || !(-1.0..1.0).contains(&cosine) {
        return Ok(None);
    }
    let sine_half = ((1.0 - cosine) * 0.5).sqrt();
    let cotangent_half = ((1.0 + cosine) / (1.0 - cosine)).sqrt();
    let rounded_gap = sine_half.recip() - 1.0;
    let extent = request.nozzle_diameter_mm * 0.25 - super::super::OFFSET_TOLERANCE_MM * 0.5;
    let uncertainty = program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM;
    let index = curve.vertex_ids[end];
    // Leave already eligible preferred contours (notably .45 mm) untouched.
    if widths[index] * 0.5 * rounded_gap + uncertainty + FIT_ERROR_MM <= extent {
        return Ok(None);
    }
    // This seed protects mandatory corner reachability, not optional image
    // detail. Original support vertices are retained before skeleton rounding.
    // The independent requested-D/source/void and mandatory-C proofs remain.
    boolean::charge(
        work,
        geo::BooleanOps::rings(request.required)
            .map(|ring| ring.0.len())
            .sum(),
    )?;
    check_cancel(cancel)?;
    let point = Point::from(vertex);
    if !request
        .required
        .polygons
        .iter()
        .any(|polygon| geo::Intersects::intersects(*polygon, &point))
    {
        return Ok(None);
    }
    // Reserve disk, sampling and fitting enclosure plus commanded uncertainty.
    // The first r=w/2 birth can grow from this source-node seed; bound that
    // growth in the original segment-foot metric before selecting the seed.
    let birth_width = 2.0 * (extent - uncertainty - 3.0 * FIT_ERROR_MM) / rounded_gap;
    let growth = 1.0 - program_coordinates::MAX_WALL_WIDTH_GRADIENT * 0.9 * cotangent_half * 0.5;
    let minimum = admitted_width_range(request)?[0];
    let proposal = (birth_width * growth).max(minimum).min(widths[index]);
    Ok((proposal < widths[index]).then_some((index, proposal)))
}

pub(super) fn cells(
    graph: &skeleton::Skeleton,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<Option<f64>>, ManufacturingError> {
    let mut cells = Vec::with_capacity(graph.edges.len());
    let [minimum, maximum] = admitted_width_range(request)?;
    let target = (2 * request.recipe.perimeter_count as usize).max(1) as f64;
    let gradient_squared = program_coordinates::MAX_WALL_WIDTH_GRADIENT.powi(2);
    let preferred = source_width_mm(request)?;
    for curve in &graph.edges {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let supports = curve.supports;
        if supports.iter().any(|site| site.point.is_some()) {
            if !point_field_cannot_separate(
                curve,
                [minimum, maximum],
                target,
                preferred,
                work,
                cancel,
            )? && !point_field_exceeds_gradient(curve, target, request, work, cancel)?
            {
                cells.push(None);
                continue;
            }
        } else {
            let midpoint = Coord::from(curve.at(0.5));
            let directions = supports.map(|site| Coord::from(sub(site.b, site.a)));
            let sides = supports.map(|site| {
                RobustKernel::orient2d(Coord::from(site.a), Coord::from(site.b), midpoint)
            });
            let tangent = RobustKernel::dot_product_sign(directions[0], directions[1]);
            let opposed = match (tangent, sides) {
                (Orientation::Collinear, _)
                | (_, [Orientation::Collinear, _])
                | (_, [_, Orientation::Collinear]) => false,
                (Orientation::CounterClockwise, [a, b]) => a != b,
                (Orientation::Clockwise, [a, b]) => a == b,
            };
            let tangent_vectors = directions.map(|v| [v.x, v.y]);
            let product = norm(tangent_vectors[0]) * norm(tangent_vectors[1]);
            let magnitude = dot(tangent_vectors[0], tangent_vectors[1]).abs();
            let thickness = curve
                .endpoints
                .map(|p| supports.into_iter().map(|site| site.radius(p)).sum::<f64>());
            if !product.is_finite()
                || !magnitude.is_finite()
                || thickness.into_iter().any(|v| !v.is_finite())
            {
                return Err(unsupported(
                    "source contour classification is unconditioned",
                ));
            }
            let count = (thickness[0].min(thickness[1]) / minimum)
                .floor()
                .max(1.0)
                .min(target);
            // For an opposing two-line field w=2r/n, the outer lane has
            // (dw/dL)^2=4(1+c)/(n²(1-c)+(1+c)), where c is normal cosine.
            // This proposes transport when even the largest admitted count
            // cannot continue; original material/process proofs still decide.
            let contour_rate = (4.0 - gradient_squared) * (product - magnitude)
                > gradient_squared * count * count * (product + magnitude);
            if !adjacent_supports(supports) && opposed && !contour_rate {
                cells.push(None);
                continue;
            }
        }
        // Segment feet bound tangential motion; point sites use the angular
        // motion of an admitted-radius offset, not their stationary feet.
        let magnitude = curve
            .endpoints
            .into_iter()
            .flatten()
            .fold(1.0f64, |m, value| m.max(value.abs()));
        let arithmetic =
            1024.0 * f64::EPSILON * magnitude + 2.0 * curve.chord_error_bound(0.0, 1.0);
        let distances = supports
            .map(|site| site_transport_distance(site, curve.endpoints, minimum, arithmetic));
        let distance = distances[0].min(distances[1]) - arithmetic;
        if distances.into_iter().any(|value| !value.is_finite()) || !distance.is_finite() {
            return Err(unsupported("source contour metric is unconditioned"));
        }
        cells.push(Some(distance.max(0.0)));
    }
    Ok(cells)
}

/// A single legal ordinary field must separate its two outer lanes. For every
/// n>=2 those lanes have centers f_i+(w/2)u_i, so their distance/w is at most
/// |f0-f1|/min_width+|u0-u1|/2. A conditioned thickness lower above max_width
/// excludes n=1. If every count below the recipe maximum exceeds max_width,
/// the cap-active maximum-count field has exactly the preferred width instead;
/// its mandatory inner pair can be tested directly. Both are necessary bounds
/// on this ordinary recipe objective, not on all possible manufacturing. They
/// propose transport only; original D/R, ports, permissions and dose still own
/// admissibility.
fn point_field_cannot_separate(
    curve: &skeleton::MedialEdge,
    widths: [f64; 2],
    target: f64,
    preferred: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    for p in curve.endpoints {
        check_cancel(cancel)?;
        boolean::charge(work, 128)?;
        let scale = p
            .into_iter()
            .chain(
                curve
                    .supports
                    .iter()
                    .flat_map(|site| site.a.into_iter().chain(site.b)),
            )
            .map(f64::abs)
            .fold(1.0, f64::max);
        let arithmetic = (1024.0 * f64::EPSILON * scale).next_up();
        let mut feet = [[0.0; 2]; 2];
        let mut errors = [0.0; 2];
        let mut radii_lower = [0.0; 2];
        let mut units = [[0.0; 2]; 2];
        let mut unit_errors = [0.0; 2];
        let mut conditioned = true;
        for (side, site) in curve.supports.iter().enumerate() {
            // canonical_foot may select an incident literal endpoint instead
            // of the projection. Enclose the ENTIRE original segment, hence
            // every allowed projection/endpoint choice, without guessing which
            // later contour/arm partition will own the incidence.
            let (foot, radius) = match site.point {
                Some(focus) => (focus, 0.0),
                None => (
                    add(site.a, mul(sub(site.b, site.a), 0.5)),
                    (norm(sub(site.b, site.a)) * 0.5).next_up(),
                ),
            };
            feet[side] = foot;
            errors[side] = (radius + arithmetic).next_up();
            let vector = sub(p, foot);
            let distance = norm(vector);
            let lower = (distance - errors[side] - arithmetic).next_down();
            if ![distance, lower, errors[side]]
                .into_iter()
                .all(f64::is_finite)
                || lower <= 0.0
            {
                conditioned = false;
                break;
            }
            radii_lower[side] = lower;
            units[side] = mul(vector, distance.recip());
            unit_errors[side] = ((2.0 * (errors[side] + arithmetic).next_up() / lower).next_up()
                + 128.0 * f64::EPSILON)
                .next_up();
        }
        if !conditioned {
            continue;
        }
        // This lower also bounds either radius-based thickness convention;
        // no exact equidistance/Voronoi assertion is used.
        let thickness_lower = (2.0 * radii_lower[0].min(radii_lower[1])).next_down();
        if !thickness_lower.is_finite() || thickness_lower <= widths[1] {
            continue;
        }
        let foot_distance = (((norm(sub(feet[0], feet[1])) + errors[0]).next_up() + errors[1])
            .next_up()
            + arithmetic)
            .next_up();
        let unit_distance = (((norm(sub(units[0], units[1])) + unit_errors[0]).next_up()
            + unit_errors[1])
            .next_up()
            + 128.0 * f64::EPSILON)
            .next_up();
        let upper =
            ((foot_distance / widths[0]).next_up() + (0.5 * unit_distance).next_up()).next_up();
        if upper.is_finite() && upper < 1.0 {
            return Ok(true);
        }
        let smaller_count_ceiling = ((target - 1.0) * widths[1]).next_up();
        let cap_threshold = (target * preferred).next_up();
        if target >= 2.0
            && (widths[0]..=widths[1]).contains(&preferred)
            && thickness_lower > smaller_count_ceiling.max(cap_threshold)
        {
            // normal/count_width admit n<target only at T/n<=max_width.
            // They admit the cap-active target ONLY at preferred. Its even
            // count has an innermost side pair at (target/2-.5)*preferred.
            let distance = (target * 0.5 - 0.5) * preferred;
            if distance.is_finite() && distance <= radii_lower[0].min(radii_lower[1]) {
                let q = [
                    add(feet[0], mul(units[0], distance)),
                    add(feet[1], mul(units[1], distance)),
                ];
                // For fixed d<=r_lower throughout the convex foot ball,
                // f -> f+d*(p-f)/|p-f| has Jacobian eigenvalues 1,1-d/r
                // in [0,1]. Thus EACH full canonical-foot ball contributes
                // at most its own radius, not an unqualified sampled normal.
                // Three arithmetic reserves cover both q evaluations and norm.
                let gap_upper = (((norm(sub(q[0], q[1])) + errors[0]).next_up() + errors[1])
                    .next_up()
                    + (3.0 * arithmetic).next_up())
                .next_up();
                if gap_upper.is_finite() && gap_upper < preferred {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Necessary rate bound for a continuing equal-width field with a point site.
/// q=f+(p-f)/(2n)+r_other*u/(2n), so |q'|<=(1+R)|p'|/n
/// for R>=r_other/r_point. Width change is |ΔT|/n: n cancels.
/// This only rejects that construction objective, never the source material.
fn point_field_exceeds_gradient(
    curve: &skeleton::MedialEdge,
    target: f64,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let cap = (target * source_width_mm(request)?).next_down();
    if point_interval_exceeds_gradient(curve, [0.0, 1.0], cap, work, cancel)? {
        return Ok(true);
    }
    check_cancel(cancel)?;
    boolean::charge(work, 24)?;
    let points = curve.endpoints;
    let deviation = curve.chord_error_bound(0.0, 1.0);
    let scale = points
        .into_iter()
        .flatten()
        .chain(
            curve
                .supports
                .into_iter()
                .flat_map(|site| site.a.into_iter().chain(site.b)),
        )
        .map(f64::abs)
        .fold(1.0, f64::max);
    let arithmetic = (1024.0 * f64::EPSILON * scale).next_up();
    let thickness = points.map(|p| {
        curve
            .supports
            .into_iter()
            .map(|site| site.radius(p))
            .sum::<f64>()
    });
    let upper =
        ((thickness[0].max(thickness[1]) + 2.0 * deviation).next_up() + arithmetic).next_up();
    let side = usize::from(thickness[1] < thickness[0]);
    let speed = (norm(sub(points[1], points[0])) + 4.0 * deviation).next_up();
    let denominator = (2.0 * speed).next_up();
    let mut budget = curve
        .supports
        .into_iter()
        .filter_map(|site| site.point)
        .map(|focus| (norm(sub(points[side], focus)) - arithmetic).next_down())
        .fold(f64::INFINITY, f64::min);
    if upper >= cap {
        budget = budget.min((cap - thickness[side] - 4.0 * deviation - arithmetic).next_down());
    }
    if ![
        deviation,
        arithmetic,
        upper,
        cap,
        speed,
        denominator,
        budget,
    ]
    .into_iter()
    .all(f64::is_finite)
        || deviation < 0.0
        || speed <= 0.0
        || denominator <= 0.0
        || budget <= 0.0
    {
        return Ok(false);
    }
    // Exactly one endpoint-local search proposal. The half-radius travel
    // reserve and cap headroom only select a range: they authorize nothing.
    // Recompute the same whole-interval predicate, including endpoint error.
    let step = (budget / denominator).next_down().clamp(0.0, 1.0);
    if step <= 0.0 {
        return Ok(false);
    }
    let range = if side == 0 {
        [0.0, step]
    } else {
        [1.0 - step, 1.0]
    };
    point_interval_exceeds_gradient(curve, range, cap, work, cancel)
}

fn point_interval_exceeds_gradient(
    curve: &skeleton::MedialEdge,
    range: [f64; 2],
    cap: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    check_cancel(cancel)?;
    boolean::charge(work, 32)?;
    let points = range.map(|t| curve.at(t));
    let deviation = curve.chord_error_bound(range[0], range[1]);
    let scale = points
        .into_iter()
        .flatten()
        .chain(
            curve
                .supports
                .into_iter()
                .flat_map(|site| site.a.into_iter().chain(site.b)),
        )
        .map(f64::abs)
        .fold(1.0, f64::max);
    let arithmetic = (1024.0 * f64::EPSILON * scale).next_up();
    let radii = points.map(|p| curve.supports.map(|site| site.radius(p)));
    let thickness = radii.map(|radii| radii.into_iter().sum::<f64>());
    let upper =
        ((thickness[0].max(thickness[1]) + 2.0 * deviation).next_up() + arithmetic).next_up();
    let uncertainty = (deviation + arithmetic).next_up();
    let length =
        ((norm(sub(points[1], points[0])) + 2.0 * deviation).next_up() + arithmetic).next_up();
    let change = ((thickness[1] - thickness[0]).abs() - 4.0 * uncertainty)
        .next_down()
        .max(0.0);
    // The nominal-core cap changes this field; do not apply the uncapped law.
    if ![uncertainty, upper, length, change]
        .into_iter()
        .all(f64::is_finite)
        || length <= 0.0
        || upper >= cap
    {
        return Ok(false);
    }
    let chord = Line::new(Coord::from(points[0]), Coord::from(points[1]));
    for side in 0..2 {
        let Some(focus) = curve.supports[side].point else {
            continue;
        };
        check_cancel(cancel)?;
        boolean::charge(work, 16)?;
        let lower = (Euclidean.distance(&chord, &Point::from(focus)) - uncertainty).next_down();
        if !lower.is_finite() || lower <= 0.0 {
            continue;
        }
        let other = curve.supports[1 - side];
        // A literal point of the other convex site gives
        // r_other<=r_point+d everywhere, without assuming exact bisector data.
        let distance = match other.point {
            Some(p) => norm(sub(p, focus)),
            None => norm(sub(other.a, focus)).min(norm(sub(other.b, focus))),
        };
        let distance = (distance + arithmetic).next_up();
        let triangle_ratio = (1.0 + (distance / lower).next_up()).next_up();
        // Distance to a convex site is convex along the chord and 1-Lipschitz
        // off it. Use this independent upper as well as the triangle bound.
        let other_upper = (radii[0][1 - side].max(radii[1][1 - side]) + uncertainty).next_up();
        let endpoint_ratio = (other_upper / lower).next_up();
        let ratio = endpoint_ratio.min(triangle_ratio);
        let denominator = ((1.0 + ratio).next_up() * length).next_up();
        let allowed = (program_coordinates::MAX_WALL_WIDTH_GRADIENT * denominator).next_up();
        if ratio.is_finite() && allowed.is_finite() && change > allowed {
            return Ok(true);
        }
    }
    Ok(false)
}

fn site_transport_distance(
    site: BoundarySupport,
    points: [[f64; 2]; 2],
    minimum_width: f64,
    uncertainty: f64,
) -> f64 {
    let Some(focus) = site.point else {
        return norm(sub(site.foot(points[1]), site.foot(points[0])));
    };
    let radii = points.map(|p| norm(sub(p, focus)));
    let lower = (radii[0].min(radii[1]) - uncertainty).next_down();
    if !lower.is_finite() || lower <= 0.0 {
        return 0.0;
    }
    let units = [
        mul(sub(points[0], focus), radii[0].recip()),
        mul(sub(points[1], focus), radii[1].recip()),
    ];
    let angular_error = ((4.0 * uncertainty / lower).next_up() + 128.0 * f64::EPSILON).next_up();
    let angular_motion = (norm(sub(units[1], units[0])) - angular_error)
        .next_down()
        .max(0.0);
    let distance = (minimum_width * 0.5 * angular_motion).next_down();
    if distance.is_finite() {
        distance.max(0.0)
    } else {
        0.0
    }
}

#[derive(Clone, Copy)]
struct Pending {
    value: f64,
    vertex: usize,
}
impl PartialEq for Pending {
    fn eq(&self, other: &Self) -> bool {
        self.value.to_bits() == other.value.to_bits() && self.vertex == other.vertex
    }
}
impl Eq for Pending {}
impl PartialOrd for Pending {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Pending {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .value
            .total_cmp(&self.value)
            .then_with(|| other.vertex.cmp(&self.vertex))
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Contour transport keeps native adjacency, anchor fields and optional corner bounds independent"
)]
pub(super) fn transport(
    graph: &skeleton::Skeleton,
    cells: &[Option<f64>],
    adjacency: &beading::Adjacency,
    anchors: &[Option<VertexField>],
    corner_widths: Option<&[f64]>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<f64>, ManufacturingError> {
    let count = graph.junctions.len();
    let queue_bound = count
        .checked_add(
            graph
                .edges
                .len()
                .checked_mul(2)
                .ok_or_else(|| resource("source contour queue bound overflow"))?,
        )
        .ok_or_else(|| resource("source contour queue bound overflow"))?;
    let comparisons = queue_bound
        .checked_mul(2)
        .and_then(|items| {
            items.checked_mul((usize::BITS - queue_bound.max(1).leading_zeros()) as usize + 1)
        })
        .ok_or_else(|| resource("source contour heap work bound overflow"))?;
    boolean::charge(work, comparisons)?;
    let mut queue = BinaryHeap::with_capacity(queue_bound);
    let mut upper = vec![f64::INFINITY; count];
    let mut negative_lower = vec![f64::INFINITY; count];
    // The two shortest-path envelopes are a Lipschitz extension of the fixed
    // incoming SIDE ports. A central-only spine has no paired contour port.
    // Choosing the closest value to preferred between them adds no width jump.
    for (labels, sign) in [(&mut upper, 1.0), (&mut negative_lower, -1.0)] {
        queue.clear();
        for (vertex, field) in anchors.iter().enumerate() {
            if let Some(field) = field.filter(|field| field.pairs > 0) {
                labels[vertex] = field.width * sign;
                queue.push(Pending {
                    value: labels[vertex],
                    vertex,
                });
            }
        }
        if sign > 0.0
            && let Some(widths) = corner_widths
        {
            for (edge, curve) in graph.edges.iter().enumerate() {
                check_cancel(cancel)?;
                boolean::charge(work, 64)?;
                if cells[edge].is_none() {
                    continue;
                }
                if let Some((vertex, limit)) =
                    corner_seed(curve, graph, widths, request, work, cancel)?
                    && limit < labels[vertex]
                {
                    labels[vertex] = limit;
                    if queue.len() >= queue_bound {
                        return Err(resource("source contour seed queue bound exceeded"));
                    }
                    queue.push(Pending {
                        value: limit,
                        vertex,
                    });
                }
            }
        }
        while let Some(Pending { value, vertex }) = queue.pop() {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            if value != labels[vertex] {
                continue;
            }
            for incident in adjacency.at(vertex) {
                boolean::charge(work, 1)?;
                let distance = cells[incident.edge].ok_or_else(|| {
                    unsupported("non-contour edge entered source width transport")
                })?;
                let next = graph.edges[incident.edge].vertex_ids[1 - incident.end];
                let proposed =
                    value + distance * program_coordinates::MAX_WALL_WIDTH_GRADIENT * 0.9;
                if !proposed.is_finite() {
                    return Err(unsupported(
                        "source contour width transport is unconditioned",
                    ));
                }
                if proposed < labels[next] {
                    labels[next] = proposed;
                    if queue.len() >= queue_bound {
                        return Err(resource("source contour queue bound exceeded"));
                    }
                    queue.push(Pending {
                        value: proposed,
                        vertex: next,
                    });
                }
            }
        }
    }
    let preferred = source_width_mm(request)?;
    for vertex in 0..count {
        let lower = -negative_lower[vertex];
        if lower > upper[vertex] {
            return Err(unsupported(&format!(
                "source contour ports have incompatible finite width gradients: vertex={vertex}, point={:?}, lower={lower}, upper={}, incoming={:?}",
                graph.junctions[vertex].point,
                upper[vertex],
                anchors[vertex].map(|field| field.width),
            )));
        }
        if let Some(anchor) = anchors[vertex].filter(|field| field.pairs > 0) {
            if lower > anchor.width || upper[vertex] < anchor.width {
                return Err(unsupported(
                    "source contour cannot preserve its incoming finite width field",
                ));
            }
            upper[vertex] = anchor.width;
        } else if upper[vertex].is_finite() {
            upper[vertex] = preferred.clamp(lower, upper[vertex]);
        } else {
            // A corner-only component has no opposing-wall allocation. This is
            // the same preferred contour-depth objective, not a failed-field
            // fallback or a claim that its eventual capsules contain material.
            upper[vertex] = preferred;
        }
    }
    Ok(upper)
}
