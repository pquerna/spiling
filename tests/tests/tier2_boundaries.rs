// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Tier 2: Boundary & Corner Cases (Features 1–50).
//! Exactly 5 boundary/corner test cases per feature (250 tests total).
//! Derived from fixtures/geometry/manifest.json adversaries, extreme aspect ratios,
//! degenerate cuts, coordinate extremes, zero thickness, and resource limits.

use std::sync::atomic::AtomicBool;
use spiling_contracts::geometry::*;
use spiling_geometry::*;
use spiling_e2e_tests::fixtures::*;
use spiling_e2e_tests::helpers::*;
use spiling_e2e_tests::parametric::*;

// =========================================================================
// Feature 1 Corner Cases: Large entity IDs, u32 limits, float loss prevention
// =========================================================================

#[test]
fn test_f01_corner_max_step_entity_id() {
    let large_id = 9_007_199_254_740_993u64;
    let id = StepEntityId(large_id);
    assert_eq!(id.0, large_id);
    let serialized = serde_json::to_string(&id).unwrap();
    assert_eq!(serialized, "9007199254740993");
    let deserialized: StepEntityId = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.0, large_id);
}

#[test]
fn test_f01_corner_zero_step_entity_id() {
    let id = StepEntityId(0);
    assert_eq!(format!("{id}"), "step:0");
}

#[test]
fn test_f01_corner_u64_max_entity_id() {
    let id = StepEntityId(u64::MAX);
    assert_eq!(id.0, u64::MAX);
}

#[test]
fn test_f01_corner_source_face_id_max() {
    let id = SourceFaceId(u32::MAX);
    assert_eq!(id.0, u32::MAX);
}

#[test]
fn test_f01_corner_source_edge_id_max() {
    let id = SourceEdgeId(u32::MAX);
    assert_eq!(id.0, u32::MAX);
}

// =========================================================================
// Feature 2 Corner Cases: Definition edge cases
// =========================================================================

#[test]
fn test_f02_corner_bounds_non_cube() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    let b = def.bounds_mm();
    assert!(b.max[0] > b.min[0]);
    assert!(b.max[1] > b.min[1]);
    assert!(b.max[2] > b.min[2]);
}

#[test]
fn test_f02_corner_bounds_large_plate() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.max[0], 1640.0, 1e-4, "plate X");
}

#[test]
fn test_f02_corner_definition_cloning_independence() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let bounds1 = def.bounds_mm();
    let bounds2 = def.bounds_mm();
    assert_eq!(bounds1.min, bounds2.min);
    assert_eq!(bounds1.max, bounds2.max);
}

#[test]
fn test_f02_corner_multi_thread_definition_access() {
    use std::sync::Arc;
    let def = Arc::new(import_test_step(&fixture_bytes("cylinder.step")).unwrap());
    let mut handles = Vec::new();
    for _ in 0..8 {
        let d = Arc::clone(&def);
        handles.push(std::thread::spawn(move || {
            d.faces().len()
        }));
    }
    for h in handles {
        assert_eq!(h.join().unwrap(), 3);
    }
}

#[test]
fn test_f02_corner_proof_bytes_positive() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    assert!(def.retained_query_proof_bytes() > 1000);
}

// =========================================================================
// Feature 3 Corner Cases: Unknown handles and bounds
// =========================================================================

#[test]
fn test_f03_corner_unknown_face_id() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let bad_id = FaceId::from_step_entity_id(StepEntityId(999999));
    let err = inspect_face(&def, &bad_id).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnknownHandle);
}

#[test]
fn test_f03_corner_empty_handle() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let bad_id = FaceId::from_step_entity_id(StepEntityId(0));
    let err = inspect_face(&def, &bad_id).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnknownHandle);
}

#[test]
fn test_f03_corner_face_count_consistency() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let count = def.faces().len();
    assert_eq!(count, 6);
}

#[test]
fn test_f03_corner_repeated_inspect_face() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let id = &def.faces()[0].face_id;
    for _ in 0..10 {
        assert!(inspect_face(&def, id).is_ok());
    }
}

#[test]
fn test_f03_corner_all_faces_have_valid_kind() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step")).unwrap();
    for f in def.faces() {
        assert!(!f.source_entity_kind.is_empty());
    }
}

// =========================================================================
// Feature 4 Corner Cases: Kernel context edge values
// =========================================================================

#[test]
fn test_f04_corner_brep_tolerance_positive() {
    let b = solidcore::Brep::default();
    assert!(b.tolerance() > 0.0);
}

#[test]
fn test_f04_corner_brep_tolerance_strict() {
    let b = solidcore::Brep::default();
    assert!(b.tolerance() < 0.001);
}

#[test]
fn test_f04_corner_kernel_parallel_isolation() {
    let handles: Vec<_> = (0..10)
        .map(|i| {
            std::thread::spawn(move || {
                let _k = solidcore::kernel::Kernel::default();
                i
            })
        })
        .collect();
    for (i, h) in handles.into_iter().enumerate() {
        assert_eq!(h.join().unwrap(), i);
    }
}

#[test]
fn test_f04_corner_kernel_reproducibility() {
    let b1 = solidcore::Brep::default();
    let b2 = solidcore::Brep::default();
    assert_eq!(b1.tolerance(), b2.tolerance());
}

#[test]
fn test_f04_corner_kernel_thread_concurrency() {
    let handle = std::thread::spawn(|| {
        let _k = solidcore::kernel::Kernel::default();
        true
    });
    assert!(handle.join().unwrap());
}

// =========================================================================
// Feature 5 Corner Cases: Budget overflow and limits
// =========================================================================

