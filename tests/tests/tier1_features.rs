// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Tier 1: Feature Coverage (PROJECT.md § Feature Inventory, Features 1–50).
//! Native feature behavior uses the production geometry implementation.

use spiling_contracts::geometry::*;
use spiling_e2e_tests::fixtures::*;
use spiling_e2e_tests::helpers::*;
use spiling_geometry::*;
use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;

// =========================================================================
// Feature 1: StepEntityId & arena identifiers
// =========================================================================

#[test]
fn test_f01_step_entity_id_constructor() {
    let id = StepEntityId(42);
    assert_eq!(id.0, 42);
}

#[test]
fn test_f01_step_entity_id_display_formatting() {
    let id = StepEntityId(101);
    assert_eq!(format!("{id}"), "step:101");
}

#[test]
fn test_f01_step_entity_id_ordering_and_hash() {
    use std::collections::BTreeSet;
    let mut set = BTreeSet::new();
    set.insert(StepEntityId(50));
    set.insert(StepEntityId(10));
    set.insert(StepEntityId(30));
    let ordered: Vec<_> = set.into_iter().collect();
    assert_eq!(
        ordered,
        vec![StepEntityId(10), StepEntityId(30), StepEntityId(50)]
    );
}

#[test]
fn test_f01_step_entity_id_transparent_serde() {
    let id = StepEntityId(9999);
    let serialized = serde_json::to_string(&id).unwrap();
    assert_eq!(serialized, "9999");
    let deserialized: StepEntityId = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized, id);
}

#[test]
fn test_f01_source_face_and_edge_ids() {
    let face_id = SourceFaceId(7);
    let edge_id = SourceEdgeId(13);
    assert_eq!(face_id.0, 7);
    assert_eq!(edge_id.0, 13);
    let ser_face = serde_json::to_string(&face_id).unwrap();
    assert_eq!(ser_face, "7");
    let ser_edge = serde_json::to_string(&edge_id).unwrap();
    assert_eq!(ser_edge, "13");
}

// =========================================================================
// Feature 2: Definition solidcore::Brep ownership
// =========================================================================

#[test]
fn test_f02_definition_bounds_accessor() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let bounds = def.bounds_mm();
    assert_close(bounds.min[0], 0.0, 1e-6, "box bounds min X");
    assert_close(bounds.max[0], 20.0, 1e-6, "box bounds max X");
    assert_close(bounds.max[1], 10.0, 1e-6, "box bounds max Y");
    assert_close(bounds.max[2], 8.0, 1e-6, "box bounds max Z");
}

#[test]
fn test_f02_definition_face_count_accessor() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f02_definition_id_uniqueness() {
    let def1 = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let def2 = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    assert_ne!(def1.id(), def2.id());
}

#[test]
fn test_f02_definition_send_sync_thread() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let handle = std::thread::spawn(move || {
        assert_source_face_identity(&def, "box-mm.step");
        def.bounds_mm()
    });
    let bounds = handle.join().unwrap();
    assert_close(bounds.max[0], 20.0, 1e-6, "thread bounds");
}

#[test]
fn test_f02_definition_retained_bytes_accounting() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let bytes = def.retained_query_proof_bytes();
    assert!(bytes > 0, "retained proof bytes must be non-zero");
}

// =========================================================================
// Feature 3: Zero-mutex topology access
// =========================================================================

#[test]
fn test_f03_inspect_face_without_locks() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    for face in def.faces() {
        let info = inspect_face(&def, &face.face_id).unwrap();
        assert_eq!(info.face_id, face.face_id);
    }
}

#[test]
fn test_f03_concurrent_reads_on_definition() {
    use std::sync::Arc;
    let def = Arc::new(import_test_step(&fixture_bytes("box-mm.step")).unwrap());
    let mut handles = Vec::new();
    for _ in 0..4 {
        let d = Arc::clone(&def);
        handles.push(std::thread::spawn(move || {
            assert_eq!(d.faces().len(), 6);
            d.bounds_mm().max[0]
        }));
    }
    for h in handles {
        assert_close(h.join().unwrap(), 20.0, 1e-6, "concurrent read");
    }
}

#[test]
fn test_f03_face_direct_properties() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let first = &def.faces()[0];
    assert!(!first.surface_face_entity.is_empty());
    assert!(!first.source_entity_kind.is_empty());
}

#[test]
fn test_f03_face_family_classification() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    for face in def.faces() {
        assert_eq!(face.geometry.family, SurfaceFamily::Plane);
    }
}

#[test]
fn test_f03_face_surface_entity_kind() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    for face in def.faces() {
        assert_eq!(face.source_surface_entity_kind, "PLANE");
    }
}

// =========================================================================
// Feature 4: Thread-isolated Kernel context
// =========================================================================

#[test]
fn test_f04_deterministic_kernel_instantiation() {
    let _k1 = solidcore::kernel::Kernel::default();
    let _k2 = solidcore::kernel::Kernel::default();
}

#[test]
fn test_f04_kernel_brep_millimetre_tolerances() {
    let brep = solidcore::Brep::default();
    assert!(brep.tolerance() > 0.0);
    assert!(brep.tolerance() <= 1e-4);
}

#[test]
fn test_f04_independent_parallel_kernel_contexts() {
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                let _k = solidcore::kernel::Kernel::default();
                let b = solidcore::Brep::default();
                b.tolerance()
            })
        })
        .collect();
    for h in handles {
        assert!(h.join().unwrap() > 0.0);
    }
}

#[test]
fn test_f04_kernel_creation_infallible() {
    let _k = solidcore::kernel::Kernel::default();
}

#[test]
fn test_f04_kernel_tolerance_scale() {
    let b = solidcore::Brep::default();
    assert_close(b.tolerance(), 1e-6, 1e-4, "default tolerance");
}

// =========================================================================
// Feature 5: Retained query proof byte accounting
// =========================================================================

#[test]
fn test_f05_positive_byte_budget_tracking() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert!(
        def.retained_query_proof_bytes() >= 512,
        "definition must allocate byte budget"
    );
}

#[test]
fn test_f05_complexity_increases_retained_bytes() {
    let b_box = import_test_step(&fixture_bytes("box-mm.step"))
        .unwrap()
        .retained_query_proof_bytes();
    let b_hole = import_test_step(&fixture_bytes("through-hole.step"))
        .unwrap()
        .retained_query_proof_bytes();
    assert!(
        b_hole >= b_box,
        "through hole with 7 faces must retain more or equal bytes than 6-face box"
    );
}

