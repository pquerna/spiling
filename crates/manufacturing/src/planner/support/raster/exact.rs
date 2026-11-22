// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Sub-cell wall and row-end placement from the exact clearance field.
//!
//! The grid keeps E, the cells where a bead centre clears by construction,
//! but its boundary is a staircase up to a few cells inside the exact
//! answer. Where an exact exclusion bounds E, the wall centreline belongs on
//! the iso-contour `F = level` of the exact clearance field (half a bead,
//! the XYZ6 projection and a small chord margin). Each densely sampled
//! boundary point moves toward its nearest exclusion by `F(p) − level`,
//! which the field's 1-Lipschitz bound keeps on the clear side. Corners can
//! follow that offset outward to a second exact exclusion. Row ends
//! extend along their lattice line to the exact offset of the first
//! exclusion they meet. A point whose clearance exceeds the level by more
//! than [`Exact::reach`] is not bounded by an exclusion (a seed, ground or
//! role edge) and keeps its grid position. Every changed segment is
//! re-checked against the exact field; any that falls short reverts to the
//! grid answer, which clears by construction.

use super::super::field::{Field, FieldBuilder, Nearest};
use super::*;

/// Margin above the bead's centre clearance at which walls and row ends are
/// placed. It covers the sagitta of chords between placed points on curved
/// offsets and floating rounding; every chord is re-checked exactly.
pub(super) const CONTOUR_MARGIN_MM: f64 = 0.001;

/// How far the independent verifier's decoded capsules can reach beyond the
/// nominal ones placement measures, summed over the support bead and the
/// object bead it clears. The field measures the support's emitted XYZ6
/// segment at its nominal half width against object geometry as planned; the
/// verifier decodes both beads and widens them:
/// - each bead's decoded width surplus plus centre displacement (XYZ6
///   rounding, arc point deviation) is at most
///   [`program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM`], which the
///   verifier enforces per command: once for the support bead, once for the
///   object bead;
/// - an object wall bead can become part of an arc run after support
///   planning; the verifier then widens its chord by the run's chord sagitta
///   (at most [`MAX_ARC_SAGITTA_MM`]) plus its point deviation and firmware
///   segmentation (together at most [`PROFILE_TOLERANCE_MM`]);
/// - each decoded endpoint carries an XYZ6 enclosure under one quantum, and
///   the verifier's interval arithmetic and tie-break add a few half quanta
///   per bound (taken as eight quanta).
///
/// The sweep includes this in every exclusion fed to the field, so a
/// certificate against the field implies the verifier's capsule-pair proof.
pub(in crate::planner::support) const VERIFIER_ENCLOSURE_MM: f64 = 2.0
    * program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM
    + MAX_ARC_SAGITTA_MM
    + PROFILE_TOLERANCE_MM
    + 2.0 * program_coordinates::XYZ6_QUANTUM_MM
    + 8.0 * program_coordinates::XYZ6_QUANTUM_MM;

pub(super) const PROJECTION_MOVE_MM: f64 =
    std::f64::consts::FRAC_1_SQRT_2 * program_coordinates::XYZ6_QUANTUM_MM;

/// Deviation allowed when simplifying a placed wall: well under the margin.
const SIMPLIFY_MM: f64 = 0.0005;

/// Deviation allowed when dropping a vertex beside a sub-command edge.
const MERGE_MM: f64 = 0.02;

/// Boundary sample spacing in cells. Every original vertex is a sample, so
/// spacing only subdivides long chords; the field places each sample
/// exactly and every chord between them is checked exactly.
const SAMPLE_CELLS: f64 = 2.0;

/// Smallest sample spacing, in cells, a failing chord is still split at.
const MIN_SPLIT_CELLS: f64 = 0.25;

/// Smallest cosine between a move and the boundary's outward normal: a
/// nearest exclusion behind or beside the wall does not bound it.
const MIN_ALIGNMENT: f64 = 0.5;

const PLACEMENT_EPS_MM: f64 = 1.0e-7;

/// Placement parameters for one layer.
#[derive(Clone, Copy, Debug)]
pub(super) struct Exact {
    /// Raw centre clearance every placed chord keeps against the already
    /// enclosed field, including its maximum movement onto the command lattice.
    pub(super) safe: f64,
    /// Clearance walls and row ends are placed at.
    pub(super) level: f64,
    /// Farthest a wall centreline moves off the grid answer. It stays below
    /// `safe`, so every moved centre remains inside the kept cells.
    pub(super) reach: f64,
    /// Grid cell edge.
    pub(super) cell: f64,
}