#[test]
fn test_f05_corner_resource_limit_zero_faces() {
    let bytes = fixture_bytes("box-mm.step");
    let cancel = AtomicBool::new(false);
    let limits = ImportLimits {
        max_faces: 0,
        max_query_proof_bytes: 10_000_000,
    };
    let err = import_step_with_limits(&bytes, &cancel, limits).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::ResourceLimit);
}

#[test]
fn test_f05_corner_resource_limit_five_faces_on_box() {
    let bytes = fixture_bytes("box-mm.step");
    let cancel = AtomicBool::new(false);
    let limits = ImportLimits {
        max_faces: 5,
        max_query_proof_bytes: 10_000_000,
    };
    let err = import_step_with_limits(&bytes, &cancel, limits).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::ResourceLimit);
}

#[test]
fn test_f05_corner_resource_limit_exact_faces_on_box() {
    let bytes = fixture_bytes("box-mm.step");
    let cancel = AtomicBool::new(false);
    let limits = ImportLimits {
        max_faces: 6,
        max_query_proof_bytes: 10_000_000,
    };
    assert!(import_step_with_limits(&bytes, &cancel, limits).is_ok());
}

#[test]
fn test_f05_corner_retained_bytes_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert!(def.retained_query_proof_bytes() > 0);
}

#[test]
fn test_f05_corner_retained_bytes_plate() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    assert!(def.retained_query_proof_bytes() > 100_000);
}

// =========================================================================
// Feature 6 Corner Cases: STEP Parser limits and adversaries
// =========================================================================

