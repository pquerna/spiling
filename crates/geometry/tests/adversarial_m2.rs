// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use solidcore::geom::vector3d::{Point3, Vec3};
use solidcore::nurbs::knots::Side;
use solidcore::nurbs::math::NOW;
use solidcore::nurbs::srf_basic::{CPoint, Surface as SrfSurface};
use solidcore::surface::plane::Plane as SolidPlane;
use solidcore::surface::surface::p2::{Surface as SolidSurface, SurfaceKind};
use spiling_contracts::geometry::{FaceId, GeometryErrorCode, PlaneMm};
use spiling_geometry::parametric::*;
use spiling_geometry::{from_step_bytes, GeometryWorkBudget};

fn fixture_path(rel_path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
        .join("geometry")
        .join(rel_path)
}

fn fixture_bytes(rel_path: &str) -> Vec<u8> {
    let p = fixture_path(rel_path);
    std::fs::read(&p).unwrap_or_else(|e| panic!("failed to read fixture at {:?}: {e}", p))
}

// =========================================================================
// Challenge 1: IEEE-754 Outward-Rounded Interval Arithmetic (Features 13, 14)
// =========================================================================

#[test]
fn test_adversarial_interval_ieee754_outward_rounding() {
    // 1. Float stepping guarantees
    let x = 1.0_f64;
    assert!(interval::down(x) < x);
    assert!(interval::up(x) > x);
    assert_eq!(interval::next_after(x, 2.0), interval::up(x));
    assert_eq!(interval::next_after(x, 0.0), interval::down(x));

    // Zero and subnormal stepping
    assert!(interval::down(0.0) < 0.0);
    assert!(interval::up(0.0) > 0.0);

    // 2. Outward rounding containment for basic operations
    let a = Interval::new(1.0, 2.0).unwrap();
    let b = Interval::new(3.0, 4.0).unwrap();

    let sum = a.add(b);
    assert!(sum.lo <= 4.0);
    assert!(sum.hi >= 6.0);

    let sub = a.sub(b);
    assert!(sub.lo <= -3.0);
    assert!(sub.hi >= -1.0);

    let mul = a.mul(b);
    assert!(mul.lo <= 3.0);
    assert!(mul.hi >= 8.0);

    let div = a.div(b).unwrap();
    assert!(div.lo <= 1.0 / 4.0);
    assert!(div.hi >= 2.0 / 3.0);

    // 3. Domain error guards
    let zero_crossing = Interval::new(-1.0, 1.0).unwrap();
    assert_eq!(
        a.div(zero_crossing).unwrap_err().code,
        GeometryErrorCode::ArithmeticDomain
    );
    assert_eq!(
        zero_crossing.reciprocal().unwrap_err().code,
        GeometryErrorCode::ArithmeticDomain
    );

    let negative_interval = Interval::new(-4.0, -1.0).unwrap();
    assert_eq!(
        negative_interval.sqrt().unwrap_err().code,
        GeometryErrorCode::ArithmeticDomain
    );

    // 4. Square and Sqrt correctness
    let containing_zero = Interval::new(-3.0, 2.0).unwrap();
    let sq = containing_zero.square();
    assert_eq!(sq.lo, 0.0);
    assert!(sq.hi >= 9.0);

    let positive_square = Interval::new(4.0, 9.0).unwrap();
    let sqrt_res = positive_square.sqrt().unwrap();
    assert!(sqrt_res.lo <= 2.0);
    assert!(sqrt_res.hi >= 3.0);
}

#[test]
fn test_adversarial_twosum_exact_cancellation() {
    // Large values that would suffer floating-point cancellation
    let val = 1e16;
    let p1 = Interval::point(val);
    let p2 = Interval::point(val);

    let diff = p1.sub(p2);
    // TwoSum exact point cancellation must yield exact 0.0!
    assert_eq!(diff.lo, 0.0);
    assert_eq!(diff.hi, 0.0);
    assert_eq!(diff.width(), 0.0);
    assert_eq!(diff.mid(), 0.0);

    let sum = p1.add(Interval::point(-val));
    assert_eq!(sum.lo, 0.0);
    assert_eq!(sum.hi, 0.0);
}

