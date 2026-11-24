// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Road lowering in the plan (RFC 11): simplify a road's own points within
//! its class budget, only in a direction its owning checks still accept.
//!
//! Bulk support walls are closed loops that trace the support grid as a
//! staircase. A loop drops a run of points only when every dropped point lies
//! on the loop's non-material side of the new chord (or on it), so the chord
//! cuts outer corners and the support region can only shrink: clearance to
//! the part never decreases and no new area is covered. Which side holds
//! material follows from the loop's winding and its nesting among the layer's
//! other support loops, not from routing direction.

use crate::{check_cancel, error};
use rayon::prelude::*;
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

/// Shortest chord lowering creates: well above the minimum command length, so
/// XYZ6 rounding never shortens one below it (as brim loop edges).
const MIN_LOWERED_CHORD_MM: f64 =
    spiling_contracts::manufacturing::program_coordinates::MIN_DEPOSITION_COMMAND_MM * 2.5;

/// Simplify every bulk support wall loop of `plan` within `budget_mm` of its
/// own points, keeping each loop on its material side. Returns the number of
/// edges removed.
pub(super) fn support_walls(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    budget_mm: f64,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    if budget_mm.is_nan() || budget_mm <= 0.0 {
        return Ok(0);
    }
    let recipe = &intent.recipe;
    let working_set = crate::working_set::current();
    plan.layers
        .par_iter_mut()
        .map(|layer| {
            crate::working_set::scope(working_set, || {
                layer_walls(layer, recipe, budget_mm, cancel)
            })
        })
        .try_fold(|| 0usize, |total, removed| removed.map(|r| total + r))
        .try_reduce(|| 0, |a, b| Ok(a + b))
}

fn eligible(path: &DepositionPath) -> bool {
    let points = &path.points_mm;
    path.kind == DepositionKind::Support
        && path.arcs.is_empty()
        && path.connectors.is_empty()
        && matches!(path.width, WidthProfile::Constant { .. })
        && points.len() >= 5
        && points.first() == points.last()
        && points.iter().all(|p| p[2] == points[0][2])
}

fn layer_walls(
    layer: &mut PrintLayer,
    recipe: &PlanarPrintRecipe,
    budget_mm: f64,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    check_cancel(cancel)?;
    let loops: Vec<usize> = (0..layer.paths.len())
        .filter(|&i| eligible(&layer.paths[i]))
        .collect();
    if loops.is_empty() {
        return Ok(0);
    }
    let bounds: Vec<[f64; 4]> = loops
        .iter()
        .map(|&i| bounds(&layer.paths[i].points_mm))
        .collect();
    let mut removed = 0usize;
    let mut volume_change = 0.0f64;
    let mut lowered = Vec::with_capacity(loops.len());
    for (slot, &index) in loops.iter().enumerate() {
        check_cancel(cancel)?;
        let points = &layer.paths[index].points_mm;
        // Material is inside a loop nested in an even number of other loops.
        let probe = points[0];
        let depth = loops
            .iter()
            .enumerate()
            .filter(|&(other, &other_index)| {
                other != slot
                    && inside_bounds(&bounds[other], probe)
                    && contains(&layer.paths[other_index].points_mm, probe)
            })
            .count();
        let area = signed_area(points);
        if area == 0.0 {
            lowered.push(None);
            continue;
        }
        let material_left = (area > 0.0) == (depth % 2 == 0);
        lowered.push(simplify(points, budget_mm, material_left, area));
    }
    for (&index, simplified) in loops.iter().zip(lowered) {
        let Some(points) = simplified else { continue };
        let path = &mut layer.paths[index];
        let before = crate::planner::nominal_volume(path, layer.index, recipe)?;
        removed += path.points_mm.len() - points.len();
        path.points_mm = points;
        let after = crate::planner::nominal_volume(path, layer.index, recipe)?;
        volume_change += after - before;
    }
    if removed > 0 {
        let coverage = &mut layer.region_coverage;
        coverage.support_nominal_deposition_volume_mm3 += volume_change;
        coverage.support_area_mm2 =
            coverage.support_nominal_deposition_volume_mm3 / (layer.z_mm - layer.lower_z_mm);
        if !coverage.support_nominal_deposition_volume_mm3.is_finite() {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "lowered support volume is not finite",
            ));
        }
    }
    Ok(removed)
}