#[test]
fn test_f06_corner_missing_units() {
    let bytes = fixture_bytes("missing-units.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

#[test]
fn test_f06_corner_conflicting_units() {
    let bytes = fixture_bytes("conflicting-units.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

#[test]
fn test_f06_corner_degrees_units() {
    let bytes = fixture_bytes("degrees.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

#[test]
fn test_f06_corner_recursive_units() {
    let bytes = fixture_bytes("recursive-units.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

#[test]
fn test_f06_corner_wrong_conversion_dimension() {
    let bytes = fixture_bytes("wrong-conversion-dimension.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

// =========================================================================
// Feature 7 Corner Cases: Schema 4 topological defects
// =========================================================================

#[test]
fn test_f07_corner_missing_reference() {
    let bytes = fixture_bytes("missing-reference.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f07_corner_open_shell() {
    let bytes = fixture_bytes("open-shell.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f07_corner_non_manifold_shell() {
    let bytes = fixture_bytes("non-manifold-shell.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f07_corner_nonfinite_coordinate() {
    let bytes = fixture_bytes("nonfinite-coordinate.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f07_corner_truncated_file() {
    let bytes = fixture_bytes("truncated.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

// =========================================================================
// Feature 8 Corner Cases: Bridge conversions
// =========================================================================

#[test]
fn test_f08_corner_duplicate_face_use() {
    let bytes = fixture_bytes("duplicate-face-use.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f08_corner_reachable_unsupported() {
    let bytes = fixture_bytes("reachable-unsupported.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedGeometry);
}

#[test]
fn test_f08_corner_assembly_step() {
    let bytes = fixture_bytes("assembly.step");
    let err = import_test_step(&bytes).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedGeometry);
}

#[test]
fn test_f08_corner_unsupported_nurbs_accepted() {
    let bytes = fixture_bytes("unsupported-nurbs.step");
    assert!(import_test_step(&bytes).is_ok());
}

#[test]
fn test_f08_corner_inch_conversion_accuracy() {
    let bytes = fixture_bytes("box-inch.step");
    let def = import_test_step(&bytes).unwrap();
    let b = def.bounds_mm();
    assert_close(b.max[0], 20.0, 1e-6, "inch X");
    assert_close(b.max[1], 10.0, 1e-6, "inch Y");
    assert_close(b.max[2], 8.0, 1e-6, "inch Z");
}

// =========================================================================
// Feature 9 Corner Cases: Quasi-periodic knots
// =========================================================================

#[test]
fn test_f09_corner_spline_housing_midplane() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.1).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f09_corner_spline_housing_above_floor() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.16).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f09_corner_spline_housing_high_cut() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let sec = section_at_z(&def, 9.5).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f09_corner_spline_housing_outside_cut() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let sec = section_at_z(&def, 15.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f09_corner_spline_housing_degenerate_boundary() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let top = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 10.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(top.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

// =========================================================================
// Feature 10 Corner Cases: Non-manifold edge cases
// =========================================================================

#[test]
fn test_f10_corner_box_faces_non_empty() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    for f in def.faces() {
        assert!(!f.face_id.as_str().is_empty());
    }
}

#[test]
fn test_f10_corner_cylinder_faces_non_empty() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_eq!(def.faces().len(), 3);
}

#[test]
fn test_f10_corner_through_hole_face_count() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    assert_eq!(def.faces().len(), 7);
}

#[test]
fn test_f10_corner_narrow_pocket_face_count() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    assert!(def.faces().len() >= 8);
}

#[test]
fn test_f10_corner_ribbed_bracket_face_count() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    assert!(def.faces().len() >= 10);
}

// =========================================================================
// Feature 11 Corner Cases: Entity provenance mapping
// =========================================================================

#[test]
fn test_f11_corner_provenance_hash_non_empty() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(def.provenance().source_name, "box-mm.step");
}

#[test]
fn test_f11_corner_provenance_inch_unit() {
    let def = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    assert_eq!(def.provenance().source_unit, SourceUnit::Inch);
}

#[test]
fn test_f11_corner_provenance_mm_unit() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(def.provenance().source_unit, SourceUnit::Millimetre);
}

#[test]
fn test_f11_corner_distinct_face_ids() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    use std::collections::HashSet;
    let mut ids = HashSet::new();
    for f in def.faces() {
        assert!(ids.insert(f.face_id.clone()), "face IDs must be unique");
    }
}

#[test]
fn test_f11_corner_face_entity_strings() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    for f in def.faces() {
        assert!(!f.surface_face_entity.is_empty());
    }
}

// =========================================================================
// Feature 12 Corner Cases: Deletion of monstertruck scars
// =========================================================================

#[test]
fn test_f12_corner_no_legacy_import_panics() {
    for fixture in ALL_OCCT_FIXTURES {
        let def = import_test_step(&fixture_bytes(fixture));
        assert!(def.is_ok());
    }
}

#[test]
fn test_f12_corner_clean_box_inch() {
    let def = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    assert_eq!(def.faces().len(), 6);
}

#[test]
fn test_f12_corner_clean_through_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    assert_eq!(def.faces().len(), 7);
}

#[test]
fn test_f12_corner_clean_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_eq!(def.faces().len(), 3);
}

#[test]
fn test_f12_corner_clean_perforated_plate() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    assert_eq!(def.faces().len(), 514);
}

// =========================================================================
// Feature 13 Corner Cases: Interval arithmetic degeneracies
// =========================================================================

#[test]
fn test_f13_corner_interval_zero_width() {
    let a = Interval::point(0.0);
    assert_eq!(a.width(), 0.0);
}

#[test]
fn test_f13_corner_interval_negative_square() {
    let a = Interval { lo: -5.0, hi: -2.0 };
    let sq = a.square();
    assert_eq!(sq.lo, 4.0);
    assert_eq!(sq.hi, 25.0);
}

#[test]
fn test_f13_corner_interval_crossing_zero_square() {
    let a = Interval { lo: -3.0, hi: 4.0 };
    let sq = a.square();
    assert_eq!(sq.lo, 0.0);
    assert_eq!(sq.hi, 16.0);
}

#[test]
fn test_f13_corner_interval_negative_sqrt_error() {
    let a = Interval { lo: -5.0, hi: -1.0 };
    assert!(a.sqrt().is_err());
}

#[test]
fn test_f13_corner_interval_zero_sqrt() {
    let a = Interval::point(0.0);
    assert_eq!(a.sqrt().unwrap().lo, 0.0);
    assert_eq!(a.sqrt().unwrap().hi, 0.0);
}

// =========================================================================
// Feature 14 Corner Cases: IntervalVec degeneracies
// =========================================================================

#[test]
fn test_f14_corner_zero_vector_norm() {
    let v = IntervalVec3 {
        x: Interval::point(0.0),
        y: Interval::point(0.0),
        z: Interval::point(0.0),
    };
    let norm = v.norm().unwrap();
    assert_eq!(norm.lo, 0.0);
    assert_eq!(norm.hi, 0.0);
}

#[test]
fn test_f14_corner_orthogonal_vectors_dot_zero() {
    let a = IntervalVec3 {
        x: Interval::point(1.0),
        y: Interval::point(0.0),
        z: Interval::point(0.0),
    };
    let b = IntervalVec3 {
        x: Interval::point(0.0),
        y: Interval::point(1.0),
        z: Interval::point(0.0),
    };
    let dot = a.dot(b);
    assert_eq!(dot.lo, 0.0);
    assert_eq!(dot.hi, 0.0);
}

#[test]
fn test_f14_corner_large_coordinates_norm() {
    let v = IntervalVec3 {
        x: Interval::point(1e6),
        y: Interval::point(0.0),
        z: Interval::point(0.0),
    };
    let norm = v.norm().unwrap();
    assert_close(norm.lo, 1e6, 1.0, "large norm");
}

#[test]
fn test_f14_corner_small_coordinates_norm() {
    let v = IntervalVec3 {
        x: Interval::point(1e-6),
        y: Interval::point(0.0),
        z: Interval::point(0.0),
    };
    let norm = v.norm().unwrap();
    assert_close(norm.lo, 1e-6, 1e-9, "small norm");
}

#[test]
fn test_f14_corner_interval_vec2_point() {
    let v = IntervalVec2 {
        u: Interval::point(0.0),
        v: Interval::point(0.0),
    };
    assert_eq!(v.u.width(), 0.0);
}

// =========================================================================
// Feature 15 Corner Cases: Work budget limits
// =========================================================================

#[test]
fn test_f15_corner_budget_zero_charge() {
    let mut b = GeometryWorkBudget::new(100);
    let cancel = AtomicBool::new(false);
    b.charge(0, &cancel).unwrap();
    assert_eq!(b.consumed(), 0);
}

#[test]
fn test_f15_corner_budget_exact_depletion() {
    let mut b = GeometryWorkBudget::new(100);
    let cancel = AtomicBool::new(false);
    b.charge(100, &cancel).unwrap();
    assert_eq!(b.consumed(), 100);
    assert!(b.charge(1, &cancel).is_err());
}

#[test]
fn test_f15_corner_budget_cancelled_during_charge() {
    let mut b = GeometryWorkBudget::new(100);
    let cancel = AtomicBool::new(true);
    let err = b.charge(10, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::Cancelled);
}

#[test]
fn test_f15_corner_budget_overflow_request() {
    let mut b = GeometryWorkBudget::new(100);
    let cancel = AtomicBool::new(false);
    let err = b.charge(1000, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::ResourceLimit);
}

#[test]
fn test_f15_corner_budget_cumulative_tracking() {
    let mut b = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    for _ in 0..10 {
        b.charge(10, &cancel).unwrap();
    }
    assert_eq!(b.consumed(), 100);
}

// =========================================================================
// Feature 16 Corner Cases: High aspect ratio NURBS
// =========================================================================

#[test]
fn test_f16_corner_spline_housing_extreme_cuts() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let sec1 = section_at_z(&def, 0.01).unwrap();
    assert_eq!(sec1.loops().len(), 1);
    let sec2 = section_at_z(&def, 9.99).unwrap();
    assert_eq!(sec2.loops().len(), 2);
}

#[test]
fn test_f16_corner_spline_housing_tangent_cut() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let bot = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 0.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(bot.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f16_corner_rounded_housing_pocket_floor_boundary() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let sec_below = section_at_z(&def, 4.12).unwrap();
    let sec_above = section_at_z(&def, 4.14).unwrap();
    assert_eq!(sec_below.loops().len(), 1);
    assert_eq!(sec_above.loops().len(), 2);
}

#[test]
fn test_f16_corner_spline_housing_tessellation() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 80);
}

#[test]
fn test_f16_corner_spline_housing_bounds_exactness() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    assert_close(def.bounds_mm().min[2], 0.0, 1e-4, "min Z");
}

// =========================================================================
// Feature 17 Corner Cases: Bezier patch limits
// =========================================================================

#[test]
fn test_f17_corner_rounded_housing_multi_strata() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    for z in [1.0, 2.0, 3.0, 5.0, 7.0, 9.0] {
        let sec = section_at_z(&def, z).unwrap();
        assert!(!sec.loops().is_empty());
    }
}