#[test]
fn test_adversarial_trigonometric_intervals() {
    // 1. Quarter periods
    let q1 = Interval::new(0.0, 0.5 * PI).unwrap();
    let (sin_q1, cos_q1) = sin_cos_interval(q1);
    assert!(sin_q1.lo <= 0.0 && sin_q1.hi >= 1.0);
    assert!(cos_q1.lo <= 0.0 && cos_q1.hi >= 1.0);

    let q2 = Interval::new(0.5 * PI, PI).unwrap();
    let (sin_q2, cos_q2) = sin_cos_interval(q2);
    assert!(sin_q2.lo <= 0.0 && sin_q2.hi >= 1.0);
    assert!(cos_q2.lo <= -1.0 && cos_q2.hi >= 0.0);

    // 2. Full period and multiple periods
    let full = Interval::new(0.0, 2.0 * PI).unwrap();
    let (sin_full, cos_full) = sin_cos_interval(full);
    assert_eq!(sin_full.lo, -1.0);
    assert_eq!(sin_full.hi, 1.0);
    assert_eq!(cos_full.lo, -1.0);
    assert_eq!(cos_full.hi, 1.0);

    let multi = Interval::new(-10.0 * PI, 10.0 * PI).unwrap();
    let (sin_m, cos_m) = sin_cos_interval(multi);
    assert_eq!(sin_m.lo, -1.0);
    assert_eq!(sin_m.hi, 1.0);
    assert_eq!(cos_m.lo, -1.0);
    assert_eq!(cos_m.hi, 1.0);

    // 3. Small interval around extremum
    let peak = Interval::new(0.4 * PI, 0.6 * PI).unwrap();
    let (sin_peak, cos_peak) = sin_cos_interval(peak);
    assert_eq!(sin_peak.hi, 1.0);
    assert!(cos_peak.contains(0.0));
}

#[test]
fn test_adversarial_interval_vec3_and_norm_safety() {
    // 1. Vector addition, scaling, dot and cross
    let u = IntervalVec3::point(1.0, 0.0, 0.0);
    let v = IntervalVec3::point(0.0, 1.0, 0.0);

    let dot = u.dot(v);
    assert!(dot.contains(0.0));

    let cross = u.cross(v);
    assert!(cross.x.contains(0.0));
    assert!(cross.y.contains(0.0));
    assert!(cross.z.contains(1.0));

    // Orthogonality: u . (u x v) must contain 0.0
    let ortho = u.dot(cross);
    assert!(ortho.contains(0.0));

    // 2. Norm squared and Norm non-negativity guarantees
    let zero_v = IntervalVec3::point(0.0, 0.0, 0.0);
    let norm_zero = zero_v.norm().unwrap();
    assert!(norm_zero.lo >= 0.0);
    assert!(norm_zero.hi >= 0.0);

    let wide_v = IntervalVec3::new(
        Interval::new(-2.0, 2.0).unwrap(),
        Interval::new(-3.0, 3.0).unwrap(),
        Interval::new(-6.0, 6.0).unwrap(),
    );
    let norm_wide = wide_v.norm().unwrap();
    assert!(norm_wide.lo >= 0.0);
    assert!(norm_wide.hi >= 7.0);
}

// =========================================================================
// Challenge 2: Lock-Free Atomic GeometryWorkBudget Debiting (Feature 15)
// =========================================================================

#[test]
fn test_adversarial_atomic_work_budget_concurrency() {
    let initial_budget = 50_000_u64;
    let budget = Arc::new(GeometryWorkBudget::new(initial_budget));
    let cancel = Arc::new(AtomicBool::new(false));

    let num_threads = 10;
    let debits_per_thread = 500;
    let debit_amount = 10_u64;

    let mut handles = Vec::new();
    for _ in 0..num_threads {
        let b = Arc::clone(&budget);
        let c = Arc::clone(&cancel);
        handles.push(thread::spawn(move || {
            let mut success_count = 0;
            for _ in 0..debits_per_thread {
                if b.charge(debit_amount, &c).is_ok() {
                    success_count += 1;
                }
            }
            success_count
        }));
    }

    let mut total_successful_debits = 0;
    for h in handles {
        total_successful_debits += h.join().unwrap();
    }

    let expected_consumed = total_successful_debits as u64 * debit_amount;
    assert_eq!(
        budget.consumed(),
        expected_consumed,
        "atomic debiting must accurately count consumed units"
    );
    assert_eq!(
        budget.remaining(),
        initial_budget - expected_consumed,
        "remaining budget must be exact"
    );

    // Now exhaust budget completely
    let remaining = budget.remaining();
    assert!(budget.charge(remaining, &cancel).is_ok());
    assert_eq!(budget.remaining(), 0);

    // Next charge must fail with ResourceLimit
    let err = budget.charge(1, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::ResourceLimit);

    // Cancellation check
    cancel.store(true, Ordering::Relaxed);
    let cancel_err = budget.charge(0, &cancel).unwrap_err();
    assert_eq!(cancel_err.code, GeometryErrorCode::Cancelled);
}