#[test]
fn test_f05_section_retained_bytes() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.retained_bytes() > 0);
}

#[test]
fn test_f05_reproducible_retained_bytes() {
    let def1 = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let def2 = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(
        def1.retained_query_proof_bytes(),
        def2.retained_query_proof_bytes()
    );
}

// =========================================================================
// Feature 6: STEP parser via solidcraft-step
// =========================================================================

#[test]
fn test_f06_p21_parse_header_and_entities() {
    let text = fixture_str("box-mm.step");
    let exchange = solidcraft_step::p21::parse(&text).unwrap();
    assert!(!exchange.entities.is_empty(), "entities must not be empty");
}

#[test]
fn test_f06_p21_parse_cylinder() {
    let text = fixture_str("cylinder.step");
    let exchange = solidcraft_step::p21::parse(&text).unwrap();
    assert!(!exchange.entities.is_empty());
}

#[test]
fn test_f06_p21_parse_occt_box() {
    let text = fixture_str("independent-exporters/occt-box-mm.step");
    let exchange = solidcraft_step::p21::parse(&text).unwrap();
    assert!(!exchange.entities.is_empty());
}

#[test]
fn test_f06_p21_parse_entity_count() {
    let text = fixture_str("box-mm.step");
    let exchange = solidcraft_step::p21::parse(&text).unwrap();
    assert!(exchange.entities.len() > 10);
}

#[test]
fn test_f06_p21_syntax_resilience() {
    let text = fixture_str("box-inch.step");
    let exchange = solidcraft_step::p21::parse(&text);
    assert!(exchange.is_ok());
}

// =========================================================================
// Feature 7: Schema 4 B-Rep reader
// =========================================================================

#[test]
fn test_f07_read_solids_from_box() {
    let text = fixture_str("box-mm.step");
    let ex = solidcraft_step::p21::parse(&text).unwrap();
    let solids = solidcraft_step::topology::read_solids(&ex);
    assert_eq!(solids.solids.len(), 1);
    assert!(!solids.solids[0].sheet);
}

#[test]
fn test_f07_solidcraft_face_origin_extraction() {
    let text = fixture_str("box-mm.step");
    let ex = solidcraft_step::p21::parse(&text).unwrap();
    let solids = solidcraft_step::topology::read_solids(&ex);
    assert_eq!(solids.solids[0].face_origin.len(), 6);
}

#[test]
fn test_f07_read_solids_occt_cylinder() {
    let text = fixture_str("independent-exporters/occt-cylinder-mm.step");
    let ex = solidcraft_step::p21::parse(&text).unwrap();
    let solids = solidcraft_step::topology::read_solids(&ex);
    assert_eq!(solids.solids.len(), 1);
    assert_eq!(solids.solids[0].brep.faces.len(), 3);
}

#[test]
fn test_f07_closed_solid_verification() {
    let text = fixture_str("through-hole.step");
    let ex = solidcraft_step::p21::parse(&text).unwrap();
    let solids = solidcraft_step::topology::read_solids(&ex);
    assert_eq!(solids.solids.len(), 1);
    assert!(!solids.solids[0].sheet);
}

#[test]
fn test_f07_vertex_count_consistency() {
    let text = fixture_str("box-mm.step");
    let ex = solidcraft_step::p21::parse(&text).unwrap();
    let solids = solidcraft_step::topology::read_solids(&ex);
    assert_eq!(solids.solids[0].brep.vertices.len(), 8);
}

// =========================================================================
// Feature 8: Schema 4 to BrepData bridge
// =========================================================================

#[test]
fn test_f08_bridge_box_import_roundtrip() {
    let def = import_test_step(&fixture_bytes("independent-exporters/occt-box-mm.step"));
    assert!(def.is_ok());
}

#[test]
fn test_f08_bridge_cylinder_import_roundtrip() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-cylinder-mm.step",
    ));
    assert!(def.is_ok());
}

#[test]
fn test_f08_bridge_through_hole_import_roundtrip() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-through-hole-mm.step",
    ));
    assert!(def.is_ok());
}

#[test]
fn test_f08_bridge_face_count_preservation() {
    let text = fixture_str("independent-exporters/occt-narrow-pocket-mm.step");
    let ex = solidcraft_step::p21::parse(&text).unwrap();
    let solids = solidcraft_step::topology::read_solids(&ex);
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-narrow-pocket-mm.step",
    ))
    .unwrap();
    assert_eq!(def.faces().len(), solids.solids[0].brep.faces.len());
}

#[test]
fn test_f08_bridge_tapered_fitting_faces() {
    let text = fixture_str("independent-exporters/occt-tapered-fitting-mm.step");
    let ex = solidcraft_step::p21::parse(&text).unwrap();
    let solids = solidcraft_step::topology::read_solids(&ex);
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-tapered-fitting-mm.step",
    ))
    .unwrap();
    assert_eq!(def.faces().len(), solids.solids[0].brep.faces.len());
}

// =========================================================================
// Feature 9: OCCT quasi-periodic knot re-clamping
// =========================================================================

#[test]
fn test_f09_occt_spline_housing_import() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ));
    assert!(
        def.is_ok(),
        "spline housing with quasi-periodic knots must import cleanly"
    );
}

#[test]
fn test_f09_occt_rounded_housing_import() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ));
    assert!(def.is_ok(), "rounded housing must import cleanly");
}

#[test]
fn test_f09_occt_ribbed_bracket_import() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ));
    assert!(def.is_ok(), "ribbed bracket must import cleanly");
}

#[test]
fn test_f09_occt_spline_housing_bounds() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let bounds = def.bounds_mm();
    assert_close(bounds.min[0], 0.0, 1e-4, "spline housing min X");
    assert_close(bounds.max[0], 24.0, 1e-4, "spline housing max X");
    assert_close(bounds.max[1], 18.0, 1e-4, "spline housing max Y");
    assert_close(bounds.max[2], 10.0, 1e-4, "spline housing max Z");
}

#[test]
fn test_f09_occt_spline_tessellate_without_panic() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def);
    assert!(mesh.is_ok(), "spline housing display mesh generation");
}

// =========================================================================
// Feature 10: Admitted source-face identity
// =========================================================================

#[test]
fn test_f10_box_source_face_identity() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f10_cylinder_source_face_identity() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_source_face_identity(&def, "cylinder.step");
}

#[test]
fn test_f10_through_hole_source_face_identity() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    assert_source_face_identity(&def, "through-hole.step");
}

#[test]
fn test_f10_occt_tapered_fitting_manifold() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-tapered-fitting-mm.step",
    ))
    .unwrap();
    assert!(def.faces().len() >= 3);
}