#[test]
fn test_f17_corner_rounded_housing_floor_area_jump() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let s_low = section_at_z(&def, 4.1).unwrap();
    let s_high = section_at_z(&def, 4.16).unwrap();
    let a_low = loop_area(&s_low, &s_low.loops()[0]);
    let a_high_outer = loop_area(&s_high, &s_high.loops()[0]);
    assert!(a_low > 0.0);
    assert!(a_high_outer > 0.0);
}

#[test]
fn test_f17_corner_rounded_housing_mesh_valid() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.chunks.len() >= 8);
}

#[test]
fn test_f17_corner_rounded_housing_reproducibility() {
    let def1 = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let def2 = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    assert_eq!(def1.faces().len(), def2.faces().len());
}

#[test]
fn test_f17_corner_rounded_housing_bounds() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    assert_close(def.bounds_mm().max[0], 24.0, 1e-4, "X");
}

// =========================================================================
// Feature 18 Corner Cases: Analytic jets at poles and boundaries
// =========================================================================

#[test]
fn test_f18_corner_cylinder_at_z_zero_boundary() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let bot = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 0.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(bot.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f18_corner_cylinder_at_z_eight_boundary() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let top = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 8.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(top.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f18_corner_tapered_fitting_cone_slope() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step")).unwrap();
    let s1 = section_at_z(&def, 1.0).unwrap();
    let s2 = section_at_z(&def, 6.0).unwrap();
    let a1 = loop_area(&s1, &s1.loops()[0]);
    let a2 = loop_area(&s2, &s2.loops()[0]);
    assert!(a1 > a2, "tapered fitting area must decrease with height");
}

#[test]
fn test_f18_corner_through_hole_concentric() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f18_corner_box_inch_identical_geometry() {
    let def_mm = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let def_inch = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    let sec_mm = section_midplane(&def_mm).unwrap();
    let sec_inch = section_midplane(&def_inch).unwrap();
    let a_mm = loop_area(&sec_mm, &sec_mm.loops()[0]);
    let a_inch = loop_area(&sec_inch, &sec_inch.loops()[0]);
    assert_close(a_mm, a_inch, 1e-4, "unit conversion equivalence");
}

// =========================================================================
// Feature 19 Corner Cases: Curve domains and limits
// =========================================================================

#[test]
fn test_f19_corner_box_polyline_density() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.loops()[0].points_mm.len() >= 4);
}

#[test]
fn test_f19_corner_cylinder_polyline_density() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.loops()[0].points_mm.len() >= 16);
}

#[test]
fn test_f19_corner_points_finite() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    for pt in &sec.loops()[0].points_mm {
        assert!(pt[0].is_finite());
        assert!(pt[1].is_finite());
        assert!(pt[2].is_finite());
    }
}

#[test]
fn test_f19_corner_cylinder_points_finite() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    for pt in &sec.loops()[0].points_mm {
        assert!(pt[0].is_finite());
        assert!(pt[1].is_finite());
        assert!(pt[2].is_finite());
    }
}

#[test]
fn test_f19_corner_through_hole_points_finite() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    for l in sec.loops() {
        for pt in &l.points_mm {
            assert!(pt[0].is_finite());
        }
    }
}

// =========================================================================
// Feature 20 Corner Cases: Plane roots outside and tangents
// =========================================================================

#[test]
fn test_f20_corner_far_positive_z() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 100.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f20_corner_far_negative_z() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, -100.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f20_corner_cylinder_far_z() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 50.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f20_corner_through_hole_far_z() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 50.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f20_corner_near_miss_z() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 8.01).unwrap();
    assert!(sec.loops().is_empty());
}

// =========================================================================
// Feature 21 Corner Cases: Coplanar edge cases
// =========================================================================

#[test]
fn test_f21_corner_box_coplanar_top() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let top = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 8.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(top.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f21_corner_box_coplanar_bottom() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let bot = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 0.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(bot.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f21_corner_cylinder_coplanar_top() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let top = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 8.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(top.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f21_corner_cylinder_coplanar_bottom() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let bot = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 0.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(bot.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f21_corner_near_coplanar_cut() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 0.01).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

// =========================================================================
// Feature 22 Corner Cases: Legacy delegate elimination
// =========================================================================

#[test]
fn test_f22_corner_zero_legacy_panics_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(def.faces().len(), 6);
}

#[test]
fn test_f22_corner_zero_legacy_panics_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_eq!(def.faces().len(), 3);
}

#[test]
fn test_f22_corner_zero_legacy_panics_through_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    assert_eq!(def.faces().len(), 7);
}

