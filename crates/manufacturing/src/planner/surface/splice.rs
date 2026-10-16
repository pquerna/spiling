// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Native-preserving M union A over an attributed, sampled supporting-face chain.
use super::proof::{certify_retention, certify_simple, charge};
use super::{PreparedFinish, PreparedShape, RibField, SURFACE_ROUNDING_MM, region, unsupported};
use crate::{check_cancel, error};
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::line_intersection::line_intersection;
use geo::{Area, Contains, Coord, Intersects, Line, LineString, MultiPolygon, Point, Polygon};
use spiling_contracts::manufacturing::{
    MAX_DEPOSITION_SEGMENTS, ManufacturingError, ManufacturingErrorCode,
};
use std::sync::atomic::AtomicBool;

// Metadata reserves the other half for source/pose floating arithmetic.
const FRAME_MM: f64 = SURFACE_ROUNDING_MM / 2.0;

struct Chain {
    polygon: usize,
    start: usize,
    edges: usize,
    forward: bool,
    parameters: Vec<f64>, // source millimetres, increasing in native ring direction
}
impl Chain {
    fn vertex(&self, ring: &LineString<f64>, offset: usize) -> Coord<f64> {
        ring.0[(self.start + offset) % (ring.0.len() - 1)]
    }
    fn owns(&self, edge: usize, count: usize) -> bool {
        (edge + count - self.start) % count < self.edges
    }
}
struct Replacement {
    polygon: usize,
    start: usize,
    edges: usize,
    points: Vec<Coord<f64>>, // includes unchanged native chain endpoints
}
#[derive(Clone, Copy)]
struct Node {
    parameter: f64,
    native: Option<usize>,
}
fn matches(a: Coord<f64>, b: Coord<f64>, width: f64) -> bool {
    let scale =
        a.x.abs()
            .max(a.y.abs())
            .max(b.x.abs())
            .max(b.y.abs())
            .max(width)
            .max(1.0);
    (a.x - b.x).hypot(a.y - b.y) <= 512.0 * f64::EPSILON * scale
}
fn progression(a: Coord<f64>, b: Coord<f64>, from: Coord<f64>, to: Coord<f64>) -> bool {
    RobustKernel::dot_product_sign(
        Coord {
            x: b.x - a.x,
            y: b.y - a.y,
        },
        Coord {
            x: to.x - from.x,
            y: to.y - from.y,
        },
    ) == Orientation::CounterClockwise
}
fn base(a: Coord<f64>, b: Coord<f64>, fraction: f64) -> Coord<f64> {
    Coord {
        x: a.x + (b.x - a.x) * fraction,
        y: a.y + (b.y - a.y) * fraction,
    }
}

struct CandidateChain<'a> {
    ring: &'a LineString<f64>,
    polygon: usize,
    start: usize,
    end: usize,
    forward: bool,
}

