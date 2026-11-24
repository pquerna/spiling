// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Arc runs on wall paths: consecutive points that lie on an exact circle of
//! the source BREP. Centers come from the placed analytic faces, never from a
//! fit to the polyline, so an arc command reproduces the source curve. The
//! points stay the path's geometry; a run only says they share one circle.

use crate::{check_cancel, error};
use rayon::prelude::*;
use spiling_contracts::manufacturing::*;
use spiling_geometry::tolerant::{CircleSource, SectionCircle};
use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;

/// Width of the radial band every point of a run lies in. Inside the
/// contract allowance, so a run and any piece of it a seam splits off
/// validate against their own first point. Walls of sampled sections sit
/// within a few tenths of a micrometre of their circle.
const RUN_BAND_MM: f64 = MAX_ARC_POINT_DEVIATION_MM * 0.9;
/// Chord sagitta allowed when forming a run. The contract recomputes the
/// radius from the stored run, which can sit anywhere in its radial band, so
/// keep a margin below the contract's limit.
const SAGITTA_LIMIT_MM: f64 = MAX_ARC_SAGITTA_MM * 0.9;
/// Sweep allowed when forming a run. The program rounds the run's ends and
/// center to its coordinate step, which can turn a run planned at the
/// contract's limit a few nanoradians past it when the verifier decodes it;
/// keep a margin below the limit.
const SWEEP_LIMIT_RADIANS: f64 = MAX_ARC_SWEEP_RADIANS * 0.99;
/// Largest offset of a wall from the section circle it follows.
const WALL_BAND_MM: f64 = 5.0;
/// Fewest edges a fitted run spans: any three points lie on a circle, so a
/// fitted circle needs at least one point more to be evidence of a curve.
const MIN_FITTED_RUN_EDGES: usize = 3;
/// Largest turn of one edge in a run; a sharper turn is a corner.
const MAX_EDGE_TURN_RADIANS: f64 = PI / 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Candidate {
    first: usize,
    last: usize,
    center: [f64; 2],
    clockwise: bool,
}

/// Attach arc runs to every wall path of `plan` from the circle sources of
/// its occurrences.
pub(super) fn attach(
    plan: &mut NormalizedPrintPlan,
    sources: &[CircleSource],
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let working_set = crate::working_set::current();
    // Each layer reads the same immutable source and only writes its own
    // paths. If several layers fail, return the first failure in plan order.
    plan.layers
        .par_iter_mut()
        .map_init(Vec::new, |circles, layer| {
            crate::working_set::scope(working_set, || {
                attach_layer(layer, sources, circles, cancel)
            })
        })
        .find_map_first(Result::err)
        .map_or(Ok(()), Err)
}

fn attach_layer(
    layer: &mut PrintLayer,
    sources: &[CircleSource],
    circles: &mut Vec<SectionCircle>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    circles.clear();
    for source in sources {
        source.circles_at(layer.section_z_mm, circles);
    }
    dedupe(circles);
    for path in &mut layer.paths {
        if path.owner == Some(DepositionOwner::Wall) && path.points_mm.len() >= 3 {
            path.arcs = runs(path, circles, cancel)?;
        }
    }
    Ok(())
}

/// One candidate per distinct center; concentric faces share it.
fn dedupe(circles: &mut Vec<SectionCircle>) {
    circles.sort_by(|a, b| {
        a.center[0]
            .total_cmp(&b.center[0])
            .then(a.center[1].total_cmp(&b.center[1]))
            .then(a.radius.total_cmp(&b.radius))
    });
    circles.dedup_by(|a, b| a.center == b.center);
}