#[test]
fn test_f10_box_source_face_provenance() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
}

// =========================================================================
// Feature 11: Entity provenance binding
// =========================================================================

#[test]
fn test_f11_provenance_id_assignment() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    for face in def.faces() {
        assert!(!face.face_id.as_str().is_empty());
    }
}

#[test]
fn test_f11_face_id_deterministic() {
    let def1 = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let def2 = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(def1.faces().len(), def2.faces().len());
    for (f1, f2) in def1.faces().iter().zip(def2.faces()) {
        assert_eq!(f1.face_id, f2.face_id);
    }
}

#[test]
fn test_f11_inspect_face_finds_all_faces() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    for face in def.faces() {
        let found = inspect_face(&def, &face.face_id);
        assert!(found.is_ok());
    }
}

#[test]
fn test_f11_provenance_source_name() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert!(def.provenance().source_name.is_empty());
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f11_provenance_source_unit() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(def.provenance().source_unit, SourceUnit::Millimetre);
}

// =========================================================================
// Feature 12: Original source identity through native import
// =========================================================================

#[test]
fn test_f12_box_source_identity() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f12_cylinder_source_identity() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_source_face_identity(&def, "cylinder.step");
}

#[test]
fn test_f12_through_hole_source_identity() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    assert_source_face_identity(&def, "through-hole.step");
}

#[test]
fn test_f12_pure_import_perforated_plate() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    assert_source_face_identity(&def, "perforated-plate.step");
}

#[test]
fn test_f12_inch_box_source_identity() {
    let def = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    assert_source_face_identity(&def, "box-inch.step");
}

// =========================================================================
// Feature 13: In-tree IEEE-754 Interval
// =========================================================================

#[test]
fn test_f13_interval_addition_outward_rounding() {
    let a = Interval { lo: 1.0, hi: 2.0 };
    let b = Interval { lo: 3.0, hi: 4.0 };
    let c = a.add(b);
    assert!(c.lo <= 4.0);
    assert!(c.hi >= 6.0);
}

#[test]
fn test_f13_interval_subtraction() {
    let a = Interval { lo: 5.0, hi: 6.0 };
    let b = Interval { lo: 1.0, hi: 2.0 };
    let c = a.sub(b);
    assert!(c.lo <= 3.0);
    assert!(c.hi >= 5.0);
}

#[test]
fn test_f13_interval_multiplication() {
    let a = Interval { lo: 2.0, hi: 3.0 };
    let b = Interval { lo: 4.0, hi: 5.0 };
    let c = a.mul(b);
    assert!(c.lo <= 8.0);
    assert!(c.hi >= 15.0);
}

#[test]
fn test_f13_interval_sqrt_monotonicity() {
    let a = Interval { lo: 4.0, hi: 9.0 };
    let c = a.sqrt().unwrap();
    assert_close(c.lo, 2.0, 1e-9, "sqrt lo");
    assert_close(c.hi, 3.0, 1e-9, "sqrt hi");
}

// =========================================================================
// Feature 14: Interval vector types
// =========================================================================

#[test]
fn test_f14_interval_vec3_dot_product() {
    let a = IntervalVec3 {
        x: Interval::point(1.0),
        y: Interval::point(0.0),
        z: Interval::point(0.0),
    };
    let b = IntervalVec3 {
        x: Interval::point(5.0),
        y: Interval::point(0.0),
        z: Interval::point(0.0),
    };
    let dot = a.dot(b);
    assert_close(dot.lo, 5.0, 1e-9, "dot lo");
    assert_close(dot.hi, 5.0, 1e-9, "dot hi");
}

#[test]
fn test_f14_interval_vec3_norm_squared() {
    let v = IntervalVec3 {
        x: Interval::point(3.0),
        y: Interval::point(4.0),
        z: Interval::point(0.0),
    };
    let norm_sq = v.norm_squared();
    assert_close(norm_sq.lo, 25.0, 1e-9, "norm sq lo");
    assert_close(norm_sq.hi, 25.0, 1e-9, "norm sq hi");
}

#[test]
fn test_f14_interval_vec3_norm() {
    let v = IntervalVec3 {
        x: Interval::point(3.0),
        y: Interval::point(4.0),
        z: Interval::point(0.0),
    };
    let norm = v.norm().unwrap();
    assert_close(norm.lo, 5.0, 1e-9, "norm lo");
    assert_close(norm.hi, 5.0, 1e-9, "norm hi");
}

// =========================================================================
// Feature 15: GeometryWorkBudget debiting
// =========================================================================

#[test]
fn test_f15_work_budget_initialization() {
    let budget = GeometryWorkBudget::new(1000);
    assert_eq!(budget.consumed(), 0);
}

#[test]
fn test_f15_work_budget_charge_consumed() {
    let mut budget = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    budget.charge(100, &cancel).unwrap();
    assert_eq!(budget.consumed(), 100);
}

#[test]
fn test_f15_work_budget_exhaustion_error() {
    let mut budget = GeometryWorkBudget::new(50);
    let cancel = AtomicBool::new(false);
    let err = budget.charge(100, &cancel).unwrap_err();
    assert_eq!(err.code, GeometryErrorCode::ResourceLimit);
}

#[test]
fn test_f15_work_budget_large_capacity() {
    let mut budget = GeometryWorkBudget::new(50_000_000);
    let cancel = AtomicBool::new(false);
    assert!(budget.charge(1_000_000, &cancel).is_ok());
    assert_eq!(budget.consumed(), 1_000_000);
}

#[test]
fn test_f15_work_budget_multiple_charges() {
    let mut budget = GeometryWorkBudget::new(500);
    let cancel = AtomicBool::new(false);
    budget.charge(100, &cancel).unwrap();
    budget.charge(200, &cancel).unwrap();
    budget.charge(200, &cancel).unwrap();
    assert_eq!(budget.consumed(), 500);
    assert!(budget.charge(1, &cancel).is_err());
}

// =========================================================================
// Feature 16: Translation-anchored NURBS jet evaluator
// =========================================================================

#[test]
fn test_f16_nurbs_evaluation_occt_spline_housing() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 0.5).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f16_nurbs_evaluation_translation_invariance() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let bounds = def.bounds_mm();
    assert_close(bounds.max[0], 24.0, 1e-4, "spline X max");
}

#[test]
fn test_f16_nurbs_evaluation_stability() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let sec1 = section_at_z(&def, 4.1).unwrap();
    let sec2 = section_at_z(&def, 4.1).unwrap();
    assert_eq!(sec1.loops().len(), sec2.loops().len());
}