// Endpoint identity, a single contiguous monotone full-span chain and its native
// frame agreement are all required in addition to the immutable FaceId certificate.
// Native straight faces may have many samples; none is epsilon-flattened away.
fn candidate(
    query: CandidateChain<'_>,
    field: &RibField,
    work: &mut usize,
    cancel: &AtomicBool,
    retain_parameters: bool,
) -> Result<Option<Chain>, ManufacturingError> {
    let CandidateChain {
        ring,
        polygon,
        start,
        end,
        forward,
    } = query;
    let count = ring.0.len() - 1;
    let edges = (end + count - start) % count;
    if edges == 0 {
        return Ok(None);
    }
    let a = ring.0[start];
    let b = ring.0[end];
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let square = dx * dx + dy * dy;
    if !square.is_finite() || square <= 0.0 {
        return Ok(None);
    }
    let mut parameters = Vec::with_capacity(if retain_parameters { edges + 1 } else { 0 });
    let mut previous_parameter = None;
    for offset in 0..=edges {
        check_cancel(cancel)?;
        charge(work, 1)?;
        let point = ring.0[(start + offset) % count];
        let parameter = if offset == 0 {
            0.0
        } else if offset == edges {
            field.width
        } else {
            ((point.x - a.x) * dx + (point.y - a.y) * dy) / square * field.width
        };
        if !parameter.is_finite()
            || parameter < 0.0
            || parameter > field.width
            || previous_parameter.is_some_and(|previous| parameter <= previous)
        {
            return Ok(None);
        }
        let s = if forward {
            parameter
        } else {
            field.width - parameter
        };
        let declared = field.point(s, 0.0);
        if (point.x - declared.x).hypot(point.y - declared.y) > FRAME_MM {
            return Ok(None);
        }
        if offset > 0 {
            let previous = ring.0[(start + offset - 1) % count];
            if !progression(a, b, previous, point) {
                return Ok(None);
            }
            let ex = point.x - previous.x;
            let ey = point.y - previous.y;
            let agreement = (ey * field.normal[0] - ex * field.normal[1])
                / ex.hypot(ey)
                / field.normal[0].hypot(field.normal[1]);
            if !agreement.is_finite() || agreement < 1.0 - 512.0 * f64::EPSILON {
                return Ok(None);
            }
        }
        previous_parameter = Some(parameter);
        if retain_parameters {
            parameters.push(parameter);
        }
    }
    Ok(Some(Chain {
        polygon,
        start,
        edges,
        forward,
        parameters,
    }))
}
fn matched_chain(
    source: &MultiPolygon<f64>,
    field: &RibField,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Chain, ManufacturingError> {
    let mut found = None;
    for (polygon, material) in source.0.iter().enumerate() {
        if let Some(chain) = matched_polygon(material, polygon, field, true, work, cancel)? {
            if found.is_some() {
                return Err(unsupported(
                    &field.face_id,
                    "certified face does not match one unique full-span sampled exterior chain",
                ));
            }
            found = Some(chain);
        }
    }
    found.ok_or_else(|| unsupported(&field.face_id, "certified full-span native endpoints do not bound a unique monotone supporting-face chain"))
}

fn matched_polygon(
    material: &Polygon<f64>,
    polygon: usize,
    field: &RibField,
    retain_parameters: bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Chain>, ManufacturingError> {
    if material.signed_area() <= 0.0 {
        return Err(unsupported(
            &field.face_id,
            "native outer ring does not have material-left winding",
        ));
    }
    let ring = material.exterior();
    if ring.0.len() < 4 || ring.0.first() != ring.0.last() {
        return Ok(None);
    }
    let lo = field.point(0.0, 0.0);
    let hi = field.point(field.width, 0.0);
    let mut lo_index = None;
    let mut hi_index = None;
    for (index, point) in ring.0[..ring.0.len() - 1].iter().enumerate() {
        check_cancel(cancel)?;
        charge(work, 1)?;
        if matches(*point, lo, field.width) && lo_index.replace(index).is_some() {
            return Err(unsupported(
                &field.face_id,
                "certified native face start has ambiguous source samples",
            ));
        }
        if matches(*point, hi, field.width) && hi_index.replace(index).is_some() {
            return Err(unsupported(
                &field.face_id,
                "certified native face end has ambiguous source samples",
            ));
        }
    }
    let mut found = None;
    if let (Some(lo), Some(hi)) = (lo_index, hi_index) {
        for (start, end, forward) in [(lo, hi, true), (hi, lo, false)] {
            if let Some(chain) = candidate(
                CandidateChain {
                    ring,
                    polygon,
                    start,
                    end,
                    forward,
                },
                field,
                work,
                cancel,
                retain_parameters,
            )? {
                if found.is_some() {
                    return Err(unsupported(
                        &field.face_id,
                        "certified face does not match one unique full-span sampled exterior chain",
                    ));
                }
                found = Some(chain);
            }
        }
    }
    Ok(found)
}

pub(super) fn owns_native_island(
    material: &Polygon<f64>,
    field: &RibField,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    Ok(matched_polygon(material, 0, field, false, work, cancel)?.is_some())
}

// This predicate is exact for the stored source coordinates. A native vertex's
// raised point must satisfy BOTH adjacent sampled segments, never just a chord
// that would erase a roundoff-sized source sliver.
fn round_outward(
    mut point: Coord<f64>,
    constraints: &[Line<f64>],
    normal: [f64; 2],
    field: &RibField,
) -> Result<Coord<f64>, ManufacturingError> {
    let nominal = point;
    for _ in 0..=8 {
        if constraints.iter().all(|edge| {
            RobustKernel::orient2d(edge.start, edge.end, point) != Orientation::CounterClockwise
        }) {
            if (point.x - nominal.x).hypot(point.y - nominal.y) <= FRAME_MM {
                return Ok(point);
            }
            break;
        }
        if normal[0].abs() >= normal[1].abs() {
            point.x = if normal[0] > 0.0 {
                point.x.next_up()
            } else {
                point.x.next_down()
            };
        } else {
            point.y = if normal[1] > 0.0 {
                point.y.next_up()
            } else {
                point.y.next_down()
            };
        }
    }
    Err(unsupported(
        &field.face_id,
        "outward sample rounding cannot satisfy actual native segments within the declared construction bound",
    ))
}
fn anchor(chain: &Chain, ring: &LineString<f64>, parameter: f64) -> Coord<f64> {
    let interval = chain
        .parameters
        .partition_point(|value| *value <= parameter)
        .saturating_sub(1)
        .min(chain.edges - 1);
    base(
        chain.vertex(ring, interval),
        chain.vertex(ring, interval + 1),
        (parameter - chain.parameters[interval])
            / (chain.parameters[interval + 1] - chain.parameters[interval]),
    )
}
fn certify_collar(
    source: &MultiPolygon<f64>,
    field: &RibField,
    chain: &Chain,
    normal: [f64; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let ring = source.0[chain.polygon].exterior();
    let lo = anchor(chain, ring, field.margin);
    let hi = anchor(chain, ring, field.width - field.margin);
    let inward = |p: Coord<f64>, distance: f64| Coord {
        x: p.x - normal[0] * distance,
        y: p.y - normal[1] * distance,
    };
    let strip_lo = inward(lo, field.overlap / 2.0);
    let strip_hi = inward(hi, field.overlap / 2.0);
    let strip = Polygon::new(
        LineString(vec![
            strip_lo,
            strip_hi,
            inward(hi, field.overlap),
            inward(lo, field.overlap),
            strip_lo,
        ]),
        vec![],
    );
    // The full collar includes the actual segmented native boundary, not a
    // flattened carrier chord. Its material-side component contains the strip.
    let mut top = Vec::with_capacity(chain.edges + 5);
    top.push(lo);
    for offset in 1..chain.edges {
        if chain.parameters[offset] > field.margin
            && chain.parameters[offset] < field.width - field.margin
        {
            top.push(chain.vertex(ring, offset));
        }
    }
    top.push(hi);
    top.push(inward(hi, field.overlap));
    top.push(inward(lo, field.overlap));
    top.push(lo);
    let collar = Polygon::new(LineString(top), vec![]);
    charge(work, 5 * region::point_count(source))?;
    for p in [
        strip_lo,
        strip_hi,
        inward(lo, field.overlap),
        inward(hi, field.overlap),
        inward(base(lo, hi, 0.5), field.overlap / 2.0),
    ] {
        check_cancel(cancel)?;
        if !source.contains(&Point(p)) {
            return Err(unsupported(
                &field.face_id,
                "positive inward overlap is outside original native material (thin wall or protected hole)",
            ));
        }
    }
    let collar_segments = collar.exterior().0.len() - 1;
    for (owner, material) in source.0.iter().enumerate() {
        for (ring_index, boundary_ring) in std::iter::once(material.exterior())
            .chain(material.interiors())
            .enumerate()
        {
            for (index, boundary) in boundary_ring.0.windows(2).enumerate() {
                check_cancel(cancel)?;
                charge(work, 20 + 3 * collar_segments)?;
                let line = Line::new(boundary[0], boundary[1]);
                if strip.contains(&Point(boundary[0]))
                    || strip
                        .exterior()
                        .0
                        .windows(2)
                        .any(|side| line_intersection(line, Line::new(side[0], side[1])).is_some())
                {
                    return Err(unsupported(
                        &field.face_id,
                        "native boundary or hole enters the strictly inward overlap strip",
                    ));
                }
                if owner == chain.polygon
                    && ring_index == 0
                    && chain.owns(index, boundary_ring.0.len() - 1)
                {
                    continue;
                }
                if collar.contains(&Point(boundary[0]))
                    || collar
                        .exterior()
                        .0
                        .windows(2)
                        .any(|side| line_intersection(line, Line::new(side[0], side[1])).is_some())
                {
                    return Err(unsupported(
                        &field.face_id,
                        "untargeted native boundary or hole enters the inward collar",
                    ));
                }
            }
        }
    }
    Ok(())
}
fn replacement(
    source: &MultiPolygon<f64>,
    field: &RibField,
    sample: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Replacement, ManufacturingError> {
    let chain = matched_chain(source, field, work, cancel)?;
    let ring = source.0[chain.polygon].exterior();
    let normal_length = field.normal[0].hypot(field.normal[1]);
    let normal = [
        field.normal[0] / normal_length,
        field.normal[1] / normal_length,
    ];
    certify_collar(source, field, &chain, normal, work, cancel)?;
    let mut nodes = Vec::with_capacity(chain.parameters.len() + field.sample_points());
    for (offset, parameter) in chain.parameters.iter().enumerate() {
        nodes.push(Node {
            parameter: *parameter,
            native: Some(offset),
        });
    }
    for index in 0..=field.samples {
        check_cancel(cancel)?;
        let s = field.sample_parameter(index);
        nodes.push(Node {
            parameter: if chain.forward { s } else { field.width - s },
            native: None,
        });
    }
    if let Some(image) = &field.image {
        let mut events = Vec::new();
        image
            .field()?
            .append_u_events(0.0, 1.0, &mut events, field.sample_points(), cancel)
            .map_err(super::image::map_error)?;
        for u in events {
            let s = field.margin + u * (field.width - 2.0 * field.margin);
            nodes.push(Node {
                parameter: if chain.forward { s } else { field.width - s },
                native: None,
            });
        }
        for s in [
            field.margin,
            2.0 * field.margin,
            field.width - 2.0 * field.margin,
            field.width - field.margin,
        ] {
            nodes.push(Node {
                parameter: if chain.forward { s } else { field.width - s },
                native: None,
            });
        }
    }
    nodes.sort_by(|a, b| {
        a.parameter
            .total_cmp(&b.parameter)
            .then(b.native.is_some().cmp(&a.native.is_some()))
    });
    nodes.dedup_by(|a, b| a.parameter == b.parameter); // equal uniform/native parameter uses the native anchor
    if field.image.is_some() {
        let original = nodes.len();
        for index in 0..original - 1 {
            check_cancel(cancel)?;
            let a = nodes[index].parameter;
            let b = nodes[index + 1].parameter;
            let (lo, hi) = if chain.forward {
                (a, b)
            } else {
                (field.width - b, field.width - a)
            };
            if lo < field.margin || hi > field.width - field.margin || lo == hi {
                continue;
            }
            let inset = (hi - lo) / 67_108_864.0;
            let left = field.image_piece_derivative(lo + inset, lo, hi, sample);
            let right = field.image_piece_derivative(hi - inset, lo, hi, sample);
            if left * right >= 0.0 {
                continue;
            }
            charge(work, 64)?;
            let (mut low, mut high) = (lo, hi);
            for _ in 0..64 {
                let middle = low + (high - low) / 2.0;
                let derivative = field.image_piece_derivative(middle, lo, hi, sample);
                if derivative.signum() == left.signum() {
                    low = middle
                } else {
                    high = middle
                }
            }
            let s = low + (high - low) / 2.0;
            if nodes.len() >= chain.parameters.len() + field.sample_points() {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "image extrema partition exceeds admitted samples",
                ));
            }
            nodes.push(Node {
                parameter: if chain.forward { s } else { field.width - s },
                native: None,
            });
        }
        nodes.sort_by(|a, b| {
            a.parameter
                .total_cmp(&b.parameter)
                .then(b.native.is_some().cmp(&a.native.is_some()))
        });
        nodes.dedup_by(|a, b| a.parameter == b.parameter);
    }
    let mut points = Vec::with_capacity(nodes.len());
    let full_start = chain.vertex(ring, 0);
    let full_end = chain.vertex(ring, chain.edges);
    let mut interval = 0usize;
    let mut positive = false;
    for (index, node) in nodes.iter().enumerate() {
        check_cancel(cancel)?;
        charge(work, 1)?;
        while interval + 1 < chain.edges && chain.parameters[interval + 1] < node.parameter {
            interval += 1;
        }
        let source_point = node
            .native
            .map(|offset| chain.vertex(ring, offset))
            .unwrap_or_else(|| anchor(&chain, ring, node.parameter));
        let s = if chain.forward {
            node.parameter
        } else {
            field.width - node.parameter
        };
        let depth = field.displacement(s, sample);
        let edge = Line::new(
            chain.vertex(ring, interval),
            chain.vertex(ring, interval + 1),
        );
        let next_edge = node
            .native
            .filter(|offset| *offset > 0 && *offset < chain.edges)
            .map(|offset| Line::new(chain.vertex(ring, offset), chain.vertex(ring, offset + 1)));
        let constraints = [edge, next_edge.unwrap_or(edge)];
        let point = if node.native.is_some() && depth == 0.0 {
            source_point
        } else {
            round_outward(
                Coord {
                    x: source_point.x + normal[0] * depth,
                    y: source_point.y + normal[1] * depth,
                },
                &constraints,
                normal,
                field,
            )?
        };
        let declared = field.point(s, depth);
        if !point.x.is_finite()
            || !point.y.is_finite()
            || (point.x - declared.x).hypot(point.y - declared.y) > FRAME_MM
        {
            return Err(unsupported(
                &field.face_id,
                "actual native chain/frame plus rounding exceeds the reserved construction bound",
            ));
        }
        if constraints.iter().any(|line| {
            RobustKernel::orient2d(line.start, line.end, point) == Orientation::CounterClockwise
        }) {
            return Err(unsupported(
                &field.face_id,
                "constructed point would erase an actual native sampled-segment sliver",
            ));
        }
        positive |= depth > 0.0
            && RobustKernel::orient2d(edge.start, edge.end, point) == Orientation::Clockwise;
        if index > 0 {
            let previous = points[index - 1];
            // Merged native parameters guarantee this interval lies on ONE actual
            // source segment; both new endpoints must be on/outside that segment.
            let left = nodes[index - 1].parameter;
            let segment = chain
                .parameters
                .partition_point(|value| *value <= left)
                .saturating_sub(1)
                .min(chain.edges - 1);
            let a = chain.vertex(ring, segment);
            let b = chain.vertex(ring, segment + 1);
            if node.parameter > chain.parameters[segment + 1]
                || RobustKernel::orient2d(a, b, previous) == Orientation::CounterClockwise
                || RobustKernel::orient2d(a, b, point) == Orientation::CounterClockwise
                || !progression(a, b, previous, point)
                || !progression(full_start, full_end, previous, point)
            {
                return Err(unsupported(
                    &field.face_id,
                    "profile interval does not preserve its actual native sampled segment",
                ));
            }
        }
        points.push(point);
    }
    if !positive {
        return Err(unsupported(
            &field.face_id,
            "requested chain profile produced no strictly outward addition",
        ));
    }
    Ok(Replacement {
        polygon: chain.polygon,
        start: chain.start,
        edges: chain.edges,
        points,
    })
}

pub(super) fn realize(
    source: MultiPolygon<f64>,
    fields: &[PreparedFinish],
    sample: f64,
    retained: usize,
    work: &mut usize,
    cancel: &AtomicBool,
    policy: super::RealizationPolicy,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    if !fields
        .iter()
        .filter(|field| policy.includes(field))
        .any(|field| matches!(&field.shape, PreparedShape::Planar(_)))
    {
        return Ok(source);
    }
    let source_points = region::point_count(&source);
    let profile_points = fields
        .iter()
        .filter(|field| policy.includes(field))
        .filter_map(|field| match &field.shape {
            PreparedShape::Planar(field) => Some(field.sample_points()),
            PreparedShape::Cylindrical(_) => None,
        })
        .sum::<usize>();
    retained
        .checked_add(14 * source_points)
        .and_then(|v| v.checked_add(8 * profile_points))
        .filter(|v| *v <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native chain, merged sampling and splice staging point budget exceeded",
            )
        })?;
    let source_area = source.unsigned_area();
    let selected_fields = fields.iter().filter(|field| policy.includes(field)).count();
    let mut replacements = Vec::<Replacement>::with_capacity(selected_fields);
    for field in fields.iter().filter(|field| policy.includes(field)) {
        let PreparedShape::Planar(field) = &field.shape else {
            continue;
        };
        check_cancel(cancel)?;
        if field
            .image
            .as_ref()
            .is_some_and(|image| image.zero_row(sample))
        {
            continue;
        }
        if sample <= field.bounds.min[2] || sample >= field.bounds.max[2] {
            return Err(unsupported(
                &field.face_id,
                "section lies outside the certified full-height rib interval",
            ));
        }
        let next = replacement(&source, field, sample, work, cancel)?;
        let edge_count = source.0[next.polygon].exterior().0.len() - 1;
        for previous in &replacements {
            if previous.polygon == next.polygon {
                for offset in 0..next.edges {
                    check_cancel(cancel)?;
                    charge(work, 1)?;
                    if ((next.start + offset) % edge_count + edge_count - previous.start)
                        % edge_count
                        < previous.edges
                    {
                        return Err(unsupported(
                            &field.face_id,
                            "certified finish chains claim overlapping native segments",
                        ));
                    }
                }
            }
        }
        replacements.push(next);
    }
    if replacements.is_empty() {
        return Ok(source);
    }
    replacements.sort_by_key(|r| (r.polygon, r.start));
    let mut replacements = replacements.into_iter();
    let mut result = Vec::with_capacity(source.0.len());
    for (owner, polygon) in source.0.into_iter().enumerate() {
        check_cancel(cancel)?;
        let (outer, holes) = polygon.into_inner();
        let count = outer.0.len() - 1;
        let owned = replacements
            .as_slice()
            .partition_point(|r| r.polygon == owner);
        if owned == 0 {
            result.push(Polygon::new(outer, holes));
            continue;
        }
        let extra = replacements.as_slice()[..owned]
            .iter()
            .map(|r| r.points.len() - r.edges - 1)
            .sum::<usize>();
        let first = replacements.next().expect("nonempty native chains");
        let start = first.start;
        let mut visited = first.edges;
        // Reuse the first profile buffer; no copied staged profile arrays or
        // separate per-polygon reference vectors. Other buffers move into it.
        let mut boundary = first.points;
        boundary.pop();
        boundary.reserve_exact((outer.0.len() + extra).saturating_sub(boundary.len()));
        while visited < count {
            check_cancel(cancel)?;
            let index = (start + visited) % count;
            if replacements
                .as_slice()
                .first()
                .is_some_and(|r| r.polygon == owner && r.start == index)
            {
                let mut replacement = replacements.next().expect("peeked native chain");
                visited += replacement.edges;
                replacement.points.pop();
                boundary.extend(replacement.points);
            } else {
                boundary.push(outer.0[index]);
                visited += 1;
            }
        }
        if visited != count {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "native chain splice did not close its original ring traversal",
            ));
        }
        boundary.push(boundary[0]);
        let polygon = Polygon::new(LineString(boundary), holes);
        if polygon.signed_area() <= 0.0 {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "chain splice reversed native material winding",
            ));
        }
        charge(
            work,
            outer
                .0
                .len()
                .checked_mul(
                    polygon.exterior().0.len()
                        + polygon.interiors().iter().map(|h| h.0.len()).sum::<usize>(),
                )
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "source containment work overflow",
                    )
                })?,
        )?;
        // Exact stored-coordinate inclusion supplements the interval half-plane
        // proof at sampled bends. No epsilon acceptance of missing source points.
        for point in &outer.0 {
            check_cancel(cancel)?;
            if !polygon.intersects(&Point(*point)) {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "chain splice excludes an original native source vertex",
                ));
            }
        }
        certify_retention(&outer, polygon.exterior(), work, cancel)?;
        result.push(polygon);
    }
    let result = MultiPolygon(result);
    region::check_size(&result)?;
    certify_simple(&result, work, cancel)?;
    if result.unsigned_area() <= source_area {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "chain splice did not retain positive realized added area",
        ));
    }
    Ok(result)
}

