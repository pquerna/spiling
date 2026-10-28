// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! In-place command fitting. Proposal refinement is not a demand to emit a
//! micron-sized extrusion. The original continuing geometry/width enclosure is
//! retained, and joins never erase a changed boundary-support obligation.
use super::*;
use geo::{
    Coord,
    kernels::{Kernel, Orientation, RobustKernel},
};

const MERGE_ENCLOSURE_MM: f64 = 0.0024;
const TARGET_LENGTH_MM: f64 = program_coordinates::MIN_DEPOSITION_COMMAND_MM + 0.000003;
#[derive(Clone, Copy)]
struct Run {
    a: [f64; 3],
    b: [f64; 3],
    length: f64,
    mean: f64,
    source_mean: f64,
    minimum: f64,
    maximum: f64,
    geometric_error: f64,
    owner: BoundaryCorrespondence,
    first: (usize, usize),
    first_fraction: f64,
    last: (usize, usize),
    last_fraction: f64,
    source_count: usize,
}

/// Borrow the immutable proposal walk, including fractional affine-edge spans.
/// Splits retain their original source interval and XYZ6 enclosure; the mate
/// table still supplies the original order after output compaction.
pub(super) struct OriginalPath<'a> {
    pub(super) edges: &'a [BoundaryEdge],
    pub(super) mates: &'a [[Option<(usize, usize)>; 2]],
    pub(super) first: (usize, usize),
    pub(super) minimum_width_mm: f64,
    /// Total live outside ownership, including the admitted assembly peak.
    pub(super) owned_bytes: usize,
}
impl OriginalPath<'_> {
    fn next(&self, at: (usize, usize)) -> Result<Option<(usize, usize)>, ManufacturingError> {
        self.mates
            .get(at.0)
            .and_then(|ends| ends.get(1 - at.1))
            .copied()
            .ok_or_else(|| unsupported("wall fitting lost its original proposal walk"))
    }

    fn deviation(
        &self,
        run: Run,
        a: [f64; 2],
        b: [f64; 2],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<f64, ManufacturingError> {
        let chord = sub(b, a);
        let squared = dot(chord, chord);
        let mut at = run.first;
        // Endpoint proximity plus the complete source-to-chord tube also
        // bounds the reverse tube by connected projection of the source span.
        let endpoint_bound = |source: [f64; 2], command: [f64; 2]| {
            if source == command {
                return 0.0;
            }
            let scale = source
                .into_iter()
                .chain(command)
                .map(f64::abs)
                .fold(1.0_f64, f64::max);
            (norm(sub(source, command)) + 128.0 * f64::EPSILON * scale).next_up()
        };
        let mut maximum =
            endpoint_bound(source_point(self, run.first, run.first_fraction)?, a).max(
                endpoint_bound(source_point(self, run.last, run.last_fraction)?, b),
            );
        for index in 0..run.source_count {
            check_cancel(cancel)?;
            boolean::charge(work, 2)?;
            let edge = self
                .edges
                .get(at.0)
                .ok_or_else(|| unsupported("wall fitting lost an original proposal edge"))?;
            let ends = [
                point(key(edge.points[at.1])?),
                point(key(edge.points[1 - at.1])?),
            ];
            let fractions = [
                if index == 0 { run.first_fraction } else { 0.0 },
                if index + 1 == run.source_count {
                    run.last_fraction
                } else {
                    1.0
                },
            ];
            for fraction in fractions {
                let p = add(ends[0], mul(sub(ends[1], ends[0]), fraction));
                maximum = maximum.max(chord_distance_bound(p, a, b, chord, squared));
            }
            if index + 1 < run.source_count {
                at = self
                    .next(at)?
                    .ok_or_else(|| unsupported("wall fitting original walk ends before its run"))?;
            }
        }
        Ok(maximum)
    }

    /// Seal the actual consumed source cycle, not a coincident XYZ6 endpoint.
    /// Summary ranges must partition this one mate walk exactly once.
    fn cyclic_partition(
        &self,
        runs: &[Run],
        tail: Run,
        total: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        let mut cursor = Some(self.first);
        let mut fraction = 0.0;
        let mut consumed = 0usize;
        for run in runs.iter().copied().chain(std::iter::once(tail)) {
            if run.source_count == 0 || cursor != Some(run.first) || fraction != run.first_fraction
            {
                return Ok(false);
            }
            for index in 0..run.source_count {
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                let Some(at) = cursor else {
                    return Ok(false);
                };
                if index + 1 == run.source_count {
                    if at != run.last || run.last_fraction <= fraction {
                        return Ok(false);
                    }
                    if run.last_fraction < 1.0 {
                        fraction = run.last_fraction;
                        break;
                    }
                }
                consumed += 1;
                if consumed > total {
                    return Ok(false);
                }
                cursor = self.next(at)?;
                fraction = 0.0;
            }
        }
        Ok(consumed == total && cursor == Some(self.first) && fraction == 0.0)
    }
}

fn source_point(
    original: &OriginalPath<'_>,
    at: (usize, usize),
    fraction: f64,
) -> Result<[f64; 2], ManufacturingError> {
    let edge = &original.edges[at.0];
    let a = point(key(edge.points[at.1])?);
    let b = point(key(edge.points[1 - at.1])?);
    Ok(add(a, mul(sub(b, a), fraction)))
}

