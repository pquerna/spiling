// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::thread;

use spiling_contracts::geometry::{
    FaceId, GeometryErrorCode, SectionCurve2d, SectionSpan2d, SourceEdgeId, SourceFaceId,
    StepEntityId,
};
use spiling_geometry::{
    SurfaceParameterMap, from_step_bytes, from_step_bytes_with_limits, make_kernel_context,
};

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
// Challenge 1: SectionCurve2d and SectionSpan2d Serde & Degenerate Cases
// =========================================================================

#[test]
fn challenge1a_section_curve_serde_roundtrip() {
    let line = SectionCurve2d::Line {
        start_mm: [10.5, 20.25],
        end_mm: [30.75, 40.125],
    };
    let arc = SectionCurve2d::CircularArc {
        center_mm: [5.0, 5.0],
        radius_mm: 12.5,
        start_angle_rad: 0.1,
        sweep_angle_rad: std::f64::consts::FRAC_PI_2,
    };
    let ellipse = SectionCurve2d::EllipticArc {
        center_mm: [0.0, 0.0],
        major_axis_mm: [10.0, 0.0],
        minor_radius_mm: 5.0,
        start_param_rad: 0.0,
        end_param_rad: std::f64::consts::PI,
    };
    let nurbs_poly = SectionCurve2d::NurbsSpan {
        degree: 3,
        knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        control_points_mm: vec![[0.0, 0.0], [1.0, 2.0], [3.0, 2.0], [4.0, 0.0]],
        weights: None,
    };
    let nurbs_rat = SectionCurve2d::NurbsSpan {
        degree: 2,
        knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        control_points_mm: vec![[1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        weights: Some(vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0]),
    };

    let curves = vec![line, arc, ellipse, nurbs_poly, nurbs_rat];

    for curve in &curves {
        let json = serde_json::to_string(curve).expect("serialization failed");
        let deserialized: SectionCurve2d =
            serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(
            curve, &deserialized,
            "curve serde roundtrip failed for {json}"
        );
    }
}

#[test]
fn challenge1b_section_span_serde_roundtrip() {
    let span = SectionSpan2d {
        curve: SectionCurve2d::Line {
            start_mm: [0.0, 0.0],
            end_mm: [10.0, 10.0],
        },
        source_face: SourceFaceId::new(42),
        source_edge: Some(SourceEdgeId::new(99)),
        achieved_plane_error_mm: 1.2e-7,
    };
    let json = serde_json::to_string(&span).expect("serialization failed");
    let deserialized: SectionSpan2d = serde_json::from_str(&json).expect("deserialization failed");
    assert_eq!(span, deserialized);
}

#[test]
fn challenge1c_section_curve_degenerate_and_boundary_cases() {
    // 1. Degenerate zero-length line
    let degen_line = SectionCurve2d::Line {
        start_mm: [5.0, 5.0],
        end_mm: [5.0, 5.0],
    };
    assert_eq!(degen_line.start_point_mm(), [5.0, 5.0]);

    // 2. Zero-radius circle
    let zero_arc = SectionCurve2d::CircularArc {
        center_mm: [2.0, 3.0],
        radius_mm: 0.0,
        start_angle_rad: 1.5,
        sweep_angle_rad: 0.0,
    };
    assert_eq!(zero_arc.start_point_mm(), [2.0, 3.0]);

    // 3. Zero major-axis ellipse
    let zero_major_ellipse = SectionCurve2d::EllipticArc {
        center_mm: [7.0, 8.0],
        major_axis_mm: [0.0, 0.0],
        minor_radius_mm: 5.0,
        start_param_rad: 0.0,
        end_param_rad: 1.0,
    };
    assert_eq!(zero_major_ellipse.start_point_mm(), [7.0, 8.0]);

    // 4. Zero minor-radius ellipse
    let zero_minor_ellipse = SectionCurve2d::EllipticArc {
        center_mm: [0.0, 0.0],
        major_axis_mm: [10.0, 0.0],
        minor_radius_mm: 0.0,
        start_param_rad: 0.0,
        end_param_rad: 1.0,
    };
    assert_eq!(zero_minor_ellipse.start_point_mm(), [10.0, 0.0]);

    // 5. Empty NURBS control points (should not panic)
    let empty_nurbs = SectionCurve2d::NurbsSpan {
        degree: 0,
        knots: vec![],
        control_points_mm: vec![],
        weights: None,
    };
    assert_eq!(empty_nurbs.start_point_mm(), [0.0, 0.0]);
    assert!(!empty_nurbs.is_rational());

    // 6. Rational NURBS is_rational flag
    let rat_nurbs = SectionCurve2d::NurbsSpan {
        degree: 1,
        knots: vec![0.0, 0.0, 1.0, 1.0],
        control_points_mm: vec![[0.0, 0.0], [1.0, 1.0]],
        weights: Some(vec![1.0, 2.0]),
    };
    assert!(rat_nurbs.is_rational());
}

#[test]
fn challenge1d_section_curve_malformed_json_rejection() {
    let bad_json_samples = vec![
        r#"{"type":"unknown","start_mm":[0,0],"end_mm":[1,1]}"#,
        r#"{"type":"line","start_mm":[0]}"#,
        r#"{"type":"circular_arc","center_mm":[0,0]}"#,
        r#"{"type":"elliptic_arc","center_mm":[0,0],"major_axis_mm":[1,0]}"#,
        r#"{"type":"nurbs_span","degree":-1}"#,
    ];

    for bad in bad_json_samples {
        let res: Result<SectionCurve2d, _> = serde_json::from_str(bad);
        assert!(
            res.is_err(),
            "malformed json '{bad}' should fail to deserialize"
        );
    }
}

#[test]
fn challenge1e_step_entity_id_zero_edge_case() {
    // StepEntityId(0) represents #0 which is invalid in STEP
    let zero_id = StepEntityId::new(0);
    assert!(
        FaceId::try_from(zero_id).is_err(),
        "FaceId::try_from must reject StepEntityId(0)"
    );
    assert!(
        FaceId::from_step_entity_id(zero_id).is_err(),
        "FaceId::from_step_entity_id must reject StepEntityId(0)"
    );

    // FaceId::parse specifically rejects "step:0" because entity must be nonzero decimal:
    let parse_res = FaceId::parse("step:0");
    assert!(
        parse_res.is_err(),
        "FaceId::parse rejects step:0 because entities must be nonzero"
    );

    // Valid StepEntityId converts and roundtrips through serde:
    let valid_id = StepEntityId::new(42);
    let face_id: FaceId = valid_id.try_into().unwrap();
    assert_eq!(face_id.as_str(), "step:42");
    let json = serde_json::to_string(&face_id).unwrap();
    let deser: FaceId = serde_json::from_str(&json).unwrap();
    assert_eq!(deser, face_id);
}

// =========================================================================
// Challenge 2: Concurrent Multi-Threaded STEP Import & Thread Isolation
// =========================================================================

#[test]
fn challenge2_concurrent_step_import_thread_isolation() {
    let bytes = fixture_bytes("independent-exporters/occt-box-mm.step");
    let thread_count = 12;
    let iterations_per_thread = 5;

    let handles: Vec<_> = (0..thread_count)
        .map(|_| {
            let data = bytes.clone();
            thread::spawn(move || {
                let mut defs = Vec::with_capacity(iterations_per_thread);
                for _ in 0..iterations_per_thread {
                    let def =
                        from_step_bytes(&data).expect("concurrent from_step_bytes must succeed");
                    defs.push(def);
                }
                defs
            })
        })
        .collect();

    let baseline = from_step_bytes(&bytes).expect("baseline import");
    let baseline_id = baseline.id().clone();
    let baseline_bounds = baseline.bounds_mm();
    let baseline_faces = baseline.faces().len();
    let baseline_retained_bytes = baseline.retained_query_proof_bytes();

    for handle in handles {
        let thread_defs = handle.join().expect("thread join must succeed");
        for def in thread_defs {
            assert_eq!(
                def.id(),
                &baseline_id,
                "definition ID must be deterministic across threads"
            );
            assert_eq!(
                def.faces().len(),
                baseline_faces,
                "face count must match baseline"
            );
            assert_eq!(
                def.bounds_mm().min,
                baseline_bounds.min,
                "bounds min must match baseline"
            );
            assert_eq!(
                def.bounds_mm().max,
                baseline_bounds.max,
                "bounds max must match baseline"
            );
            assert_eq!(
                def.retained_query_proof_bytes(),
                baseline_retained_bytes,
                "retained proof bytes must match"
            );
        }
    }
}

// =========================================================================
// Challenge 3: Units Scaling: Dimensional Preservations & Double Scaling Bug
// =========================================================================

#[test]
fn challenge3_units_scaling_reveals_double_scaling_bug() {
    let mm_bytes = fixture_bytes("box-mm.step");
    let inch_bytes = fixture_bytes("box-inch.step");

    let mm_def = from_step_bytes(&mm_bytes).expect("box-mm import");
    let inch_def = from_step_bytes(&inch_bytes).expect("box-inch import");

    let mm_bounds = mm_def.bounds_mm();
    let inch_bounds = inch_def.bounds_mm();

    // 1. Box physical dimensions in mm are 20 x 10 x 8 mm.
    assert!((mm_bounds.min[0] - 0.0).abs() < 1e-5);
    assert!((mm_bounds.min[1] - 0.0).abs() < 1e-5);
    assert!((mm_bounds.min[2] - 0.0).abs() < 1e-5);
    assert!((mm_bounds.max[0] - 20.0).abs() < 1e-5);
    assert!((mm_bounds.max[1] - 10.0).abs() < 1e-5);
    assert!((mm_bounds.max[2] - 8.0).abs() < 1e-5);

    // 2. Query the underlying solidcore B-Rep directly:
    let k = make_kernel_context();
    let mm_bbox = solidcore::query::brep::calculate_tight_bounding_box(&k, mm_def.brep())
        .expect("mm brep bbox");
    let inch_bbox = solidcore::query::brep::calculate_tight_bounding_box(&k, inch_def.brep())
        .expect("inch brep bbox");

    // The solidcore B-Rep produced by solidcraft_step is ALREADY in millimeters!
    assert!((mm_bbox.max().x - 20.0).abs() < 1e-5);
    assert!((mm_bbox.max().y - 10.0).abs() < 1e-5);
    assert!((mm_bbox.max().z - 8.0).abs() < 1e-5);

    assert!(
        (inch_bbox.max().x - 20.0).abs() < 1e-5,
        "solidcore Brep for inch box is ALREADY scaled to 20mm by solidcraft_step, got {}",
        inch_bbox.max().x
    );
    assert!(
        (inch_bbox.max().y - 10.0).abs() < 1e-5,
        "solidcore Brep for inch box is ALREADY scaled to 10mm by solidcraft_step, got {}",
        inch_bbox.max().y
    );
    assert!(
        (inch_bbox.max().z - 8.0).abs() < 1e-5,
        "solidcore Brep for inch box is ALREADY scaled to 8mm by solidcraft_step, got {}",
        inch_bbox.max().z
    );

    // 3. EMPIRICAL DEMONSTRATION OF THE BUG:
    // In import/mod.rs lines 1151-1159:
    // bounds = AabbMm::new([bbox.min * scale], [bbox.max * scale]);
    // Because scale is 25.4 and bbox was already in mm (20x10x8),
    // inch_def.bounds_mm() is erroneously multiplied by 25.4 a SECOND time:
    // 20.0 * 25.4 = 508.0 mm!
    // 10.0 * 25.4 = 254.0 mm!
    // 3. Physical bounding box verification:
    // In import/mod.rs, bounds are constructed directly from solidcore's metric bbox.
    // Both box-mm and box-inch must have identical physical bounding boxes [20, 10, 8] mm.
    assert!(
        (inch_bounds.max[0] - 20.0).abs() < 1e-4,
        "Inch box bounds max[0] must be 20.0 mm, got {}",
        inch_bounds.max[0]
    );
    assert!(
        (inch_bounds.max[1] - 10.0).abs() < 1e-4,
        "Inch box bounds max[1] must be 10.0 mm, got {}",
        inch_bounds.max[1]
    );
    assert!(
        (inch_bounds.max[2] - 8.0).abs() < 1e-4,
        "Inch box bounds max[2] must be 8.0 mm, got {}",
        inch_bounds.max[2]
    );
    for i in 0..3 {
        assert!((inch_bounds.min[i] - mm_bounds.min[i]).abs() < 1e-5);
        assert!((inch_bounds.max[i] - mm_bounds.max[i]).abs() < 1e-5);
    }
}

// =========================================================================
// Challenge 4: Extreme Resource Budgets & Capacity Rejection
// =========================================================================

#[test]
fn challenge4a_import_face_limits_and_rejection() {
    let bytes = fixture_bytes("box-mm.step");
    let cancel = AtomicBool::new(false);

    // 1. Face limit less than solid's 6 faces: must reject with ResourceLimit
    let err = from_step_bytes_with_limits(&bytes, &cancel, 3, 32 * 1024 * 1024).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::ResourceLimit);

    // 2. Face limit 0: must reject with ResourceLimit
    let err = from_step_bytes_with_limits(&bytes, &cancel, 0, 32 * 1024 * 1024).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::ResourceLimit);

    // 3. Sufficient face limit (e.g. 6): must succeed
    let def = from_step_bytes_with_limits(&bytes, &cancel, 6, 32 * 1024 * 1024).unwrap();
    assert_eq!(def.faces().len(), 6);
}

