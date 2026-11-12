// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Arachne-style variable-width walls.
//!
//! The section's segment-Voronoi skeleton divides material into cells, each
//! bounded by one medial edge and one boundary site. Every point of a cell
//! lies on a rib from its foot on the site to the medial edge. A bead is a
//! fixed distance along those ribs, so tracing ribs along the edge traces the
//! bead. Edges between opposed walls are *central*: their bead count and width
//! come from the local thickness through [`allocate`], which already
//! distributes width the way Arachne does. Other edges (corner bisectors,
//! fans around concave vertices) inherit the beads of the nearest central
//! region, so lanes follow corners at constant width. Pieces from adjacent
//! cells meet at identical points and are stitched by exact coordinates.
//!
//! Nothing is pre-authorized. The result is checked: wall-band underfill must
//! stay within the existing policy (2% per component, and every missing point
//! within a quarter nozzle of a bead).

use super::skeleton::{MedialEdge, Skeleton};
use super::*;
use geo::{Area, BooleanOps, Coord, LineString};
use std::collections::HashMap;

/// Beads of one local wall: `count` beads across the whole thickness, each
/// `width` wide. `residual` means the full recipe count fits at the preferred
/// width and the rest of the thickness belongs to the fill.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Beading {
    count: usize,
    width: f64,
    residual: bool,
}

impl Beading {
    /// Beads for a wall `2 * radius` thick. [`allocate`] distributes width
    /// across the count; a remainder the fill could not print (narrower than
    /// the narrowest bead) is absorbed by widening the walls instead, as
    /// Arachne redistributes it, within the maximum width.
    fn at(radius: f64, request: &WallRequest<'_>) -> Result<Self, ManufacturingError> {
        let allocation = allocate(radius, request)?;
        if allocation.residual {
            let [minimum, maximum] = admitted_width_range(request)?;
            let thickness = 2.0 * radius;
            let leftover = thickness - allocation.count as f64 * allocation.width;
            let widened = thickness / allocation.count as f64;
            if leftover < minimum && widened <= maximum {
                return Ok(Self {
                    count: allocation.count,
                    width: widened,
                    residual: false,
                });
            }
        }
        Ok(Self {
            count: allocation.count,
            width: allocation.width,
            residual: allocation.residual,
        })
    }
    /// Lanes on each side of the medial axis.
    fn lanes(self) -> usize {
        self.count / 2
    }
    /// Distance of lane `k` from its boundary.
    fn offset(self, k: usize) -> f64 {
        (k as f64 + 0.5) * self.width
    }
    /// An odd count puts one bead on the medial axis itself.
    fn centre(self) -> bool {
        !self.residual && self.count % 2 == 1
    }
}

/// One traced bead piece: points and the width at each point.
struct Piece {
    points: Vec<[f64; 2]>,
    widths: Vec<f64>,
    lane: usize,
    boundary: u32,
}

/// Interior sampling along a medial edge: fine enough that the parabolic
/// chord error stays far below the profile tolerance.
const SAMPLE_MM: f64 = 0.2;
const CHORD_MM: f64 = 0.001;