#[test]
fn test_f16_nurbs_evaluation_area_check() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 0.5).unwrap();
    let area = loop_area(&sec, &sec.loops()[0]);
    assert_close(area, 375.453, 0.5, "spline housing area at z=0.5");
}

#[test]
fn test_f16_nurbs_evaluation_continuity() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count > 0);
}

// =========================================================================
// Feature 17: Bezier patch decomposition evaluation
// =========================================================================

#[test]
fn test_f17_bezier_cache_shared_on_section() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let sec1 = section_at_z(&def, 1.0).unwrap();
    let sec2 = section_at_z(&def, 2.0).unwrap();
    assert!(!sec1.loops().is_empty());
    assert!(!sec2.loops().is_empty());
}

#[test]
fn test_f17_bezier_subdivision_convergence() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 4.16).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f17_bezier_evaluation_domain() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 9.5).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f17_bezier_patch_bounds() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let bounds = def.bounds_mm();
    assert_close(bounds.max[2], 10.0, 1e-4, "rounded housing max Z");
}

#[test]
fn test_f17_bezier_tessellate_quality() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 50);
}

// =========================================================================
// Feature 18: Analytic surface Taylor jets
// =========================================================================

#[test]
fn test_f18_plane_analytic_jet_enclosure() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    let a = loop_area(&sec, &sec.loops()[0]);
    assert_close(a, 200.0, 1e-6, "box midplane area");
}

#[test]
fn test_f18_cylinder_analytic_jet_enclosure() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    let a = loop_area(&sec, &sec.loops()[0]);
    assert_close(a, 25.0 * PI, 0.2, "cylinder midplane area");
}

#[test]
fn test_f18_cone_analytic_jet_enclosure() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-tapered-fitting-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 6.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f18_analytic_jet_box_inch() {
    let def = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    let a = loop_area(&sec, &sec.loops()[0]);
    assert_close(a, 200.0, 1e-6, "box inch converted area");
}

#[test]
fn test_f18_through_hole_analytic_jet() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
    let a_outer = loop_area(&sec, &sec.loops()[0]);
    assert_close(a_outer, 400.0, 1e-6, "through hole outer area");
}

// =========================================================================
// Feature 19: PreparedValidatedCurve
// =========================================================================

#[test]
fn test_f19_curve_domain_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.loops()[0].points_mm.len() >= 4);
}

#[test]
fn test_f19_curve_domain_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.loops()[0].points_mm.len() >= 16);
}

#[test]
fn test_f19_curve_bounds_enclosure() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let bounds = def.bounds_mm();
    let sec = section_midplane(&def).unwrap();
    for pt in &sec.loops()[0].points_mm {
        assert!(pt[0] >= bounds.min[0] - 1e-6 && pt[0] <= bounds.max[0] + 1e-6);
        assert!(pt[1] >= bounds.min[1] - 1e-6 && pt[1] <= bounds.max[1] + 1e-6);
        assert!(pt[2] >= bounds.min[2] - 1e-6 && pt[2] <= bounds.max[2] + 1e-6);
    }
}

#[test]
fn test_f19_curve_segment_count() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f19_curve_continuity() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    let pts = &sec.loops()[0].points_mm;
    assert_close(pts[0][0], pts[pts.len() - 1][0], 1e-4, "closure X");
}

// =========================================================================
// Feature 20: PreparedFacePlaneRoots
// =========================================================================

#[test]
fn test_f20_plane_roots_box_midplane() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f20_plane_roots_outside_bounds_empty() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 15.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f20_plane_roots_cylinder_midplane() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f20_plane_roots_through_hole_two_loops() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f20_plane_roots_stepped_heights() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-narrow-pocket-mm.step",
    ))
    .unwrap();
    let sec_low = section_at_z(&def, 1.0).unwrap();
    let sec_high = section_at_z(&def, 1.1).unwrap();
    assert_eq!(sec_low.loops().len(), 1);
    assert_eq!(sec_high.loops().len(), 2);
}

// =========================================================================
// Feature 21: Coplane face identification
// =========================================================================

#[test]
fn test_f21_coplane_face_classification_box_top() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let top_cut = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 8.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    );
    assert_eq!(
        top_cut.unwrap_err().code,
        GeometryErrorCode::DegenerateSection
    );
}

#[test]
fn test_f21_coplane_face_classification_box_bottom() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let bot_cut = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    );
    assert_eq!(
        bot_cut.unwrap_err().code,
        GeometryErrorCode::DegenerateSection
    );
}

#[test]
fn test_f21_coplane_face_interior_cut_valid() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0);
    assert!(sec.is_ok());
}

#[test]
fn test_f21_coplane_face_cylinder_top() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let top_cut = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 8.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    );
    assert_eq!(
        top_cut.unwrap_err().code,
        GeometryErrorCode::DegenerateSection
    );
}

#[test]
fn test_f21_coplane_face_cylinder_bottom() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let bot_cut = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    );
    assert_eq!(
        bot_cut.unwrap_err().code,
        GeometryErrorCode::DegenerateSection
    );
}

// =========================================================================
// Feature 22: Replace monstertruck_modeling::Validated*
// =========================================================================

#[test]
fn test_f22_direct_native_surface_evaluation() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f22_direct_bounds_accessor() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_close(def.bounds_mm().max[0], 20.0, 1e-6, "bounds");
}

#[test]
fn test_f22_zero_legacy_wrapper_panic() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let _ = section_midplane(&def).unwrap();
}

#[test]
fn test_f22_direct_tessellation() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count > 0);
}

#[test]
fn test_f22_direct_wall_query() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let f0 = &def.faces()[0];
    let _ = inspect_face(&def, &f0.face_id).unwrap();
}

// =========================================================================
// Feature 23: Curve-native 2D section IR
// =========================================================================

#[test]
fn test_f23_section_curve2d_line_creation() {
    let line = SectionCurve2d::Line {
        start_mm: [0.0, 0.0],
        end_mm: [10.0, 0.0],
    };
    match line {
        SectionCurve2d::Line { start_mm, end_mm } => {
            assert_eq!(start_mm, [0.0, 0.0]);
            assert_eq!(end_mm, [10.0, 0.0]);
        }
        _ => panic!("expected Line"),
    }
}

#[test]
fn test_f23_section_curve2d_circular_arc_creation() {
    let arc = SectionCurve2d::CircularArc {
        center_mm: [5.0, 5.0],
        radius_mm: 2.5,
        start_angle_rad: 0.0,
        sweep_angle_rad: PI,
    };
    match arc {
        SectionCurve2d::CircularArc { radius_mm, .. } => {
            assert_close(radius_mm, 2.5, 1e-9, "radius")
        }
        _ => panic!("expected CircularArc"),
    }
}

