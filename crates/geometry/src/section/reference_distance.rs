// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Geometric correspondence only: this does not establish material or Jordan topology.

use super::reference::{CycleCut, candidate_segments};
use crate::parametric::Interval;
use crate::{GeometryError, GeometryWorkBudget};
use std::sync::atomic::AtomicBool;

type Point = [f64; 3];
type Box3 = [Interval; 3];
type Segment = [Point; 2];
const MAX_DEPTH: u8 = 52;
type Cell = ([f64; 2], u8);

// The owner must admit this concurrently with its own retained data and proposal
// scratch. Pending cells are the only depth-dependent storage. The fixed part
// conservatively includes the incident segments, endpoint boxes, distance pairs,
// and the live interval interpolation/projection/norm temporaries below.
pub(super) const CYCLE_DISTANCE_STACK_BYTES: usize = std::mem::size_of::<
    [Cell; MAX_DEPTH as usize + 1],
>() + std::mem::size_of::<[Segment; 4]>()
    + std::mem::size_of::<[Box3; 3]>()
    + std::mem::size_of::<[[Distance; 2]; 3]>()
    + std::mem::size_of::<[Interval; 48]>();

#[derive(Clone, Copy)]
struct Distance {
    lower: f64,
    upper: f64,
}

fn point_box(point: Point) -> Box3 {
    point.map(Interval::point)
}

/// Enclose the exact affine image at the binary64 parameter, not a repeatedly
/// rounded subdivision vertex. Both children retain the identical split value,
/// so the complete original segment remains covered at every refinement.
fn interpolate(segment: Segment, parameter: Interval) -> Box3 {
    if parameter.lo == 0.0 && parameter.hi == 0.0 {
        return point_box(segment[0]);
    }
    if parameter.lo == 1.0 && parameter.hi == 1.0 {
        return point_box(segment[1]);
    }
    std::array::from_fn(|axis| {
        let a = Interval::point(segment[0][axis]);
        a.add(Interval::point(segment[1][axis]).sub(a).mul(parameter))
    })
}

fn squared_norm(vector: Box3) -> Interval {
    let squared = vector
        .into_iter()
        .fold(Interval::point(0.0), |sum, x| sum.add(x.square()));
    // Outward addition can put down(0) below zero. The exact sum of squares
    // is nonnegative, including points lying exactly on a retained segment.
    Interval {
        lo: squared.lo.max(0.0),
        ..squared
    }
}

fn dot(a: Box3, b: Box3) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(a[axis].mul(b[axis]))
    })
}

fn norm(vector: Box3) -> Distance {
    match squared_norm(vector).sqrt() {
        Ok(length) => Distance {
            lower: length.lo,
            upper: length.hi,
        },
        // Overflow/conditioning cannot produce acceptance. A zero lower bound
        // is still sound, and the infinite upper forces refinement or refusal.
        Err(_) => Distance {
            lower: 0.0,
            upper: f64::INFINITY,
        },
    }
}