// =========================================================================
// Challenge 3: Translation Invariance of NURBS Jets (Features 16, 17)
// =========================================================================

fn make_bicubic_saddle_patch(offset: [f64; 3]) -> SrfSurface {
    let p = 3;
    let q = 3;
    let u_knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let v_knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];

    // 4x4 control net with saddle curvature: z = (x^2 - y^2) / 100
    let mut net = Vec::with_capacity(4);
    for i in 0..4 {
        let mut row = Vec::with_capacity(4);
        let u_val = i as f64 / 3.0 * 20.0 - 10.0;
        for j in 0..4 {
            let v_val = j as f64 / 3.0 * 20.0 - 10.0;
            let z_val = (u_val * u_val - v_val * v_val) / 100.0;
            row.push(CPoint::new(
                offset[0] + u_val,
                offset[1] + v_val,
                offset[2] + z_val,
                NOW,
            ));
        }
        net.push(row);
    }
    SrfSurface::new(net, p, q, u_knots, v_knots).unwrap()
}

#[test]
fn test_adversarial_nurbs_translation_invariance_1km() {
    let origin_patch = make_bicubic_saddle_patch([0.0, 0.0, 0.0]);
    // Translate 1 km (10^6 mm) along all axes
    let km = 1_000_000.0;
    let translated_patch = make_bicubic_saddle_patch([km, -km, km]);

    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);

    let uv_box = [[0.25, 0.75], [0.25, 0.75]];

    let enc_orig = evaluate_nurbs_surface_jet(&origin_patch, uv_box, &mut work, &cancel).unwrap();
    let enc_trans =
        evaluate_nurbs_surface_jet(&translated_patch, uv_box, &mut work, &cancel).unwrap();

    // 1st derivatives MUST be identical regardless of coordinate origin
    for k in 0..3 {
        assert!(
            (enc_orig.du[k].lo - enc_trans.du[k].lo).abs() < 1e-12,
            "du[{k}].lo mismatch under translation"
        );
        assert!(
            (enc_orig.du[k].hi - enc_trans.du[k].hi).abs() < 1e-12,
            "du[{k}].hi mismatch under translation"
        );
        assert!(
            (enc_orig.dv[k].lo - enc_trans.dv[k].lo).abs() < 1e-12,
            "dv[{k}].lo mismatch under translation"
        );
        assert!(
            (enc_orig.dv[k].hi - enc_trans.dv[k].hi).abs() < 1e-12,
            "dv[{k}].hi mismatch under translation"
        );
    }

    // 2nd derivatives MUST be identical
    for k in 0..3 {
        assert!((enc_orig.duu[k].lo - enc_trans.duu[k].lo).abs() < 1e-12);
        assert!((enc_orig.dvv[k].lo - enc_trans.dvv[k].lo).abs() < 1e-12);
        assert!((enc_orig.duv[k].lo - enc_trans.duv[k].lo).abs() < 1e-12);
    }

    // Position interval width must not swell due to float cancellation (< 1e-8 mm)
    for k in 0..3 {
        let width_diff = (enc_orig.position[k].width() - enc_trans.position[k].width()).abs();
        assert!(
            width_diff < 1e-8,
            "position[{k}] enclosure width swelled by {width_diff} mm"
        );
    }
}

// =========================================================================
// Challenge 4: Bezier Derivative Bound Convex Containment (Feature 17)
// =========================================================================