impl Exact {
    /// Placement for a bead whose projected centre clearance is `bead`
    /// (half its width plus the endpoint screen). The reach stays below
    /// `bead` alone, so every moved centre remains inside the kept cells.
    pub(super) fn new(bead: f64, cell: f64) -> Self {
        let safe = (bead + PROJECTION_MOVE_MM).next_up();
        Self {
            safe,
            level: safe + CONTOUR_MARGIN_MM,
            reach: (4.0 * cell).min(bead - 2.0 * SIMPLIFY_MM).max(0.0),
            cell,
        }
    }

    /// How far query points can land from the boundary samples: a moved
    /// wall point, or a row end and its extension.
    pub(super) fn spread(&self) -> f64 {
        2.0 * self.reach + 2.0 * self.cell
    }

    /// Clearance up to which the field must be exact.
    pub(super) fn field_reach(&self) -> f64 {
        self.level + self.reach
    }
}

/// One boundary sample: its position, the outward unit normals of the edges
/// it lies on (both the same inside an edge) and the outward normal of the
/// ring smoothed over a few samples, which follows a staircase's mean
/// direction rather than its steps.
#[derive(Clone, Copy, Debug)]
pub(super) struct Sample {
    p: [f64; 2],
    normals: [[f64; 2]; 3],
}

/// Samples on each side spanned by the smoothed normal.
const SMOOTHING: usize = 3;

impl Sample {
    /// How well moving along unit `d` leaves the boundary outward: the
    /// better cosine with either adjacent edge normal.
    fn alignment(&self, d: [f64; 2]) -> f64 {
        self.normals
            .iter()
            .map(|n| n[0] * d[0] + n[1] * d[1])
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

/// Per ring of `contour` (exteriors counterclockwise, holes clockwise, so E
/// lies on the left), samples at most one cell apart, starting at each
/// original vertex.
pub(super) fn samples(
    raster: &Raster<'_>,
    contour: &MultiPolygon<f64>,
    cell: f64,
) -> Result<Vec<Vec<Sample>>, ManufacturingError> {
    let mut rings = Vec::new();
    for polygon in &contour.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            check_cancel(raster.cancel)?;
            let points = &ring.0;
            let n = points.len().saturating_sub(1);
            let mut out = Vec::new();
            if n < 2 {
                raster.reserve(&mut rings, 1)?;
                rings.push(out);
                continue;
            }
            let normal = |i: usize| {
                let (a, b) = (points[i % n], points[(i + 1) % n]);
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let length = dx.hypot(dy);
                if length > 0.0 {
                    [dy / length, -dx / length]
                } else {
                    [0.0, 0.0]
                }
            };
            for i in 0..n {
                let (a, b) = (points[i], points[i + 1]);
                let length = (b.x - a.x).hypot(b.y - a.y);
                let pieces = (length / (SAMPLE_CELLS * cell)).ceil().max(1.0);
                if !pieces.is_finite() {
                    return Err(error(
                        ManufacturingErrorCode::NativeConditioningFailure,
                        "support wall contour has non-finite coordinates",
                    ));
                }
                let pieces = pieces as usize;
                raster.reserve(&mut out, pieces)?;
                raster.charge(pieces);
                let edge = normal(i);
                let before = normal(i + n - 1);
                for j in 0..pieces {
                    let t = j as f64 / pieces as f64;
                    out.push(Sample {
                        p: [a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t],
                        normals: [if j == 0 { before } else { edge }, edge, edge],
                    });
                }
            }
            let count = out.len();
            for k in 0..count {
                let ahead = out[(k + SMOOTHING) % count].p;
                let behind = out[(k + count - SMOOTHING % count) % count].p;
                let (dx, dy) = (ahead[0] - behind[0], ahead[1] - behind[1]);
                let length = dx.hypot(dy);
                if length > 0.0 {
                    out[k].normals[2] = [dy / length, -dx / length];
                }
            }
            raster.reserve(&mut rings, 1)?;
            rings.push(out);
        }
    }
    Ok(rings)
}

/// Every sample position, for the field's query band.
pub(super) fn sample_points(
    raster: &Raster<'_>,
    rings: &[Vec<Sample>],
) -> Result<Vec<[f64; 2]>, ManufacturingError> {
    let mut points = Vec::new();
    raster.reserve(&mut points, rings.iter().map(Vec::len).sum())?;
    points.extend(rings.iter().flatten().map(|sample| sample.p));
    Ok(points)
}

