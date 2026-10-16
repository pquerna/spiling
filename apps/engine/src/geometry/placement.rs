// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::geometry::*;

fn rotate(pose: RigidPoseMm, v: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = pose.rotation_xyzw;
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let q = [x, y, z];
    let t = cross(q, v).map(|c| 2.0 * c);
    let u = cross(q, t);
    std::array::from_fn(|i| v[i] + w * t[i] + u[i])
}
fn point(pose: RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let p = rotate(pose, p);
    std::array::from_fn(|i| p[i] + pose.translation_mm[i])
}
/// Scene framing and finite-coordinate admission only, not certified native contact.
pub fn scene_bounds(pose: RigidPoseMm, bounds: AabbMm) -> Result<AabbMm, GeometryError> {
    let first = point(pose, bounds.min);
    let mut result = AabbMm::new(first, first)?;
    for bits in 1..8 {
        result.include(point(
            pose,
            std::array::from_fn(|i| {
                if bits & (1 << i) == 0 {
                    bounds.min[i]
                } else {
                    bounds.max[i]
                }
            }),
        ))?;
    }
    Ok(result)
}
