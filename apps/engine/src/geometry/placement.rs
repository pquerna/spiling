// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::geometry::*;

pub fn rotate(pose: RigidPoseMm, v: [f64; 3]) -> [f64; 3] {
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
pub fn point(pose: RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let p = rotate(pose, p);
    std::array::from_fn(|i| p[i] + pose.translation_mm[i])
}
pub fn inverse_plane(pose: RigidPoseMm, plane: PlaneMm) -> Result<PlaneMm, GeometryError> {
    let [x, y, z, w] = pose.rotation_xyzw;
    let inverse = RigidPoseMm {
        translation_mm: [0.0; 3],
        rotation_xyzw: [-x, -y, -z, w],
    };
    PlaneMm {
        origin_mm: rotate(
            inverse,
            std::array::from_fn(|i| plane.origin_mm[i] - pose.translation_mm[i]),
        ),
        normal: rotate(inverse, plane.normal),
    }
    .normalized()
}
pub fn placed_bounds(pose: RigidPoseMm, bounds: AabbMm) -> Result<AabbMm, GeometryError> {
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

/// Equal normalized oriented plane equations share native work within this job only.
/// Canonicalizing the origin removes in-plane offsets, but never merges approximate planes.
pub fn canonical_plane(plane: PlaneMm) -> Result<(PlaneMm, [u64; 4]), GeometryError> {
    let plane = plane.normalized()?;
    let distance = plane
        .origin_mm
        .iter()
        .zip(plane.normal)
        .map(|(a, b)| a * b)
        .sum::<f64>();
    let zero = |v: f64| if v == 0.0 { 0.0 } else { v };
    let key = [
        zero(plane.normal[0]).to_bits(),
        zero(plane.normal[1]).to_bits(),
        zero(plane.normal[2]).to_bits(),
        zero(distance).to_bits(),
    ];
    let canonical = PlaneMm {
        origin_mm: plane.normal.map(|v| zero(v * distance)),
        normal: plane.normal,
    };
    canonical.validate()?;
    Ok((canonical, key))
}