/// Opens a field builder over the samples' band, or `None` without samples.
pub(super) fn builder<'r, 'a>(
    raster: &'r Raster<'a>,
    rings: &[Vec<Sample>],
    exact: &Exact,
) -> Result<Option<FieldBuilder<'r, 'a>>, ManufacturingError> {
    let points = sample_points(raster, rings)?;
    FieldBuilder::new(raster, &points, exact.spread(), exact.field_reach())
}

/// Moves one sample toward its nearest exclusion onto the placement level,
/// when an exclusion bounds it. Each step of `F − level` toward the nearest
/// point keeps `F ≥ level` on the way, by the 1-Lipschitz bound.
fn place(
    sample: &Sample,
    mut nearest: Nearest,
    field: &Field,
    exact: &Exact,
    work: &mut usize,
) -> Option<[f64; 2]> {
    let mut q = sample.p;
    let mut moved = 0.0;
    let first = nearest.clearance - exact.level;
    if !(first > 0.0 && first <= exact.reach) {
        return None;
    }
    // A few refinements: after the first step only a different nearest
    // feature can leave a remainder, and it shrinks quickly.
    for _ in 0..4 {
        let Nearest {
            clearance,
            closest: Some(c),
        } = nearest
        else {
            break;
        };
        let step = clearance - exact.level;
        if step.is_nan() || step <= PLACEMENT_EPS_MM || moved + step > exact.reach {
            break;
        }
        let away = [q[0] - c[0], q[1] - c[1]];
        let length = away[0].hypot(away[1]);
        if length.is_nan() || length <= 0.0 {
            break;
        }
        let u = [away[0] / length, away[1] / length];
        if sample.alignment([-u[0], -u[1]]) < MIN_ALIGNMENT {
            break;
        }
        q = [q[0] - u[0] * step, q[1] - u[1] * step];
        moved += step;
        nearest = field.nearest(q, work);
    }
    (moved > 0.0 && nearest.clearance >= exact.safe)
        .then(|| corner(sample, q, nearest, field, exact, work).unwrap_or(q))
}