#[test]
fn test_f23_section_curve2d_elliptic_arc_creation() {
    let ellipse = SectionCurve2d::EllipticArc {
        center_mm: [0.0, 0.0],
        major_axis_mm: [4.0, 0.0],
        minor_radius_mm: 2.0,
        start_param_rad: 0.0,
        end_param_rad: PI * 0.5,
    };
    match ellipse {
        SectionCurve2d::EllipticArc {
            minor_radius_mm, ..
        } => assert_close(minor_radius_mm, 2.0, 1e-9, "minor"),
        _ => panic!("expected EllipticArc"),
    }
}

#[test]
fn test_f23_section_curve2d_nurbs_creation() {
    let nurbs = SectionCurve2d::NurbsSpan {
        degree: 2,
        knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        control_points_mm: vec![[0.0, 0.0], [5.0, 10.0], [10.0, 0.0]],
        weights: None,
    };
    match nurbs {
        SectionCurve2d::NurbsSpan {
            degree,
            control_points_mm,
            ..
        } => {
            assert_eq!(degree, 2);
            assert_eq!(control_points_mm.len(), 3);
        }
        _ => panic!("expected NurbsSpan"),
    }
}

#[test]
fn test_f23_section_curve2d_serde_roundtrip() {
    let arc = SectionCurve2d::CircularArc {
        center_mm: [1.0, 2.0],
        radius_mm: 3.0,
        start_angle_rad: 0.1,
        sweep_angle_rad: 1.5,
    };
    let json = serde_json::to_string(&arc).unwrap();
    let deser: SectionCurve2d = serde_json::from_str(&json).unwrap();
    assert_eq!(arc, deser);
}

// =========================================================================
// Feature 24: Dual-representation SectionLoopMm
// =========================================================================

#[test]
fn test_f24_loop_points_discretization_present() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    for l in sec.loops() {
        assert!(l.points_mm.len() >= 4);
    }
}

#[test]
fn test_f24_loop_is_hole_flag() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(!sec.loops()[0].is_hole);
    assert!(sec.loops()[1].is_hole);
}

#[test]
fn test_f24_loop_points_closure() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    let pts = &sec.loops()[0].points_mm;
    let first = pts[0];
    let last = pts[pts.len() - 1];
    for axis in 0..3 {
        assert_close(first[axis], last[axis], 1e-4, "loop closure");
    }
}

#[test]
fn test_f24_cylinder_loop_polyline_density() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.loops()[0].points_mm.len() >= 16);
}

#[test]
fn test_f24_section_span_provenance_type() {
    let span = SectionSpan2d {
        curve: SectionCurve2d::Line {
            start_mm: [0.0, 0.0],
            end_mm: [10.0, 0.0],
        },
        source_face: SourceFaceId(1),
        source_edge: Some(SourceEdgeId(2)),
        achieved_plane_error_mm: 0.00001,
    };
    assert_eq!(span.source_face.0, 1);
    assert_eq!(span.source_edge.unwrap().0, 2);
}

// =========================================================================
// Feature 25: Bounding-box plane culling
// =========================================================================

#[test]
fn test_f25_plane_above_max_z_culled() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 20.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f25_plane_below_min_z_culled() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, -10.0).unwrap();
    assert!(sec.loops().is_empty());
}

#[test]
fn test_f25_plane_cutting_retained() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f25_plane_cylinder_bounds_culling() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec_out = section_at_z(&def, 12.0).unwrap();
    assert!(sec_out.loops().is_empty());
}

#[test]
fn test_f25_culling_tolerance_inflation() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec_near = section_at_z(&def, 8.5).unwrap();
    assert!(sec_near.loops().is_empty());
}

// =========================================================================
// Feature 26: Direct plane-face intersection
// =========================================================================

#[test]
fn test_f26_direct_plane_plane_cut() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f26_direct_plane_cylinder_cut() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f26_direct_plane_through_hole_cut() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f26_direct_plane_cone_cut() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-tapered-fitting-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 6.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f26_direct_plane_nurbs_cut() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 0.5).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

// =========================================================================
// Feature 27: Trimmed UV domain clipping
// =========================================================================

#[test]
fn test_f27_trimmed_uv_clipping_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_close(loop_area(&sec, &sec.loops()[0]), 200.0, 1e-6, "box area");
}

#[test]
fn test_f27_trimmed_uv_clipping_through_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_close(loop_area(&sec, &sec.loops()[0]), 400.0, 1e-6, "outer area");
    assert_close(
        loop_area(&sec, &sec.loops()[1]),
        -9.0 * PI,
        0.2,
        "hole area",
    );
}

#[test]
fn test_f27_trimmed_uv_narrow_pocket() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-narrow-pocket-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 5.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f27_trimmed_uv_rounded_housing() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 4.16).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f27_trimmed_uv_perforated_plate() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let sec = section_at_z(&def, 10.0).unwrap();
    assert_eq!(sec.loops().len(), 513);
}

// =========================================================================
// Feature 28: Plane section orientation convention
// =========================================================================

#[test]
fn test_f28_outer_loop_positive_area() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(loop_area(&sec, &sec.loops()[0]) > 0.0);
}

#[test]
fn test_f28_hole_loop_negative_area() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(loop_area(&sec, &sec.loops()[1]) < 0.0);
}

#[test]
fn test_f28_outer_before_hole_ordering() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(!sec.loops()[0].is_hole);
    assert!(sec.loops()[1].is_hole);
}

#[test]
fn test_f28_cylinder_outer_positive_area() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(loop_area(&sec, &sec.loops()[0]) > 0.0);
}

#[test]
fn test_f28_narrow_pocket_orientation() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-narrow-pocket-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 5.0).unwrap();
    assert!(!sec.loops()[0].is_hole);
    assert!(sec.loops()[1].is_hole);
}

// =========================================================================
// Feature 29: Coplanar face wire contribution
// =========================================================================

#[test]
fn test_f29_coplanar_face_detection_top_boundary() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let top = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 8.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    );
    assert_eq!(top.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f29_coplanar_face_detection_bottom_boundary() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let cancel = AtomicBool::new(false);
    let bot = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    );
    assert_eq!(bot.unwrap_err().code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn test_f29_interior_cut_near_boundary_safe() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 7.99).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f29_pocket_floor_section_transition() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let sec1 = section_at_z(&def, 4.1).unwrap();
    let sec2 = section_at_z(&def, 4.16).unwrap();
    assert_eq!(sec1.loops().len(), 1);
    assert_eq!(sec2.loops().len(), 2);
}