/// The non-overlapping arc runs of one path, longest first.
fn runs(
    path: &DepositionPath,
    circles: &[SectionCircle],
    cancel: &AtomicBool,
) -> Result<Vec<ArcRun>, ManufacturingError> {
    let points = &path.points_mm;
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in points {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(p[axis]);
            hi[axis] = hi[axis].max(p[axis]);
        }
    }
    let mut found = Vec::new();
    let mut distances = Vec::with_capacity(points.len());
    for circle in circles {
        check_cancel(cancel)?;
        // The path must come within the wall band of the circle.
        let gap = |axis: usize| {
            (lo[axis] - circle.center[axis])
                .max(circle.center[axis] - hi[axis])
                .max(0.0)
        };
        if gap(0).hypot(gap(1)) > circle.radius + WALL_BAND_MM {
            continue;
        }
        distances.clear();
        distances.extend(
            points
                .iter()
                .map(|p| (p[0] - circle.center[0]).hypot(p[1] - circle.center[1])),
        );
        runs_about(path, circle.center, &distances, &mut found);
    }
    found.sort_by(|a, b| {
        (b.last - b.first)
            .cmp(&(a.last - a.first))
            .then(a.first.cmp(&b.first))
    });
    let mut chosen: Vec<Candidate> = Vec::new();
    for candidate in found {
        if chosen
            .iter()
            .all(|c| candidate.last <= c.first || c.last <= candidate.first)
        {
            chosen.push(candidate);
        }
    }
    chosen.sort_by_key(|c| c.first);
    // Between the source-circle runs, fit runs to the wall's own points: the
    // points stay the geometry, so a fitted run moves no printed surface.
    let mut fitted = Vec::new();
    let mut from = 0;
    for c in chosen
        .iter()
        .map(|c| (c.first, c.last))
        .chain([(points.len() - 1, points.len() - 1)])
    {
        check_cancel(cancel)?;
        fit_runs(path, from, c.0, &mut fitted);
        from = c.1;
    }
    chosen.extend(fitted);
    chosen.sort_by_key(|c| c.first);
    chosen
        .into_iter()
        .map(|c| {
            Ok(ArcRun {
                first: u32::try_from(c.first).map_err(|_| too_long())?,
                last: u32::try_from(c.last).map_err(|_| too_long())?,
                center_mm: c.center,
                clockwise: c.clockwise,
            })
        })
        .collect()
}

/// Greedy circle runs over points `from..=to` of a wall path: from each start,
/// grow the run while the circle through its first, middle and last points
/// keeps every point in the run band and every edge within the same turning,
/// sagitta, sweep, width and Z rules the source-circle runs follow.
fn fit_runs(path: &DepositionPath, from: usize, to: usize, found: &mut Vec<Candidate>) {
    let points = &path.points_mm;
    let width = |edge: usize| match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm[edge],
    };
    let mut i = from;
    while i + 2 <= to {
        let mut best: Option<Candidate> = None;
        let mut j = i + 2;
        while j <= to {
            let confirmed = j - i >= MIN_FITTED_RUN_EDGES;
            let Some(center) = circle_through(points[i], points[(i + j) / 2], points[j]) else {
                break;
            };
            if !valid_run(points, i, j, center, &width) {
                break;
            }
            let step = |k: usize| {
                let (a, b) = (points[k], points[k + 1]);
                let (ax, ay) = (a[0] - center[0], a[1] - center[1]);
                let (bx, by) = (b[0] - center[0], b[1] - center[1]);
                (ax * by - ay * bx).atan2(ax * bx + ay * by)
            };
            if confirmed {
                best = Some(Candidate {
                    first: i,
                    last: j,
                    center,
                    clockwise: step(i) < 0.0,
                });
            }
            j += 1;
        }
        match best {
            Some(run) => {
                i = run.last;
                found.push(run);
            }
            None => i += 1,
        }
    }
}

