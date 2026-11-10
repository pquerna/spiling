// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use crate::*;

/// Continuous FP64 segment/open-box intersection. Boundary-only contact is
/// permitted by this declared model; no physical clearance/calibration claim.
fn penetrates(a: PointMm, b: PointMm, bounds: BoundsMm) -> bool {
    let mut enter: f64 = 0.;
    let mut leave: f64 = 1.;
    for axis in 0..3 {
        let delta = b.0[axis] - a.0[axis];
        if delta == 0. {
            if a.0[axis] <= bounds.min[axis] || a.0[axis] >= bounds.max[axis] {
                return false;
            }
        } else {
            let u = (bounds.min[axis] - a.0[axis]) / delta;
            let v = (bounds.max[axis] - a.0[axis]) / delta;
            enter = enter.max(u.min(v));
            leave = leave.min(u.max(v));
        }
    }
    enter < leave
}
pub(crate) fn clear(
    problem: &Problem,
    built: &[BoundsMm],
    built_top_mm: f64,
    a: PointMm,
    b: PointMm,
    meter: &mut Meter<'_>,
) -> Result<bool, Error> {
    if !problem.envelope_mm.contains(a) || !problem.envelope_mm.contains(b) {
        return Ok(false);
    }
    // Exact shortcut: no head component reaches below the top of all earlier
    // material, so no open expanded material box can contain the segment.
    let floor = problem.head_mm.iter().map(|h| h.min[2]).fold(0., f64::min);
    meter.charge(1)?;
    let built = if a.0[2].min(b.0[2]) + floor >= built_top_mm {
        &[]
    } else {
        built
    };
    for obstacle in problem.obstacles_mm.iter().chain(built) {
        for head in &problem.head_mm {
            meter.charge(1)?;
            let expanded = BoundsMm {
                min: std::array::from_fn(|i| obstacle.min[i] - head.max[i]),
                max: std::array::from_fn(|i| obstacle.max[i] - head.min[i]),
            };
            if (0..3).any(|i| !expanded.min[i].is_finite() || !expanded.max[i].is_finite()) {
                return Err(Error::InvalidInput);
            }
            // Disjoint closed segment bounds cannot enter the open box.
            if (0..3).any(|i| {
                a.0[i].max(b.0[i]) <= expanded.min[i] || a.0[i].min(b.0[i]) >= expanded.max[i]
            }) {
                continue;
            }
            if penetrates(a, b, expanded) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
