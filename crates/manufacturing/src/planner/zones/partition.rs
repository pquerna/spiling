// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! One arrangement of the ORIGINAL boundary and two parallel planes.
//!
//! The coverage proof is combinatorial, not a Boolean operation on the rounded
//! output: every source edge has the ordered symbolic parameters 0, Cut(0),
//! Cut(1), 1 (in source direction), omitting only cuts outside its open span.
//! Consecutive tokens cover [0,1] exactly once. A Cut token denotes the exact
//! real intersection of the binary64 input operands; its emitted coordinate is
//! separately enclosed by directed arithmetic. It is NOT asserted that rounded
//! intersection points are literally collinear with the original source edge.
//! Each artificial edge is inserted together with its opposite, using the very
//! same nodes. Removing those pairs leaves precisely the complete source ledger.

use crate::{check_cancel, error};
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{Area, Contains, Coord, LineString, MultiPolygon, Point, Polygon};
use spiling_contracts::geometry::rounded_sum_interval;
use spiling_contracts::manufacturing::{ManufacturingError, ManufacturingErrorCode};
use std::cmp::Ordering;
use std::mem::size_of;
use std::sync::atomic::AtomicBool;

use super::super::boolean;

const NONE: usize = usize::MAX;
type Interval = [f64; 2];

fn unsupported(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::UnsupportedGeometry, message)
}
fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}
fn component(p: Coord<f64>, axis: usize) -> f64 {
    if axis == 0 { p.x } else { p.y }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parameter {
    Start,
    Cut(usize),
    End,
}
#[derive(Clone, Copy)]
enum Origin {
    Source {
        edge: usize,
        from: Parameter,
        to: Parameter,
    },
    Cut {
        plane: usize,
        opposite: usize,
    },
}
struct Edge {
    from: usize,
    to: usize,
    owner: usize,
    origin: Origin,
    next_out: usize,
    successor: usize,
    predecessors: u8,
    used: bool,
}
enum Position<'a> {
    Original(&'a Coord<f64>),
    Intersection {
        rounded: Coord<f64>,
        transverse: Interval,
    },
}
struct Node<'a> {
    position: Position<'a>,
    plane: Option<usize>,
    // Signed even-odd crossing events on the two infinitesimal sides.
    crossing: [i32; 2],
    // Original, on-plane boundary intervals, assigned to material's side only.
    boundary: [i32; 2],
    outgoing: [usize; 3],
    incoming: [usize; 3],
}
impl Node<'_> {
    fn point(&self) -> Coord<f64> {
        match &self.position {
            Position::Original(p) => **p,
            Position::Intersection { rounded, .. } => *rounded,
        }
    }
    fn enclosure(&self, axis: usize) -> Interval {
        match &self.position {
            Position::Original(p) => [component(**p, 1 - axis); 2],
            Position::Intersection { transverse, .. } => *transverse,
        }
    }
}
struct Cycle {
    ring: LineString<f64>,
    owner: usize,
    positive: bool,
}

fn sum(a: Interval, b: Interval) -> Interval {
    [
        rounded_sum_interval(a[0], b[0])[0],
        rounded_sum_interval(a[1], b[1])[1],
    ]
}
fn difference(a: f64, b: f64) -> Interval {
    rounded_sum_interval(a, -b)
}
fn product(a: Interval, b: Interval) -> Interval {
    let values = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
    [
        values.into_iter().fold(f64::INFINITY, f64::min).next_down(),
        values
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max)
            .next_up(),
    ]
}
fn quotient(a: Interval, b: Interval) -> Result<Interval, ManufacturingError> {
    if a.into_iter().chain(b).any(|v| !v.is_finite()) || (b[0] <= 0.0 && b[1] >= 0.0) {
        return Err(unsupported(
            "axis partition intersection denominator is not certified",
        ));
    }
    let values = [a[0] / b[0], a[0] / b[1], a[1] / b[0], a[1] / b[1]];
    let result = [
        values.into_iter().fold(f64::INFINITY, f64::min).next_down(),
        values
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max)
            .next_up(),
    ];
    if result.iter().any(|v| !v.is_finite()) {
        return Err(unsupported(
            "axis partition intersection arithmetic overflow",
        ));
    }
    Ok(result)
}