#[test]
fn challenge4b_import_query_proof_byte_limits() {
    let bytes = fixture_bytes("box-mm.step");
    let cancel = AtomicBool::new(false);

    // 1. Unconstrained limit: inspect actual retained bytes
    let def = from_step_bytes(&bytes).expect("import box-mm");
    let actual_bytes = def.retained_query_proof_bytes();

    // 2. Setting limit below actual retained bytes: must reject with ResourceLimit
    let err = from_step_bytes_with_limits(
        &bytes,
        &cancel,
        spiling_contracts::geometry::MAX_NATIVE_FACES,
        actual_bytes / 2,
    )
    .unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::ResourceLimit);
}

// =========================================================================
// Challenge 5: Adversarial STEP Corpus Rejection
// =========================================================================

#[test]
fn challenge5_adversarial_step_corpus_rejection() {
    let fixtures = &[
        ("missing-units.step", GeometryErrorCode::UnsupportedUnits),
        (
            "conflicting-units.step",
            GeometryErrorCode::UnsupportedUnits,
        ),
        ("missing-reference.step", GeometryErrorCode::InvalidGeometry),
        ("open-shell.step", GeometryErrorCode::InvalidGeometry),
        (
            "non-manifold-shell.step",
            GeometryErrorCode::InvalidGeometry,
        ),
        (
            "reachable-unsupported.step",
            GeometryErrorCode::UnsupportedGeometry,
        ),
        ("assembly.step", GeometryErrorCode::UnsupportedGeometry),
        (
            "nonfinite-coordinate.step",
            GeometryErrorCode::InvalidGeometry,
        ),
        ("truncated.step", GeometryErrorCode::InvalidGeometry),
        (
            "wrong-conversion-dimension.step",
            GeometryErrorCode::UnsupportedUnits,
        ),
        ("recursive-units.step", GeometryErrorCode::UnsupportedUnits),
        ("degrees.step", GeometryErrorCode::UnsupportedUnits),
        (
            "duplicate-face-use.step",
            GeometryErrorCode::InvalidGeometry,
        ),
    ];

    for &(name, expected_code) in fixtures {
        let bytes = fixture_bytes(name);
        let res = from_step_bytes(&bytes);
        assert!(
            res.is_err(),
            "adversarial fixture {name} must be rejected, but succeeded!"
        );
        let err = res.unwrap_err();
        assert_eq!(
            err.code, expected_code,
            "adversarial fixture {name} expected error code {expected_code:?}, but got {err:?}"
        );
        assert!(
            err.message.len() <= spiling_contracts::geometry::MAX_ERROR_MESSAGE_BYTES as usize,
            "error message too long: {}",
            err.message.len()
        );
    }
}

