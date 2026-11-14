// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent bed exclusion check of decoded deposition: a bead of
//! half-width `radius` from `from` to `to` (decoded XYZ6, top at the decoded
//! Z) may not overlap the open rectangle of a region whose ceiling it
//! exceeds. The distance is computed here from the rectangle's four edges,
//! separately from the contract's own predicate the planner uses.

use super::failed;
use spiling_contracts::manufacturing::{BedExclusion, ManufacturingError};

pub(super) fn check(
    exclusions: &[BedExclusion],
    from: [f64; 3],
    to: [f64; 3],
    radius: f64,
) -> Result<(), ManufacturingError> {
    let top = from[2].max(to[2]);
    for x in exclusions {
        if top <= x.max_material_z_mm {
            continue;
        }
        let (lo, hi) = (x.min_xy_mm, x.max_xy_mm);
        let strictly_inside =
            |p: [f64; 3]| p[0] > lo[0] && p[0] < hi[0] && p[1] > lo[1] && p[1] < hi[1];
        let corners = [
            [lo[0], lo[1]],
            [hi[0], lo[1]],
            [hi[0], hi[1]],
            [lo[0], hi[1]],
        ];
        let nearest = (0..4)
            .map(|i| {
                segments(
                    [from[0], from[1]],
                    [to[0], to[1]],
                    corners[i],
                    corners[(i + 1) % 4],
                )
            })
            .fold(f64::INFINITY, f64::min);
        if strictly_inside(from) || strictly_inside(to) || nearest < radius {
            return Err(failed(format!(
                "decoded deposition enters bed exclusion region {}",
                x.id
            )));
        }
    }
    Ok(())
}

/// Distance between two closed segments in the plane.
fn segments(p: [f64; 2], q: [f64; 2], r: [f64; 2], s: [f64; 2]) -> f64 {
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let (d1, d2, d3, d4) = (
        cross(r, s, p),
        cross(r, s, q),
        cross(p, q, r),
        cross(p, q, s),
    );
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return 0.0;
    }
    let to_segment = |x: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
        let l = vx * vx + vy * vy;
        let t = if l == 0.0 {
            0.0
        } else {
            (((x[0] - a[0]) * vx + (x[1] - a[1]) * vy) / l).clamp(0.0, 1.0)
        };
        (x[0] - a[0] - t * vx).hypot(x[1] - a[1] - t * vy)
    };
    to_segment(p, r, s)
        .min(to_segment(q, r, s))
        .min(to_segment(r, p, q))
        .min(to_segment(s, p, q))
}