/// At a corner, the first offset can stop one component of the move while
/// the other is still inset. Follow its tangent to a second exact exclusion,
/// not to the edge of the search band.
fn corner(
    sample: &Sample,
    placed: [f64; 2],
    nearest: Nearest,
    field: &Field,
    exact: &Exact,
    work: &mut usize,
) -> Option<[f64; 2]> {
    let [before, after, _] = sample.normals;
    if before == after || (nearest.clearance - exact.level).abs() > PLACEMENT_EPS_MM {
        return None;
    }
    let closest = nearest.closest?;
    let away = [placed[0] - closest[0], placed[1] - closest[1]];
    let distance = away[0].hypot(away[1]);
    if distance <= 0.0 {
        return None;
    }
    let forward = [-away[1] / distance, away[0] / distance];
    let reverse = [-forward[0], -forward[1]];
    let alignment = |direction: [f64; 2]| {
        [before, after]
            .map(|normal| normal[0] * direction[0] + normal[1] * direction[1])
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let direction = if alignment(forward) >= alignment(reverse) {
        forward
    } else {
        reverse
    };
    if alignment(direction) < MIN_ALIGNMENT {
        return None;
    }
    let offset = [placed[0] - sample.p[0], placed[1] - sample.p[1]];
    let along = offset[0] * direction[0] + offset[1] * direction[1];
    let across = offset[0] * direction[1] - offset[1] * direction[0];
    let remaining = (exact.reach * exact.reach - across * across).sqrt() - along;
    if !remaining.is_finite() || remaining <= PLACEMENT_EPS_MM {
        return None;
    }
    let step = field.first_hit(
        placed,
        direction,
        remaining,
        exact.level - PLACEMENT_EPS_MM,
        work,
    )?;
    if step <= PLACEMENT_EPS_MM {
        return None;
    }
    let end = [
        placed[0] + step * direction[0],
        placed[1] + step * direction[1],
    ];
    (field.segment_clearance(placed, end, work) >= exact.safe).then_some(end)
}

/// Recover the rounding loss on straight interface faces not bounded by a nearby
/// exact exclusion. The shifted run and both transitions must fit entirely
/// inside the already grounded, clearance-certified cells.
pub(super) fn refine_kept_faces(
    raster: &Raster<'_>,
    kept: &Bits,
    interface: &Bits,
    contour: &mut MultiPolygon<f64>,
    field: &Field,
    exact: &Exact,
) -> Result<(), ManufacturingError> {
    let shift = ((exact.safe / exact.cell).ceil() * exact.cell - exact.level)
        .min(exact.cell * 0.5 - PLACEMENT_EPS_MM);
    if shift <= PLACEMENT_EPS_MM {
        return Ok(());
    }
    let mut work = 0usize;
    let mut refine = |ring: &mut geo::LineString<f64>| -> Result<(), ManufacturingError> {
        let mut points = Vec::new();
        for (index, edge) in ring.0.windows(2).enumerate() {
            check_cancel(raster.cancel)?;
            work += 1;
            let start: [f64; 2] = edge[0].into();
            let end: [f64; 2] = edge[1].into();
            let dx = end[0] - start[0];
            let dy = end[1] - start[1];
            let length = dx.abs() + dy.abs();
            let midpoint = [(start[0] + end[0]) * 0.5, (start[1] + end[1]) * 0.5];
            let shifted = if (dx == 0.0 || dy == 0.0)
                && length > 2.0 * exact.safe
                && raster.holds_all(interface, midpoint)
                && field.segment_clearance(start, end, &mut work) >= exact.level + exact.cell
            {
                let along = [dx / length * exact.cell, dy / length * exact.cell];
                let outward = [dy / length * shift, -dx / length * shift];
                let lead = [
                    start[0] + along[0] + outward[0],
                    start[1] + along[1] + outward[1],
                ];
                let trail = [
                    end[0] - along[0] + outward[0],
                    end[1] - along[1] + outward[1],
                ];
                let mut fits = true;
                for (from, to) in [(start, lead), (lead, trail), (trail, end)] {
                    let (escape, inspected) = raster.frame.escaped(kept, from, to, exact.safe)?;
                    work += inspected;
                    if escape.is_some() {
                        fits = false;
                        break;
                    }
                }
                fits.then_some([Coord::from(lead), Coord::from(trail)])
            } else {
                None
            };
            if points.is_empty() {
                let Some(shifted) = shifted else { continue };
                raster.reserve(&mut points, ring.0.len() + shifted.len())?;
                points.extend_from_slice(&ring.0[..=index]);
                points.extend(shifted);
            } else {
                raster.reserve(&mut points, 1 + shifted.map_or(0, |pair| pair.len()))?;
                points.push(edge[0]);
                points.extend(shifted.into_iter().flatten());
            }
        }
        if !points.is_empty() {
            raster.reserve(&mut points, 1)?;
            points.push(*ring.0.last().expect("a shifted ring has edges"));
            ring.0 = points;
        }
        Ok(())
    };
    for polygon in &mut contour.0 {
        polygon.try_exterior_mut(&mut refine)?;
        polygon.try_interiors_mut(|rings| rings.iter_mut().try_for_each(&mut refine))?;
    }
    raster.charge(work);
    Ok(())
}

/// The contour with every exclusion-bounded wall moved onto the exact
/// placement level, or `None` when nothing moves. Changed rings are
/// simplified within [`SIMPLIFY_MM`] by chords that keep the exact
/// clearance, and overlapping rings are joined.
pub(super) fn displace(
    raster: &Raster<'_>,
    rings: &[Vec<Sample>],
    field: &Field,
    exact: &Exact,
) -> Result<Option<MultiPolygon<f64>>, ManufacturingError> {
    let mut work = 0usize;
    let mut placed_rings: Vec<Vec<[f64; 2]>> = Vec::new();
    raster.reserve(&mut placed_rings, rings.len())?;
    let mut any = false;
    let mut points = Vec::new();
    let mut nearest = Vec::new();
    for ring in rings {
        check_cancel(raster.cancel)?;
        let n = ring.len();
        // One flat batch of exact clearances per ring.
        points.clear();
        raster.reserve(&mut points, n)?;
        points.extend(ring.iter().map(|sample| sample.p));
        raster.reserve(&mut nearest, n)?;
        field.nearest_batch(&points, &mut nearest, &mut work);
        let mut placed: Vec<[f64; 2]> = Vec::new();
        let mut moved: Vec<bool> = Vec::new();
        raster.reserve(&mut placed, n)?;
        raster.reserve(&mut moved, n)?;
        for (sample, &first) in ring.iter().zip(&nearest) {
            match place(sample, first, field, exact, &mut work) {
                Some(q) => {
                    placed.push(q);
                    moved.push(true);
                }
                None => {
                    placed.push(sample.p);
                    moved.push(false);
                }
            }
        }
        if n < 3 || !moved.iter().any(|&m| m) {
            // Unchanged: keep the original vertices only.
            let mut original = Vec::new();
            raster.reserve(&mut original, n)?;
            original.extend(
                ring.iter()
                    .zip(0..)
                    .filter(|(_, k)| is_vertex(ring, *k))
                    .map(|(s, _)| s.p),
            );
            placed_rings.push(original);
            continue;
        }
        // Every kept chord touching a moved point keeps the bead's exact
        // clearance. A sample pair that does not (a chord across a tight
        // offset curve) gains its placed midpoint; one too short to split,
        // or whose midpoint does not place, reverts both ends to the grid
        // contour. Until no pair fails.
        let mut local: Vec<Sample> = Vec::new();
        raster.reserve(&mut local, n)?;
        local.extend_from_slice(ring);
        let simplified = loop {
            match simplify(raster, &placed, &moved, field, exact, &mut work)? {
                Ok(out) => break Some(out),
                Err(failures) => {
                    let count = local.len();
                    let mut split = Vec::new();
                    raster.reserve(&mut split, count)?;
                    split.resize(count, None);
                    for i in failures {
                        let j = (i + 1) % count;
                        let (a, b) = (local[i].p, local[j].p);
                        let edge = local[i].normals[1];
                        let mid = Sample {
                            p: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5],
                            normals: [edge, edge, local[i].normals[2]],
                        };
                        let spacing = (b[0] - a[0]).hypot(b[1] - a[1]);
                        // Only a pair on one original edge splits, so an
                        // unplaced midpoint would still lie on the contour.
                        let same_edge = local[j].normals[0] == edge;
                        let placed_mid = (same_edge && spacing > MIN_SPLIT_CELLS * exact.cell)
                            .then(|| {
                                let first = field.nearest(mid.p, &mut work);
                                place(&mid, first, field, exact, &mut work)
                            })
                            .flatten();
                        match placed_mid {
                            Some(q) => split[i] = Some((mid, q)),
                            None => {
                                for k in [i, j] {
                                    placed[k] = local[k].p;
                                    moved[k] = false;
                                }
                            }
                        }
                    }
                    if split.iter().any(Option::is_some) {
                        let extra = split.iter().filter(|s| s.is_some()).count();
                        let mut next_local = Vec::new();
                        let mut next_placed = Vec::new();
                        let mut next_moved = Vec::new();
                        raster.reserve(&mut next_local, count + extra)?;
                        raster.reserve(&mut next_placed, count + extra)?;
                        raster.reserve(&mut next_moved, count + extra)?;
                        for k in 0..count {
                            next_local.push(local[k]);
                            next_placed.push(placed[k]);
                            next_moved.push(moved[k]);
                            if let Some((mid, q)) = split[k] {
                                next_local.push(mid);
                                next_placed.push(q);
                                next_moved.push(true);
                            }
                        }
                        local = next_local;
                        placed = next_placed;
                        moved = next_moved;
                    }
                    if !moved.iter().any(|&m| m) {
                        break None;
                    }
                }
            }
        };
        match simplified {
            Some(out) => {
                any = true;
                placed_rings.push(out);
            }
            None => {
                let mut original = Vec::new();
                raster.reserve(&mut original, n)?;
                original.extend(
                    ring.iter()
                        .zip(0..)
                        .filter(|(_, k)| is_vertex(ring, *k))
                        .map(|(s, _)| s.p),
                );
                placed_rings.push(original);
            }
        }
    }
    raster.charge(work);
    if !any {
        return Ok(None);
    }
    let joined = join(raster, &placed_rings)?;
    Ok(Some(joined))
}