/// The numerical projection chooses just one fixed parameter q in [0,1]. Its
/// accuracy is not a premise: interval arithmetic encloses the distance to the
/// exact affine point at q on the actual 3D segment. In particular, no planar
/// projection or assumption that the proposed crossing is exact is involved.
fn point_segment_distance(point: Box3, segment: Segment) -> Distance {
    if segment.iter().any(|endpoint| {
        (0..3).all(|axis| point[axis].lo == endpoint[axis] && point[axis].hi == endpoint[axis])
    }) {
        return Distance {
            lower: 0.0,
            upper: 0.0,
        };
    }
    let direction: Point = std::array::from_fn(|axis| segment[1][axis] - segment[0][axis]);
    let relative: Point = std::array::from_fn(|axis| point[axis].mid() - segment[0][axis]);
    let denominator: f64 = direction.into_iter().map(|x| x * x).sum();
    let numerator: f64 = (0..3).map(|axis| relative[axis] * direction[axis]).sum();
    let proposal = numerator / denominator;
    let parameter = if proposal.is_finite() {
        proposal.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = interpolate(segment, Interval::point(parameter));
    let upper = norm(std::array::from_fn(|axis| point[axis].sub(q[axis]))).upper;

    // Independently enclose the true closest-point parameter for every point in
    // the input box. Its clamped projection contains the actual minimizer; the
    // norm's lower endpoint therefore bounds distance to the WHOLE segment.
    // This witness prevents a clearly unviable erased cycle from expanding an
    // entire depth-52 tree. Unresolved/degenerate arithmetic gives lower zero.
    let direction: Box3 = std::array::from_fn(|axis| {
        Interval::point(segment[1][axis]).sub(Interval::point(segment[0][axis]))
    });
    let relative: Box3 =
        std::array::from_fn(|axis| point[axis].sub(Interval::point(segment[0][axis])));
    let denominator = squared_norm(direction);
    let lower = if segment[0] == segment[1] {
        norm(relative).lower
    } else if denominator.lo > 0.0 && denominator.hi.is_finite() {
        match dot(relative, direction).div(denominator) {
            Ok(parameter) if parameter.lo.is_finite() && parameter.hi.is_finite() => {
                let parameter = Interval {
                    lo: parameter.lo.clamp(0.0, 1.0),
                    hi: parameter.hi.clamp(0.0, 1.0),
                };
                let q = interpolate(segment, parameter);
                norm(std::array::from_fn(|axis| point[axis].sub(q[axis]))).lower
            }
            _ => 0.0,
        }
    } else {
        0.0
    };
    Distance { lower, upper }
}

fn cover_segment(
    source: Segment,
    polygon: &[Point],
    cut: &CycleCut,
    maximum_distance: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
    stack[0] = ([0.0, 1.0], 0);
    let mut pending = 1;
    let mut achieved: f64 = 0.0;
    // Do not spend the entire remaining section budget on an early loose
    // witness: later cycles share it. Refine the enclosure, not the geometry.
    let resolution = maximum_distance * (1.0 / 32.0);
    while pending != 0 {
        work.charge(1, cancel)?;
        pending -= 1;
        let (range, depth) = stack[pending];
        let midpoint = range[0] * 0.5 + range[1] * 0.5;
        let points = [range[0], range[1], midpoint]
            .map(|parameter| interpolate(source, Interval::point(parameter)));
        let mut lower = [f64::INFINITY; 3];
        let mut upper = f64::INFINITY;
        for target in candidate_segments(polygon, cut) {
            let mut target_upper = 0.0_f64;
            for (index, point) in points.into_iter().enumerate() {
                work.charge(1, cancel)?;
                let distance = point_segment_distance(point, target);
                lower[index] = lower[index].min(distance.lower);
                if index < 2 {
                    target_upper = target_upper.max(distance.upper);
                }
            }
            // The SAME target must cover both leaf endpoints. Distance to
            // that convex segment then bounds the complete source leaf.
            upper = upper.min(target_upper);
            if upper <= resolution {
                break;
            }
        }
        let lower = lower.into_iter().fold(0.0_f64, f64::max);
        if upper <= maximum_distance && (upper <= resolution || upper - lower <= resolution) {
            achieved = achieved.max(upper);
            continue;
        }
        if lower > maximum_distance {
            // Every retained candidate segment misses this source point.
            return Ok(None);
        }
        if depth >= MAX_DEPTH || midpoint <= range[0] || midpoint >= range[1] {
            return Ok(None);
        }
        stack[pending] = ([midpoint, range[1]], depth + 1);
        stack[pending + 1] = ([range[0], midpoint], depth + 1);
        pending += 2;
    }
    Ok(Some(achieved))
}

/// Bound the complete 3D bidirectional Hausdorff distance from the old closed
/// polygon to precisely the cycle candidate described by `cut`. Unchanged edges
/// reuse identity. The new edges share an old endpoint with their respective
/// original edge; convexity bounds candidate-to-old by the crossing's distance
/// to that same original segment. Every discarded original edge, including all
/// discarded portions of both crossing edges, is independently covered below.
/// `None` is a conservative proposal refusal, not a material/topology judgment.
pub(super) fn cycle_distance(
    points: &[[f64; 3]],
    cut: &CycleCut,
    maximum_distance: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    work.charge(1, cancel)?;
    let Some(n) = points.len().checked_sub(1) else {
        return Ok(None);
    };
    let i = cut.first_edge;
    let j = cut.second_edge;
    if n < 4
        || i >= n
        || j >= n
        || i + 1 >= j
        || (i == 0 && j == n - 1)
        || points[0] != points[n]
        || !maximum_distance.is_finite()
        || maximum_distance < 0.0
        || cut.point_mm.iter().any(|value| !value.is_finite())
    {
        return Ok(None);
    }
    for point in points {
        work.charge(1, cancel)?;
        if point.iter().any(|value| !value.is_finite()) {
            return Ok(None);
        }
    }
    let p = cut.point_mm;
    let mut achieved: f64 = 0.0;
    for edge in [i, j] {
        work.charge(1, cancel)?;
        let upper = point_segment_distance(point_box(p), [points[edge], points[edge + 1]]).upper;
        if !upper.is_finite() || upper > maximum_distance {
            return Ok(None);
        }
        achieved = achieved.max(upper);
    }
    // Outside keeps 0..i and j+1..n; middle keeps i+1..j. Both
    // crossing edges must be checked in full, not merely their inner endpoints.
    let ranges = if cut.keep_middle {
        [j..n, 0..i + 1]
    } else {
        [i..j + 1, 0..0]
    };
    for range in ranges {
        for edge in range {
            work.charge(1, cancel)?;
            let Some(bound) = cover_segment(
                [points[edge], points[edge + 1]],
                points,
                cut,
                maximum_distance,
                work,
                cancel,
            )?
            else {
                return Ok(None);
            };
            achieved = achieved.max(bound);
        }
    }
    Ok(Some(achieved))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::geometry::GeometryErrorCode;

    #[test]
    fn cycle_correspondence_uses_the_complete_retained_polygon() {
        let points = [
            [-1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0],
            [10.0, 10.0, 0.0],
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
        ];
        let cut = CycleCut {
            first_edge: 0,
            second_edge: 3,
            point_mm: [0.0; 3],
            keep_middle: false,
        };
        let bound = cycle_distance(
            &points,
            &cut,
            1.0e-10,
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap()
        .expect("retained reversed edges cover the whole erased cycle");
        assert!(bound <= 1.0e-10);
    }

    fn skinny_bowtie(height: f64) -> ([Point; 7], CycleCut) {
        (
            [
                [-10.0, 0.0, 0.0],
                [1.0, height, 0.0],
                [-1.0, height, 0.0],
                [10.0, 0.0, 0.0],
                [10.0, -10.0, 0.0],
                [-10.0, -10.0, 0.0],
                [-10.0, 0.0, 0.0],
            ],
            CycleCut {
                first_edge: 0,
                second_edge: 2,
                point_mm: [0.0, height * (10.0 / 11.0), 0.0],
                keep_middle: false,
            },
        )
    }

    #[test]
    fn skinny_bowtie_hausdorff_ignores_large_tangent_backtracking() {
        let height = 2.0_f64.powi(-20);
        let (points, cut) = skinny_bowtie(height);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let bound = cycle_distance(&points, &cut, 2.0 * height, &mut work, &cancel)
            .unwrap()
            .expect("complete skinny-cycle cover");
        assert!(bound > 0.0 && bound <= 2.0 * height);
        // The reversed inventory chooses the large middle cycle instead. The
        // discarded wrapped chain must receive the same complete geometric cover.
        let middle_points = [
            points[1], points[2], points[3], points[4], points[5], points[0], points[1],
        ];
        let middle = CycleCut {
            first_edge: 1,
            second_edge: 5,
            keep_middle: true,
            ..cut
        };
        assert!(
            cycle_distance(&middle_points, &middle, 2.0 * height, &mut work, &cancel)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn large_erased_cycle_cannot_acquire_small_hausdorff_allowance() {
        let (points, cut) = skinny_bowtie(1.0);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        assert!(
            cycle_distance(&points, &cut, 0.001, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let middle = CycleCut {
            keep_middle: true,
            ..cut
        };
        assert!(
            cycle_distance(&points, &middle, 0.001, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn nonplanar_proposed_crossing_charges_actual_rounding_displacement() {
        let height = 2.0_f64.powi(-20);
        let (points, mut cut) = skinny_bowtie(height);
        let offset = 8.0 * height;
        cut.point_mm[2] = offset;
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        assert!(
            cycle_distance(&points, &cut, offset * 0.5, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let bound = cycle_distance(&points, &cut, offset * 2.0, &mut work, &cancel)
            .unwrap()
            .expect("nonplanar distance paid by sufficient allowance");
        assert!(bound >= offset && bound <= offset * 2.0);
    }

    #[test]
    fn cycle_correspondence_refuses_nonfinite_and_propagates_cancel_and_work_exhaustion() {
        let (points, cut) = skinny_bowtie(0.001);
        let mut work = GeometryWorkBudget::new(1_000);
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            cycle_distance(&points, &cut, 1.0, &mut work, &cancelled)
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
        let cancel = AtomicBool::new(false);
        let mut exhausted = GeometryWorkBudget::new(0);
        assert_eq!(
            cycle_distance(&points, &cut, 1.0, &mut exhausted, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        let invalid = CycleCut {
            point_mm: [f64::NAN, 0.0, 0.0],
            ..cut
        };
        assert!(
            cycle_distance(&points, &invalid, 1.0, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }
}
