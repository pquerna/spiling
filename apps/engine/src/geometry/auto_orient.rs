// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Deterministic engine-owned automatic print orientation on the build plate.
//!
//! Evaluates candidate resting directions from the part's single-authority
//! tolerant model (current orientation, 6 cardinal axes, and top planar face
//! normal clusters), scoring each for envelope fit, unsupported overhang area
//! (> 45° above the bed), flat bed-contact area, and build height.
//! Both CLI (`--auto-orient`) and workbench (`Auto-orient`) consume this one
//! rule through `Geometry.PlaceOnBed { auto_orient: true }`.

use super::{check_cancel, error};
use spiling_contracts::{geometry::*, manufacturing::PrinterSpecification};
use spiling_geometry::{source::SourceDefinition as Definition, tolerant::TolerantPolicy};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

/// Cosine of 45°: downward-facing triangles steeper than 45° from vertical
/// (`n_z < -COS_45`) count as overhangs requiring support when above the bed.
const OVERHANG_COS: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Triangles whose vertices sit within this height of the minimum Z count as
/// flat bed contact when facing downward (`n_z <= -0.995`).
const BED_CONTACT_Z_MM: f64 = 0.15;
const FLAT_DOWN_COS: f64 = 0.995;
/// Deduplicate candidate directions closer than ~1.5° (`cos(1.5°) ≈ 0.99966`).
const MERGE_COS: f64 = 0.99966;
/// Maximum planar face-normal clusters examined alongside current + 6 cardinals.
const MAX_FACE_CLUSTERS: usize = 16;

#[derive(Debug, Clone, Copy)]
struct ScoredOrientation {
    rotation_xyzw: [f64; 4],
    fits: bool,
    overhang_mm2: f64,
    bed_contact_mm2: f64,
    height_mm: f64,
    score: f64,
}

/// Choose the best printing rotation for `definition` starting from `current_pose`,
/// respecting `printer.build_envelope`. Preserves the existing translation (which
/// `bed_pose` + `bed_group` subsequently rests on the bed and centres as needed).
pub(super) fn choose_pose(
    definition: &Definition,
    current_pose: RigidPoseMm,
    printer: &PrinterSpecification,
    additional_bytes: usize,
    cancel: &AtomicBool,
) -> Result<RigidPoseMm, GeometryError> {
    check_cancel(cancel)?;
    let (model, _) =
        definition.prepare_with_budget(TolerantPolicy::default(), additional_bytes, cancel)?;
    if model.triangles().is_empty() || model.positions().is_empty() {
        return Ok(current_pose);
    }
    let rotation_xyzw = choose_rotation_for_mesh(
        model.positions(),
        model.triangles(),
        current_pose.rotation_xyzw,
        printer.build_envelope,
        printer.motion_resolution.xy_mm,
        cancel,
    )?;
    let pose = RigidPoseMm {
        translation_mm: current_pose.translation_mm,
        rotation_xyzw,
    };
    pose.validate()?;
    Ok(pose)
}