/// Whether points `i..=j` form one run about `center` under the contract's
/// run rules (with this module's planning margins).
fn valid_run(
    points: &[[f64; 3]],
    i: usize,
    j: usize,
    center: [f64; 2],
    width: &impl Fn(usize) -> f64,
) -> bool {
    let distance = |p: [f64; 3]| (p[0] - center[0]).hypot(p[1] - center[1]);
    let radius = distance(points[i]);
    if !(MIN_ARC_RADIUS_MM..=MAX_ARC_RADIUS_MM).contains(&radius) {
        return false;
    }
    let (mut lo, mut hi) = (radius, radius);
    let mut direction = 0.0f64;
    let mut sweep = 0.0;
    for k in i..j {
        let (a, b) = (points[k], points[k + 1]);
        let (ax, ay) = (a[0] - center[0], a[1] - center[1]);
        let (bx, by) = (b[0] - center[0], b[1] - center[1]);
        let step = (ax * by - ay * bx).atan2(ax * bx + ay * by);
        let half = (b[0] - a[0]).hypot(b[1] - a[1]) * 0.5;
        let sagitta = radius - (radius * radius - half * half).max(0.0).sqrt();
        let d = distance(b);
        lo = lo.min(d);
        hi = hi.max(d);
        if step.abs() <= 1e-12
            || step.abs() >= MAX_EDGE_TURN_RADIANS
            || (direction != 0.0 && step.signum() != direction)
            || hi - lo > RUN_BAND_MM
            || b[2] != points[i][2]
            || sagitta > SAGITTA_LIMIT_MM
            || width(k) != width(i)
        {
            return false;
        }
        direction = step.signum();
        sweep += step.abs();
        if sweep > SWEEP_LIMIT_RADIANS {
            return false;
        }
    }
    true
}