// =========================================================================
// Challenge 6: SurfaceParameterMap Invertibility & Normalization
// =========================================================================

#[test]
fn challenge6_surface_parameter_map_roundtrip() {
    let maps = vec![
        SurfaceParameterMap::Identity,
        SurfaceParameterMap::ScaleOffset {
            u_scale: 180.0 / std::f64::consts::PI,
            u_offset: 0.5,
            v_scale: 1.0,
            v_offset: 0.0,
        },
        SurfaceParameterMap::ScaleOffset {
            u_scale: 180.0 / std::f64::consts::PI,
            u_offset: -std::f64::consts::PI,
            v_scale: 180.0 / std::f64::consts::PI,
            v_offset: 0.25,
        },
    ];

    let test_points = vec![
        [0.0, 0.0],
        [1.0, 2.5],
        [-std::f64::consts::PI, std::f64::consts::FRAC_PI_2],
    ];

    for map in &maps {
        for &pt in &test_points {
            let core = map.step_to_core(pt);
            let back = map.core_to_step(core);
            assert!(
                (back[0] - pt[0]).abs() < 1e-12,
                "u coordinate roundtrip failed: orig={pt:?} core={core:?} back={back:?}"
            );
            assert!(
                (back[1] - pt[1]).abs() < 1e-12,
                "v coordinate roundtrip failed: orig={pt:?} core={core:?} back={back:?}"
            );
        }
    }
}