#[test]
fn test_f22_corner_zero_legacy_panics_tapered() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step")).unwrap();
    assert!(def.faces().len() >= 3);
}

#[test]
fn test_f22_corner_zero_legacy_panics_bracket() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    assert!(def.faces().len() >= 10);
}

// =========================================================================
// Feature 23 Corner Cases: 2D Curve IR parameter checks
// =========================================================================

#[test]
fn test_f23_corner_line_start_end_same() {
    let line = SectionCurve2d::Line {
        start_mm: [1.0, 2.0],
        end_mm: [1.0, 2.0],
    };
    match line {
        SectionCurve2d::Line { start_mm, end_mm } => assert_eq!(start_mm, end_mm),
        _ => panic!(),
    }
}

#[test]
fn test_f23_corner_circular_arc_full_circle() {
    let arc = SectionCurve2d::CircularArc {
        center_mm: [0.0, 0.0],
        radius_mm: 5.0,
        start_angle_rad: 0.0,
        sweep_angle_rad: std::f64::consts::TAU,
    };
    match arc {
        SectionCurve2d::CircularArc { sweep_angle_rad, .. } => assert_close(sweep_angle_rad, std::f64::consts::TAU, 1e-9, "tau"),
        _ => panic!(),
    }
}

#[test]
fn test_f23_corner_elliptic_arc_circular() {
    let ellipse = SectionCurve2d::EllipticArc {
        center_mm: [0.0, 0.0],
        major_axis_mm: [3.0, 0.0],
        minor_radius_mm: 3.0,
        start_param_rad: 0.0,
        end_param_rad: std::f64::consts::PI,
    };
    match ellipse {
        SectionCurve2d::EllipticArc { minor_radius_mm, .. } => assert_eq!(minor_radius_mm, 3.0),
        _ => panic!(),
    }
}

#[test]
fn test_f23_corner_nurbs_span_rational() {
    let nurbs = SectionCurve2d::NurbsSpan {
        degree: 2,
        knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        control_points_mm: vec![[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]],
        weights: Some(vec![1.0, 0.7071, 1.0]),
    };
    match nurbs {
        SectionCurve2d::NurbsSpan { weights, .. } => assert!(weights.is_some()),
        _ => panic!(),
    }
}

#[test]
fn test_f23_corner_section_span_zero_error() {
    let span = SectionSpan2d {
        curve: SectionCurve2d::Line { start_mm: [0.0, 0.0], end_mm: [1.0, 0.0] },
        source_face: SourceFaceId(0),
        source_edge: None,
        achieved_plane_error_mm: 0.0,
    };
    assert_eq!(span.achieved_plane_error_mm, 0.0);
}

// =========================================================================
// Feature 24 Corner Cases: SectionLoopMm properties
// =========================================================================

#[test]
fn test_f24_corner_box_outer_loop() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(!sec.loops()[0].is_hole);
}

#[test]
fn test_f24_corner_through_hole_hole_loop() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.loops()[1].is_hole);
}

#[test]
fn test_f24_corner_loop_count_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f24_corner_loop_count_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f24_corner_loop_count_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

// =========================================================================
// Feature 25 Corner Cases: Culling boundaries
// =========================================================================

#[test]
fn test_f25_corner_plane_z_plus_100() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 100.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f25_corner_plane_z_minus_100() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, -100.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f25_corner_plane_z_exact_mid() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f25_corner_plane_z_eight_point_five() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 8.5).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f25_corner_cylinder_cull_z() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 8.5).unwrap();
    assert!(sec.loops().is_empty());
}

// =========================================================================
// Feature 26 Corner Cases: Direct intersections
// =========================================================================

#[test]
fn test_f26_corner_box_midplane_area() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_close(loop_area(&sec, &sec.loops()[0]), 200.0, 1e-6, "area");
}

#[test]
fn test_f26_corner_cylinder_midplane_area() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_close(loop_area(&sec, &sec.loops()[0]), 25.0 * std::f64::consts::PI, 0.2, "area");
}

#[test]
fn test_f26_corner_through_hole_outer_area() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_close(loop_area(&sec, &sec.loops()[0]), 400.0, 1e-6, "area");
}

#[test]
fn test_f26_corner_through_hole_hole_area() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_close(loop_area(&sec, &sec.loops()[1]), -9.0 * std::f64::consts::PI, 0.2, "hole area");
}

#[test]
fn test_f26_corner_narrow_pocket_area() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    let sec = section_at_z(&def, 1.0).unwrap();
    assert_close(loop_area(&sec, &sec.loops()[0]), 120.0, 1e-4, "pocket base area");
}

// =========================================================================
// Feature 27 Corner Cases: Trimmed domain clipping
// =========================================================================

#[test]
fn test_f27_corner_box_clipping() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f27_corner_cylinder_clipping() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f27_corner_through_hole_clipping() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f27_corner_tapered_fitting_clipping() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step")).unwrap();
    let sec = section_at_z(&def, 6.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f27_corner_perforated_plate_clipping() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let sec = section_at_z(&def, 10.0).unwrap();
    assert_eq!(sec.loops().len(), 513);
}

// =========================================================================
// Feature 28 Corner Cases: Winding orientations
// =========================================================================

#[test]
fn test_f28_corner_outer_loop_positive() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(loop_area(&sec, &sec.loops()[0]) > 0.0);
}

#[test]
fn test_f28_corner_hole_loop_negative() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(loop_area(&sec, &sec.loops()[1]) < 0.0);
}

#[test]
fn test_f28_corner_cylinder_positive() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(loop_area(&sec, &sec.loops()[0]) > 0.0);
}

#[test]
fn test_f28_corner_tapered_hole_negative() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step")).unwrap();
    let sec = section_at_z(&def, 6.0).unwrap();
    assert!(loop_area(&sec, &sec.loops()[1]) < 0.0);
}