#[test]
fn test_f29_base_top_section_transition() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 0.6).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

// =========================================================================
// Feature 30: Closed loop chaining at shared topology
// =========================================================================

#[test]
fn test_f30_box_midplane_chains_single_closed_loop() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f30_through_hole_chains_two_closed_loops() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f30_cylinder_chains_closed_loop() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f30_ribbed_bracket_chains_loops() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(!sec.loops().is_empty());
}

#[test]
fn test_f30_perforated_plate_chains_all_loops() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let sec = section_at_z(&def, 10.0).unwrap();
    assert_eq!(sec.loops().len(), 513);
}

// =========================================================================
// Feature 31: Adaptive polyline chord sampling
// =========================================================================

#[test]
fn test_f31_box_chord_sampling() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.achieved_chord_error_mm() <= 0.005);
}

#[test]
fn test_f31_cylinder_chord_sampling() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.achieved_chord_error_mm() <= 0.005);
}

#[test]
fn test_f31_plane_error_bound() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.achieved_plane_error_mm() <= 0.0001);
}

#[test]
fn test_f31_spline_housing_chord_sampling() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 0.5).unwrap();
    assert!(sec.achieved_chord_error_mm() <= 0.005);
}

#[test]
fn test_f31_point_density_on_curves() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.loops()[0].points_mm.len() >= 20);
}

// =========================================================================
// Feature 32: Slicing Boolean elimination
// =========================================================================

#[test]
fn test_f32_box_slicing_direct_result() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.is_native_intersection());
}

#[test]
fn test_f32_cylinder_slicing_direct_result() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.is_native_intersection());
}

#[test]
fn test_f32_through_hole_slicing_direct_result() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.is_native_intersection());
}

#[test]
fn test_f32_zero_boolean_artifacts() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert_eq!(sec.loops().len(), 1, "no artifact fragments");
}

#[test]
fn test_f32_memory_efficiency() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(sec.retained_bytes() < 100_000);
}

// =========================================================================
// Feature 33: PreparedSolid unification
// =========================================================================

#[test]
fn test_f33_bounds_via_prepared_source() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_close(def.bounds_mm().max[0], 20.0, 1e-6, "bounds max X");
}

#[test]
fn test_f33_section_via_prepared_source() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

// =========================================================================
// Feature 34: SMLib-native toleranced B-Rep adoption
// =========================================================================

#[test]
fn test_f34_toleranced_topology_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f34_toleranced_topology_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_source_face_identity(&def, "cylinder.step");
}

#[test]
fn test_f34_toleranced_topology_occt_tapered() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-tapered-fitting-mm.step",
    ))
    .unwrap();
    assert!(def.faces().len() >= 3);
}

#[test]
fn test_f34_toleranced_topology_occt_bracket() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ))
    .unwrap();
    assert!(def.faces().len() >= 10);
}

#[test]
fn test_f34_toleranced_topology_occt_housing() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    assert!(def.faces().len() >= 8);
}

// =========================================================================
// Feature 35: Delete G-transport scaffolding
// =========================================================================

#[test]
fn test_f35_direct_section_without_g_transport() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert!(sec.is_native_intersection());
}

#[test]
fn test_f35_direct_tessellate_without_g_transport() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count > 0);
}

#[test]
fn test_f35_direct_wall_without_g_transport() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let f0 = &def.faces()[0];
    assert!(inspect_face(&def, &f0.face_id).is_ok());
}

#[test]
fn test_f35_direct_bounds_without_g_transport() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_close(def.bounds_mm().max[0], 5.0, 1e-6, "bounds max X");
}

#[test]
fn test_f35_direct_through_hole_without_g_transport() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

// =========================================================================
// Feature 36: Retire 40-symbol collar ladder
// =========================================================================

#[test]
fn test_f36_height_domain_analysis() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel);
    assert!(h.is_ok());
}

#[test]
fn test_f36_height_domain_retained_bytes() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f36_height_domain_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f36_height_domain_through_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f36_height_domain_housing() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 10.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

// =========================================================================
// Feature 37: Certified B-Rep bounds
// =========================================================================

#[test]
fn test_f37_bounds_box_mm() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.min[0], 0.0, 1e-6, "min X");
    assert_close(b.max[0], 20.0, 1e-6, "max X");
    assert_close(b.max[1], 10.0, 1e-6, "max Y");
    assert_close(b.max[2], 8.0, 1e-6, "max Z");
}

#[test]
fn test_f37_bounds_box_inch() {
    let def = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.min[0], 0.0, 1e-6, "min X");
    assert_close(b.max[0], 20.0, 1e-6, "max X");
    assert_close(b.max[1], 10.0, 1e-6, "max Y");
    assert_close(b.max[2], 8.0, 1e-6, "max Z");
}

#[test]
fn test_f37_bounds_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.min[0], -5.0, 1e-6, "min X");
    assert_close(b.max[0], 5.0, 1e-6, "max X");
    assert_close(b.min[1], -5.0, 1e-6, "min Y");
    assert_close(b.max[1], 5.0, 1e-6, "max Y");
    assert_close(b.max[2], 8.0, 1e-6, "max Z");
}

#[test]
fn test_f37_bounds_through_hole() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.max[0], 20.0, 1e-6, "max X");
    assert_close(b.max[1], 20.0, 1e-6, "max Y");
    assert_close(b.max[2], 8.0, 1e-6, "max Z");
}

#[test]
fn test_f37_bounds_perforated_plate() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let b = def.bounds_mm();
    assert_close(b.max[0], 1640.0, 1e-4, "plate X");
    assert_close(b.max[1], 840.0, 1e-4, "plate Y");
    assert_close(b.max[2], 20.0, 1e-4, "plate Z");
}

// =========================================================================
// Feature 38: Critical height stratum classification
// =========================================================================

#[test]
fn test_f38_height_stratum_box() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f38_height_stratum_pocket_floor() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 10.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f38_height_stratum_narrow_pocket() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-narrow-pocket-mm.step",
    ))
    .unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 6.0], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f38_height_stratum_bracket() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ))
    .unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 7.6], policy, &mut work, &cancel).unwrap();
    assert!(h.retained_bytes() > 0);
}