fn intersection(
    a: Coord<f64>,
    b: Coord<f64>,
    axis: usize,
    cut: f64,
) -> Result<Position<'static>, ManufacturingError> {
    let aa = component(a, axis);
    let ba = component(b, axis);
    let at = component(a, 1 - axis);
    let bt = component(b, 1 - axis);
    let parameter = quotient(difference(cut, aa), difference(ba, aa))?;
    let transverse = sum([at; 2], product(parameter, difference(bt, at)));
    let t = (cut - aa) / (ba - aa);
    let value = at + t * (bt - at);
    if transverse.iter().any(|v| !v.is_finite())
        || !value.is_finite()
        || value < transverse[0]
        || value > transverse[1]
    {
        return Err(unsupported(
            "axis partition cannot enclose an interior source intersection",
        ));
    }
    let rounded = if axis == 0 {
        Coord { x: cut, y: value }
    } else {
        Coord { x: value, y: cut }
    };
    Ok(Position::Intersection {
        rounded,
        transverse,
    })
}

fn reserve<T>(values: &mut Vec<T>, count: usize) -> Result<(), ManufacturingError> {
    values
        .try_reserve_exact(count)
        .map_err(|_| resource("axis partition admitted allocation refused"))
}
fn grow<T>(values: &mut Vec<T>, additional: usize, limit: usize) -> Result<(), ManufacturingError> {
    let needed = values
        .len()
        .checked_add(additional)
        .ok_or_else(|| resource("axis partition record count overflow"))?;
    if needed > limit {
        return Err(resource("axis partition record bound exceeded"));
    }
    if needed <= values.capacity() {
        return Ok(());
    }
    let capacity = values
        .capacity()
        .saturating_mul(2)
        .max(4)
        .max(needed)
        .min(limit);
    reserve(values, capacity - values.len())
}
fn new_node(position: Position<'_>, plane: Option<usize>) -> Node<'_> {
    Node {
        position,
        plane,
        crossing: [0; 2],
        boundary: [0; 2],
        outgoing: [NONE; 3],
        incoming: [0; 3],
    }
}
fn add_edge(
    nodes: &mut [Node<'_>],
    edges: &mut Vec<Edge>,
    from: usize,
    to: usize,
    owner: usize,
    origin: Origin,
) -> Result<(), ManufacturingError> {
    if from == to || nodes[from].point() == nodes[to].point() {
        return Err(unsupported(
            "axis partition has an unrepresentable positive source interval",
        ));
    }
    if edges.len() == edges.capacity() {
        return Err(resource("axis partition edge capacity bound exceeded"));
    }
    let next_out = nodes[from].outgoing[owner];
    nodes[from].outgoing[owner] = edges.len();
    nodes[to].incoming[owner] += 1;
    edges.push(Edge {
        from,
        to,
        owner,
        origin,
        next_out,
        successor: NONE,
        predecessors: 0,
        used: false,
    });
    Ok(())
}

/// Signed twice-area with translated coordinates and outward arithmetic.
/// No positive area is discarded: an uncertifiable sign is a typed refusal.
fn positive_area(
    ring: &LineString<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let origin = ring.0[0];
    let mut area = [0.0; 2];
    for edge in ring.0.windows(2) {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let a = product(
            difference(edge[0].x, origin.x),
            difference(edge[1].y, origin.y),
        );
        let b = product(
            difference(edge[0].y, origin.y),
            difference(edge[1].x, origin.x),
        );
        area = sum(area, sum(a, [-b[1], -b[0]]));
    }
    if area.iter().any(|v| !v.is_finite()) || (area[0] <= 0.0 && area[1] >= 0.0) {
        return Err(unsupported(
            "axis partition cannot certify cycle area and winding",
        ));
    }
    Ok(area[0] > 0.0)
}

// Angular order is needed only at an ORIGINAL on-plane tangent vertex. At an
// ordinary intersection each regional vertex has one outgoing edge. Robust
// orientation is used only within the existing predicate arithmetic envelope.
fn angular(
    origin: Coord<f64>,
    a: Coord<f64>,
    b: Coord<f64>,
) -> Result<Ordering, ManufacturingError> {
    let half = |p: Coord<f64>| p.y > origin.y || (p.y == origin.y && p.x >= origin.x);
    if half(a) != half(b) {
        return Ok(if half(a) {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    if [origin, a, b].into_iter().any(|p| {
        !super::super::native::merge_predicate_coordinate(p.x)
            || !super::super::native::merge_predicate_coordinate(p.y)
    }) {
        return Err(unsupported(
            "axis partition tangent orientation is not conditioned",
        ));
    }
    Ok(match RobustKernel::orient2d(origin, a, b) {
        Orientation::CounterClockwise => Ordering::Less,
        Orientation::Clockwise => Ordering::Greater,
        Orientation::Collinear => Ordering::Equal,
    })
}

fn connect(
    nodes: &[Node<'_>],
    edges: &mut [Edge],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for index in 0..edges.len() {
        check_cancel(cancel)?;
        let node = &nodes[edges[index].to];
        let owner = edges[index].owner;
        let mut candidate = node.outgoing[owner];
        let mut count = 0usize;
        let mut before = NONE;
        let mut last = NONE;
        while candidate != NONE {
            boolean::charge(work, 1)?;
            check_cancel(cancel)?;
            count += 1;
            let point = nodes[edges[candidate].to].point();
            // The immediately clockwise outgoing ray from the reversed incoming
            // ray keeps the material face on the left, including point tangency.
            if edges[candidate].next_out == NONE && count == 1 {
                before = candidate;
                break;
            }
            let order = angular(node.point(), point, nodes[edges[index].from].point())?;
            if order == Ordering::Equal {
                return Err(unsupported(
                    "axis partition has coincident opposite boundary rays",
                ));
            }
            if last == NONE || angular(node.point(), nodes[edges[last].to].point(), point)?.is_lt()
            {
                last = candidate;
            }
            if order.is_lt()
                && (before == NONE
                    || angular(node.point(), nodes[edges[before].to].point(), point)?.is_lt())
            {
                before = candidate;
            }
            candidate = edges[candidate].next_out;
        }
        if count != node.incoming[owner] || count == 0 {
            return Err(unsupported(
                "axis partition regional boundary incidence is unbalanced",
            ));
        }
        let successor = if before != NONE { before } else { last };
        edges[index].successor = successor;
        edges[successor].predecessors = edges[successor]
            .predecessors
            .checked_add(1)
            .ok_or_else(|| unsupported("axis partition boundary predecessor overflow"))?;
    }
    if edges.iter().any(|e| e.predecessors != 1) {
        return Err(unsupported(
            "axis partition tangent continuation is not a permutation",
        ));
    }
    Ok(())
}

fn plane_edges(
    nodes: &mut [Node<'_>],
    edges: &mut Vec<Edge>,
    events: &mut Vec<usize>,
    plane: usize,
    axis: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    events.clear();
    for (index, node) in nodes.iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        if node.plane == Some(plane) {
            events.push(index);
        }
    }
    boolean::sort_by(
        events,
        |i| component(nodes[*i].point(), 1 - axis),
        work,
        cancel,
    )?;
    let mut winding = [0i32; 2];
    let mut boundary = [0i32; 2];
    for (position, &index) in events.iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        for side in 0..2 {
            winding[side] = winding[side]
                .checked_add(nodes[index].crossing[side])
                .ok_or_else(|| unsupported("axis partition crossing count overflow"))?;
            boundary[side] = boundary[side]
                .checked_add(nodes[index].boundary[side])
                .ok_or_else(|| unsupported("axis partition on-plane count overflow"))?;
            if !(0..=1).contains(&winding[side]) || !(0..=1).contains(&boundary[side]) {
                return Err(unsupported(
                    "axis partition cut events do not certify even-odd material",
                ));
            }
        }
        let Some(&next) = events.get(position + 1) else {
            break;
        };
        if nodes[index].enclosure(axis)[1] >= nodes[next].enclosure(axis)[0] {
            return Err(unsupported(
                "axis partition cannot certify distinct cut-node ordering",
            ));
        }
        if winding == [1, 1] {
            if boundary != [0, 0] {
                return Err(unsupported(
                    "axis partition duplicates an original on-plane boundary",
                ));
            }
            let (from, to) = if axis == 0 {
                (index, next)
            } else {
                (next, index)
            };
            let first = edges.len();
            add_edge(
                nodes,
                edges,
                from,
                to,
                plane,
                Origin::Cut {
                    plane,
                    opposite: first + 1,
                },
            )?;
            add_edge(
                nodes,
                edges,
                to,
                from,
                plane + 1,
                Origin::Cut {
                    plane,
                    opposite: first,
                },
            )?;
        } else if boundary != winding {
            return Err(unsupported(
                "axis partition has an unmatched one-sided cut interval",
            ));
        }
    }
    if winding != [0, 0] || boundary != [0, 0] {
        return Err(unsupported(
            "axis partition cut sweep has nonzero final parity",
        ));
    }
    Ok(())
}

fn polygon_graph<'a>(
    polygon: &'a Polygon<f64>,
    axis: usize,
    cuts: [f64; 2],
    nodes: &mut Vec<Node<'a>>,
    edges: &mut Vec<Edge>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let mut source_edge = 0usize;
    for (ring_index, ring) in std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .enumerate()
    {
        check_cancel(cancel)?;
        if ring.0.len() < 4
            || ring.0.first() != ring.0.last()
            || positive_area(ring, work, cancel)? != (ring_index == 0)
        {
            return Err(unsupported(
                "axis partition requires closed material-left original rings",
            ));
        }
        let base = nodes.len();
        let count = ring.0.len() - 1;
        for point in &ring.0[..count] {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            if !point.x.is_finite() || !point.y.is_finite() {
                return Err(unsupported(
                    "axis partition source has nonfinite coordinates",
                ));
            }
            let value = component(*point, axis);
            let plane = cuts.iter().position(|cut| *cut == value);
            nodes.push(new_node(Position::Original(point), plane));
        }
        for local in 0..count {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let a = ring.0[local];
            let b = ring.0[local + 1];
            if a == b {
                return Err(unsupported("axis partition original edge has zero length"));
            }
            let av = component(a, axis);
            let bv = component(b, axis);
            let mut chain = [(NONE, Parameter::Start); 4];
            chain[0] = (base + local, Parameter::Start);
            let mut length = 1;
            // Exact source-parameter order follows the monotone source axis;
            // neither rounded interpolation nor a floating t comparison orders it.
            for plane in if av < bv { [0, 1] } else { [1, 0] } {
                let cut = cuts[plane];
                if av.min(bv) < cut && cut < av.max(bv) {
                    let node = nodes.len();
                    nodes.push(new_node(intersection(a, b, axis, cut)?, Some(plane)));
                    chain[length] = (node, Parameter::Cut(plane));
                    length += 1;
                }
            }
            chain[length] = (base + (local + 1) % count, Parameter::End);
            length += 1;
            for plane in 0..2 {
                let cut = cuts[plane];
                if av == cut && bv == cut {
                    let increasing = component(a, 1 - axis) < component(b, 1 - axis);
                    // x-plane/up => material west; y-plane/right => material north.
                    let side = usize::from(increasing != (axis == 0));
                    let first = base + local;
                    let last = base + (local + 1) % count;
                    let (low, high) = if increasing {
                        (first, last)
                    } else {
                        (last, first)
                    };
                    nodes[low].boundary[side] += 1;
                    nodes[high].boundary[side] -= 1;
                } else if av.min(bv) <= cut && cut <= av.max(bv) {
                    let index = chain[..length]
                        .iter()
                        .find_map(|&(index, _)| {
                            (nodes[index].plane == Some(plane)).then_some(index)
                        })
                        .ok_or_else(|| {
                            unsupported("axis partition source ledger lost a plane event")
                        })?;
                    let delta = if (bv > av) == (axis == 0) { 1 } else { -1 };
                    // Half-open side events retain an on-plane vertex exactly
                    // once for each incident side, without a sampled epsilon.
                    if av < cut || bv < cut {
                        nodes[index].crossing[0] += delta;
                    }
                    if av > cut || bv > cut {
                        nodes[index].crossing[1] += delta;
                    }
                }
            }
            let mut previous = Parameter::Start;
            for span in chain[..length].windows(2) {
                let [(from, start), (to, end)] = [span[0], span[1]];
                if start != previous || start == end {
                    return Err(unsupported(
                        "axis partition source parameter ledger is discontinuous",
                    ));
                }
                previous = end;
                let owner = if av == bv && cuts.contains(&av) {
                    let plane = cuts.iter().position(|cut| *cut == av).unwrap();
                    let increasing = component(a, 1 - axis) < component(b, 1 - axis);
                    plane + usize::from(increasing != (axis == 0))
                } else {
                    // A symbolic open interval is wholly in one slab. Using
                    // the endpoint axis bounds avoids a rounded midpoint.
                    let low = component(nodes[from].point(), axis)
                        .min(component(nodes[to].point(), axis));
                    let high = component(nodes[from].point(), axis)
                        .max(component(nodes[to].point(), axis));
                    if high <= cuts[0] {
                        0
                    } else if low >= cuts[1] {
                        2
                    } else if low >= cuts[0] && high <= cuts[1] {
                        1
                    } else {
                        return Err(unsupported(
                            "axis partition source interval crosses an unrecorded plane",
                        ));
                    }
                };
                add_edge(
                    nodes,
                    edges,
                    from,
                    to,
                    owner,
                    Origin::Source {
                        edge: source_edge,
                        from: start,
                        to: end,
                    },
                )?;
            }
            if previous != Parameter::End {
                return Err(unsupported(
                    "axis partition source ledger does not cover [0,1]",
                ));
            }
            source_edge += 1;
        }
    }
    Ok(source_edge)
}

fn audit(
    nodes: &[Node<'_>],
    edges: &[Edge],
    axis: usize,
    cuts: [f64; 2],
    expected_sources: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut source = 0usize;
    let mut cursor = Parameter::Start;
    for (index, edge) in edges.iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        for node in [edge.from, edge.to] {
            let value = component(nodes[node].point(), axis);
            if (edge.owner > 0 && value < cuts[edge.owner - 1])
                || (edge.owner < 2 && value > cuts[edge.owner])
            {
                return Err(unsupported(
                    "axis partition boundary violates closed halfspace ownership",
                ));
            }
        }
        match edge.origin {
            Origin::Source {
                edge: original,
                from,
                to,
            } => {
                if original != source || from != cursor {
                    return Err(unsupported(
                        "axis partition source ledger is duplicated or incomplete",
                    ));
                }
                cursor = to;
                if to == Parameter::End {
                    source += 1;
                    cursor = Parameter::Start;
                }
            }
            Origin::Cut { plane, opposite } => {
                let other = edges
                    .get(opposite)
                    .ok_or_else(|| unsupported("axis partition shared boundary pair is missing"))?;
                if edge.from != other.to
                    || edge.to != other.from
                    || nodes[edge.from].plane != Some(plane)
                    || nodes[edge.to].plane != Some(plane)
                    || !matches!(other.origin, Origin::Cut { plane: p, opposite: o } if p == plane && o == index)
                    || !((edge.owner == plane && other.owner == plane + 1)
                        || (other.owner == plane && edge.owner == plane + 1))
                {
                    return Err(unsupported(
                        "axis partition shared boundary does not cancel oppositely",
                    ));
                }
            }
        }
    }
    if cursor != Parameter::Start || source != expected_sources {
        return Err(unsupported(
            "axis partition has an incomplete source ledger",
        ));
    }
    Ok(())
}

fn trace(
    nodes: &[Node<'_>],
    edges: &mut [Edge],
    cycles: &mut Vec<Cycle>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for start in 0..edges.len() {
        check_cancel(cancel)?;
        if edges[start].used {
            continue;
        }
        let mut count = 0usize;
        let mut index = start;
        loop {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            if edges[index].used || count >= edges.len() {
                return Err(unsupported("axis partition cycle tracing does not close"));
            }
            edges[index].used = true;
            count += 1;
            index = edges[index].successor;
            if index == start {
                break;
            }
        }
        if count < 3 {
            return Err(unsupported(
                "axis partition contains a positive unrepresentable boundary cycle",
            ));
        }
        let mut coordinates = Vec::new();
        reserve(&mut coordinates, count + 1)?;
        index = start;
        loop {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            coordinates.push(nodes[edges[index].from].point());
            index = edges[index].successor;
            if index == start {
                break;
            }
        }
        coordinates.push(coordinates[0]);
        let ring = LineString(coordinates);
        let positive = positive_area(&ring, work, cancel)?;
        cycles.push(Cycle {
            ring,
            owner: edges[start].owner,
            positive,
        });
    }
    if edges.iter().any(|e| !e.used) {
        return Err(unsupported("axis partition left an untraced boundary"));
    }
    Ok(())
}

/// Split into lower / band / upper using one shared source-edge arrangement.
///
/// `admit` receives a conservative peak of LOCAL coordinate-equivalent storage,
/// excluding `original` (which remains borrowed). It includes node/edge/event/
/// trace metadata, polygon/ring descriptors and every output closing coordinate,
/// before any allocation. Add other live owners in the callback, counting the
/// original once. After return, retain/count all three outputs with the existing
/// `region::point_count`; no proof geometry or original-coordinate copies remain.
pub(super) fn split(
    original: &MultiPolygon<f64>,
    axis: usize,
    cuts: [f64; 2],
    mut admit: impl FnMut(usize) -> Result<(), ManufacturingError>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<[MultiPolygon<f64>; 3], ManufacturingError> {
    check_cancel(cancel)?;
    if axis > 1 || cuts.iter().any(|v| !v.is_finite()) || cuts[0] >= cuts[1] {
        return Err(unsupported(
            "axis partition requires two ordered finite X/Y planes",
        ));
    }
    let mut total_cycles = 0usize;
    let mut total_coordinates = 0usize;
    let (mut nodes_bound, mut edges_bound, mut events_bound, mut cycles_bound) =
        (0usize, 0usize, 0usize, 0usize);
    for polygon in &original.0 {
        let (mut count, mut intersections) = (0usize, 0usize);
        let mut events = [0usize; 2];
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            check_cancel(cancel)?;
            boolean::charge(
                work,
                ring.0
                    .len()
                    .checked_mul(4)
                    .ok_or_else(|| resource("axis partition counting work overflow"))?,
            )?;
            let vertices = ring.0.len().saturating_sub(1);
            count = count
                .checked_add(vertices)
                .ok_or_else(|| resource("axis partition source size overflow"))?;
            for point in &ring.0[..vertices] {
                for plane in 0..2 {
                    if component(*point, axis) == cuts[plane] {
                        events[plane] += 1;
                    }
                }
            }
            for edge in ring.0.windows(2) {
                let a = component(edge[0], axis);
                let b = component(edge[1], axis);
                for plane in 0..2 {
                    if a.min(b) < cuts[plane] && cuts[plane] < a.max(b) {
                        intersections += 1;
                        events[plane] += 1;
                    }
                }
            }
        }
        let nodes = count
            .checked_add(intersections)
            .ok_or_else(|| resource("axis partition node bound overflow"))?;
        let pairs = events
            .iter()
            .try_fold(0usize, |n, &events| n.checked_add(events.saturating_sub(1)))
            .and_then(|n| n.checked_mul(2))
            .ok_or_else(|| resource("axis partition cut edge bound overflow"))?;
        let edges = nodes
            .checked_add(pairs)
            .ok_or_else(|| resource("axis partition edge bound overflow"))?;
        let cycles = edges / 3; // trace refuses shorter cycles before coordinate allocation.
        total_cycles = total_cycles
            .checked_add(cycles)
            .ok_or_else(|| resource("axis partition cycle bound overflow"))?;
        total_coordinates = total_coordinates
            .checked_add(edges)
            .and_then(|n| n.checked_add(cycles))
            .ok_or_else(|| resource("axis partition coordinate bound overflow"))?;
        nodes_bound = nodes_bound.max(nodes);
        edges_bound = edges_bound.max(edges);
        events_bound = events_bound.max(events[0].max(events[1]));
        cycles_bound = cycles_bound.max(cycles);
    }
    // Capacities are bounded by actual crossings/events, not every possible cut.
    // Output descriptor growth includes simultaneous old/new allocations;
    // source coordinate arrays remain borrowed and are never cloned.
    let bytes = nodes_bound
        .checked_mul(size_of::<Node<'_>>())
        .and_then(|n| {
            edges_bound
                .checked_mul(size_of::<Edge>())
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| {
            events_bound
                .checked_mul(size_of::<usize>())
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| {
            cycles_bound
                .checked_mul(size_of::<Cycle>())
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| {
            total_coordinates
                .checked_mul(size_of::<Coord<f64>>())
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| {
            total_cycles
                .checked_mul(6 * size_of::<Polygon<f64>>() + 8 * size_of::<LineString<f64>>())
                .and_then(|v| n.checked_add(v))
        })
        .ok_or_else(|| resource("axis partition storage bound overflow"))?;
    admit(bytes.div_ceil(size_of::<Coord<f64>>()))?;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut events = Vec::new();
    let mut cycles = Vec::new();
    reserve(&mut nodes, nodes_bound)?;
    check_cancel(cancel)?;
    reserve(&mut edges, edges_bound)?;
    check_cancel(cancel)?;
    reserve(&mut events, events_bound)?;
    check_cancel(cancel)?;
    reserve(&mut cycles, cycles_bound)?;
    let mut result: [MultiPolygon<f64>; 3] = std::array::from_fn(|_| MultiPolygon(Vec::new()));
    for polygon in &original.0 {
        nodes.clear();
        edges.clear();
        cycles.clear();
        let source_count =
            polygon_graph(polygon, axis, cuts, &mut nodes, &mut edges, work, cancel)?;
        for plane in 0..2 {
            plane_edges(
                &mut nodes,
                &mut edges,
                &mut events,
                plane,
                axis,
                work,
                cancel,
            )?;
        }
        audit(&nodes, &edges, axis, cuts, source_count, work, cancel)?;
        connect(&nodes, &mut edges, work, cancel)?;
        trace(&nodes, &mut edges, &mut cycles, work, cancel)?;
        let bases: [usize; 3] = std::array::from_fn(|i| result[i].0.len());
        for cycle in cycles.iter_mut().filter(|c| c.positive) {
            check_cancel(cancel)?;
            let output = &mut result[cycle.owner].0;
            grow(output, 1, total_cycles)?;
            output.push(Polygon::new(
                std::mem::replace(&mut cycle.ring, LineString(Vec::new())),
                Vec::new(),
            ));
        }
        for cycle in cycles.drain(..).filter(|c| !c.positive) {
            check_cancel(cancel)?;
            let output = &mut result[cycle.owner].0;
            let mut owner = None;
            // A hole may touch a plane at an original tangent vertex. Use any
            // strict interior witness, not an arbitrary first/seam vertex.
            for (index, outer) in output.iter().enumerate().skip(bases[cycle.owner]) {
                let predicate_points = outer.exterior().0.len()
                    + outer.interiors().iter().map(|h| h.0.len()).sum::<usize>();
                for point in std::iter::once(outer.exterior())
                    .chain(outer.interiors())
                    .chain(std::iter::once(&cycle.ring))
                    .flat_map(|r| &r.0)
                {
                    check_cancel(cancel)?;
                    boolean::charge(work, 1)?;
                    if !super::super::native::merge_predicate_coordinate(point.x)
                        || !super::super::native::merge_predicate_coordinate(point.y)
                    {
                        return Err(unsupported(
                            "axis partition hole predicate is not conditioned",
                        ));
                    }
                }
                let mut contains = false;
                for &point in &cycle.ring.0[..cycle.ring.0.len() - 1] {
                    check_cancel(cancel)?;
                    boolean::charge(work, predicate_points)?;
                    if outer.contains(&Point(point)) {
                        contains = true;
                        break;
                    }
                }
                if contains {
                    if owner.is_some() {
                        return Err(unsupported(
                            "axis partition hole has ambiguous exterior ownership",
                        ));
                    }
                    owner = Some(index);
                }
            }
            let owner = owner
                .ok_or_else(|| unsupported("axis partition hole lost its containing exterior"))?;
            let (exterior, mut holes) =
                std::mem::replace(&mut output[owner], Polygon::empty()).into_inner();
            grow(&mut holes, 1, total_cycles)?;
            holes.push(cycle.ring);
            output[owner] = Polygon::new(exterior, holes);
        }
    }
    // No epsilon, filtering, simplification, reorientation, reunion or overlay.
    // Refuse an unrepresentable result rather than silently losing its material.
    for output in &result {
        for polygon in &output.0 {
            check_cancel(cancel)?;
            boolean::charge(
                work,
                polygon.exterior().0.len()
                    + polygon.interiors().iter().map(|h| h.0.len()).sum::<usize>(),
            )?;
            let area = polygon.unsigned_area();
            if !area.is_finite() || area <= 0.0 {
                return Err(unsupported(
                    "axis partition output material area is not representable",
                ));
            }
        }
    }
    Ok(result)
}
