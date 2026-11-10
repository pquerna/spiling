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
    a: PointMm,
    b: PointMm,
    meter: &mut Meter<'_>,
) -> Result<bool, Error> {
    if !problem.envelope_mm.contains(a) || !problem.envelope_mm.contains(b) {
        return Ok(false);
    }
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
            if penetrates(a, b, expanded) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