/// A long exact-offset rail may reach a second exclusion beyond the
/// displacement bound of an individual grid sample. Its changed chords
/// must prove both exact clearance and continuous kept centerlines.
pub(super) fn extend_interface_turns(
    raster: &Raster<'_>,
    kept: &Bits,
    interface: &Bits,
    contour: &mut MultiPolygon<f64>,
    field: &Field,
    exact: &Exact,
) -> Result<(), ManufacturingError> {
    let mut work = 0;
    let mut changed = false;
    for polygon in &mut contour.0 {
        let mut extend = |ring: &mut geo::LineString<f64>| {
            changed |= extend_ring_turns(
                raster,
                kept,
                interface,
                &mut ring.0,
                field,
                exact,
                &mut work,
            )?;
            Ok(())
        };
        polygon.try_exterior_mut(&mut extend)?;
        polygon.try_interiors_mut(|rings| rings.iter_mut().try_for_each(&mut extend))?;
    }
    raster.charge(work);
    if changed {
        let mut rings = Vec::new();
        for polygon in &contour.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                let count = ring.0.len().saturating_sub(1);
                let mut points = Vec::new();
                raster.reserve(&mut points, count)?;
                points.extend(ring.0[..count].iter().copied().map(<[f64; 2]>::from));
                raster.reserve(&mut rings, 1)?;
                rings.push(points);
            }
        }
        *contour = join(raster, &rings)?;
    }
    Ok(())
}