pub(super) fn plan(
    request: WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<WallPlan, ManufacturingError> {
    check_cancel(cancel)?;
    request.limits.validate()?;
    if request.material.0.is_empty() {
        return Err(unsupported("wall construction requires material"));
    }
    let preferred = source_width_mm(&request)?;
    let mut offset = |region: &MultiPolygon<f64>, distance: f64, work: &mut usize| {
        region::offset_admitted(
            region,
            distance,
            |scratch, scratch_bytes, overlay| {
                if request
                    .retained_points
                    .checked_add(scratch)
                    .is_none_or(|n| n > MAX_RETAINED_GEOMETRY_UNITS)
                {
                    return Err(resource("wall offset staging exceeded"));
                }
                admit_stage(&request, scratch_bytes)?;
                if let Some((a, b)) = overlay {
                    boolean::budget_accumulated(a, b, work, cancel)?;
                }
                Ok(())
            },
            cancel,
        )
    };
    let _ = preferred;

    // The skeleton needs boundary rings that neither touch nor cross.
    // Sampled sections can pinch to a point; opening the material by half
    // the profile tolerance separates such pinches and moves no boundary
    // farther than that.
    let conditioned = {
        let pinch = 0.5 * super::super::OFFSET_TOLERANCE_MM;
        let eroded = offset(request.material, -pinch, work)?;
        let opened = offset(&eroded, pinch, work)?;
        // The opening rounds every corner with micrometre arcs; drop them so
        // no boundary edge is shorter than the skeleton's grid can hold.
        // Topology-preserving, so the simplification itself cannot make
        // two rings touch.
        let mut clean = geo::SimplifyVwPreserve::simplify_vw_preserve(&opened, 1e-6);
        clean.0.retain(|polygon| polygon.unsigned_area() > 0.0);
        region::canonicalize(&mut clean);
        clean
    };
    let graph = skeleton::construct(
        &conditioned,
        request.retained_points,
        request.owned_bytes,
        work,
        cancel,
    );
    #[cfg(test)]
    if graph.is_err() {
        let mut probe = request;
        probe.material = &conditioned;
        dump(&probe, &[], &MultiPolygon(Vec::new()), request.material);
    }
    let graph = graph?;
    let contour = contours::cells(
        &graph,
        &request,
        beading::Construction::SourceFields,
        work,
        cancel,
    )?;
    let inherited = inherit(&graph, &contour, &request)?;

    let mut pieces = Vec::new();
    for (index, edge) in graph.edges.iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 16)?;
        let central = contour[index].is_none();
        trace_edge(
            edge,
            central,
            [inherited[edge.vertex_ids[0]], inherited[edge.vertex_ids[1]]],
            &request,
            &mut pieces,
        )?;
    }
    let mut chains = stitch(pieces, work, cancel)?;
    chains.sort_by_key(|piece| piece.lane);

    // A margin over the minimum survives projection onto the program lattice.
    let minimum = program_coordinates::MIN_DEPOSITION_COMMAND_MM + 0.000_01;
    let narrowest = admitted_width_range(&request)?[0];
    let mut paths = Vec::new();
    let mut correspondence = Vec::new();
    for chain in chains {
        check_cancel(cancel)?;
        let closed = chain.points.len() > 2 && chain.points.first() == chain.points.last();
        let (mut points, mut widths) = simplify(&chain.points, &chain.widths, minimum, closed);
        enforce_minimum(&mut points, &mut widths, minimum, closed);
        if points.len() < 2 || (closed && points.len() < 4) {
            continue;
        }
        let mut edge_widths: Vec<f64> = widths.windows(2).map(|w| 0.5 * (w[0] + w[1])).collect();
        limit_gradient(&points, &mut edge_widths, closed);
        if edge_widths.iter().any(|w| *w < narrowest) {
            return Err(unsupported(
                "wall transition narrows a bead below the admitted width",
            ));
        }
        let path_index = paths.len();
        correspondence.extend(
            (0..edge_widths.len()).map(|edge_index| BoundaryCorrespondence {
                path_index,
                edge_index,
                supports: [chain.boundary; 2],
                facing_boundary: Some(chain.boundary),
                ordinal: chain.lane,
            }),
        );
        let constant = edge_widths.iter().all(|w| *w == edge_widths[0]);
        paths.push(DepositionPath {
            material_domain: None,
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: if constant {
                WidthProfile::Constant {
                    width_mm: edge_widths[0],
                }
            } else {
                WidthProfile::PerEdge {
                    widths_mm: edge_widths,
                }
            },
            physical_z_mm: [request.lower_z, request.upper_z],
            points_mm: points
                .iter()
                .map(|p| [p[0], p[1], request.upper_z])
                .collect(),
            arcs: Vec::new(),
        });
    }
    if paths.is_empty() {
        return Err(unsupported("section has no printable wall bead"));
    }
    let mut fill = cover(&request, &paths, &mut offset, work, cancel)?;

    let retained_bytes = std::mem::size_of::<WallPlan>()
        .checked_add(path_bytes(&paths))
        .and_then(|b| {
            b.checked_add(correspondence.capacity() * std::mem::size_of::<BoundaryCorrespondence>())
        })
        .and_then(|b| b.checked_add(super::super::native::material_bytes(&mut fill)))
        .ok_or_else(|| resource("retained wall byte inventory overflow"))?;
    let retained_points =
        paths.iter().map(|p| p.points_mm.len()).sum::<usize>() + region::point_count(&fill);
    if retained_points
        .checked_add(request.retained_points)
        .is_none_or(|n| n > MAX_RETAINED_GEOMETRY_UNITS)
    {
        return Err(resource("retained wall construction limit exceeded"));
    }
    let section_area_mm2 = request.material.unsigned_area();
    let wall_area_mm2 = (section_area_mm2 - fill.unsigned_area()).max(0.0);
    let protected_hole_area_mm2 = request
        .material
        .0
        .iter()
        .flat_map(Polygon::interiors)
        .map(|ring| Polygon::new(ring.clone(), Vec::new()).unsigned_area())
        .sum();
    Ok(WallPlan {
        paths,
        fill,
        correspondence,
        retained_points,
        retained_bytes,
        section_area_mm2,
        wall_area_mm2,
        protected_hole_area_mm2,
    })
}