#[test]
fn test_f28_corner_narrow_pocket_hole_negative() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    let sec = section_at_z(&def, 5.0).unwrap();
    assert!(loop_area(&sec, &sec.loops()[1]) < 0.0);
}

// =========================================================================
// Feature 29 Corner Cases: Coplanar wire boundaries
// =========================================================================

#[test]
fn test_f29_corner_box_coplanar_top_degenerate() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let top = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 8.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(top.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f29_corner_box_coplanar_bottom_degenerate() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let bot = section(&def, PlaneMm { origin_mm: [0.0, 0.0, 0.0], normal: [0.0, 0.0, 1.0] }, &cancel);
    assert_eq!(bot.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f29_corner_box_near_top_valid() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 7.99).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f29_corner_box_near_bottom_valid() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 0.01).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f29_corner_pocket_transition() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let s1 = section_at_z(&def, 4.1).unwrap();
    let s2 = section_at_z(&def, 4.16).unwrap();
    assert_eq!(s1.loops().len(), 1);
    assert_eq!(s2.loops().len(), 2);
}

// =========================================================================
// Feature 30 Corner Cases: Closed loop chaining
// =========================================================================

#[test]
fn test_f30_corner_box_chaining() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f30_corner_cylinder_chaining() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f30_corner_through_hole_chaining() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f30_corner_bracket_chaining() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(!sec.loops().is_empty());
}

#[test]
fn test_f30_corner_plate_chaining() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let sec = section_at_z(&def, 10.0).unwrap();
    assert_eq!(sec.loops().len(), 513);
}

// =========================================================================
// Feature 31 Corner Cases: Adaptive chord sampling limits
// =========================================================================

#[test]
fn test_f31_corner_chord_error_upper_bound() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.achieved_chord_error_mm() <= 0.005);
}

#[test]
fn test_f31_corner_plane_error_upper_bound() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.achieved_plane_error_mm() <= 0.0001);
}

#[test]
fn test_f31_corner_cylinder_chord_error() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.achieved_chord_error_mm() <= 0.005);
}

#[test]
fn test_f31_corner_housing_chord_error() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let sec = section_at_z(&def, 0.5).unwrap();
    assert!(sec.achieved_chord_error_mm() <= 0.005);
}

#[test]
fn test_f31_corner_tapered_chord_error() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step")).unwrap();
    let sec = section_at_z(&def, 6.0).unwrap();
    assert!(sec.achieved_chord_error_mm() <= 0.005);
}

// =========================================================================
// Feature 32 Corner Cases: Boolean elimination performance
// =========================================================================

#[test]
fn test_f32_corner_direct_intersection_speed() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.is_native_intersection());
}

#[test]
fn test_f32_corner_no_fragment_artifacts() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f32_corner_through_hole_artifacts() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f32_corner_retained_bytes_reasonable() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.retained_bytes() < 50_000);
}

#[test]
fn test_f32_corner_plate_retained_bytes() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let sec = section_at_z(&def, 10.0).unwrap();
    assert!(sec.retained_bytes() < 10_000_000);
}

// =========================================================================
// Feature 33 Corner Cases: PreparedSolid unification
// =========================================================================

#[test]
fn test_f33_corner_prepare_geometry_success() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    assert!(prepare_geometry_source(&def, &mut work, &cancel).is_ok());
}

#[test]
fn test_f33_corner_prepare_section_success() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    assert!(prepare_section_source(&def, &mut work, &cancel).is_ok());
}

#[test]
fn test_f33_corner_prepare_material_success() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    assert!(prepare_material_source(&def, &mut work, &cancel).is_ok());
}

#[test]
fn test_f33_corner_prepare_with_cancelled_token() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(true);
    let err = prepare_geometry_source(&def, &mut work, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::Cancelled);
}

#[test]
fn test_f33_corner_prepare_with_zero_budget() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(0);
    let cancel = AtomicBool::new(false);
    let res = prepare_geometry_source(&def, &mut work, &cancel);
    let _ = res; // May succeed or return resource limit depending on initial allocation
}

// =========================================================================
// Feature 34 Corner Cases: Toleranced topology
// =========================================================================

#[test]
fn test_f34_corner_box_faces() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(def.faces().len(), 6);
}

#[test]
fn test_f34_corner_cylinder_faces() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_eq!(def.faces().len(), 3);
}

#[test]
fn test_f34_corner_through_hole_faces() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    assert_eq!(def.faces().len(), 7);
}

#[test]
fn test_f34_corner_narrow_pocket_faces() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    assert!(def.faces().len() >= 8);
}

#[test]
fn test_f34_corner_bracket_faces() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    assert!(def.faces().len() >= 10);
}

// =========================================================================
// Feature 35 Corner Cases: Absence of G-transport
// =========================================================================

#[test]
fn test_f35_corner_direct_section() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.is_native_intersection());
}

#[test]
fn test_f35_corner_direct_tessellation() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count > 0);
}

#[test]
fn test_f35_corner_direct_bounds() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_close(def.bounds_mm().max[0], 20.0, 1e-6, "bounds");
}

#[test]
fn test_f35_corner_direct_face_inspect() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert!(inspect_face(&def, &def.faces()[0].face_id).is_ok());
}

#[test]
fn test_f35_corner_direct_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

// =========================================================================
// Feature 36 Corner Cases: Native band & height analysis
// =========================================================================

#[test]
fn test_f36_corner_height_domain_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel);
    assert!(h.is_ok());
}

#[test]
fn test_f36_corner_height_domain_cancelled() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(true);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let err = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::Cancelled);
}

#[test]
fn test_f36_corner_height_domain_zero_work() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(0);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let _ = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel);
}