#[test]
fn test_adversarial_bezier_derivative_containment_dense_sampling() {
    let patch = make_bicubic_saddle_patch([0.0, 0.0, 0.0]);
    let uv_domain = [[0.1, 0.9], [0.1, 0.9]];

    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);

    let enc = evaluate_nurbs_surface_jet(&patch, uv_domain, &mut work, &cancel).unwrap();

    // Sample dense 15x15 grid of points and verify all fall inside enc.position
    let grid_size = 15;
    for i in 0..=grid_size {
        let u = 0.1 + (0.9 - 0.1) * (i as f64 / grid_size as f64);
        for j in 0..=grid_size {
            let v = 0.1 + (0.9 - 0.1) * (j as f64 / grid_size as f64);

            let pt = patch.eval(u, v, Side::Left, Side::Left).unwrap();
            assert!(
                enc.position[0].contains(pt.x),
                "pt.x {x} outside position[0]",
                x = pt.x
            );
            assert!(
                enc.position[1].contains(pt.y),
                "pt.y {y} outside position[1]",
                y = pt.y
            );
            assert!(
                enc.position[2].contains(pt.z),
                "pt.z {z} outside position[2]",
                z = pt.z
            );

            // Numerical derivative approximations with small step h
            let h = 1e-5;
            if u + h <= 0.9 && u - h >= 0.1 {
                let pt_plus = patch.eval(u + h, v, Side::Left, Side::Left).unwrap();
                let pt_minus = patch.eval(u - h, v, Side::Left, Side::Left).unwrap();
                let num_du = [
                    (pt_plus.x - pt_minus.x) / (2.0 * h),
                    (pt_plus.y - pt_minus.y) / (2.0 * h),
                    (pt_plus.z - pt_minus.z) / (2.0 * h),
                ];
                for axis in 0..3 {
                    assert!(
                        enc.du[axis].lo <= num_du[axis] + 1e-4
                            && enc.du[axis].hi >= num_du[axis] - 1e-4,
                        "numerical du[{axis}] {num_du:?} outside enc.du"
                    );
                }
            }

            if v + h <= 0.9 && v - h >= 0.1 {
                let pt_plus = patch.eval(u, v + h, Side::Left, Side::Left).unwrap();
                let pt_minus = patch.eval(u, v - h, Side::Left, Side::Left).unwrap();
                let num_dv = [
                    (pt_plus.x - pt_minus.x) / (2.0 * h),
                    (pt_plus.y - pt_minus.y) / (2.0 * h),
                    (pt_plus.z - pt_minus.z) / (2.0 * h),
                ];
                for axis in 0..3 {
                    assert!(
                        enc.dv[axis].lo <= num_dv[axis] + 1e-4
                            && enc.dv[axis].hi >= num_dv[axis] - 1e-4,
                        "numerical dv[{axis}] {num_dv:?} outside enc.dv"
                    );
                }
            }
        }
    }
}

// =========================================================================
// Challenge 5: PreparedValidatedCurve over solidcore B-Rep Edges (Feature 19)
// =========================================================================