/// One bead set per skeleton junction, so every edge meeting there agrees
/// and lane pieces meet at identical points. A junction on a central edge
/// takes the beads of its own thickness. Any other junction takes the beads
/// of the nearest such junction by path length through non-central edges;
/// junctions that reach none (corner-only regions of thick parts) take the
/// full recipe count at the preferred width.
fn inherit(
    graph: &Skeleton,
    contour: &[Option<f64>],
    request: &WallRequest<'_>,
) -> Result<Vec<Beading>, ManufacturingError> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let count = graph.junctions.len();
    let mut value: Vec<Option<Beading>> = vec![None; count];
    let mut adjacency: Vec<Vec<(usize, f64)>> = vec![Vec::new(); count];
    for (index, edge) in graph.edges.iter().enumerate() {
        let [a, b] = edge.vertex_ids;
        if contour[index].is_none() {
            for (end, vertex) in [a, b].into_iter().enumerate() {
                if value[vertex].is_none() {
                    let p = edge.endpoints[end];
                    value[vertex] = Some(Beading::at(radius(edge, p), request)?);
                }
            }
        } else {
            let length = norm(sub(edge.endpoints[1], edge.endpoints[0]));
            adjacency[a].push((b, length));
            adjacency[b].push((a, length));
        }
    }
    let mut distance = vec![f64::INFINITY; count];
    let mut heap = BinaryHeap::new();
    for vertex in 0..count {
        if value[vertex].is_some() {
            distance[vertex] = 0.0;
            heap.push((Reverse(0u64), vertex));
        }
    }
    while let Some((Reverse(bits), vertex)) = heap.pop() {
        let d = f64::from_bits(bits);
        if d > distance[vertex] {
            continue;
        }
        for &(next, length) in &adjacency[vertex] {
            let candidate = d + length;
            if candidate < distance[next] {
                distance[next] = candidate;
                value[next] = value[vertex];
                heap.push((Reverse(candidate.to_bits()), next));
            }
        }
    }
    let full = Beading {
        count: 2 * request.recipe.perimeter_count as usize,
        width: source_width_mm(request)?,
        residual: true,
    };
    Ok(value.into_iter().map(|b| b.unwrap_or(full)).collect())
}

/// Distance from a medial point to the boundary, the same for both cells of
/// the edge so both sides find identical crossings.
fn radius(edge: &MedialEdge, p: [f64; 2]) -> f64 {
    edge.supports[0].radius(p).min(edge.supports[1].radius(p))
}