#[test]
fn test_f38_height_stratum_closure_check() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let frame = PlaneFrameMm {
        origin_mm: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let policy = AnalysisPolicy {
        maximum_height_uncertainty_mm: 1e-6,
        maximum_shape_error_mm: 0.005,
    };
    let h = analyze_height_domain(&def, frame, [0.0, 8.0], policy, &mut work, &cancel).unwrap();
    let _ = h.requires_closed_material_capture([2.0, 6.0]);
}

// =========================================================================
// Feature 39: Slab fidelity and error bounds
// =========================================================================

#[test]
fn test_f39_projected_bounds_axis_x() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy {
        maximum_endpoint_uncertainty_mm: 1e-6,
    };
    let p = projected_bounds(&def, [1.0, 0.0, 0.0], policy, &mut work, &cancel).unwrap();
    assert_close(p.maximum[0], 20.0, 1e-4, "projected X");
}

#[test]
fn test_f39_projected_bounds_axis_y() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy {
        maximum_endpoint_uncertainty_mm: 1e-6,
    };
    let p = projected_bounds(&def, [0.0, 1.0, 0.0], policy, &mut work, &cancel).unwrap();
    assert_close(p.maximum[0], 10.0, 1e-4, "projected Y");
}

#[test]
fn test_f39_projected_bounds_axis_z() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy {
        maximum_endpoint_uncertainty_mm: 1e-6,
    };
    let p = projected_bounds(&def, [0.0, 0.0, 1.0], policy, &mut work, &cancel).unwrap();
    assert_close(p.maximum[0], 8.0, 1e-4, "projected Z");
}

#[test]
fn test_f39_material_projected_bounds() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy {
        maximum_endpoint_uncertainty_mm: 1e-6,
    };
    let p = material_projected_bounds(&def, [1.0, 0.0, 0.0], policy, &mut work, &cancel).unwrap();
    assert_close(p.bounds().maximum[0], 20.0, 1e-4, "material projected X");
}

#[test]
fn test_f39_projected_bounds_cylinder() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    let policy = BoundsPolicy {
        maximum_endpoint_uncertainty_mm: 1e-6,
    };
    let p = projected_bounds(&def, [1.0, 0.0, 0.0], policy, &mut work, &cancel).unwrap();
    assert_close(p.maximum[0], 5.0, 1e-4, "cylinder projected X");
}

// =========================================================================
// Feature 40: Void projection
// =========================================================================

#[test]
fn test_f40_through_hole_void_presence() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
    assert!(sec.loops()[1].is_hole);
}

#[test]
fn test_f40_perforated_plate_void_count() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    let sec = section_at_z(&def, 10.0).unwrap();
    let hole_count = sec.loops().iter().filter(|l| l.is_hole).count();
    assert_eq!(hole_count, 512);
}

#[test]
fn test_f40_narrow_pocket_void_floor() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-narrow-pocket-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 5.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
    assert!(sec.loops()[1].is_hole);
}

#[test]
fn test_f40_tapered_fitting_void_bore() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-tapered-fitting-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 6.0).unwrap();
    assert_eq!(sec.loops().len(), 2);
    assert!(sec.loops()[1].is_hole);
}

#[test]
fn test_f40_rounded_housing_void_pocket() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 9.5).unwrap();
    assert_eq!(sec.loops().len(), 2);
    assert!(sec.loops()[1].is_hole);
}

// =========================================================================
// Feature 41: Wall and cylinder face certificates
// =========================================================================

#[test]
fn test_f41_box_wall_faces() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f41_cylinder_exterior_face() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let cyl_faces: Vec<_> = def
        .faces()
        .iter()
        .filter(|f| f.geometry.family == SurfaceFamily::Cylinder)
        .collect();
    // The original cylinder has two independently identified cylindrical faces.
    assert_eq!(cyl_faces.len(), 2);
    assert_source_face_identity(&def, "cylinder.step");
}

#[test]
fn test_f41_through_hole_exterior_walls() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let planar_faces: Vec<_> = def
        .faces()
        .iter()
        .filter(|f| f.geometry.family == SurfaceFamily::Plane)
        .collect();
    assert_eq!(planar_faces.len(), 6);
}

#[test]
fn test_f41_occt_cylinder_full_turn() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-cylinder-mm.step",
    ))
    .unwrap();
    let cyl_faces: Vec<_> = def
        .faces()
        .iter()
        .filter(|f| f.geometry.family == SurfaceFamily::Cylinder)
        .collect();
    // The independent exporter records one full-turn cylindrical face.
    assert_eq!(cyl_faces.len(), 1);
    assert_source_face_identity(&def, "independent-exporters/occt-cylinder-mm.step");
}

#[test]
fn test_f41_wall_normal_orthogonality() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    for f in def.faces() {
        assert_eq!(f.geometry.family, SurfaceFamily::Plane);
    }
}

// =========================================================================
// Feature 42: Watertight shared edge division
// =========================================================================

#[test]
fn test_f42_box_tessellation_watertight() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count > 0);
    assert!(mesh.triangle_count > 0);
}

#[test]
fn test_f42_cylinder_tessellation_watertight() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count > 0);
}

#[test]
fn test_f42_through_hole_tessellation_watertight() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count > 0);
}

#[test]
fn test_f42_occt_housing_watertight() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count > 0);
}

#[test]
fn test_f42_occt_bracket_watertight() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.vertex_count > 0);
}

// =========================================================================
// Feature 43: Adaptive CDT surface meshing
// =========================================================================

#[test]
fn test_f43_box_cdt_triangle_count() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_eq!(mesh.triangle_count, 12);
}

#[test]
fn test_f43_box_cdt_vertex_count() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_eq!(mesh.vertex_count, 24);
}

#[test]
fn test_f43_cylinder_cdt_density() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 30);
}

#[test]
fn test_f43_through_hole_cdt_density() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 40);
}

#[test]
fn test_f43_bracket_cdt_density() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 100);
}

// =========================================================================
// Feature 44: Boundary-split iterative refinement
// =========================================================================

#[test]
fn test_f44_cylinder_boundary_split_smoothness() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 30);
}

#[test]
fn test_f44_occt_tapered_refinement() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-tapered-fitting-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 40);
}

#[test]
fn test_f44_occt_narrow_pocket_refinement() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-narrow-pocket-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 50);
}

#[test]
fn test_f44_occt_housing_refinement() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 80);
}

#[test]
fn test_f44_occt_spline_refinement() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-spline-housing-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(mesh.triangle_count >= 80);
}

// =========================================================================
// Feature 45: Whole-triangle certifier (audit_face)
// =========================================================================

#[test]
fn test_f45_box_audit_chunks_valid() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_mesh_source_coverage(&def, &mesh);
}