#[test]
fn test_adversarial_prepared_validated_curve_box_edges() {
    let cancel = AtomicBool::new(false);
    let bytes = fixture_bytes("box-mm.step");
    let definition = from_step_bytes(&bytes).unwrap();

    let mut work = GeometryWorkBudget::new(10_000);

    let edges = definition.brep().edges();
    assert!(!edges.is_empty(), "box-mm must have edges");

    for (_, edge) in edges.iter() {
        if let Some(curve) = edge.curve.as_ref() {
            let prepared = PreparedValidatedCurve::new(
                curve,
                [0.0, 1.0],
                1.0,
                32 * 1024 * 1024,
                &mut work,
                &cancel,
            )
            .unwrap();

            // Evaluate mid point
            let mid_pt = prepared.evaluate(0.5, &mut work, &cancel).unwrap();

            // Evaluate enclosure jet
            let enc = prepared
                .jet([0.0, 1.0], &mut work, &cancel)
                .unwrap();

            // Verify mid point is inside enclosure
            assert!(enc.position[0].contains(mid_pt.position[0]));
            assert!(enc.position[1].contains(mid_pt.position[1]));
            assert!(enc.position[2].contains(mid_pt.position[2]));

            // Smoothness qualification
            assert!(prepared.is_smooth_on([0.0, 1.0], &mut work, &cancel).unwrap());

            // Projected jet along X axis
            let proj = prepared
                .projected_jet([0.0, 1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &mut work, &cancel)
                .unwrap();
            assert!(proj.position.contains(mid_pt.position[0]));
        }
    }
}

// =========================================================================
// Challenge 6: PreparedFacePlaneRoots Isolation & Coplane (Features 20, 21)
// =========================================================================

#[test]
fn test_adversarial_coplane_classification() {
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);

    // Carrier plane: Z = 50.0, normal = [0, 0, 1]
    let normal_vec = Vec3::new(0.0, 0.0, 1.0);
    let pt_vec = Point3::new(10.0, 20.0, 50.0);
    let plane_surf = SolidSurface::new(SurfaceKind::Plane(SolidPlane::from_normal(pt_vec, normal_vec)));

    // 1. Same normal, distance = 0
    let q_same = PlaneMm {
        origin_mm: [0.0, 0.0, 50.0],
        normal: [0.0, 0.0, 1.0],
    };
    assert_eq!(
        classify_face_coplane(true, &plane_surf, &q_same, 1e-5, 1e-4, &mut work, &cancel).unwrap(),
        CoplaneClassification::CoplanarSameNormal
    );

    // 2. Opposite normal, distance = 0
    let q_opp = PlaneMm {
        origin_mm: [0.0, 0.0, 50.0],
        normal: [0.0, 0.0, -1.0],
    };
    assert_eq!(
        classify_face_coplane(true, &plane_surf, &q_opp, 1e-5, 1e-4, &mut work, &cancel).unwrap(),
        CoplaneClassification::CoplanarOppositeNormal
    );

    // 3. Parallel disjoint (Z = 51.0, distance = 1.0 mm)
    let q_parallel = PlaneMm {
        origin_mm: [0.0, 0.0, 51.0],
        normal: [0.0, 0.0, 1.0],
    };
    assert_eq!(
        classify_face_coplane(true, &plane_surf, &q_parallel, 1e-5, 1e-4, &mut work, &cancel).unwrap(),
        CoplaneClassification::ParallelDisjoint
    );

    // 4. Transverse (X = 0.0, normal = [1, 0, 0])
    let q_trans = PlaneMm {
        origin_mm: [0.0, 0.0, 0.0],
        normal: [1.0, 0.0, 0.0],
    };
    assert_eq!(
        classify_face_coplane(true, &plane_surf, &q_trans, 1e-5, 1e-4, &mut work, &cancel).unwrap(),
        CoplaneClassification::Transverse
    );

    // 5. Within angular and positional tolerance:
    // angle 5e-6 rad (< COPLANE_ANGULAR_TOLERANCE 1e-5), distance 5e-5 mm (< 1e-4 mm)
    let eps_ang = 5e-6;
    let normal_pert = Vec3::new(eps_ang, 0.0, (1.0 - eps_ang * eps_ang).sqrt());
    let pt_pert = Point3::new(0.0, 0.0, 50.0 + 5e-5);
    let plane_pert = SolidSurface::new(SurfaceKind::Plane(SolidPlane::from_normal(pt_pert, normal_pert)));
    assert_eq!(
        classify_face_coplane(true, &plane_pert, &q_same, 1e-5, 1e-4, &mut work, &cancel).unwrap(),
        CoplaneClassification::CoplanarSameNormal
    );

    // 6. Outside positional tolerance (distance 2e-4 mm > 1e-4 mm) -> ParallelDisjoint
    let pt_far = Point3::new(0.0, 0.0, 50.0 + 2e-4);
    let plane_far = SolidSurface::new(SurfaceKind::Plane(SolidPlane::from_normal(pt_far, normal_vec)));
    assert_eq!(
        classify_face_coplane(true, &plane_far, &q_same, 1e-5, 1e-4, &mut work, &cancel).unwrap(),
        CoplaneClassification::ParallelDisjoint
    );
}

#[test]
fn test_adversarial_face_plane_roots_isolation() {
    let cancel = AtomicBool::new(false);
    let bytes = fixture_bytes("box-mm.step");
    let definition = from_step_bytes(&bytes).unwrap();

    let mut work = GeometryWorkBudget::new(20_000);

    let faces = definition.brep().faces();
    let (&face_id, face) = faces.iter().next().unwrap();
    let surface = face.surface.as_ref().unwrap();

    // 1. Plane completely disjoint from the box (Z = 1000 mm)
    let disjoint_plane = PlaneMm {
        origin_mm: [0.0, 0.0, 1000.0],
        normal: [0.0, 0.0, 1.0],
    };

    let roots_disjoint = PreparedFacePlaneRoots::new(
        face_id,
        surface,
        true,
        [[0.0, 1.0], [0.0, 1.0]],
        &disjoint_plane,
        &[],
        32 * 1024 * 1024,
        &mut work,
        &cancel,
    )
    .unwrap();

    assert_eq!(roots_disjoint.relation, FacePlaneRelation::Excluded);
    assert_eq!(roots_disjoint.boundary_roots.len(), 0);
}