/// Centre of the circle through three points, or None when they are
/// (nearly) collinear.
fn circle_through(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Option<[f64; 2]> {
    let (ax, ay, bx, by, cx, cy) = (a[0], a[1], b[0], b[1], c[0], c[1]);
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    let scale = [ax, ay, bx, by, cx, cy]
        .iter()
        .fold(1.0f64, |m, v| m.max(v.abs()));
    if d.abs() <= 1e-12 * scale * scale {
        return None;
    }
    let (a2, b2, c2) = (ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy);
    let center = [
        (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d,
        (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d,
    ];
    center.iter().all(|v| v.is_finite()).then_some(center)
}

fn too_long() -> ManufacturingError {
    error(
        ManufacturingErrorCode::ResourceLimit,
        "wall path is too long to index arc runs",
    )
}

/// Maximal runs of points equidistant from `center`, turning one way.
fn runs_about(
    path: &DepositionPath,
    center: [f64; 2],
    distances: &[f64],
    found: &mut Vec<Candidate>,
) {
    let points = &path.points_mm;
    let width = |edge: usize| match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm[edge],
    };
    let n = points.len() - 1;
    let mut i = 0;
    while i < n {
        let radius = distances[i];
        if !(MIN_ARC_RADIUS_MM..=MAX_ARC_RADIUS_MM).contains(&radius) {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut sweep = 0.0;
        let mut direction = 0.0f64;
        let mut band = [radius, radius];
        while j < n {
            let (a, b) = (points[j], points[j + 1]);
            let (ax, ay) = (a[0] - center[0], a[1] - center[1]);
            let (bx, by) = (b[0] - center[0], b[1] - center[1]);
            let step = (ax * by - ay * bx).atan2(ax * bx + ay * by);
            let half = (b[0] - a[0]).hypot(b[1] - a[1]) * 0.5;
            let sagitta = radius - (radius * radius - half * half).max(0.0).sqrt();
            if step.abs() <= 1e-12
                || step.abs() >= MAX_EDGE_TURN_RADIANS
                || (direction != 0.0 && step.signum() != direction)
                || band[1].max(distances[j + 1]) - band[0].min(distances[j + 1]) > RUN_BAND_MM
                || b[2] != points[i][2]
                || sagitta > SAGITTA_LIMIT_MM
                || sweep + step.abs() > SWEEP_LIMIT_RADIANS
                || width(j) != width(i)
            {
                break;
            }
            direction = step.signum();
            sweep += step.abs();
            band = [band[0].min(distances[j + 1]), band[1].max(distances[j + 1])];
            j += 1;
        }
        if j - i >= 2 {
            found.push(Candidate {
                first: i,
                last: j,
                center,
                clockwise: direction < 0.0,
            });
        }
        i = j.max(i + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(points: Vec<[f64; 3]>) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.0, 0.2],
            points_mm: points,
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }
    }

    fn circle(center: [f64; 2], radius: f64) -> SectionCircle {
        SectionCircle {
            center,
            radius,
            face: 0,
        }
    }

    #[test]
    fn a_wall_around_a_hole_is_two_runs_about_the_exact_center() {
        // A closed wall 0.3 mm outside a 5 mm hole, sampled as a section is.
        let center = [40.0, 60.0];
        let r = 5.3;
        let n = 180;
        let mut points: Vec<[f64; 3]> = (0..n)
            .map(|k| {
                let t = 2.0 * PI * k as f64 / n as f64;
                [center[0] + r * t.cos(), center[1] + r * t.sin(), 0.2]
            })
            .collect();
        points.push(points[0]);
        let mut path = wall(points);
        path.arcs = runs(
            &path,
            &[circle(center, 5.0), circle([0.0, 0.0], 5.0)],
            &AtomicBool::new(false),
        )
        .unwrap();
        // The whole loop, split so no run sweeps past its limit.
        assert_eq!(path.arcs.len(), 2);
        assert_eq!(path.arcs[0].first, 0);
        assert_eq!(path.arcs[1].last as usize, n);
        assert_eq!(path.arcs[0].last, path.arcs[1].first);
        assert!(
            path.arcs
                .iter()
                .all(|a| a.center_mm == center && !a.clockwise)
        );
        path.validate_arcs().unwrap();
    }

    #[test]
    fn corners_lines_and_other_centers_stay_chords() {
        // A rounded rectangle: two straight sides and two 90-degree corners
        // of radius 2 about known centers; a far center matches nothing.
        let mut points = vec![[0.0, 0.0, 0.2], [10.0, 0.0, 0.2]];
        let corner = [10.0, 2.0];
        for k in 1..=18 {
            let t = -PI / 2.0 + (PI / 2.0) * k as f64 / 18.0;
            points.push([corner[0] + 2.0 * t.cos(), corner[1] + 2.0 * t.sin(), 0.2]);
        }
        points.push([12.0, 10.0, 0.2]);
        let path = wall(points);
        let arcs = runs(
            &path,
            &[circle(corner, 2.0), circle([100.0, 100.0], 3.0)],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            arcs,
            [ArcRun {
                first: 1,
                last: 19,
                center_mm: corner,
                clockwise: false,
            }]
        );
        // Off the circle by more than the run allowance, it stays chords.
        let mut bent = path.clone();
        bent.points_mm[5][0] += 0.01;
        let arcs = runs(&bent, &[circle(corner, 2.0)], &AtomicBool::new(false)).unwrap();
        assert!(!arcs.is_empty() && arcs.iter().all(|a| a.last < 5 || a.first > 5));
    }

    #[test]
    fn walls_on_circles_the_source_does_not_name_get_fitted_runs() {
        // A half circle of radius 3 about (5, 5) with no source circles.
        let points: Vec<[f64; 3]> = (0..=48)
            .map(|k| {
                let t = PI * k as f64 / 48.0;
                [5.0 + 3.0 * t.cos(), 5.0 + 3.0 * t.sin(), 0.2]
            })
            .collect();
        let arcs = runs(&wall(points), &[], &AtomicBool::new(false)).unwrap();
        assert_eq!(arcs.len(), 1, "{arcs:?}");
        assert_eq!((arcs[0].first, arcs[0].last), (0, 48));
        assert!(
            (arcs[0].center_mm[0] - 5.0).abs() < 1e-9 && (arcs[0].center_mm[1] - 5.0).abs() < 1e-9
        );
        assert!(!arcs[0].clockwise);
    }

    #[test]
    fn zigzags_and_kinks_are_not_fitted() {
        let zigzag: Vec<[f64; 3]> = (0..12)
            .map(|k| [k as f64 * 0.5, if k % 2 == 0 { 0.0 } else { 0.3 }, 0.2])
            .collect();
        assert!(
            runs(&wall(zigzag), &[], &AtomicBool::new(false))
                .unwrap()
                .is_empty()
        );
        // Three points always lie on a circle; one kink is not a curve.
        let kink = vec![
            [0.0, 0.0, 0.2],
            [1.0, 0.0, 0.2],
            [2.0, 0.1, 0.2],
            [3.0, 0.1, 0.2],
        ];
        assert!(
            runs(&wall(kink), &[], &AtomicBool::new(false))
                .unwrap()
                .is_empty()
        );
    }
}