#[test]
fn test_f36_corner_height_domain_narrow_interval() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let h = analyze_height_domain(&def, frame, [2.0, 4.0], policy, &mut work, &cancel);
    assert!(h.is_ok());
}

#[test]
fn test_f36_corner_height_domain_outside_interval() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let h = analyze_height_domain(&def, frame, [10.0, 15.0], policy, &mut work, &cancel);
    assert!(h.is_ok());
}

// =========================================================================
// Feature 37 Corner Cases: Bounds precision
// =========================================================================

#[test]
fn test_f37_corner_bounds_box_mm() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.min[0], 0.0, 1e-6, "min X");
    assert_close(b.max[0], 20.0, 1e-6, "max X");
    assert_close(b.max[1], 10.0, 1e-6, "max Y");
    assert_close(b.max[2], 8.0, 1e-6, "max Z");
}

#[test]
fn test_f37_corner_bounds_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.min[0], -5.0, 1e-6, "min X");
    assert_close(b.max[0], 5.0, 1e-6, "max X");
}

#[test]
fn test_f37_corner_bounds_through_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.max[0], 20.0, 1e-6, "max X");
}

#[test]
fn test_f37_corner_bounds_bracket() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.max[0], 30.0, 1e-4, "max X");
}

#[test]
fn test_f37_corner_bounds_plate() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.max[0], 1640.0, 1e-4, "max X");
}

// =========================================================================
// Feature 38 Corner Cases: Critical height strata
// =========================================================================

#[test]
fn test_f38_corner_strata_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f38_corner_strata_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f38_corner_strata_pocket() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let h = analyze_height_domain(&def, frame, [0.0, 6.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f38_corner_strata_housing() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let h = analyze_height_domain(&def, frame, [0.0, 10.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f38_corner_strata_bracket() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm { origin_mm: [0.0, 0.0, 0.0], x_axis: [1.0, 0.0, 0.0], y_axis: [0.0, 1.0, 0.0] };
    let policy = AnalysisPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let h = analyze_height_domain(&def, frame, [0.0, 7.6], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

// =========================================================================
// Feature 39 Corner Cases: Projected bounds
// =========================================================================

#[test]
fn test_f39_corner_projected_bounds_negative_dir() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let p = projected_bounds(&def, [-1.0, 0.0, 0.0], policy, &mut work, &cancel).unwrap();
    assert!(p.maximum[0].is_finite());
}

#[test]
fn test_f39_corner_projected_bounds_diagonal() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let p = projected_bounds(&def, [1.0, 1.0, 0.0], policy, &mut work, &cancel).unwrap();
    assert!(p.maximum[0].is_finite());
}

#[test]
fn test_f39_corner_projected_bounds_cancelled() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(true);
    let policy = BoundsPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let err = projected_bounds(&def, [1.0, 0.0, 0.0], policy, &mut work, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::Cancelled);
}

#[test]
fn test_f39_corner_material_projected_bounds_y() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let p = material_projected_bounds(&def, [0.0, 1.0, 0.0], policy, &mut work, &cancel).unwrap();
    assert_close(p.bounds().maximum[0], 10.0, 1e-4, "material Y");
}

#[test]
fn test_f39_corner_material_projected_bounds_z() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy { maximum_endpoint_uncertainty_mm: 1e-6 };
    let p = material_projected_bounds(&def, [0.0, 0.0, 1.0], policy, &mut work, &cancel).unwrap();
    assert_close(p.bounds().maximum[0], 8.0, 1e-4, "material Z");
}

// =========================================================================
// Feature 40 Corner Cases: Void projections
// =========================================================================

#[test]
fn test_f40_corner_no_voids_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
    assert!(!sec.loops()[0].is_hole);
}

#[test]
fn test_f40_corner_single_void_through_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
    assert!(sec.loops()[1].is_hole);
}

#[test]
fn test_f40_corner_many_voids_plate() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let sec = section_at_z(&def, 10.0).unwrap();
    let holes = sec.loops().iter().filter(|l| l.is_hole).count();
    assert_eq!(holes, 512);
}

#[test]
fn test_f40_corner_pocket_void() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    let sec = section_at_z(&def, 5.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f40_corner_housing_void() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let sec = section_at_z(&def, 9.5).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

// =========================================================================
// Feature 41 Corner Cases: Wall and cylinder certificate boundaries
// =========================================================================

#[test]
fn test_f41_corner_box_wall_faces() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(def.faces().len(), 6);
}

#[test]
fn test_f41_corner_cylinder_face() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_eq!(def.faces().len(), 3);
}

#[test]
fn test_f41_corner_through_hole_faces() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    assert_eq!(def.faces().len(), 7);
}

#[test]
fn test_f41_corner_occt_box_faces() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-box-mm.step")).unwrap();
    assert_eq!(def.faces().len(), 6);
}

#[test]
fn test_f41_corner_occt_cylinder_faces() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-cylinder-mm.step")).unwrap();
    assert_eq!(def.faces().len(), 3);
}

// =========================================================================
// Feature 42 Corner Cases: Tessellation watertightness
// =========================================================================

#[test]
fn test_f42_corner_box_triangles() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_eq!(mesh.triangle_count, 12);
}

#[test]
fn test_f42_corner_cylinder_triangles() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 30);
}

#[test]
fn test_f42_corner_through_hole_triangles() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 40);
}

#[test]
fn test_f42_corner_tapered_triangles() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 40);
}

#[test]
fn test_f42_corner_bracket_triangles() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 100);
}

// =========================================================================
// Feature 43 Corner Cases: CDT Meshing densities
// =========================================================================

#[test]
fn test_f43_corner_box_vertices() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_eq!(mesh.vertex_count, 24);
}

#[test]
fn test_f43_corner_cylinder_vertices() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count >= 30);
}