fn extend_ring_turns(
    raster: &Raster<'_>,
    kept: &Bits,
    interface: &Bits,
    ring: &mut Vec<Coord<f64>>,
    field: &Field,
    exact: &Exact,
    work: &mut usize,
) -> Result<bool, ManufacturingError> {
    let count = ring.len().saturating_sub(1);
    if count < 3 {
        return Ok(false);
    }
    let mut rebuilt: Vec<Coord<f64>> = Vec::new();
    for index in 0..count {
        check_cancel(raster.cancel)?;
        *work += 1;
        let origin: [f64; 2] = ring[index].into();
        let previous: [f64; 2] = rebuilt
            .last()
            .copied()
            .unwrap_or(ring[(index + count - 1) % count])
            .into();
        let next: [f64; 2] = if index + 1 == count {
            rebuilt.first().copied().unwrap_or(ring[0])
        } else {
            ring[index + 1]
        }
        .into();
        let mut replacement = None;
        for (rail, adjacent, bend_before) in [(previous, next, false), (next, previous, true)] {
            let dx = origin[0] - rail[0];
            let dy = origin[1] - rail[1];
            let length = dx.abs() + dy.abs();
            if (dx.abs() > PLACEMENT_EPS_MM && dy.abs() > PLACEMENT_EPS_MM)
                || length <= 2.0 * exact.cell
            {
                continue;
            }
            if ((adjacent[0] - origin[0]) * dy - (adjacent[1] - origin[1]) * dx).abs()
                <= PLACEMENT_EPS_MM * length
            {
                continue;
            }
            let direction = if dx.abs() <= PLACEMENT_EPS_MM {
                [0.0, dy.signum()]
            } else {
                [dx.signum(), 0.0]
            };
            let nearest = field.nearest(origin, work);
            let Some(closest) = nearest.closest else {
                continue;
            };
            if (nearest.clearance - exact.level).abs() > PLACEMENT_EPS_MM
                || ((origin[0] - closest[0]) * direction[0]
                    + (origin[1] - closest[1]) * direction[1])
                    .abs()
                    > PLACEMENT_EPS_MM
            {
                continue;
            }
            let Some(step) = field.first_hit(
                origin,
                direction,
                (2.0 * exact.safe).min(exact.spread()),
                exact.level - PLACEMENT_EPS_MM,
                work,
            ) else {
                continue;
            };
            if step <= exact.reach {
                continue;
            }
            let end = [
                origin[0] + step * direction[0],
                origin[1] + step * direction[1],
            ];
            let enclosure = program_coordinates::XYZ6_PROJECTION_SCREEN_MM;
            let (escape, inspected) = raster.frame.escaped(interface, origin, end, enclosure)?;
            *work += inspected;
            if escape.is_some() {
                continue;
            }
            let bend = [0, 1].map(|axis| (adjacent[axis] + end[axis]) * 0.25 + origin[axis] * 0.5);
            let mut fits = true;
            for (from, to) in [(rail, end), (end, bend), (bend, adjacent)] {
                let (escape, inspected) = raster.frame.escaped(kept, from, to, enclosure)?;
                *work += inspected;
                if escape.is_some() || field.segment_clearance(from, to, work) < exact.safe {
                    fits = false;
                    break;
                }
            }
            if fits {
                replacement = Some(if bend_before {
                    [Coord::from(bend), Coord::from(end)]
                } else {
                    [Coord::from(end), Coord::from(bend)]
                });
                break;
            }
        }
        if let Some(replacement) = replacement {
            if rebuilt.is_empty() {
                raster.reserve(&mut rebuilt, ring.len() + 1)?;
                rebuilt.extend_from_slice(&ring[..index]);
            }
            raster.reserve(&mut rebuilt, replacement.len())?;
            rebuilt.extend(replacement);
        } else if !rebuilt.is_empty() {
            raster.reserve(&mut rebuilt, 1)?;
            rebuilt.push(ring[index]);
        }
    }
    let changed = !rebuilt.is_empty();
    if changed {
        raster.reserve(&mut rebuilt, 1)?;
        rebuilt.push(rebuilt[0]);
        *ring = rebuilt;
    }
    Ok(changed)
}