/// The loop with every run of points dropped whose chord keeps all of them
/// within `budget_mm` and on the non-material side, or None when nothing
/// changes or the loop would lose more than half its area.
fn simplify(
    points: &[[f64; 3]],
    budget_mm: f64,
    material_left: bool,
    area: f64,
) -> Option<Vec<[f64; 3]>> {
    let n = points.len() - 1;
    let side = if material_left { 1.0 } else { -1.0 };
    let mut kept = vec![points[0]];
    let mut anchor = 0;
    while anchor < n {
        let mut best = anchor + 1;
        let mut end = anchor + 2;
        while end <= n {
            let a = points[anchor];
            let b = points[end];
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let length = dx.hypot(dy);
            if length.is_nan() || length <= 0.0 {
                break;
            }
            let fits = points[anchor + 1..end].iter().all(|p| {
                let offset = (dx * (p[1] - a[1]) - dy * (p[0] - a[0])) / length;
                // Positive offset is left of the chord; a dropped point must sit
                // on the non-material side (or on the chord) and within budget.
                side * offset <= 1e-9 && offset.abs() <= budget_mm
            });
            if !fits {
                break;
            }
            // A new chord must stay a printable command; a shorter one (a loop
            // folding back near its start) waits for a longer extension.
            if length >= MIN_LOWERED_CHORD_MM {
                best = end;
            }
            end += 1;
        }
        kept.push(points[best]);
        anchor = best;
    }
    if kept.len() == points.len() || kept.len() < 4 {
        return None;
    }
    let lowered_area = signed_area(&kept);
    (lowered_area.signum() == area.signum() && lowered_area.abs() >= 0.5 * area.abs())
        .then_some(kept)
}

fn signed_area(points: &[[f64; 3]]) -> f64 {
    points
        .windows(2)
        .map(|w| w[0][0] * w[1][1] - w[1][0] * w[0][1])
        .sum::<f64>()
        * 0.5
}

fn bounds(points: &[[f64; 3]]) -> [f64; 4] {
    points.iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        },
    )
}

fn inside_bounds(b: &[f64; 4], p: [f64; 3]) -> bool {
    b[0] <= p[0] && p[0] <= b[2] && b[1] <= p[1] && p[1] <= b[3]
}

/// Even-odd ray test of `p` against a closed loop.
fn contains(points: &[[f64; 3]], p: [f64; 3]) -> bool {
    let mut inside = false;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_staircase() -> Vec<[f64; 3]> {
        // A 2 x 2 square whose top-right corner is a 4-step staircase.
        let mut p = vec![[0.0, 0.0, 1.0], [2.0, 0.0, 1.0], [2.0, 1.6, 1.0]];
        for k in 0..4 {
            let x = 2.0 - 0.1 * k as f64;
            let y = 1.6 + 0.1 * k as f64;
            p.push([x, y + 0.1, 1.0]);
            p.push([x - 0.1, y + 0.1, 1.0]);
        }
        p.push([0.0, 2.0, 1.0]);
        p.push([0.0, 0.0, 1.0]);
        p
    }

    #[test]
    fn staircase_loses_outer_corners_only() {
        let points = square_staircase();
        let area = signed_area(&points);
        assert!(area > 0.0);
        let lowered = simplify(&points, 0.2, true, area).expect("simplified");
        assert!(lowered.len() < points.len());
        // Every dropped point is outside or on the lowered loop: the region shrinks.
        for p in &points {
            if !lowered.contains(p) {
                assert!(
                    !contains(&lowered, *p) || on_boundary(&lowered, *p),
                    "{p:?}"
                );
            }
        }
        assert!(signed_area(&lowered) <= area + 1e-12);
    }

    #[test]
    fn a_hole_keeps_material_outside() {
        let points = square_staircase();
        let area = signed_area(&points);
        // Same loop as a hole: material is outside, so the loop may only grow.
        let lowered = simplify(&points, 0.2, false, area).expect("simplified");
        assert!(signed_area(&lowered) >= area - 1e-12);
    }

    #[test]
    fn budget_limits_the_chord() {
        let points = square_staircase();
        let area = signed_area(&points);
        // Only exactly collinear points may go: the region is unchanged.
        if let Some(lowered) = simplify(&points, 1e-6, true, area) {
            assert!((signed_area(&lowered) - area).abs() < 1e-12);
        }
    }

    fn on_boundary(points: &[[f64; 3]], p: [f64; 3]) -> bool {
        points.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
            cross.abs() < 1e-9
                && p[0] >= a[0].min(b[0]) - 1e-9
                && p[0] <= a[0].max(b[0]) + 1e-9
                && p[1] >= a[1].min(b[1]) - 1e-9
                && p[1] <= a[1].max(b[1]) + 1e-9
        })
    }
}