/// A fractional view of the SAME immutable source walk. Full clipped source
/// intervals, continuous endpoint widths and the new XYZ6 endpoint displacement
/// all enter the existing fit enclosure; no source vertex or interval is lost.
fn fractional_run(
    original: &OriginalPath<'_>,
    owner: BoundaryCorrespondence,
    first: (usize, usize),
    first_fraction: f64,
    last: (usize, usize),
    last_fraction: f64,
    z: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Run>, ManufacturingError> {
    let a = point(key(source_point(original, first, first_fraction)?)?);
    let b = point(key(source_point(original, last, last_fraction)?)?);
    if a == b {
        return Ok(None);
    }
    let mut at = first;
    let mut count = 0usize;
    let mut length = 0.0;
    let mut moment = 0.0;
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    // The summary belongs to exactly these source intervals, not to a wider
    // parent's association when an affine suffix is split.
    let first_edge = &original.edges[first.0];
    let mut owner = BoundaryCorrespondence {
        supports: first_edge.supports,
        facing_boundary: first_edge.facing_boundary,
        ordinal: first_edge.ordinal,
        ..owner
    };
    let mut prior_width = None;
    loop {
        check_cancel(cancel)?;
        boolean::charge(work, 8)?;
        count += 1;
        if count > original.edges.len() {
            return Err(resource("fractional wall span exceeds its source walk"));
        }
        let edge = &original.edges[at.0];
        let association = BoundaryCorrespondence {
            supports: edge.supports,
            facing_boundary: edge.facing_boundary,
            ordinal: edge.ordinal,
            ..owner
        };
        let Some(merged) = merge_owner(owner, association) else {
            return Ok(None);
        };
        owner = merged;
        let source_widths = [edge.source_widths[at.1], edge.source_widths[1 - at.1]];
        if prior_width.is_some_and(|width| width != source_widths[0]) {
            return Ok(None);
        }
        prior_width = Some(source_widths[1]);
        let fractions = [
            if count == 1 { first_fraction } else { 0.0 },
            if at == last { last_fraction } else { 1.0 },
        ];
        if fractions[0] >= fractions[1] {
            return Ok(None);
        }
        let points = [
            source_point(original, at, fractions[0])?,
            source_point(original, at, fractions[1])?,
        ];
        let piece_length = norm(sub(points[1], points[0]));
        let widths = if edge.width_mm == source_widths[0].min(source_widths[1]) {
            fractions
                .map(|fraction| source_widths[0] + (source_widths[1] - source_widths[0]) * fraction)
        } else {
            // An ordinary adjacent-lane packing cap is still authoritative for
            // this proposal. A split cannot inflate it from endpoint metadata.
            [edge.width_mm; 2]
        };
        minimum = minimum.min(widths[0].min(widths[1]));
        maximum = maximum.max(widths[0].max(widths[1]));
        length += piece_length;
        moment += (widths[0] + widths[1]) * 0.5 * piece_length;
        if at == last {
            break;
        }
        at = original
            .next(at)?
            .ok_or_else(|| unsupported("fractional wall span lost its original endpoint"))?;
    }
    if length <= 0.0 {
        return Ok(None);
    }
    let mean = moment / length;
    let mut run = Run {
        a: [a[0], a[1], z],
        b: [b[0], b[1], z],
        length,
        mean,
        source_mean: mean,
        minimum,
        maximum,
        geometric_error: 0.0,
        owner,
        first,
        first_fraction,
        last,
        last_fraction,
        source_count: count,
    };
    run.geometric_error = original.deviation(run, a, b, work, cancel)?;
    Ok(Some(run))
}

fn advance_fraction(
    original: &OriginalPath<'_>,
    mut at: (usize, usize),
    mut fraction: f64,
    mut distance: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<((usize, usize), f64), ManufacturingError> {
    for _ in 0..original.edges.len() {
        check_cancel(cancel)?;
        boolean::charge(work, 2)?;
        let length = norm(sub(
            source_point(original, at, 1.0)?,
            source_point(original, at, 0.0)?,
        ));
        let available = (1.0 - fraction) * length;
        if length > 0.0 && distance <= available {
            return Ok((at, (fraction + distance / length).min(1.0)));
        }
        distance -= available;
        at = original
            .next(at)?
            .ok_or_else(|| unsupported("fractional wall split exceeds its source walk"))?;
        fraction = 0.0;
    }
    Err(resource(
        "fractional wall split exceeds its bounded source walk",
    ))
}

/// Propose free joins for a curved short tail using its end tangent lines.
/// Every original interval, endpoint displacement, width and material corridor
/// still passes the ordinary fitting proof; the bevel is not extra authority.
#[allow(clippy::too_many_arguments)]
fn fit_corner(
    prior: Run,
    tail: Run,
    consumed: usize,
    total: usize,
    permissions: &[Junction],
    original: &OriginalPath<'_>,
    permission_work: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<([Run; 3], usize)>, ManufacturingError> {
    check_cancel(cancel)?;
    boolean::charge(work, 64)?;
    if consumed == total || tail.last_fraction != 1.0 {
        return Ok(None);
    }
    let mut at = original.next(tail.last)?;
    let mut end = tail.last;
    let mut added = 0;
    let mut reach = 0.0;
    while reach < 2.0 * TARGET_LENGTH_MM && consumed + added < total {
        check_cancel(cancel)?;
        boolean::charge(work, 8)?;
        let Some(current) = at else { return Ok(None) };
        let edge = &original.edges[current.0];
        if merge_owner(
            tail.owner,
            BoundaryCorrespondence {
                supports: edge.supports,
                facing_boundary: edge.facing_boundary,
                ordinal: edge.ordinal,
                ..tail.owner
            },
        )
        .is_none()
        {
            return Ok(None);
        }
        reach += norm(sub(
            source_point(original, current, 1.0)?,
            source_point(original, current, 0.0)?,
        ));
        end = current;
        at = original.next(current)?;
        added += 1;
    }
    if added == 0 {
        return Ok(None);
    }
    let a = xy(prior.a);
    let c = point(key(source_point(original, end, 1.0)?)?);
    let [u, v] = [prior.first, end].map(|index| {
        let edge = &original.edges[index.0];
        let delta = sub(edge.points[1 - index.1], edge.points[index.1]);
        mul(delta, 1.0 / norm(delta))
    });
    let determinant = u[0] * v[1] - u[1] * v[0];
    let diagonal = norm(add(u, v));
    if determinant == 0.0 || !determinant.is_finite() || diagonal == 0.0 {
        return Ok(None);
    }
    let delta = sub(c, a);
    let distance = (delta[0] * v[1] - delta[1] * v[0]) / determinant;
    let intersection = add(a, mul(u, distance));
    let offset = TARGET_LENGTH_MM / diagonal;
    let joins = [
        sub(intersection, mul(u, offset)),
        add(intersection, mul(v, offset)),
    ];
    if joins.iter().flatten().any(|value| !value.is_finite()) {
        return Ok(None);
    }
    let [Ok(first), Ok(second)] = joins.map(key) else {
        return Ok(None);
    };
    let joins = [point(first), point(second)];
    let positions = [a, joins[0], joins[1], c];
    if positions
        .windows(2)
        .any(|p| norm(sub(p[1], p[0])) < program_coordinates::MIN_DEPOSITION_COMMAND_MM)
    {
        return Ok(None);
    }
    // Interior projections propose ordered fractional cuts, not membership.
    let mut cuts: [Option<(f64, usize, (usize, usize), f64)>; 2] = [None; 2];
    let mut current = prior.first;
    for order in 0..original.edges.len() {
        check_cancel(cancel)?;
        boolean::charge(work, 16)?;
        let p = source_point(original, current, 0.0)?;
        let q = source_point(original, current, 1.0)?;
        let direction = sub(q, p);
        let squared = dot(direction, direction);
        if squared > 0.0 {
            for index in 0..2 {
                let fraction = dot(sub(joins[index], p), direction) / squared;
                if fraction > 0.0
                    && fraction < 1.0
                    && (current != prior.first || fraction > prior.first_fraction)
                {
                    let delta = sub(joins[index], add(p, mul(direction, fraction)));
                    let error = dot(delta, delta);
                    if cuts[index].is_none_or(|best| error < best.0) {
                        cuts[index] = Some((error, order, current, fraction));
                    }
                }
            }
        }
        if current == end {
            break;
        }
        current = original
            .next(current)?
            .ok_or_else(|| unsupported("corner fitting lost its complete source interval"))?;
    }
    let [Some(first), Some(second)] = cuts else {
        return Ok(None);
    };
    if first.1 > second.1 || first.1 == second.1 && first.3 >= second.3 {
        return Ok(None);
    }
    let starts = [
        (prior.first, prior.first_fraction),
        (first.2, first.3),
        (second.2, second.3),
    ];
    let ends = [(first.2, first.3), (second.2, second.3), (end, 1.0)];
    let mut fitted = [None; 3];
    for index in 0..3 {
        let Some(mut candidate) = fractional_run(
            original,
            tail.owner,
            starts[index].0,
            starts[index].1,
            ends[index].0,
            ends[index].1,
            tail.a[2],
            work,
            cancel,
        )?
        else {
            return Ok(None);
        };
        candidate.a = [positions[index][0], positions[index][1], tail.a[2]];
        candidate.b = [positions[index + 1][0], positions[index + 1][1], tail.a[2]];
        candidate.geometric_error = original.deviation(
            candidate,
            positions[index],
            positions[index + 1],
            work,
            cancel,
        )?;
        boolean::charge(work, permission_work)?;
        fitted[index] = fit_candidate(candidate, permissions, original, work, cancel)?;
        if fitted[index].is_none() {
            return Ok(None);
        }
    }
    Ok(Some((fitted.map(Option::unwrap), consumed + added)))
}

fn xy(p: [f64; 3]) -> [f64; 2] {
    [p[0], p[1]]
}
fn chord_distance_bound(
    p: [f64; 2],
    a: [f64; 2],
    b: [f64; 2],
    chord: [f64; 2],
    squared: f64,
) -> f64 {
    if (0..2).all(|axis| p[axis] >= a[axis].min(b[axis]) && p[axis] <= a[axis].max(b[axis]))
        && RobustKernel::orient2d(Coord::from(a), Coord::from(b), Coord::from(p))
            == Orientation::Collinear
    {
        return 0.0;
    }
    let t = (dot(sub(p, a), chord) / squared).clamp(0.0, 1.0);
    let magnitude = p
        .into_iter()
        .chain(a)
        .chain(b)
        .fold(1.0_f64, |largest, value| largest.max(value.abs()));
    (norm(sub(p, add(a, mul(chord, t)))) + 128.0 * f64::EPSILON * magnitude).next_up()
}
/// Supports name boundary RINGS, not exact medial sites. A continuing lane can
/// lose a secondary ring as it leaves a pocket cell, but its command must retain
/// the union of every consumed obligation. The fixed pair cannot represent two
/// distinct secondary rings; neither proximity nor a common facing ring waives
/// that limit. Unfaced medial lanes still require the same complete support set.
fn merge_owner(
    a: BoundaryCorrespondence,
    b: BoundaryCorrespondence,
) -> Option<BoundaryCorrespondence> {
    if a.ordinal != b.ordinal {
        return None;
    }
    let mut left = a.supports;
    left.sort_unstable();
    let mut right = b.supports;
    right.sort_unstable();
    if left == right {
        let facing = |owner: BoundaryCorrespondence| {
            owner.facing_boundary.map_or(left, |boundary| [boundary; 2])
        };
        return (facing(a) == facing(b)).then_some(BoundaryCorrespondence {
            supports: left,
            facing_boundary: a.facing_boundary.or(b.facing_boundary),
            ..a
        });
    }
    let facing = a.facing_boundary?;
    if b.facing_boundary != Some(facing) || !left.contains(&facing) || !right.contains(&facing) {
        return None;
    }
    let mut supports = [facing; 2];
    for ring in left.into_iter().chain(right) {
        if ring != facing {
            if supports[1] != facing && supports[1] != ring {
                return None;
            }
            supports[1] = ring;
        }
    }
    supports.sort_unstable();
    Some(BoundaryCorrespondence { supports, ..a })
}
fn merge(
    a: Run,
    b: Run,
    permissions: &[Junction],
    original: &OriginalPath<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Run>, ManufacturingError> {
    let Some(owner) = merge_owner(a.owner, b.owner) else {
        return Ok(None);
    };
    if a.b != b.a {
        return Ok(None);
    }
    let chord = sub(xy(b.b), xy(a.a));
    let squared = dot(chord, chord);
    if squared <= 0.0 {
        return Ok(None);
    }
    let distance = chord_distance_bound(xy(a.b), xy(a.a), xy(b.b), chord, squared);
    // Both previous chords lie in the new chord's distance tube. All original
    // chords remain connected and include both new endpoints, so continuous
    // projection onto the new chord also gives the reverse Hausdorff bound.
    let previous_error = a.geometric_error.max(b.geometric_error);
    let geometric_error = if distance == 0.0 {
        previous_error
    } else {
        (previous_error + distance).next_up()
    };
    let length = a.length + b.length;
    let source_mean = if a.source_mean == b.source_mean {
        a.source_mean
    } else {
        (a.source_mean * a.length + b.source_mean * b.length) / length
    };
    let overlap = usize::from(
        a.last == b.first && a.last_fraction == b.first_fraction && a.last_fraction < 1.0,
    );
    let source_count = a
        .source_count
        .checked_add(b.source_count)
        .and_then(|count| count.checked_sub(overlap))
        .ok_or_else(|| resource("wall fitting source run count overflow"))?;
    fit_candidate(
        Run {
            a: a.a,
            b: b.b,
            length,
            mean: source_mean,
            source_mean,
            minimum: a.minimum.min(b.minimum),
            maximum: a.maximum.max(b.maximum),
            geometric_error,
            owner,
            first: a.first,
            first_fraction: a.first_fraction,
            last: b.last,
            last_fraction: b.last_fraction,
            source_count,
        },
        permissions,
        original,
        work,
        cancel,
    )
}

fn fit_candidate(
    mut run: Run,
    permissions: &[Junction],
    original: &OriginalPath<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Run>, ManufacturingError> {
    let centered_allowed = permissions.iter().any(|permission| {
        qualification::permission_contains_capsule(
            permission,
            [xy(run.a), xy(run.b)],
            run.source_mean * 0.5 + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
        )
    });
    let fitted_width = |error: f64| {
        if centered_allowed {
            run.source_mean
        } else if error == 0.0 {
            run.minimum
        } else {
            (run.minimum - 2.0 * error).next_down()
        }
    };
    let admitted = |mean: f64, error: f64| {
        mean >= original.minimum_width_mm
            && error + (mean - run.minimum).abs().max((run.maximum - mean).abs()) * 0.5
                <= MERGE_ENCLOSURE_MM
    };
    let mut error = run.geometric_error;
    let mut mean = fitted_width(error);
    if !admitted(mean, error) {
        error = original.deviation(run, xy(run.a), xy(run.b), work, cancel)?;
        mean = fitted_width(error);
        if !admitted(mean, error) {
            return Ok(None);
        }
    }
    run.mean = mean;
    run.geometric_error = error;
    Ok(Some(run))
}

// Coexistent minimum/batch/cyclic/fractional-suffix frames and bounded DFS stack.
const FIT_STACK_BYTES: usize = 28 * std::mem::size_of::<Option<Run>>()
    + std::mem::size_of::<[(usize, usize); usize::BITS as usize]>()
    + 16 * std::mem::size_of::<usize>();

fn remember(
    runs: &mut Vec<Run>,
    index: usize,
    run: Run,
    owned_bytes: usize,
    work: &mut usize,
) -> Result<(), ManufacturingError> {
    boolean::charge(work, 1)?;
    if index == runs.len() {
        if runs.len() == runs.capacity() {
            let next = runs
                .capacity()
                .max(4)
                .checked_mul(2)
                .ok_or_else(|| resource("wall fit summary capacity overflow"))?;
            let peak = runs
                .capacity()
                .checked_add(next)
                .and_then(|n| n.checked_mul(std::mem::size_of::<Run>()))
                .and_then(|b| b.checked_add(owned_bytes))
                .and_then(|b| b.checked_add(FIT_STACK_BYTES))
                .ok_or_else(|| resource("wall fit summary ownership overflow"))?;
            if peak as u64 > MAX_MANUFACTURING_RETAINED_BYTES {
                return Err(resource("wall fit summary staging byte limit exceeded"));
            }
            boolean::charge(work, runs.len())?;
            runs.try_reserve_exact(next - runs.len())
                .map_err(|_| resource("wall fit summary allocation failed"))?;
            let retained = owned_bytes
                .checked_add(FIT_STACK_BYTES)
                .and_then(|b| {
                    runs.capacity()
                        .checked_mul(std::mem::size_of::<Run>())
                        .and_then(|n| b.checked_add(n))
                })
                .ok_or_else(|| resource("wall fit summary retained overflow"))?;
            if retained as u64 > MAX_MANUFACTURING_RETAINED_BYTES {
                return Err(resource("wall fit summary retained byte limit exceeded"));
            }
        }
        runs.push(run);
    } else {
        *runs
            .get_mut(index)
            .ok_or_else(|| unsupported("wall fit lost its legal run partition"))? = run;
    }
    Ok(())
}

fn chord_length(run: Run) -> f64 {
    norm(sub(xy(run.b), xy(run.a)))
}
fn gradient_admitted(a: Run, b: Run) -> bool {
    (a.mean - b.mean).abs()
        <= program_coordinates::MAX_WALL_WIDTH_GRADIENT
            * 0.9
            * (chord_length(a) + chord_length(b))
            * 0.5
}

/// Largest contained width field below the independently fitted commands.
/// One-sided sample minima are not midpoint samples of a continuing field;
/// their quantization bias must not create an artificial gradient jump.
/// Only shrinking is permitted, within the SAME original width/error tube.
fn contained_gradient(
    runs: &mut [Run],
    closed: bool,
    minimum_width: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let count = runs.len();
    for _ in 0..if closed { 2 } else { 1 } {
        for index in 0..count {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            if index == 0 && !closed {
                continue;
            }
            let prior = (index + count - 1) % count;
            // Direct the bound before comparing: a rounded sum can equal the
            // current mean while its subtraction still exceeds the allowance.
            let upper = (runs[prior].mean
                + program_coordinates::MAX_WALL_WIDTH_GRADIENT
                    * 0.9
                    * (chord_length(runs[prior]) + chord_length(runs[index]))
                    * 0.5)
                .next_down();
            if upper < runs[index].mean {
                runs[index].mean = upper;
            }
        }
        for index in (0..count).rev() {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            if index + 1 == count && !closed {
                continue;
            }
            let next = (index + 1) % count;
            let upper = (runs[next].mean
                + program_coordinates::MAX_WALL_WIDTH_GRADIENT
                    * 0.9
                    * (chord_length(runs[next]) + chord_length(runs[index]))
                    * 0.5)
                .next_down();
            if upper < runs[index].mean {
                runs[index].mean = upper;
            }
        }
    }
    for run in runs {
        let width_error = (run.mean - run.minimum)
            .abs()
            .max((run.maximum - run.mean).abs())
            * 0.5;
        if run.mean < minimum_width || run.geometric_error + width_error > MERGE_ENCLOSURE_MM {
            return Err(unsupported(
                "continuing command widths have no contained physical gradient field",
            ));
        }
    }
    Ok(())
}

/// Repair a compatible source suffix, not merely its last two whole commands.
/// The finite command-count search is bounded by already consumed source
/// commands; each trial partitions one original interval without gaps/overlap.
#[allow(clippy::too_many_arguments)]
fn repartition_suffix(
    legal: &mut Vec<Run>,
    tail: Run,
    ceiling: usize,
    permissions: &[Junction],
    original: &OriginalPath<'_>,
    permission_work: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<usize>, ManufacturingError> {
    let mut trial = Vec::new();
    for start in (0..legal.len()).rev() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let first = legal[start];
        if merge_owner(first.owner, tail.owner).is_none() {
            break;
        }
        let Some(span) = fractional_run(
            original,
            tail.owner,
            first.first,
            first.first_fraction,
            tail.last,
            tail.last_fraction,
            tail.a[2],
            work,
            cancel,
        )?
        else {
            break;
        };
        let maximum =
            ((span.length / TARGET_LENGTH_MM).floor() as usize).min(ceiling.saturating_sub(start));
        let trial_outside = original
            .owned_bytes
            .checked_add(legal.capacity() * std::mem::size_of::<Run>())
            .ok_or_else(|| resource("wall suffix trial ownership overflow"))?;
        for commands in 1..=maximum {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            trial.clear();
            let mut at = span.first;
            let mut fraction = span.first_fraction;
            let mut admitted = true;
            for command in 0..commands {
                if fraction == 1.0 {
                    at = original.next(at)?.ok_or_else(|| {
                        unsupported("wall suffix split lost its next source edge")
                    })?;
                    fraction = 0.0;
                }
                let end = if command + 1 == commands {
                    (span.last, span.last_fraction)
                } else {
                    advance_fraction(
                        original,
                        at,
                        fraction,
                        span.length / commands as f64,
                        work,
                        cancel,
                    )?
                };
                let Some(candidate) = fractional_run(
                    original, tail.owner, at, fraction, end.0, end.1, tail.a[2], work, cancel,
                )?
                else {
                    admitted = false;
                    break;
                };
                boolean::charge(work, permission_work)?;
                let Some(candidate) =
                    fit_candidate(candidate, permissions, original, work, cancel)?.filter(|run| {
                        chord_length(*run) >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
                    })
                else {
                    admitted = false;
                    break;
                };
                remember(&mut trial, command, candidate, trial_outside, work)?;
                at = end.0;
                fraction = end.1;
            }
            if !admitted {
                continue;
            }
            if start > 0 {
                let left = legal[start - 1];
                let upper = left.mean
                    + program_coordinates::MAX_WALL_WIDTH_GRADIENT
                        * 0.9
                        * (chord_length(left) + chord_length(trial[0]))
                        * 0.5;
                if upper < trial[0].mean {
                    trial[0].mean = upper.next_down();
                }
            }
            match contained_gradient(&mut trial, false, original.minimum_width_mm, work, cancel) {
                Ok(()) => {}
                Err(error) if error.code == ManufacturingErrorCode::UnsupportedGeometry => continue,
                Err(error) => return Err(error),
            }
            if start > 0 && !gradient_admitted(legal[start - 1], trial[0]) {
                continue;
            }
            let replacement_outside = original
                .owned_bytes
                .checked_add(trial.capacity() * std::mem::size_of::<Run>())
                .ok_or_else(|| resource("wall suffix replacement ownership overflow"))?;
            legal.truncate(start);
            for (index, run) in trial.iter().copied().enumerate() {
                remember(legal, start + index, run, replacement_outside, work)?;
            }
            return Ok(Some(start));
        }
    }
    Ok(None)
}

fn batch(
    runs: &[Run],
    permissions: &[Junction],
    original: &OriginalPath<'_>,
    permission_work: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Run>, ManufacturingError> {
    let mut proposal = runs[0];
    proposal.b = runs.last().unwrap().b;
    let chord = sub(xy(proposal.b), xy(proposal.a));
    let squared = dot(chord, chord);
    if squared <= 0.0 {
        return Ok(None);
    }
    let mut length = 0.0;
    let mut moment = 0.0;
    let mut count = 0usize;
    let mut error = 0.0_f64;
    let constant_mean = runs
        .iter()
        .all(|run| run.source_mean == proposal.source_mean);
    for (index, &run) in runs.iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 8)?;
        let Some(owner) = merge_owner(proposal.owner, run.owner) else {
            return Ok(None);
        };
        if index > 0 && runs[index - 1].b != run.a {
            return Ok(None);
        }
        proposal.owner = owner;
        length += run.length;
        moment += run.source_mean * run.length;
        let overlap = usize::from(
            index > 0
                && runs[index - 1].last == run.first
                && runs[index - 1].last_fraction == run.first_fraction
                && run.first_fraction > 0.0,
        );
        count = count
            .checked_add(run.source_count)
            .and_then(|count| count.checked_sub(overlap))
            .ok_or_else(|| resource("wall batch source count overflow"))?;
        proposal.minimum = proposal.minimum.min(run.minimum);
        proposal.maximum = proposal.maximum.max(run.maximum);
        let departure =
            chord_distance_bound(xy(run.a), xy(proposal.a), xy(proposal.b), chord, squared).max(
                chord_distance_bound(xy(run.b), xy(proposal.a), xy(proposal.b), chord, squared),
            );
        let bound = if departure == 0.0 {
            run.geometric_error
        } else {
            (departure + run.geometric_error).next_up()
        };
        error = error.max(bound);
    }
    proposal.length = length;
    if !constant_mean {
        proposal.source_mean = moment / length;
    }
    proposal.source_count = count;
    proposal.last = runs.last().unwrap().last;
    proposal.last_fraction = runs.last().unwrap().last_fraction;
    proposal.geometric_error = error;
    boolean::charge(work, permission_work)?;
    fit_candidate(proposal, permissions, original, work, cancel)
}

/// Minimum fitting first repairs every short tail. Aggregate only WHOLE legal
/// runs, so a larger command cannot leave an unprintable remainder. Unproved
/// batches are subdivided, not treated as monotone admission facts. Original
/// ranges are disjoint at each balanced depth: full-chain work is O(N log N).
fn maximize(
    path: &mut DepositionPath,
    correspondence: &mut Vec<BoundaryCorrespondence>,
    base: usize,
    runs: &[Run],
    permissions: &[Junction],
    original: &OriginalPath<'_>,
    maximum: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if runs.is_empty() {
        return Ok(());
    }
    let closed = runs[0].a == runs.last().unwrap().b;
    let permission_work = permissions
        .iter()
        .map(|p| p.transition.as_ref().map_or(1, |c| c.points.len()))
        .sum();
    let mut stack = [(0usize, 0usize); usize::BITS as usize];
    stack[0] = (0, runs.len());
    let mut pending = 1;
    let mut written = 0;
    let mut previous = None;
    let mut first = None;
    while pending > 0 {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        pending -= 1;
        let (start, end) = stack[pending];
        let candidate = if end - start == 1 {
            Some(runs[start])
        } else {
            batch(
                &runs[start..end],
                permissions,
                original,
                permission_work,
                work,
                cancel,
            )?
        };
        let left = previous.or_else(|| closed.then(|| *runs.last().unwrap()));
        let right = runs
            .get(end)
            .copied()
            .or_else(|| closed.then(|| first.unwrap_or(runs[0])));
        if let Some(run) = candidate.filter(|run| {
            chord_length(*run) >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
                && left.is_none_or(|left| gradient_admitted(left, *run))
                && right.is_none_or(|right| gradient_admitted(*run, right))
        }) {
            write_run(path, correspondence, base, written, run, maximum)?;
            if first.is_none() {
                first = Some(run);
            }
            previous = Some(run);
            written += 1;
        } else if end - start > 1 {
            let middle = start + (end - start) / 2;
            if pending + 2 > stack.len() {
                return Err(resource("wall fitting batch depth exceeded"));
            }
            stack[pending] = (middle, end);
            stack[pending + 1] = (start, middle);
            pending += 2;
        } else {
            return Err(unsupported(
                "legal wall run partition violates continuing gradient",
            ));
        }
    }
    path.points_mm.truncate(written + 1);
    if let WidthProfile::PerEdge { widths_mm } = &mut path.width {
        widths_mm.truncate(written);
        if let Some(&first) = widths_mm.first() {
            if widths_mm.iter().all(|&w| w == first) {
                path.width = WidthProfile::Constant { width_mm: first };
            }
        }
    }
    correspondence.truncate(base + written);
    Ok(())
}
fn width(path: &DepositionPath, edge: usize, original_constant: Option<f64>) -> f64 {
    // A constant source may acquire a variable fitted output prefix; its
    // unread source widths must not then be read from that compacted prefix.
    original_constant.unwrap_or_else(|| match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm[edge],
    })
}
fn reserve_widths(
    widths: &mut Vec<f64>,
    needed: usize,
    maximum: usize,
) -> Result<(), ManufacturingError> {
    if needed <= widths.capacity() {
        return Ok(());
    }
    if needed > maximum {
        return Err(resource(
            "wall fitting width inventory exceeds source commands",
        ));
    }
    // Assembly admits TWO maximum-width buffers before fitting: old allocation
    // plus its bounded geometric replacement, not semantic command points.
    let old_capacity = widths.capacity();
    let next = needed
        .max(old_capacity.max(4).saturating_mul(2))
        .min(maximum);
    widths
        .try_reserve_exact(next - widths.len())
        .map_err(|_| resource("wall fitting width allocation failed"))?;
    if old_capacity.saturating_add(widths.capacity()) > maximum.saturating_mul(2) {
        return Err(resource(
            "wall fitting width allocation exceeded its admitted peak",
        ));
    }
    Ok(())
}
fn write_run(
    path: &mut DepositionPath,
    correspondence: &mut [BoundaryCorrespondence],
    base: usize,
    index: usize,
    run: Run,
    maximum: usize,
) -> Result<(), ManufacturingError> {
    if let WidthProfile::Constant { width_mm } = path.width {
        if width_mm != run.mean {
            let mut widths_mm = Vec::new();
            reserve_widths(&mut widths_mm, index + 1, maximum)?;
            widths_mm.resize(index, width_mm);
            widths_mm.push(run.mean);
            path.width = WidthProfile::PerEdge { widths_mm };
        }
    } else if let WidthProfile::PerEdge { widths_mm } = &mut path.width {
        reserve_widths(widths_mm, index + 1, maximum)?;
        if index == widths_mm.len() {
            widths_mm.push(run.mean);
        } else {
            widths_mm[index] = run.mean;
        }
    }
    path.points_mm[index] = run.a;
    path.points_mm[index + 1] = run.b;
    correspondence[base + index] = BoundaryCorrespondence {
        edge_index: index,
        ..run.owner
    };
    Ok(())
}

pub(super) fn coalesce(
    path: &mut DepositionPath,
    correspondence: &mut Vec<BoundaryCorrespondence>,
    base: usize,
    permissions: &[Junction],
    original: OriginalPath<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let count = path.checked_edges()?.len();
    let original_constant = match path.width {
        WidthProfile::Constant { width_mm } => Some(width_mm),
        _ => None,
    };
    let permission_work = permissions
        .iter()
        .map(|permission| {
            permission
                .transition
                .as_ref()
                .map_or(1, |corridor| corridor.points.len())
        })
        .sum::<usize>();
    boolean::charge(work, permission_work)?;
    let mut legal: Vec<Run> = Vec::new();
    let mut read = 0;
    let mut written = 0;
    let mut previous: Option<Run> = None;
    let mut cursor = Some(original.first);
    while read < count {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let a = path.points_mm[read];
        let b = path.points_mm[read + 1];
        let w = width(path, read, original_constant);
        let first =
            cursor.ok_or_else(|| unsupported("wall fitting source traversal is incomplete"))?;
        let mut run = Run {
            a,
            b,
            length: norm(sub(xy(b), xy(a))),
            mean: w,
            source_mean: w,
            minimum: w,
            maximum: w,
            geometric_error: 0.0,
            owner: correspondence[base + read],
            first,
            first_fraction: 0.0,
            last: first,
            last_fraction: 1.0,
            source_count: 1,
        };
        cursor = original.next(first)?;
        read += 1;
        boolean::charge(work, permission_work)?;
        let mut checkpoint = match previous {
            Some(prior) => merge(prior, run, permissions, &original, work, cancel)?
                .map(|merged| (read, cursor, merged)),
            None => None,
        };
        while norm(sub(xy(run.b), xy(run.a))) < TARGET_LENGTH_MM && read < count {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let a = path.points_mm[read];
            let b = path.points_mm[read + 1];
            let w = width(path, read, original_constant);
            let first =
                cursor.ok_or_else(|| unsupported("wall fitting source traversal is incomplete"))?;
            let next = Run {
                a,
                b,
                length: norm(sub(xy(b), xy(a))),
                mean: w,
                source_mean: w,
                minimum: w,
                maximum: w,
                geometric_error: 0.0,
                owner: correspondence[base + read],
                first,
                first_fraction: 0.0,
                last: first,
                last_fraction: 1.0,
                source_count: 1,
            };
            boolean::charge(work, permission_work)?;
            let Some(merged) = merge(run, next, permissions, &original, work, cancel)? else {
                break;
            };
            run = merged;
            read += 1;
            cursor = original.next(first)?;
            boolean::charge(work, permission_work)?;
            if let Some(prior) = previous {
                if let Some(merged) = merge(prior, run, permissions, &original, work, cancel)? {
                    checkpoint = Some((read, cursor, merged));
                }
            }
        }
        if norm(sub(xy(run.b), xy(run.a))) < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
            let Some(prior) = previous else {
                return Err(unsupported(&format!(
                    "first wall command cannot fit minimum length and boundary enclosure: {:?}->{:?}, width={}..{}, error={}, owner={:?}, source_edges={}, consumed={}, next={:?}",
                    run.a,
                    run.b,
                    run.minimum,
                    run.maximum,
                    run.geometric_error,
                    run.owner,
                    count,
                    read,
                    (read < count).then(|| (
                        path.points_mm[read],
                        path.points_mm[read + 1],
                        width(path, read, original_constant),
                        correspondence[base + read]
                    )),
                )));
            };
            let Some((resume, resume_cursor, merged)) = checkpoint else {
                if let Some((mut replacement, resume)) = fit_corner(
                    prior,
                    run,
                    read,
                    count,
                    permissions,
                    &original,
                    permission_work,
                    work,
                    cancel,
                )? {
                    match contained_gradient(
                        &mut replacement,
                        false,
                        original.minimum_width_mm,
                        work,
                        cancel,
                    ) {
                        Ok(())
                            if written < 2
                                || gradient_admitted(legal[written - 2], replacement[0]) =>
                        {
                            for (offset, fitted) in replacement.into_iter().enumerate() {
                                remember(
                                    &mut legal,
                                    written - 1 + offset,
                                    fitted,
                                    original.owned_bytes,
                                    work,
                                )?;
                                write_run(
                                    path,
                                    correspondence,
                                    base,
                                    written - 1 + offset,
                                    fitted,
                                    count,
                                )?;
                            }
                            written += 2;
                            previous = legal.last().copied();
                            read = resume;
                            cursor = original.next(legal.last().unwrap().last)?;
                            continue;
                        }
                        Ok(()) => {}
                        Err(error) if error.code == ManufacturingErrorCode::UnsupportedGeometry => {
                        }
                        Err(error) => return Err(error),
                    }
                }
                if let Some(start) = repartition_suffix(
                    &mut legal,
                    run,
                    read,
                    permissions,
                    &original,
                    permission_work,
                    work,
                    cancel,
                )? {
                    for index in start..legal.len() {
                        write_run(path, correspondence, base, index, legal[index], count)?;
                    }
                    written = legal.len();
                    previous = legal.last().copied();
                    continue;
                }
                // A closed playback seam is not an open source port. If the
                // SAME consumed source walk closes, its final short tail may
                // fit the FIRST legal run even when it cannot fit the last.
                // Keep every range once and move the seam only to the tail's
                // existing original XYZ6 vertex; no point or width is snapped.
                let wrapped = if read == count
                    && cursor == Some(original.first)
                    && legal.first().is_some_and(|first| first.a == run.b)
                    && legal.last().is_some_and(|last| last.b == run.a)
                    && original.cyclic_partition(&legal, run, count, work, cancel)?
                {
                    boolean::charge(work, permission_work)?;
                    merge(run, legal[0], permissions, &original, work, cancel)?
                } else {
                    None
                };
                if let Some(merged) = wrapped.filter(|merged| {
                    chord_length(*merged) >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
                        && gradient_admitted(prior, *merged)
                        && legal
                            .get(1)
                            .is_some_and(|second| gradient_admitted(*merged, *second))
                }) {
                    remember(&mut legal, 0, merged, original.owned_bytes, work)?;
                    write_run(path, correspondence, base, 0, merged, count)?;
                    break;
                }
                return Err(unsupported(&format!(
                    "wall command cannot fit minimum length without losing its boundary/width enclosure: prior {:?}->{:?} w={}..{} error={}, next {:?}->{:?} w={}..{} error={}, support {:?}/{:?}, facing {:?}/{:?}, ordinal {}/{}",
                    prior.a,
                    prior.b,
                    prior.minimum,
                    prior.maximum,
                    prior.geometric_error,
                    run.a,
                    run.b,
                    run.minimum,
                    run.maximum,
                    run.geometric_error,
                    prior.owner.supports,
                    run.owner.supports,
                    prior.owner.facing_boundary,
                    run.owner.facing_boundary,
                    prior.owner.ordinal,
                    run.owner.ordinal,
                )));
            };
            // A failed lookahead has not mutated any source edge. Resume at
            // the last certified shared endpoint rather than consuming a tiny
            // piece beyond a sharp turn and then waiving the command policy.
            read = resume;
            cursor = resume_cursor;
            if norm(sub(xy(merged.b), xy(merged.a)))
                < program_coordinates::MIN_DEPOSITION_COMMAND_MM
            {
                return Err(unsupported(
                    "wall stroke is shorter than the declared minimum deposition command",
                ));
            }
            remember(&mut legal, written - 1, merged, original.owned_bytes, work)?;
            write_run(path, correspondence, base, written - 1, merged, count)?;
            previous = Some(merged);
        } else {
            remember(&mut legal, written, run, original.owned_bytes, work)?;
            write_run(path, correspondence, base, written, run, count)?;
            previous = Some(run);
            written += 1;
        }
    }
    path.points_mm.truncate(written + 1);
    if let WidthProfile::PerEdge { widths_mm } = &mut path.width {
        widths_mm.truncate(written);
        if let Some(&first) = widths_mm.first() {
            if widths_mm.iter().all(|&value| value == first) {
                path.width = WidthProfile::Constant { width_mm: first };
            }
        }
    }
    correspondence.truncate(base + written);
    contained_gradient(
        &mut legal,
        path.points_mm.first() == path.points_mm.last(),
        original.minimum_width_mm,
        work,
        cancel,
    )?;
    maximize(
        path,
        correspondence,
        base,
        &legal,
        permissions,
        &original,
        count,
        work,
        cancel,
    )?;
    let mut first = None;
    let mut prior = None;
    let check_gradient = |a: (f64, f64), b: (f64, f64)| -> Result<(), ManufacturingError> {
        // Preserve the same producer headroom as source-field construction.
        // Actual E8/XYZ6 command intervals remain an independent verifier gate.
        if (a.0 - b.0).abs()
            > program_coordinates::MAX_WALL_WIDTH_GRADIENT * 0.9 * (a.1 + b.1) * 0.5
        {
            return Err(unsupported(&format!(
                "contained wall fit exceeds continuing width-gradient proposal: {a:?}, {b:?}"
            )));
        }
        Ok(())
    };
    for edge in path.checked_edges()? {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        if edge.width_mm < original.minimum_width_mm {
            return Err(unsupported(
                "contained wall fit falls below the admitted process width floor",
            ));
        }
        let current = (
            edge.width_mm,
            norm(sub(xy(edge.points_mm[1]), xy(edge.points_mm[0]))),
        );
        if let Some(previous) = prior {
            check_gradient(previous, current)?;
        }
        if first.is_none() {
            first = Some(current);
        }
        prior = Some(current);
    }
    if path.points_mm.first() == path.points_mm.last() {
        if let (Some(a), Some(b)) = (prior, first) {
            check_gradient(a, b)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contained_gradient_directs_equal_rounded_bounds_inside_the_closed_predicate() {
        let points = [
            [100.675, 106.274126, 0.2],
            [100.675, 106.305467, 0.2],
            [100.675, 106.336808, 0.2],
        ];
        let fields = [
            (0.4260406793175658, 0.42886136931756624, 0.4274510693175659),
            (0.43168205931756665, 0.4345027493175671, 0.43309235931756634),
        ];
        for reverse in [false, true] {
            let mut runs = std::array::from_fn::<_, 2, _>(|index| {
                let (minimum, maximum, source_mean) =
                    fields[if reverse { 1 - index } else { index }];
                Run {
                    a: points[index],
                    b: points[index + 1],
                    length: norm(sub(xy(points[index + 1]), xy(points[index]))),
                    mean: minimum,
                    source_mean,
                    minimum,
                    maximum,
                    geometric_error: 0.0,
                    owner: BoundaryCorrespondence {
                        path_index: 0,
                        edge_index: index,
                        supports: [0, 0],
                        facing_boundary: Some(0),
                        ordinal: 1,
                    },
                    first: (index, 0),
                    first_fraction: 0.0,
                    last: (index, 0),
                    last_fraction: 1.0,
                    source_count: 1,
                }
            });
            // The sum rounds onto the proposed command, although the equivalent
            // subtraction lies strictly outside the same unchanged allowance.
            assert!(!gradient_admitted(runs[0], runs[1]));
            let original = runs;
            contained_gradient(&mut runs, false, 0.302, &mut 0, &AtomicBool::new(false)).unwrap();
            assert!(gradient_admitted(runs[0], runs[1]));
            for (run, prior) in runs.into_iter().zip(original) {
                assert_eq!(run.a, prior.a);
                assert_eq!(run.b, prior.b);
                assert!(run.mean <= prior.mean);
                assert!(prior.mean - run.mean <= f64::EPSILON);
                assert!(
                    run.geometric_error
                        + (run.mean - run.minimum)
                            .abs()
                            .max((run.maximum - run.mean).abs())
                            * 0.5
                        <= MERGE_ENCLOSURE_MM
                );
            }
        }
    }

    #[test]
    fn pocket_ring_handoff_retains_source_union_and_contained_short_tail() {
        let points = [
            [-2.572680, 4.319726],
            [-2.525740, 4.347341],
            [-2.508494, 4.357318],
        ];
        for supports in [[1, 0], [0, 1]] {
            let edges = std::array::from_fn::<_, 2, _>(|index| BoundaryEdge {
                points: [points[index], points[index + 1]],
                width_mm: 0.6,
                source_widths: [0.6; 2],
                supports: if index == 0 { supports } else { [0, 0] },
                facing_boundary: Some(0),
                ordinal: 0,
            });
            let mates = [[None, Some((1, 0))], [Some((0, 1)), None]];
            let original = OriginalPath {
                edges: &edges,
                mates: &mates,
                first: (0, 0),
                minimum_width_mm: 0.302,
                owned_bytes: 0,
            };
            let mut path = DepositionPath {
                kind: DepositionKind::Perimeter,
                owner: Some(DepositionOwner::Wall),
                width: WidthProfile::Constant { width_mm: 0.6 },
                physical_z_mm: [0.0, 0.2],
                points_mm: points.iter().map(|p| [p[0], p[1], 0.2]).collect(),
            };
            let mut correspondence = edges
                .iter()
                .enumerate()
                .map(|(edge_index, edge)| BoundaryCorrespondence {
                    path_index: 0,
                    edge_index,
                    supports: edge.supports,
                    facing_boundary: edge.facing_boundary,
                    ordinal: edge.ordinal,
                })
                .collect::<Vec<_>>();
            let owner = correspondence[0];
            let cancel = AtomicBool::new(false);
            let mut work = 0;
            // The original mate walk and every interval survive in the summary.
            let span = fractional_run(
                &original,
                owner,
                (0, 0),
                0.0,
                (1, 0),
                1.0,
                0.2,
                &mut work,
                &cancel,
            )
            .unwrap()
            .unwrap();
            assert_eq!(span.owner.supports, [0, 1]);
            assert_eq!(span.source_count, 2);
            assert_eq!((span.first, span.last), ((0, 0), (1, 0)));
            // A split must not borrow a support from an unconsumed interval.
            let tail = fractional_run(
                &original,
                span.owner,
                (1, 0),
                0.0,
                (1, 0),
                1.0,
                0.2,
                &mut work,
                &cancel,
            )
            .unwrap()
            .unwrap();
            assert_eq!(tail.owner.supports, [0, 0]);
            for incompatible in [
                BoundaryCorrespondence {
                    supports: [0, 2],
                    ..owner
                },
                BoundaryCorrespondence {
                    facing_boundary: Some(1),
                    ..owner
                },
                BoundaryCorrespondence {
                    ordinal: 1,
                    ..owner
                },
            ] {
                assert!(merge_owner(span.owner, incompatible).is_none());
            }
            assert!(
                merge_owner(
                    BoundaryCorrespondence {
                        facing_boundary: None,
                        ..owner
                    },
                    BoundaryCorrespondence {
                        supports: [0, 0],
                        facing_boundary: None,
                        ..owner
                    },
                )
                .is_none()
            );
            coalesce(
                &mut path,
                &mut correspondence,
                0,
                &[],
                original,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(xy(path.points_mm[0]), points[0]);
            assert_eq!(xy(*path.points_mm.last().unwrap()), points[2]);
            assert_eq!(correspondence.len(), 1);
            assert_eq!(correspondence[0].supports, [0, 1]);
            assert_eq!(correspondence[0].facing_boundary, Some(0));
            assert_eq!(correspondence[0].ordinal, 0);
            let edge = path.checked_edges().unwrap().next().unwrap();
            assert!(edge.width_mm >= 0.302 && edge.width_mm < 0.6);
            let a = xy(edge.points_mm[0]);
            let b = xy(edge.points_mm[1]);
            let chord = sub(b, a);
            let error = points
                .into_iter()
                .map(|p| chord_distance_bound(p, a, b, chord, dot(chord, chord)))
                .fold(0.0_f64, f64::max);
            assert!(error + (0.6 - edge.width_mm) * 0.5 <= MERGE_ENCLOSURE_MM);
            assert!(error + edge.width_mm * 0.5 <= 0.3);
            assert!(norm(chord) >= program_coordinates::MIN_DEPOSITION_COMMAND_MM);
        }
    }

    #[test]
    fn ordinary_inward_fit_preserves_unread_constant_source_widths() {
        let points = [
            [0.0, 0.0],
            [0.01, 0.0003],
            [0.02, 0.0003],
            [0.03, 0.0],
            [0.04, 0.0],
            [0.05, 0.0],
            [0.06, 0.0],
        ];
        let count = points.len() - 1;
        let edges = points
            .windows(2)
            .enumerate()
            .map(|(index, pair)| BoundaryEdge {
                points: [pair[0], pair[1]],
                width_mm: 0.45,
                supports: [0, 0],
                source_widths: [0.45; 2],
                facing_boundary: Some(0),
                ordinal: usize::from(index >= 3),
            })
            .collect::<Vec<_>>();
        let mates = (0..count)
            .map(|index| {
                [
                    (index > 0).then(|| (index - 1, 1)),
                    (index + 1 < count).then(|| (index + 1, 0)),
                ]
            })
            .collect::<Vec<_>>();
        let mut path = DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.0, 0.2],
            points_mm: points.map(|p| [p[0], p[1], 0.2]).to_vec(),
        };
        let mut correspondence = (0..count)
            .map(|edge_index| BoundaryCorrespondence {
                path_index: 0,
                edge_index,
                supports: [0, 0],
                facing_boundary: Some(0),
                ordinal: usize::from(edge_index >= 3),
            })
            .collect();
        coalesce(
            &mut path,
            &mut correspondence,
            0,
            &[],
            OriginalPath {
                edges: &edges,
                mates: &mates,
                first: (0, 0),
                minimum_width_mm: 0.302,
                owned_bytes: 0,
            },
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut output = path.checked_edges().unwrap();
        assert!(
            output.next().unwrap().width_mm < 0.45,
            "ordinary curved fitting must spend radius, not just report a centerline error tube"
        );
        assert_eq!(
            path.checked_edges().unwrap().last().unwrap().width_mm,
            0.45,
            "materializing a variable output prefix must not replace unread source widths"
        );
        for edge in path.checked_edges().unwrap() {
            assert!(edge.width_mm >= 0.302 && edge.width_mm <= 0.45);
            assert!(
                norm(sub(xy(edge.points_mm[1]), xy(edge.points_mm[0])))
                    >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
            );
        }
    }

    #[test]
    fn finite_curved_stroke_keeps_long_commands_and_a_legal_short_tail() {
        let count = 2053;
        let points = (0..=count)
            .map(|i| {
                let angle = i as f64 / count as f64;
                point(key([20.0 * angle.cos(), 20.0 * angle.sin()]).unwrap())
            })
            .collect::<Vec<_>>();
        let edges = points
            .windows(2)
            .enumerate()
            .map(|(index, pair)| {
                let widths = [
                    0.45 + 0.01 * index as f64 / count as f64,
                    0.45 + 0.01 * (index + 1) as f64 / count as f64,
                ];
                BoundaryEdge {
                    points: [pair[0], pair[1]],
                    width_mm: (widths[0] + widths[1]) * 0.5,
                    source_widths: widths,
                    supports: [0, 0],
                    facing_boundary: Some(0),
                    ordinal: 0,
                }
            })
            .collect::<Vec<_>>();
        let mates = (0..count)
            .map(|index| {
                [
                    (index > 0).then(|| (index - 1, 1)),
                    (index + 1 < count).then(|| (index + 1, 0)),
                ]
            })
            .collect::<Vec<_>>();
        let mut path = DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::PerEdge {
                widths_mm: edges.iter().map(|edge| edge.width_mm).collect(),
            },
            physical_z_mm: [0.0, 0.2],
            points_mm: points.iter().map(|p| [p[0], p[1], 0.2]).collect(),
        };
        let mut correspondence = (0..count)
            .map(|edge_index| BoundaryCorrespondence {
                path_index: 0,
                edge_index,
                supports: [0, 0],
                facing_boundary: Some(0),
                ordinal: 0,
            })
            .collect();
        coalesce(
            &mut path,
            &mut correspondence,
            0,
            &[],
            OriginalPath {
                edges: &edges,
                mates: &mates,
                first: (0, 0),
                minimum_width_mm: 0.302,
                owned_bytes: 0,
            },
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(xy(path.points_mm[0]), points[0]);
        assert_eq!(xy(*path.points_mm.last().unwrap()), points[count]);
        let mut longest = 0.0_f64;
        let mut minimum = f64::INFINITY;
        let mut maximum = 0.0_f64;
        let mut previous: Option<(f64, f64)> = None;
        for edge in path.checked_edges().unwrap() {
            let length = norm(sub(xy(edge.points_mm[1]), xy(edge.points_mm[0])));
            assert!(length >= program_coordinates::MIN_DEPOSITION_COMMAND_MM);
            assert!(edge.width_mm >= 0.302 && edge.width_mm <= 0.46);
            if let Some((width, prior_length)) = previous {
                assert!(
                    (edge.width_mm - width).abs()
                        <= program_coordinates::MAX_WALL_WIDTH_GRADIENT
                            * (length + prior_length)
                            * 0.5
                );
            }
            previous = Some((edge.width_mm, length));
            longest = longest.max(length);
            minimum = minimum.min(edge.width_mm);
            maximum = maximum.max(edge.width_mm);
        }
        assert!(
            longest >= 0.1,
            "min-G1 is a floor, not a preferred command length"
        );
        assert!(
            maximum - minimum > 0.005,
            "batch fitting must retain the source width field"
        );
        for p in points {
            let distance = path
                .points_mm
                .windows(2)
                .map(|pair| {
                    let a = xy(pair[0]);
                    let chord = sub(xy(pair[1]), a);
                    let t = (dot(sub(p, a), chord) / dot(chord, chord)).clamp(0.0, 1.0);
                    norm(sub(p, add(a, mul(chord, t))))
                })
                .fold(f64::INFINITY, f64::min);
            assert!(distance <= MERGE_ENCLOSURE_MM);
        }
    }

    #[test]
    fn closed_source_cycle_keeps_boundary_enclosure_and_width_gradient() {
        let radius = 0.05;
        let mut points = vec![
            [0.0, radius],
            [0.0, 4.0],
            [4.0, 4.0],
            [4.0, 0.0],
            [radius, 0.0],
        ];
        for i in 1..=80 {
            let angle = -std::f64::consts::FRAC_PI_2 * (1.0 + i as f64 / 80.0);
            points.push(point(
                key([radius + radius * angle.cos(), radius + radius * angle.sin()]).unwrap(),
            ));
        }
        let count = points.len() - 1;
        let edges = points
            .windows(2)
            .map(|pair| BoundaryEdge {
                points: [pair[0], pair[1]],
                width_mm: 0.45,
                supports: [0, 0],
                source_widths: [0.45; 2],
                facing_boundary: Some(0),
                ordinal: 0,
            })
            .collect::<Vec<_>>();
        let mates = (0..count)
            .map(|index| {
                [
                    Some(((index + count - 1) % count, 1)),
                    Some(((index + 1) % count, 0)),
                ]
            })
            .collect::<Vec<_>>();
        let mut path = DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.0, 0.2],
            points_mm: points.iter().map(|p| [p[0], p[1], 0.2]).collect(),
        };
        let mut correspondence: Vec<_> = (0..count)
            .map(|edge_index| BoundaryCorrespondence {
                path_index: 0,
                edge_index,
                supports: [0, 0],
                facing_boundary: Some(0),
                ordinal: 0,
            })
            .collect();
        let permissions = [Junction {
            center: [radius, radius],
            radius_mm: 0.4,
            valence: 3,
            ports: Vec::new(),
            transition: None,
        }];
        coalesce(
            &mut path,
            &mut correspondence,
            0,
            &permissions,
            OriginalPath {
                edges: &edges,
                mates: &mates,
                first: (0, 0),
                minimum_width_mm: 0.302,
                owned_bytes: 0,
            },
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(path.points_mm.first(), path.points_mm.last());
        let mut first: Option<(f64, f64)> = None;
        let mut previous: Option<(f64, f64)> = None;
        for edge in path.checked_edges().unwrap() {
            let length = norm(sub(xy(edge.points_mm[1]), xy(edge.points_mm[0])));
            assert!(length >= program_coordinates::MIN_DEPOSITION_COMMAND_MM);
            let current = (edge.width_mm, length);
            if let Some((width, prior_length)) = previous {
                assert!(
                    (current.0 - width).abs()
                        <= program_coordinates::MAX_WALL_WIDTH_GRADIENT
                            * (length + prior_length)
                            * 0.5
                );
            }
            if first.is_none() {
                first = Some(current);
            }
            previous = Some(current);
        }
        let (first, last) = (first.unwrap(), previous.unwrap());
        assert!(
            (first.0 - last.0).abs()
                <= program_coordinates::MAX_WALL_WIDTH_GRADIENT * (first.1 + last.1) * 0.5
        );
        for p in points {
            let distance = path
                .points_mm
                .windows(2)
                .map(|pair| {
                    let a = xy(pair[0]);
                    let chord = sub(xy(pair[1]), a);
                    let t = (dot(sub(p, a), chord) / dot(chord, chord)).clamp(0.0, 1.0);
                    norm(sub(p, add(a, mul(chord, t))))
                })
                .fold(f64::INFINITY, f64::min);
            assert!(distance <= MERGE_ENCLOSURE_MM);
        }
    }

    #[test]
    fn dense_curved_proposals_fit_legal_commands_without_spending_more_enclosure() {
        let count = 180;
        let points = (0..=count)
            .map(|i| {
                let angle = 0.4 * i as f64 / count as f64;
                point(key([1.0 + 0.09 * angle.cos(), 1.0 + 0.09 * angle.sin()]).unwrap())
            })
            .collect::<Vec<_>>();
        let edges = points
            .windows(2)
            .map(|pair| BoundaryEdge {
                points: [pair[0], pair[1]],
                width_mm: 0.45,
                supports: [0, 0],
                source_widths: [0.45; 2],
                facing_boundary: Some(0),
                ordinal: 0,
            })
            .collect::<Vec<_>>();
        let mates = (0..count)
            .map(|index| {
                [
                    (index > 0).then(|| (index - 1, 1)),
                    (index + 1 < count).then(|| (index + 1, 0)),
                ]
            })
            .collect::<Vec<_>>();
        let mut path = DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.0, 0.2],
            points_mm: points.iter().map(|p| [p[0], p[1], 0.2]).collect(),
        };
        let mut correspondence: Vec<BoundaryCorrespondence> = (0..count)
            .map(|edge_index| BoundaryCorrespondence {
                path_index: 0,
                edge_index,
                supports: [0, 0],
                facing_boundary: Some(0),
                ordinal: 0,
            })
            .collect();
        let mut work = 0;
        let mut denied = path.clone();
        let mut denied_correspondence = correspondence.clone();
        assert!(
            coalesce(
                &mut denied,
                &mut denied_correspondence,
                0,
                &[],
                OriginalPath {
                    edges: &edges,
                    mates: &mates,
                    first: (0, 0),
                    minimum_width_mm: 0.302,
                    owned_bytes: 0
                },
                &mut work,
                &AtomicBool::new(false)
            )
            .is_err(),
            "this short complete arc cannot fit min-G1 and ordinary inward containment together"
        );
        // A source-declared junction permits the centered constant-width fit;
        // merely retaining the old minimum width without that authority does not.
        let permissions = [Junction {
            center: [1.0, 1.0],
            radius_mm: 0.4,
            valence: 3,
            ports: Vec::new(),
            transition: None,
        }];
        coalesce(
            &mut path,
            &mut correspondence,
            0,
            &permissions,
            OriginalPath {
                edges: &edges,
                mates: &mates,
                first: (0, 0),
                minimum_width_mm: 0.302,
                owned_bytes: 0,
            },
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(xy(path.points_mm[0]), points[0]);
        assert_eq!(xy(*path.points_mm.last().unwrap()), points[count]);
        assert_eq!(correspondence.len() + 1, path.points_mm.len());
        for edge in path.checked_edges().unwrap() {
            assert_eq!(edge.width_mm, 0.45);
            assert!(
                norm(sub(xy(edge.points_mm[1]), xy(edge.points_mm[0])))
                    >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
            );
        }
        for p in points {
            let nearest = path
                .points_mm
                .windows(2)
                .map(|pair| {
                    let a = xy(pair[0]);
                    let chord = sub(xy(pair[1]), a);
                    let t = (dot(sub(p, a), chord) / dot(chord, chord)).clamp(0.0, 1.0);
                    norm(sub(p, add(a, mul(chord, t))))
                })
                .fold(f64::INFINITY, f64::min);
            assert!(
                nearest <= MERGE_ENCLOSURE_MM,
                "the complete original proposal must remain enclosed: {nearest}"
            );
        }
    }

    #[test]
    fn minimum_commands_follow_small_curved_corners_within_the_declared_corridor() {
        let mut points = (0..=20)
            .map(|i| [-0.02, -0.04 + 0.002 * i as f64])
            .collect::<Vec<_>>();
        points.extend((1..=90).map(|i| {
            let angle = std::f64::consts::PI - (i as f64).to_radians();
            point(key([0.02 * angle.cos(), 0.02 * angle.sin()]).unwrap())
        }));
        points.extend((1..=20).map(|i| [0.002 * i as f64, 0.02]));
        let edges = points
            .windows(2)
            .map(|p| BoundaryEdge {
                points: [p[0], p[1]],
                width_mm: 0.45,
                source_widths: [0.45; 2],
                supports: [0, 0],
                facing_boundary: Some(0),
                ordinal: 0,
            })
            .collect::<Vec<_>>();
        let count = edges.len();
        let mates = (0..count)
            .map(|i| {
                [
                    (i > 0).then(|| (i - 1, 1)),
                    (i + 1 < count).then(|| (i + 1, 0)),
                ]
            })
            .collect::<Vec<_>>();
        let original = || OriginalPath {
            edges: &edges,
            mates: &mates,
            first: (0, 0),
            minimum_width_mm: 0.302,
            owned_bytes: 0,
        };
        let mut path = DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.0, 0.2],
            points_mm: points.iter().map(|p| [p[0], p[1], 0.2]).collect(),
        };
        let mut correspondence = (0..count)
            .map(|edge_index| BoundaryCorrespondence {
                path_index: 0,
                edge_index,
                supports: [0, 0],
                facing_boundary: Some(0),
                ordinal: 0,
            })
            .collect::<Vec<_>>();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            coalesce(
                &mut path.clone(),
                &mut correspondence.clone(),
                0,
                &[],
                original(),
                &mut 0,
                &cancel
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::UnsupportedGeometry
        );
        let permissions = [Junction {
            center: [0.0, 0.0],
            radius_mm: 0.4,
            valence: 3,
            ports: Vec::new(),
            transition: None,
        }];
        coalesce(
            &mut path,
            &mut correspondence,
            0,
            &permissions,
            original(),
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert_eq!(xy(path.points_mm[0]), points[0]);
        assert_eq!(xy(*path.points_mm.last().unwrap()), *points.last().unwrap());
        for edge in path.checked_edges().unwrap() {
            assert!(
                norm(sub(xy(edge.points_mm[1]), xy(edge.points_mm[0])))
                    >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
            );
            assert!(edge.width_mm >= 0.302);
        }
        let distance = |p: [f64; 2], segment: [[f64; 2]; 2]| {
            let chord = sub(segment[1], segment[0]);
            chord_distance_bound(p, segment[0], segment[1], chord, dot(chord, chord))
        };
        for p in &points {
            let error = path
                .points_mm
                .windows(2)
                .enumerate()
                .map(|(index, s)| {
                    distance(*p, [xy(s[0]), xy(s[1])])
                        + (width(&path, index, None) - 0.45).abs() * 0.5
                })
                .fold(f64::INFINITY, f64::min);
            assert!(error <= MERGE_ENCLOSURE_MM);
        }
        for (index, segment) in path.points_mm.windows(2).enumerate() {
            for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let p = add(
                    xy(segment[0]),
                    mul(sub(xy(segment[1]), xy(segment[0])), fraction),
                );
                let error = points
                    .windows(2)
                    .map(|s| distance(p, [s[0], s[1]]))
                    .fold(f64::INFINITY, f64::min)
                    + (width(&path, index, None) - 0.45).abs() * 0.5;
                assert!(error <= MERGE_ENCLOSURE_MM);
            }
        }
    }
}