/// Lift a diagnostic witness through the SAME native-edge evaluator/rounding
/// predicates as construction; no second realized polygon or erosion is built.
pub(super) fn witness(
    field: &RibField,
    ring: &LineString<f64>,
    point: Coord<f64>,
    sample: f64,
    cancel: &AtomicBool,
) -> Result<Option<(Coord<f64>, f64)>, ManufacturingError> {
    let s = (point.x - field.origin[0]) * field.horizontal[0]
        + (point.y - field.origin[1]) * field.horizontal[1];
    let count = ring.0.len().saturating_sub(1);
    for (index, edge) in ring.0.windows(2).enumerate() {
        check_cancel(cancel)?;
        let projected = |p: Coord<f64>| {
            (p.x - field.origin[0]) * field.horizontal[0]
                + (p.y - field.origin[1]) * field.horizontal[1]
        };
        let distance = |p: Coord<f64>| {
            (p.x - field.origin[0]) * field.normal[0] + (p.y - field.origin[1]) * field.normal[1]
        };
        if distance(edge[0]).abs() > FRAME_MM || distance(edge[1]).abs() > FRAME_MM {
            continue;
        }
        let a = projected(edge[0]);
        let b = projected(edge[1]);
        if a == b || s < a.min(b) - FRAME_MM || s > a.max(b) + FRAME_MM {
            continue;
        }
        let t = ((s - a) / (b - a)).clamp(0.0, 1.0);
        let source = if t == 0.0 {
            edge[0]
        } else if t == 1.0 {
            edge[1]
        } else {
            base(edge[0], edge[1], t)
        };
        let depth = field.displacement(s, sample);
        if depth == 0.0 {
            return Ok(Some((source, index as f64 + t)));
        }
        let normal_length = field.normal[0].hypot(field.normal[1]);
        let normal = [
            field.normal[0] / normal_length,
            field.normal[1] / normal_length,
        ];
        let own = Line::new(edge[0], edge[1]);
        let adjacent = if t == 0.0 {
            Line::new(ring.0[(index + count - 1) % count], edge[0])
        } else if t == 1.0 {
            Line::new(edge[1], ring.0[(index + 2) % count])
        } else {
            own
        };
        let lifted = round_outward(
            Coord {
                x: source.x + depth * normal[0],
                y: source.y + depth * normal[1],
            },
            &[own, adjacent],
            normal,
            field,
        )?;
        return Ok(Some((lifted, index as f64 + t)));
    }
    Ok(None)
}