/// Whether sample `k` starts an original edge (its normal is the corner
/// bisector). Samples within an edge share the edge normal and lie on it.
fn is_vertex(ring: &[Sample], k: usize) -> bool {
    let n = ring.len();
    if n < 3 {
        return true;
    }
    let (a, b, c) = (ring[(k + n - 1) % n].p, ring[k].p, ring[(k + 1) % n].p);
    let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
    cross != 0.0
}

/// Douglas–Peucker on a closed ring of placed points, admitting only chords
/// within [`SIMPLIFY_MM`] of the points they replace that keep the bead's
/// exact clearance.
fn simplify(
    raster: &Raster<'_>,
    points: &[[f64; 2]],
    moved: &[bool],
    field: &Field,
    exact: &Exact,
    work: &mut usize,
) -> Result<Result<Vec<[f64; 2]>, Vec<usize>>, ManufacturingError> {
    let n = points.len();
    let mut keep = Vec::new();
    raster.reserve(&mut keep, n)?;
    keep.resize(n, false);
    // Split the closed ring at the first point and the point farthest from it.
    let far = (1..n)
        .max_by(|&i, &j| {
            let d = |k: usize| (points[k][0] - points[0][0]).hypot(points[k][1] - points[0][1]);
            d(i).total_cmp(&d(j)).then(j.cmp(&i))
        })
        .unwrap_or(1);
    keep[0] = true;
    keep[far] = true;
    let mut stack: Vec<(usize, usize)> = Vec::new();
    raster.reserve(&mut stack, 2)?;
    stack.push((0, far));
    stack.push((far, n));
    let at = |k: usize| points[k % n];
    let mut failures = Vec::new();
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            // A sample pair: kept as is, it must clear exactly when it
            // leaves the grid contour.
            if (moved[i % n] || moved[j % n])
                && field.segment_clearance(at(i), at(j), work) < exact.safe
            {
                raster.reserve(&mut failures, 1)?;
                failures.push(i % n);
            }
            continue;
        }
        let (a, b) = (at(i), at(j));
        let mut worst = (0.0, i + 1);
        for k in i + 1..j {
            let c = super::super::field::closest_on_segment(at(k), a, b);
            let d = (at(k)[0] - c[0]).hypot(at(k)[1] - c[1]);
            if d > worst.0 {
                worst = (d, k);
            }
        }
        *work += j - i;
        if worst.0 <= SIMPLIFY_MM && field.segment_clearance(a, b, work) >= exact.safe {
            continue;
        }
        let split = if worst.0 > SIMPLIFY_MM {
            worst.1
        } else {
            (i + j) / 2
        };
        keep[split % n] = true;
        raster.reserve(&mut stack, 2)?;
        stack.push((i, split));
        stack.push((split, j));
    }
    if !failures.is_empty() {
        return Ok(Err(failures));
    }
    let mut out = Vec::new();
    raster.reserve(&mut out, keep.iter().filter(|&&k| k).count())?;
    out.extend((0..n).filter(|&k| keep[k]).map(|k| points[k]));
    merge_short(&mut out, field, exact, work);
    Ok(Ok(out))
}

/// Drops a vertex beside an edge shorter than a comfortable command when the
/// chord replacing it stays within [`MERGE_MM`] of it and keeps the bead's
/// exact clearance, so command fitting is not left a sub-command edge it
/// can only omit.
fn merge_short(points: &mut Vec<[f64; 2]>, field: &Field, exact: &Exact, work: &mut usize) {
    let short = 1.5 * program_coordinates::MIN_DEPOSITION_COMMAND_MM;
    let length = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]).hypot(b[1] - a[1]);
    let mut k = 0;
    while points.len() > 3 && k < points.len() {
        let n = points.len();
        let (a, p, b) = (points[(k + n - 1) % n], points[k], points[(k + 1) % n]);
        *work += 1;
        if (length(a, p) < short || length(p, b) < short) && {
            let c = super::super::field::closest_on_segment(p, a, b);
            length(p, c) <= MERGE_MM && field.segment_clearance(a, b, work) >= exact.safe
        } {
            points.remove(k);
            k = k.saturating_sub(1);
        } else {
            k += 1;
        }
    }
}