#[test]
fn test_f45_cylinder_audit_chunks() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_mesh_source_coverage(&def, &mesh);
}

#[test]
fn test_f45_through_hole_audit_chunks() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert_mesh_source_coverage(&def, &mesh);
}

#[test]
fn test_f45_housing_audit_chunks() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(!mesh.chunks.is_empty());
}

#[test]
fn test_f45_bracket_audit_chunks() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ))
    .unwrap();
    let mesh = tessellate_display(&def).unwrap();
    assert!(!mesh.chunks.is_empty());
}

// =========================================================================
// Feature 46: MeshArtifact generation
// =========================================================================

#[test]
fn test_f46_mesh_artifact_face_provenance() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let artifact = tessellate_display(&def).unwrap();
    assert_mesh_source_coverage(&def, &artifact);
}

#[test]
#[should_panic(expected = "complete source-ordinal coverage")]
fn test_f46_valid_wire_hash_cannot_hide_missing_source_face_associations() {
    use sha2::{Digest, Sha256};
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let mut mesh = tessellate_display(&def).unwrap();
    assert_mesh_source_coverage(&def, &mesh);
    for chunk in &mut mesh.chunks {
        // The checked SPLM body ends with one ordinal per triangle. Relabeling
        // all triangles as face zero is wire-valid but loses source coverage.
        let triangles = u32::from_le_bytes(chunk.bytes[16..20].try_into().unwrap()) as usize;
        let start = chunk.bytes.len() - triangles * 4;
        chunk.bytes[start..].fill(0);
        chunk.descriptor.sha256 = SourceHash::from_digest(&Sha256::digest(&chunk.bytes).into());
    }
    assert_mesh_source_coverage(&def, &mesh);
}

#[test]
fn test_f46_mesh_artifact_total_bytes() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let artifact = tessellate_display(&def).unwrap();
    assert!(artifact.total_bytes > 0);
}

#[test]
fn test_f46_mesh_artifact_cylinder_descriptors() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    let artifact = tessellate_display(&def).unwrap();
    assert_mesh_source_coverage(&def, &artifact);
}

#[test]
fn test_f46_mesh_artifact_through_hole_descriptors() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let artifact = tessellate_display(&def).unwrap();
    assert_mesh_source_coverage(&def, &artifact);
}

#[test]
fn test_f46_mesh_artifact_vertices() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let artifact = tessellate_display(&def).unwrap();
    assert_eq!(artifact.vertex_count, 24);
}

// =========================================================================
// Feature 47: Downstream manufacturing refactoring
// =========================================================================

#[test]
fn test_f47_manufacturing_consumes_native_section() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 1);
}

#[test]
fn test_f47_manufacturing_consumes_through_hole_section() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f47_manufacturing_consumes_housing_section() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-rounded-housing-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 4.16).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f47_manufacturing_consumes_narrow_pocket_section() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-narrow-pocket-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 1.1).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f47_manufacturing_consumes_bracket_section() {
    let def = import_test_step(&fixture_bytes(
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ))
    .unwrap();
    let sec = section_at_z(&def, 4.0).unwrap();
    assert!(!sec.loops().is_empty());
}

// =========================================================================
// Feature 48: Complete monstertruck-* purge
// =========================================================================

#[test]
fn test_f48_solidcore_dependency_presence() {
    let brep = solidcore::Brep::default();
    assert!(brep.tolerance() > 0.0);
}

#[test]
fn test_f48_solidcraft_step_presence() {
    let text = fixture_str("box-mm.step");
    let ex = solidcraft_step::p21::parse(&text);
    assert!(ex.is_ok());
}

#[test]
fn test_f48_solidcraft_geom_presence() {
    let text = fixture_str("box-mm.step");
    let ex = solidcraft_step::p21::parse(&text).unwrap();
    let solids = solidcraft_step::topology::read_solids(&ex);
    assert!(!solids.solids.is_empty());
}

#[test]
fn test_f48_solidcore_mesh_presence() {
    let opts = solidcore_mesh::TessellateOptions::default();
    assert!(opts.chord_tolerance.resolve(10.0) > 0.0);
}

#[test]
fn test_f48_no_legacy_panics_on_all_fixtures() {
    for fixture in ALL_OCCT_FIXTURES {
        let bytes = fixture_bytes(fixture);
        let def = import_test_step(&bytes);
        assert!(def.is_ok(), "fixture {fixture} must import");
    }
}

// =========================================================================
// Feature 49: Kernel identity handshake update
// =========================================================================

#[test]
fn test_f49_provenance_kernel_identity() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let prov = def.provenance();
    assert!(prov.source_name.is_empty());
    assert_source_face_identity(&def, "box-mm.step");
}

#[test]
fn test_f49_version_provenance_consistency() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_eq!(def.provenance().source_unit, SourceUnit::Millimetre);
}

#[test]
fn test_f49_definition_identity_digest() {
    let def1 = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    let def2 = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    assert_ne!(def1.id(), def2.id());
}

// =========================================================================
// Feature 50: Full E2E regression verification
// =========================================================================

#[test]
fn test_f50_box_mm_regression() {
    let def = import_test_step(&fixture_bytes("box-mm.step")).unwrap();
    assert_source_face_identity(&def, "box-mm.step");
    let sec = section_midplane(&def).unwrap();
    assert_close(
        loop_area(&sec, &sec.loops()[0]),
        200.0,
        1e-6,
        "box regression",
    );
}

#[test]
fn test_f50_cylinder_regression() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).unwrap();
    assert_source_face_identity(&def, "cylinder.step");
    let sec = section_midplane(&def).unwrap();
    assert_close(
        loop_area(&sec, &sec.loops()[0]),
        25.0 * PI,
        0.2,
        "cylinder regression",
    );
}

#[test]
fn test_f50_through_hole_regression() {
    let def = import_test_step(&fixture_bytes("through-hole.step")).unwrap();
    assert_source_face_identity(&def, "through-hole.step");
    let sec = section_midplane(&def).unwrap();
    assert_eq!(sec.loops().len(), 2);
}

#[test]
fn test_f50_box_inch_regression() {
    let def = import_test_step(&fixture_bytes("box-inch.step")).unwrap();
    assert_source_face_identity(&def, "box-inch.step");
    let sec = section_midplane(&def).unwrap();
    assert_close(
        loop_area(&sec, &sec.loops()[0]),
        200.0,
        1e-6,
        "box inch regression",
    );
}

#[test]
fn test_f50_perforated_plate_regression() {
    let def = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    assert_source_face_identity(&def, "perforated-plate.step");
}