/// Trace every lane of both cells of one medial edge, plus the centre bead of
/// an odd count.
fn trace_edge(
    edge: &MedialEdge,
    central: bool,
    ends: [Beading; 2],
    request: &WallRequest<'_>,
    pieces: &mut Vec<Piece>,
) -> Result<(), ManufacturingError> {
    let length = norm(sub(edge.endpoints[1], edge.endpoints[0]));
    let mut steps = (length / SAMPLE_MM).ceil().max(1.0) as usize;
    while steps < 4096 && edge.chord_error_bound(0.0, 1.0 / steps as f64) > CHORD_MM {
        steps *= 2;
    }
    // Junction beads are authoritative at the ends. Central edges follow
    // their own thickness between; others switch at the middle.
    let beading_at = |t: f64| -> Result<Beading, ManufacturingError> {
        if t == 0.0 {
            Ok(ends[0])
        } else if t == 1.0 {
            Ok(ends[1])
        } else if central {
            Ok(Beading::at(radius(edge, edge.at(t)), request)?)
        } else {
            // A junction too thin for any bead lends nothing to a corner
            // edge; take the other end's beads instead.
            let (near, far) = if t < 0.5 {
                (ends[0], ends[1])
            } else {
                (ends[1], ends[0])
            };
            Ok(if near.count > 0 { near } else { far })
        }
    };
    // Samples, refined so a bead-count change and every lane's end are found
    // to within a micrometre along the edge.
    let mut samples: Vec<(f64, Beading)> = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let beading = beading_at(t)?;
        if let Some(&(previous_t, previous)) = samples.last()
            && previous != beading
            && previous.count != beading.count
        {
            let (mut lo, mut hi) = (previous_t, t);
            while (hi - lo) * length > 0.001 {
                let mid = 0.5 * (lo + hi);
                if beading_at(mid)?.count == previous.count {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            samples.push((lo, beading_at(lo)?));
            samples.push((hi, beading_at(hi)?));
        }
        samples.push((t, beading));
    }

    // On central edges the bead count is the thickness's. Elsewhere a lane
    // follows its boundary for as long as the material reaches it, up to the
    // recipe's wall count, at the inherited width.
    let recipe_lanes = request.recipe.perimeter_count as usize;
    let lane_limit = |beading: Beading| {
        if central || beading.count == 0 {
            beading.lanes()
        } else {
            recipe_lanes
        }
    };
    let lanes = samples.iter().map(|s| lane_limit(s.1)).max().unwrap_or(0);
    for side in 0..2 {
        let support = edge.supports[side];
        for lane in 0..lanes {
            // A lane exists where the beads have it and the rib reaches it.
            let exists = |t: f64, beading: Beading| {
                lane < lane_limit(beading) && radius(edge, edge.at(t)) >= beading.offset(lane)
            };
            let point = |t: f64, beading: Beading| -> [f64; 2] {
                let p = edge.at(t);
                let foot = support.foot(p);
                let length = norm(sub(p, foot));
                let d = beading.offset(lane);
                if length <= d {
                    p
                } else {
                    add(foot, mul(sub(p, foot), d / length))
                }
            };
            let mut current: Option<Piece> = None;
            for window in 0..samples.len() {
                let (t, beading) = samples[window];
                let here = exists(t, beading);
                if let Some(previous) = window.checked_sub(1).map(|i| samples[i])
                    && exists(previous.0, previous.1) != here
                    && previous.1.count == beading.count
                {
                    // The rib reaches the lane between these samples: the
                    // lane meets the medial axis there, at the same point
                    // for both cells of this edge.
                    let (mut lo, mut hi) = (previous.0, t);
                    let inside_lo = exists(lo, beading);
                    while (hi - lo) * length > 0.000_5 {
                        let mid = 0.5 * (lo + hi);
                        if exists(mid, beading) == inside_lo {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    let crossing = edge.at(if inside_lo { lo } else { hi });
                    let piece = current.get_or_insert_with(|| Piece {
                        points: Vec::new(),
                        widths: Vec::new(),
                        lane,
                        boundary: support.boundary_index,
                    });
                    piece.points.push(crossing);
                    piece.widths.push(beading.width);
                }
                if here {
                    let piece = current.get_or_insert_with(|| Piece {
                        points: Vec::new(),
                        widths: Vec::new(),
                        lane,
                        boundary: support.boundary_index,
                    });
                    piece.points.push(point(t, beading));
                    piece.widths.push(beading.width);
                } else if let Some(piece) = current.take() {
                    pieces.push(piece);
                }
            }
            if let Some(piece) = current.take() {
                pieces.push(piece);
            }
        }
    }
    if central {
        let mut current: Option<Piece> = None;
        for &(t, beading) in &samples {
            if beading.centre() && beading.count > 0 {
                let piece = current.get_or_insert_with(|| Piece {
                    points: Vec::new(),
                    widths: Vec::new(),
                    lane: beading.lanes(),
                    boundary: edge.supports[0].boundary_index,
                });
                piece.points.push(edge.at(t));
                piece.widths.push(beading.width);
            } else if let Some(piece) = current.take() {
                pieces.push(piece);
            }
        }
        if let Some(piece) = current.take() {
            pieces.push(piece);
        }
    }
    Ok(())
}

/// Join pieces of one lane whose ends coincide. Pieces of adjacent cells
/// meet at one point in exact arithmetic, but the two cells of a segment and
/// its endpoint compute it from different feet, and Voronoi vertices are
/// quantized, so the ends can differ by a micrometre fraction. Ends within
/// `STITCH_MM` join, at the earlier piece's point; degenerate pieces (one
/// point) only mark a junction and are dropped.
fn stitch(
    pieces: Vec<Piece>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<Piece>, ManufacturingError> {
    const STITCH_MM: f64 = 0.000_1;
    let cell = |p: [f64; 2]| {
        (
            (p[0] / STITCH_MM).floor() as i64,
            (p[1] / STITCH_MM).floor() as i64,
        )
    };
    let mut pieces: Vec<Option<Piece>> = pieces
        .into_iter()
        .map(|mut piece| {
            piece.points.dedup();
            piece.widths.truncate(piece.points.len());
            piece
        })
        .filter(|piece| piece.points.len() >= 2)
        .map(Some)
        .collect();
    let mut ends: HashMap<((i64, i64), usize), Vec<(usize, bool)>> = HashMap::new();
    for (index, piece) in pieces.iter().enumerate() {
        let piece = piece.as_ref().expect("fresh piece");
        ends.entry((cell(piece.points[0]), piece.lane))
            .or_default()
            .push((index, false));
        ends.entry((cell(*piece.points.last().unwrap()), piece.lane))
            .or_default()
            .push((index, true));
    }
    // The unused piece end nearest `p` on this lane, within the tolerance.
    let find = |pieces: &[Option<Piece>], p: [f64; 2], lane: usize| {
        let (cx, cy) = cell(p);
        let mut best: Option<(f64, usize, bool)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(candidates) = ends.get(&((cx + dx, cy + dy), lane)) else {
                    continue;
                };
                for &(index, at_end) in candidates {
                    let Some(piece) = &pieces[index] else {
                        continue;
                    };
                    let q = if at_end {
                        *piece.points.last().unwrap()
                    } else {
                        piece.points[0]
                    };
                    let d = norm(sub(p, q));
                    if d <= STITCH_MM && best.is_none_or(|b| d < b.0) {
                        best = Some((d, index, at_end));
                    }
                }
            }
        }
        best.map(|(_, index, at_end)| (index, at_end))
    };
    let closes = |chain: &Piece| {
        chain.points.len() > 2
            && norm(sub(*chain.points.last().unwrap(), chain.points[0])) <= STITCH_MM
    };
    let mut chains = Vec::new();
    for start in 0..pieces.len() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let Some(mut chain) = pieces[start].take() else {
            continue;
        };
        // Extend forward from the tail, then backward from the head.
        for _direction in 0..2 {
            loop {
                if closes(&chain) {
                    break;
                }
                let tail = *chain.points.last().unwrap();
                let Some((next, at_end)) = find(&pieces, tail, chain.lane) else {
                    break;
                };
                let mut piece = pieces[next].take().expect("unused piece");
                if at_end {
                    piece.points.reverse();
                    piece.widths.reverse();
                }
                chain.points.extend_from_slice(&piece.points[1..]);
                chain.widths.extend_from_slice(&piece.widths[1..]);
            }
            if closes(&chain) {
                break;
            }
            chain.points.reverse();
            chain.widths.reverse();
        }
        if closes(&chain) {
            let first = chain.points[0];
            *chain.points.last_mut().unwrap() = first;
        }
        chains.push(chain);
    }
    Ok(chains)
}

/// Drop points that a straight command reproduces within half the profile
/// tolerance, and points closer than one minimum command to the last kept
/// point. Ends are kept; a closed chain stays closed.
fn simplify(
    points: &[[f64; 2]],
    widths: &[f64],
    minimum: f64,
    closed: bool,
) -> (Vec<[f64; 2]>, Vec<f64>) {
    let mut kept_points = vec![points[0]];
    let mut kept_widths = vec![widths[0]];
    let last = points.len() - 1;
    let mut anchor = 0;
    for i in 1..last {
        let a = points[anchor];
        let b = points[i + 1];
        let chord = sub(b, a);
        let length = norm(chord);
        let deviation = (anchor + 1..=i)
            .map(|j| {
                if length == 0.0 {
                    norm(sub(points[j], a))
                } else {
                    cross(chord, sub(points[j], a)).abs() / length
                }
            })
            .fold(0.0, f64::max);
        let width_change = (anchor + 1..=i)
            .map(|j| (widths[j] - widths[anchor]).abs())
            .fold(0.0, f64::max);
        let far_enough = norm(sub(points[i], a)) >= minimum;
        // Within half the profile tolerance: the centreline by its deviation,
        // a bead edge by half its width change.
        if far_enough && (deviation > 0.005 || width_change > 0.01) {
            kept_points.push(points[i]);
            kept_widths.push(widths[i]);
            anchor = i;
        }
    }
    let end = if closed { kept_points[0] } else { points[last] };
    while kept_points.len() > 1 && norm(sub(end, *kept_points.last().unwrap())) < minimum {
        kept_points.pop();
        kept_widths.pop();
    }
    kept_points.push(end);
    kept_widths.push(widths[last]);
    (kept_points, kept_widths)
}

/// Every command at least `minimum` long: drop the far point of a short
/// edge (the near point of a short final edge) until none remain. A closed
/// path keeps its first point as its last.
fn enforce_minimum(points: &mut Vec<[f64; 2]>, widths: &mut Vec<f64>, minimum: f64, closed: bool) {
    let mut i = 0;
    while i + 1 < points.len() {
        if norm(sub(points[i + 1], points[i])) >= minimum {
            i += 1;
            continue;
        }
        let last = points.len() - 1;
        let drop = if i + 1 < last {
            i + 1
        } else if i > 0 {
            i
        } else {
            last
        };
        points.remove(drop);
        widths.remove(drop);
        if drop == i && i > 0 {
            i -= 1;
        }
        if points.len() < 2 {
            break;
        }
    }
    if closed && points.len() >= 2 && points.first() != points.last() {
        points.clear();
        widths.clear();
    }
}

/// Arachne's transitions, as a limit: along a continuing wall the width may
/// change by at most the wall gradient times the mean length of the two
/// edges. Widths only narrow, so a transition can leave a thin seam (judged
/// by the printable-void check) but never overfill.
fn limit_gradient(points: &[[f64; 2]], widths: &mut [f64], closed: bool) {
    let gradient = 0.9 * program_coordinates::MAX_WALL_WIDTH_GRADIENT;
    let n = widths.len();
    if n < 2 {
        return;
    }
    let length = |i: usize| norm(sub(points[i + 1], points[i]));
    let lengths: Vec<f64> = (0..n).map(length).collect();
    // Decoded widths differ from planned ones by a little: arc runs take their
    // extrusion from the source circle's swept length. Reserve one micrometre
    // per transition so short edges stay inside the verifier's gradient.
    const DECODE_SLACK_MM: f64 = 0.001;
    let allowed = |a: usize, b: usize| {
        (gradient * 0.5 * (lengths[a] + lengths[b]) - DECODE_SLACK_MM).max(0.0)
    };
    for _ in 0..if closed { 3 } else { 1 } {
        for i in 1..n {
            widths[i] = widths[i].min(widths[i - 1] + allowed(i - 1, i));
        }
        for i in (0..n - 1).rev() {
            widths[i] = widths[i].min(widths[i + 1] + allowed(i, i + 1));
        }
        if closed {
            widths[0] = widths[0].min(widths[n - 1] + allowed(n - 1, 0));
            widths[n - 1] = widths[n - 1].min(widths[0] + allowed(n - 1, 0));
        }
    }
}

/// The fill is what the walls leave, as in Arachne: the material outside
/// every bead, with slivers too thin for any bead removed. What is neither
/// bead nor fill must be too thin for any bead; a void that could hold one
/// is a generator failure.
fn cover(
    request: &WallRequest<'_>,
    paths: &[DepositionPath],
    offset: &mut impl FnMut(
        &MultiPolygon<f64>,
        f64,
        &mut usize,
    ) -> Result<MultiPolygon<f64>, ManufacturingError>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut capsules = Vec::new();
    for path in paths {
        check_cancel(cancel)?;
        for edge in path.checked_edges()? {
            let a = [edge.points_mm[0][0], edge.points_mm[0][1]];
            let b = [edge.points_mm[1][0], edge.points_mm[1][1]];
            capsules.push(capsule(a, b, 0.5 * edge.width_mm));
        }
    }
    boolean::charge(work, capsules.len().saturating_mul(64))?;
    let beads = geo::unary_union(&capsules);
    // Narrower than the narrowest bead is not printable by anything: not the
    // material's tips, and not a sliver left between beads.
    let reach_radius = 0.5 * admitted_width_range(request)?[0] - super::super::OFFSET_TOLERANCE_MM;
    let open = |region: &MultiPolygon<f64>,
                work: &mut usize,
                offset: &mut dyn FnMut(
        &MultiPolygon<f64>,
        f64,
        &mut usize,
    ) -> Result<MultiPolygon<f64>, ManufacturingError>|
     -> Result<MultiPolygon<f64>, ManufacturingError> {
        let eroded = offset(region, -reach_radius, work)?;
        offset(&eroded, reach_radius, work)
    };
    let printable = open(request.material, work, offset)?;
    let mut fill = open(&request.material.difference(&beads), work, offset)?;
    // The fill boundary inherits every bead cap's arc vertices; it only
    // clips infill rows, so half the profile tolerance is its precision.
    fill = geo::Simplify::simplify(&fill, 0.5 * super::super::OFFSET_TOLERANCE_MM);
    fill.0.retain(|polygon| polygon.unsigned_area() > 0.0);
    region::canonicalize(&mut fill);
    let missing = printable.difference(&beads).difference(&fill);
    // A generator failure leaves printable material unprinted: a void that
    // holds the narrowest bead. Voids thinner than that (the centre of a wide
    // wedge, the corner between two round bead ends) no bead and no infill
    // line could fill, so they are not a defect.
    let printable_void = open(&missing, work, offset)?;
    if printable_void.unsigned_area() > 1e-6 {
        #[cfg(test)]
        dump(request, paths, &fill, &printable_void);
        let largest = printable_void
            .0
            .iter()
            .max_by(|a, b| a.unsigned_area().total_cmp(&b.unsigned_area()));
        let bounds = largest.and_then(geo::BoundingRect::bounding_rect);
        return Err(unsupported(&format!(
            "walls leave a printable void unfilled: {:.6} mm2 in {} pieces, largest {:.6} mm2 at {:?}",
            printable_void.unsigned_area(),
            printable_void.0.len(),
            largest.map_or(0.0, |p| p.unsigned_area()),
            bounds.map(|r| [r.min().x, r.min().y, r.max().x, r.max().y]),
        )));
    }
    Ok(fill)
}

fn capsule(a: [f64; 2], b: [f64; 2], radius: f64) -> Polygon<f64> {
    const SIDES: usize = 12;
    let d = sub(b, a);
    let length = norm(d);
    let angle = if length > 0.0 { d[1].atan2(d[0]) } else { 0.0 };
    let mut ring = Vec::with_capacity(2 * SIDES + 3);
    for (centre, start) in [
        (b, angle - std::f64::consts::FRAC_PI_2),
        (a, angle + std::f64::consts::FRAC_PI_2),
    ] {
        for i in 0..=SIDES {
            let theta = start + std::f64::consts::PI * i as f64 / SIDES as f64;
            ring.push(Coord {
                x: centre[0] + radius * theta.cos(),
                y: centre[1] + radius * theta.sin(),
            });
        }
    }
    ring.push(ring[0]);
    Polygon::new(LineString(ring), Vec::new())
}

/// Test-only: write a failing layer's material, fill, beads and uncovered
/// region as JSON into `SPILING_ARACHNE_DUMP`, numbered, for rendering.
#[cfg(test)]
fn dump(
    request: &WallRequest<'_>,
    paths: &[DepositionPath],
    fill: &MultiPolygon<f64>,
    uncovered: &MultiPolygon<f64>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let Ok(dir) = std::env::var("SPILING_ARACHNE_DUMP") else {
        return;
    };
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= 40 {
        return;
    }
    let rings = |m: &MultiPolygon<f64>| -> Vec<Vec<[f64; 2]>> {
        m.0.iter()
            .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
            .map(|r| r.0.iter().map(|c| [c.x, c.y]).collect())
            .collect()
    };
    let beads: Vec<(Vec<[f64; 2]>, Vec<f64>)> = paths
        .iter()
        .map(|p| {
            let widths = match &p.width {
                WidthProfile::Constant { width_mm } => vec![*width_mm; p.points_mm.len() - 1],
                WidthProfile::PerEdge { widths_mm } => widths_mm.clone(),
            };
            (p.points_mm.iter().map(|q| [q[0], q[1]]).collect(), widths)
        })
        .collect();
    let text = format!(
        "{{\"z\":{},\"material\":{:?},\"far\":{:?},\"fill\":{:?},\"beads\":{:?}}}",
        request.upper_z,
        rings(request.material),
        rings(uncovered),
        rings(fill),
        beads
    )
    .replace('(', "[")
    .replace(')', "]");
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(format!("{dir}/dump_{n:02}.json"), text).ok();
}