/// Joins placed rings into polygons: a point is inside where the rings'
/// winding is positive, so overlapping walls of separate pieces merge and a
/// ring folded over itself keeps only its positively wound part.
fn join(
    raster: &Raster<'_>,
    rings: &[Vec<[f64; 2]>],
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    use i_overlay::core::overlay::ShapeType;
    use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
    use i_overlay::float::overlay::FloatOverlay;
    use i_overlay::i_float::{adapter::FloatPointAdapter, float::rect::FloatRect};
    let rings: Vec<&Vec<[f64; 2]>> = rings.iter().filter(|ring| ring.len() >= 3).collect();
    if rings.is_empty() {
        return Ok(MultiPolygon(Vec::new()));
    }
    let [lo, hi] = raster.frame.bounds();
    let adapter = FloatPointAdapter::new(FloatRect {
        min_x: lo[0],
        min_y: lo[1],
        max_x: hi[0],
        max_y: hi[1],
    });
    let points: usize = rings.iter().map(|ring| ring.len()).sum();
    (raster.admit)(points.saturating_mul(64))?;
    raster.charge(points);
    let contours: Vec<Vec<[f64; 2]>> = rings.into_iter().cloned().collect();
    let shapes = FloatOverlay::with_adapter(adapter, points)
        .unsafe_add_source(&contours, ShapeType::Subject)
        .overlay(OverlayRule::Subject, FillRule::Positive);
    let mut joined = MultiPolygon(
        shapes
            .into_iter()
            .map(|shape| {
                let mut rings = shape.into_iter().map(|ring| {
                    let mut line =
                        geo::LineString(ring.into_iter().map(geo::Coord::from).collect());
                    line.close();
                    line
                });
                geo::Polygon::new(
                    rings.next().unwrap_or_else(geo::LineString::empty),
                    rings.collect(),
                )
            })
            .collect(),
    );
    region::canonicalize(&mut joined);
    region::check_size(&joined)?;
    Ok(joined)
}

/// Extends each run end of one lattice line that borders cells outside E
/// along the line to the exact placement level of the first exclusion it
/// meets within twice the wall reach. Ends at a role seam (E continues) and
/// ends no exclusion bounds keep their grid position; an extension stays in
/// the kept support cells, short of the neighbouring run, and keeps the
/// bead's exact clearance.
#[expect(
    clippy::too_many_arguments,
    reason = "Grid operands, the exact field and the line stay explicit beside the counted work"
)]
pub(super) fn extend_runs(
    raster: &Raster<'_>,
    support: &Bits,
    centers: &Bits,
    field: &Field,
    exact: &Exact,
    fixed: usize,
    coordinate: f64,
    hits: &mut [f64],
) {
    let point = |v: f64| {
        if fixed == 1 {
            [v, coordinate]
        } else {
            [coordinate, v]
        }
    };
    let along = |sign: f64| {
        if fixed == 1 { [sign, 0.0] } else { [0.0, sign] }
    };
    let mut work = 0usize;
    let pairs = hits.len() / 2;
    let probe = exact.cell * 0.5 + 2.0 * program_coordinates::XYZ6_QUANTUM_MM;
    let length = 2.0 * exact.reach;
    for k in 0..pairs {
        for (end, sign) in [(2 * k + 1, 1.0), (2 * k, -1.0)] {
            let from = hits[end];
            if raster.holds_all(centers, point(from + sign * probe)) {
                continue;
            }
            let p = point(from);
            let d = along(sign);
            let Some(t) = field.first_hit(p, d, length, exact.level, &mut work) else {
                continue;
            };
            if t.is_nan() || t <= 0.0 {
                continue;
            }
            let to = from + sign * t;
            let clear_of_neighbour = if sign > 0.0 {
                k + 1 >= pairs || to < hits[2 * k + 2]
            } else {
                k == 0 || to > hits[2 * k - 1]
            };
            if clear_of_neighbour
                && raster.holds_any(support, point(to))
                && field.segment_clearance(p, point(to), &mut work) >= exact.safe
            {
                hits[end] = to;
            }
        }
    }
    raster.charge(work);
}