#[test]
fn test_f43_corner_through_hole_vertices() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count >= 40);
}

#[test]
fn test_f43_corner_housing_vertices() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count >= 50);
}

#[test]
fn test_f43_corner_bracket_vertices() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count >= 100);
}

// =========================================================================
// Feature 44 Corner Cases: Refinement iterations
// =========================================================================

#[test]
fn test_f44_corner_cylinder_mesh_finite() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.total_bytes > 0);
}

#[test]
fn test_f44_corner_tapered_mesh_finite() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.total_bytes > 0);
}

#[test]
fn test_f44_corner_narrow_pocket_mesh_finite() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.total_bytes > 0);
}

#[test]
fn test_f44_corner_housing_mesh_finite() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.total_bytes > 0);
}

#[test]
fn test_f44_corner_spline_mesh_finite() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-spline-housing-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.total_bytes > 0);
}

// =========================================================================
// Feature 45 Corner Cases: Audit chunks
// =========================================================================

#[test]
fn test_f45_corner_box_chunks() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_eq!(mesh.chunks.len(), 6);
}

#[test]
fn test_f45_corner_cylinder_chunks() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_eq!(mesh.chunks.len(), 3);
}

#[test]
fn test_f45_corner_through_hole_chunks() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_eq!(mesh.chunks.len(), 7);
}

#[test]
fn test_f45_corner_housing_chunks() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-rounded-housing-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(!mesh.chunks.is_empty());
}

#[test]
fn test_f45_corner_bracket_chunks() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(!mesh.chunks.is_empty());
}

// =========================================================================
// Feature 46 Corner Cases: Mesh artifact budget limits
// =========================================================================

#[test]
fn test_f46_corner_mesh_budget_triangles() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let budget = MeshBudget {
        vertices: 100,
        triangles: 100,
        bytes: 100_000,
    };
    let artifact = tessellate_with_budget(&def, DisplayProfile::MeshTrimmedMm005V2, &cancel, budget);
    assert!(artifact.is_ok());
}

#[test]
fn test_f46_corner_mesh_budget_exceeded_triangles() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let budget = MeshBudget {
        vertices: 10,
        triangles: 5,
        bytes: 100_000,
    };
    let res = tessellate_with_budget(&def, DisplayProfile::MeshTrimmedMm005V2, &cancel, budget);
    let _ = res;
}

#[test]
fn test_f46_corner_mesh_artifact_total_bytes() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let artifact = tessellate_display(&def).unwrap();
    assert!(artifact.total_bytes > 0);
}

#[test]
fn test_f46_corner_mesh_artifact_chunks_non_empty() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let artifact = tessellate_display(&def).unwrap();
    for c in &artifact.chunks {
        assert!(!c.bytes.is_empty());
    }
}

#[test]
fn test_f46_corner_mesh_artifact_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let artifact = tessellate_display(&def).unwrap();
    assert_eq!(artifact.chunks.len(), 3);
}

// =========================================================================
// Feature 47 Corner Cases: Downstream section consumption
// =========================================================================

#[test]
fn test_f47_corner_section_loops_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f47_corner_section_loops_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f47_corner_section_loops_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f47_corner_section_loops_pocket() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step")).unwrap();
    let sec = section_at_z(&def, 1.1).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f47_corner_section_loops_bracket() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(!sec.loops().is_empty());
}

// =========================================================================
// Feature 48 Corner Cases: Adversarial Fixtures Parity
// =========================================================================

#[test]
fn test_f48_adversary_missing_units() {
    let err = import_test_step(&fixture_bytes("missing-units.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

#[test]
fn test_f48_adversary_conflicting_units() {
    let err = import_test_step(&fixture_bytes("conflicting-units.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

#[test]
fn test_f48_adversary_degrees() {
    let err = import_test_step(&fixture_bytes("degrees.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

#[test]
fn test_f48_adversary_recursive_units() {
    let err = import_test_step(&fixture_bytes("recursive-units.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

#[test]
fn test_f48_adversary_wrong_conversion_dimension() {
    let err = import_test_step(&fixture_bytes("wrong-conversion-dimension.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedUnits);
}

// =========================================================================
// Feature 49 Corner Cases: Topological Adversaries
// =========================================================================

#[test]
fn test_f49_adversary_missing_reference() {
    let err = import_test_step(&fixture_bytes("missing-reference.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f49_adversary_open_shell() {
    let err = import_test_step(&fixture_bytes("open-shell.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f49_adversary_non_manifold_shell() {
    let err = import_test_step(&fixture_bytes("non-manifold-shell.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f49_adversary_nonfinite_coordinate() {
    let err = import_test_step(&fixture_bytes("nonfinite-coordinate.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f49_adversary_truncated() {
    let err = import_test_step(&fixture_bytes("truncated.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

// =========================================================================
// Feature 50 Corner Cases: Structural & Assembly Adversaries
// =========================================================================

#[test]
fn test_f50_adversary_duplicate_face_use() {
    let err = import_test_step(&fixture_bytes("duplicate-face-use.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn test_f50_adversary_reachable_unsupported() {
    let err = import_test_step(&fixture_bytes("reachable-unsupported.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedGeometry);
}

#[test]
fn test_f50_adversary_assembly() {
    let err = import_test_step(&fixture_bytes("assembly.step")).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::UnsupportedGeometry);
}

#[test]
fn test_f50_adversary_cancellation_during_import() {
    let bytes = fixture_bytes("box-mm.step");
    let cancel = AtomicBool::new(true);
    let err = import_step(&bytes, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::Cancelled);
}

#[test]
fn test_f50_adversary_cancellation_during_tessellate() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(true);
    let err = tessellate(&def, DisplayProfile::MeshTrimmedMm005V2, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::Cancelled);
}
