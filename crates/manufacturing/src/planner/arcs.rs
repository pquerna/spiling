// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Arc runs on wall paths: consecutive points that lie on an exact circle of
//! the source BREP. Centers come from the placed analytic faces, never from a
//! fit to the polyline, so an arc command reproduces the source curve. The
//! points stay the path's geometry; a run only says they share one circle.

use crate::{check_cancel, error};
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
    if sources.iter().all(CircleSource::is_empty) {
        return Ok(());
    }
    let mut circles = Vec::new();
    for layer in &mut plan.layers {
        check_cancel(cancel)?;
        circles.clear();
        for source in sources {
            source.circles_at(layer.section_z_mm, &mut circles);
        }
        dedupe(&mut circles);
        if circles.is_empty() {
            continue;
        }
        for path in &mut layer.paths {
            if path.owner == Some(DepositionOwner::Wall) && path.points_mm.len() >= 3 {
                path.arcs = runs(path, &circles, cancel)?;
            }
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
}