fn choose_rotation_for_mesh(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    current_q: [f64; 4],
    envelope: AabbMm,
    xy_slack: f64,
    cancel: &AtomicBool,
) -> Result<[f64; 4], GeometryError> {
    let mut normals_and_areas: Vec<([f64; 3], f64, [f64; 3])> = Vec::with_capacity(triangles.len());
    let mut buckets: BTreeMap<[i32; 3], ([f64; 3], f64)> = BTreeMap::new();
    let mut total_area = 0.0_f64;

    for (idx, &[i0, i1, i2]) in triangles.iter().enumerate() {
        if idx & 4095 == 0 {
            check_cancel(cancel)?;
        }
        let a = positions[i0 as usize];
        let b = positions[i1 as usize];
        let c = positions[i2 as usize];
        let ab = sub(b, a);
        let ac = sub(c, a);
        let cross = cross(ab, ac);
        let twice_area = length(cross);
        if !twice_area.is_finite() || twice_area <= 1e-12 {
            continue;
        }
        let area = 0.5 * twice_area;
        let normal = scale(cross, 1.0 / twice_area);
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        normals_and_areas.push((normal, area, centroid));
        total_area += area;
        let key = [
            (normal[0] * 200.0).round() as i32,
            (normal[1] * 200.0).round() as i32,
            (normal[2] * 200.0).round() as i32,
        ];
        let entry = buckets.entry(key).or_insert(([0.0; 3], 0.0));
        entry.0[0] += normal[0] * area;
        entry.0[1] += normal[1] * area;
        entry.0[2] += normal[2] * area;
        entry.1 += area;
    }

    if normals_and_areas.is_empty() {
        return Ok(current_q);
    }

    // Build deterministic deduplicated candidate resting directions (outward
    // local normal that will point along -Z onto the bed).
    let mut candidates: Vec<[f64; 4]> = Vec::new();
    let mut seen_down: Vec<[f64; 3]> = Vec::new();

    // Candidate 0 is always the exact current pose so already-optimal parts stay unchanged.
    let current_down = rotate_inv(current_q, [0.0, 0.0, -1.0]);
    seen_down.push(current_down);
    candidates.push(current_q);

    let mut push_down = |down: [f64; 3]| {
        let len = length(down);
        if !len.is_finite() || len <= 1e-12 {
            return;
        }
        let unit = scale(down, 1.0 / len);
        if seen_down.iter().any(|&other| dot(other, unit) >= MERGE_COS) {
            return;
        }
        seen_down.push(unit);
        let base_q = rotation_aligning(unit, [0.0, 0.0, -1.0]);
        candidates.push(base_q);
        // Also evaluate a 90° Z yaw when the bed is non-square so long parts can fit.
        let bed_x = envelope.max[0] - envelope.min[0];
        let bed_y = envelope.max[1] - envelope.min[1];
        if (bed_x - bed_y).abs() > 1.0 {
            candidates.push(quat_mul(
                [
                    0.0,
                    0.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                    std::f64::consts::FRAC_1_SQRT_2,
                ],
                base_q,
            ));
        }
    };

    // 6 cardinal axes
    for axis in [
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
    ] {
        push_down(axis);
    }

    // Top planar face clusters ordered deterministically by descending area.
    let mut clusters: Vec<([f64; 3], f64)> = buckets.into_values().collect();
    clusters.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.0[0].total_cmp(&b.0[0]))
            .then_with(|| a.0[1].total_cmp(&b.0[1]))
            .then_with(|| a.0[2].total_cmp(&b.0[2]))
    });
    for (sum_n, _) in clusters.into_iter().take(MAX_FACE_CLUSTERS) {
        push_down(sum_n);
    }

    let area_scale = total_area.sqrt().max(1.0);
    let bed_size = [
        envelope.max[0] - envelope.min[0] + 2.0 * xy_slack,
        envelope.max[1] - envelope.min[1] + 2.0 * xy_slack,
        envelope.max[2] - envelope.min[2],
    ];

    let evaluate = |q: [f64; 4]| -> Result<ScoredOrientation, GeometryError> {
        check_cancel(cancel)?;
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for &p in positions {
            let r = rotate(q, p);
            for a in 0..3 {
                lo[a] = lo[a].min(r[a]);
                hi[a] = hi[a].max(r[a]);
            }
        }
        let span = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        let fits = span[0] <= bed_size[0] && span[1] <= bed_size[1] && span[2] <= bed_size[2];
        let mut bed_contact_mm2 = 0.0_f64;
        let mut overhang_mm2 = 0.0_f64;
        let mut elevated_overhang_mm2 = 0.0_f64;
        for &(n_local, area, c_local) in &normals_and_areas {
            let nz = rotate_z(q, n_local);
            if nz >= -OVERHANG_COS {
                continue;
            }
            let cz = rotate_z(q, c_local);
            let dz = (cz - lo[2]).max(0.0);
            if nz <= -FLAT_DOWN_COS && dz <= BED_CONTACT_Z_MM {
                bed_contact_mm2 += area;
            } else if dz > BED_CONTACT_Z_MM {
                overhang_mm2 += area;
                elevated_overhang_mm2 += area * (1.0 + 0.015 * dz);
            }
        }
        // Lower score is better: strongly penalize overhangs (especially elevated),
        // reward flat bed contact for stability, and lightly prefer lower build height.
        let score = 2.5 * overhang_mm2 + elevated_overhang_mm2 - 0.35 * bed_contact_mm2
            + 0.02 * span[2] * area_scale;
        Ok(ScoredOrientation {
            rotation_xyzw: q,
            fits,
            overhang_mm2,
            bed_contact_mm2,
            height_mm: span[2],
            score,
        })
    };

    let current = evaluate(candidates[0])?;
    let mut best = current;
    for &q in &candidates[1..] {
        let scored = evaluate(q)?;
        let better = match (scored.fits, best.fits) {
            (true, false) => true,
            (false, true) => false,
            _ => scored.score < best.score - 1e-6,
        };
        if better {
            best = scored;
        }
    }

    // Avoid rotating already-optimal symmetric parts unless the candidate
    // fixes an envelope miss or improves the score by at least 1%.
    let meaningful = (best.fits && !current.fits)
        || best.score + 1e-3 < current.score - 0.01 * current.score.abs().max(1.0);
    let _ = (best.overhang_mm2, best.bed_contact_mm2, best.height_mm);
    if meaningful {
        let normalized = normalize_quat(best.rotation_xyzw).ok_or_else(|| {
            error(
                GeometryErrorCode::InvalidPose,
                "auto-orient produced a nonfinite orientation",
            )
        })?;
        Ok(normalized)
    } else {
        Ok(current_q)
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = q;
    let t = [
        2.0 * (y * v[2] - z * v[1]),
        2.0 * (z * v[0] - x * v[2]),
        2.0 * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}
fn rotate_z(q: [f64; 4], v: [f64; 3]) -> f64 {
    let [x, y, z, w] = q;
    let tx = 2.0 * (y * v[2] - z * v[1]);
    let ty = 2.0 * (z * v[0] - x * v[2]);
    let tz = 2.0 * (x * v[1] - y * v[0]);
    v[2] + w * tz + (x * ty - y * tx)
}
fn rotate_inv(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    rotate([-q[0], -q[1], -q[2], q[3]], v)
}
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    normalize_quat([
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ])
    .unwrap_or([0.0, 0.0, 0.0, 1.0])
}
fn normalize_quat(q: [f64; 4]) -> Option<[f64; 4]> {
    let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if !norm.is_finite() || norm <= 1e-12 {
        return None;
    }
    let out = q.map(|c| {
        let v = c / norm;
        if v == 0.0 { 0.0 } else { v }
    });
    out.iter().all(|v| v.is_finite()).then_some(out)
}
/// Canonical minimal unit quaternion rotating unit vector `from` to unit vector `to`.
fn rotation_aligning(from: [f64; 3], to: [f64; 3]) -> [f64; 4] {
    let d = dot(from, to);
    if d >= 1.0 - 1e-12 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    if d <= -1.0 + 1e-12 {
        // 180° flip around a deterministic axis orthogonal to `from`.
        let fallback = if from[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let axis = cross(from, fallback);
        let len = length(axis);
        let u = scale(axis, 1.0 / len);
        return normalize_quat([u[0], u[1], u[2], 0.0]).unwrap_or([1.0, 0.0, 0.0, 0.0]);
    }
    let c = cross(from, to);
    normalize_quat([c[0], c[1], c[2], 1.0 + d]).unwrap_or([0.0, 0.0, 0.0, 1.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_mesh(min: [f64; 3], max: [f64; 3]) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        let v = vec![
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ];
        // Outward CCW winding
        let t = vec![
            [0, 2, 1],
            [0, 3, 2], // -Z
            [4, 5, 6],
            [4, 6, 7], // +Z
            [0, 1, 5],
            [0, 5, 4], // -Y
            [2, 3, 7],
            [2, 7, 6], // +Y
            [0, 4, 7],
            [0, 7, 3], // -X
            [1, 2, 6],
            [1, 6, 5], // +X
        ];
        (v, t)
    }

    fn t_bracket_mesh() -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        // Narrow stem 10x10x30 topped by a wide 50x10x5 flange (massive overhang upright).
        let (mut v, mut t) = box_mesh([-5.0, -5.0, 0.0], [5.0, 5.0, 30.0]);
        let (v2, t2) = box_mesh([-25.0, -5.0, 30.0], [25.0, 5.0, 35.0]);
        let base = v.len() as u32;
        v.extend(v2);
        t.extend(t2.into_iter().map(|tri| tri.map(|i| i + base)));
        (v, t)
    }

    #[test]
    fn t_bracket_flips_onto_wide_flange_to_eliminate_overhangs() {
        let (v, t) = t_bracket_mesh();
        let envelope = AabbMm::new([0.0; 3], [220.0; 3]).unwrap();
        let cancel = AtomicBool::new(false);
        let q = choose_rotation_for_mesh(&v, &t, [0.0, 0.0, 0.0, 1.0], envelope, 0.0125, &cancel)
            .unwrap();
        // Either flipped upside-down (flange on bed: +Z -> -Z) or laid flat on its
        // wide 50x35 side (±Y -> -Z); either eliminates the elevated overhang!
        let down = rotate_inv(q, [0.0, 0.0, -1.0]);
        assert!(
            down[2] > 0.99 || down[1].abs() > 0.99,
            "expected flange-down or flat-side pose, got down={down:?}"
        );
    }

    #[test]
    fn already_flat_plate_stays_unchanged() {
        let (v, t) = box_mesh([0.0, 0.0, 0.0], [60.0, 40.0, 8.0]);
        let envelope = AabbMm::new([0.0; 3], [220.0; 3]).unwrap();
        let cancel = AtomicBool::new(false);
        let q = choose_rotation_for_mesh(&v, &t, [0.0, 0.0, 0.0, 1.0], envelope, 0.0125, &cancel)
            .unwrap();
        assert_eq!(q, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn tilted_plate_snaps_flat_onto_bed_from_face_normal_cluster() {
        let (v0, t) = box_mesh([0.0, 0.0, 0.0], [50.0, 40.0, 6.0]);
        // Rotate plate by 37 degrees around X so no cardinal axis is flat!
        let half = 37.0_f64.to_radians() * 0.5;
        let tilt = [half.sin(), 0.0, 0.0, half.cos()];
        let v: Vec<[f64; 3]> = v0.into_iter().map(|p| rotate(tilt, p)).collect();
        let envelope = AabbMm::new([0.0; 3], [220.0; 3]).unwrap();
        let cancel = AtomicBool::new(false);
        let q = choose_rotation_for_mesh(&v, &t, [0.0, 0.0, 0.0, 1.0], envelope, 0.0125, &cancel)
            .unwrap();
        // After auto-orient, the thin Z height should be ~6 mm again!
        let mut z_min = f64::INFINITY;
        let mut z_max = f64::NEG_INFINITY;
        for &p in &v {
            let z = rotate_z(q, p);
            z_min = z_min.min(z);
            z_max = z_max.max(z);
        }
        assert!(
            ((z_max - z_min) - 6.0).abs() < 0.05,
            "expected tilted 6 mm plate to lay flat, got height {:.3} mm",
            z_max - z_min
        );
    }

    #[test]
    fn tall_part_rotates_to_fit_low_z_envelope() {
        let (v, t) = box_mesh([0.0, 0.0, 0.0], [20.0, 20.0, 180.0]);
        let envelope = AabbMm::new([0.0; 3], [220.0, 220.0, 100.0]).unwrap();
        let cancel = AtomicBool::new(false);
        let q = choose_rotation_for_mesh(&v, &t, [0.0, 0.0, 0.0, 1.0], envelope, 0.0125, &cancel)
            .unwrap();
        let mut z_min = f64::INFINITY;
        let mut z_max = f64::NEG_INFINITY;
        for &p in &v {
            let z = rotate_z(q, p);
            z_min = z_min.min(z);
            z_max = z_max.max(z);
        }
        assert!(
            ((z_max - z_min) - 20.0).abs() < 0.05,
            "expected tall part to lay flat inside Z=100 envelope, got {:.3} mm",
            z_max - z_min
        );
    }
}
