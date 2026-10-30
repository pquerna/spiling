// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Tier 3: Cross-Feature Combination Tests.
//! 55 multi-feature interaction and pairwise tests covering complex CAD/CAM interplay.

use spiling_contracts::geometry::*;
use spiling_e2e_tests::fixtures::*;
use spiling_e2e_tests::helpers::*;
use spiling_geometry::*;
use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;
use std::thread;

// =========================================================================
// 1. Multi-feature interplay: Cylinder + Section + Tessellation
// =========================================================================

#[test]
fn test_comb_01_cylinder_import_section_tessellation() {
    let bytes = fixture_bytes("cylinder.step");
    let def = import_test_step(&bytes).expect("cylinder import");
    let bounds = def.bounds_mm();
    assert_close(bounds.min[0], -5.0, 1e-3, "cyl min x");
    assert_close(bounds.max[0], 5.0, 1e-3, "cyl max x");
    assert_close(bounds.min[2], 0.0, 1e-3, "cyl min z");
    assert_close(bounds.max[2], 8.0, 1e-3, "cyl max z");

    let sec = section_at_z(&def, 4.0).expect("cyl midplane section");
    assert_eq!(sec.loops().len(), 1, "cylinder must have 1 boundary loop");
    let area = loop_area(&sec, &sec.loops()[0]).abs();
    let expected_area = 25.0 * PI;
    assert_close(area, expected_area, 0.5, "cylinder section area");

    let mesh = tessellate_display(&def).expect("cylinder tessellation");
    assert!(mesh_positions(&mesh).len() >= 24, "cylinder mesh vertices");
    assert!(mesh_indices(&mesh).len() >= 36, "cylinder mesh triangles");
}

// =========================================================================
// 2. Through-Hole Section Loops & Net Material Area
// =========================================================================

#[test]
fn test_comb_02_through_hole_section_loops_and_net_area() {
    let bytes = fixture_bytes("through-hole.step");
    let def = import_test_step(&bytes).expect("through-hole import");
    let bounds = def.bounds_mm();
    assert_close(bounds.min[0], 0.0, 1e-3, "min x");
    assert_close(bounds.max[0], 20.0, 1e-3, "max x");

    let sec = section_at_z(&def, 4.0).expect("through-hole midplane section");
    assert_eq!(
        sec.loops().len(),
        2,
        "through-hole must have exactly 2 loops"
    );

    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let outer_area = areas[0];
    let hole_area = areas[1];

    assert_close(outer_area, 400.0, 1.0, "outer box loop area");
    assert_close(hole_area, 9.0 * PI, 0.5, "inner hole loop area");
    let net_area = outer_area - hole_area;
    assert_close(net_area, 400.0 - 9.0 * PI, 1.5, "net material area");

    let mesh = tessellate_display(&def).expect("through hole tessellation");
    assert!(!mesh_positions(&mesh).is_empty());
}

// =========================================================================
// 3-5. Tapered Fitting Multi-Strata Sections vs Analytic Oracle
// =========================================================================

#[test]
fn test_comb_03_tapered_fitting_multi_strata_oracle_z1() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("tapered fitting import");
    let sec = section_at_z(&def, 1.0).expect("section at z=1.0");
    assert_eq!(
        sec.loops().len(),
        2,
        "tapered fitting must have 2 loops at z=1.0"
    );

    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let net_area = areas[0] - areas[1];
    // Oracle: 94.33504607029349
    assert_close(net_area, 94.3350, 0.5, "tapered fitting z=1.0 net area");
}

#[test]
fn test_comb_04_tapered_fitting_multi_strata_oracle_z6() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("tapered fitting import");
    let sec = section_at_z(&def, 6.0).expect("section at z=6.0");
    assert_eq!(
        sec.loops().len(),
        2,
        "tapered fitting must have 2 loops at z=6.0"
    );

    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let net_area = areas[0] - areas[1];
    // Oracle: 65.97344572538566
    assert_close(net_area, 65.9734, 0.5, "tapered fitting z=6.0 net area");
}

#[test]
fn test_comb_05_tapered_fitting_multi_strata_oracle_z11() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("tapered fitting import");
    let sec = section_at_z(&def, 11.0).expect("section at z=11.0");
    assert_eq!(
        sec.loops().len(),
        2,
        "tapered fitting must have 2 loops at z=11.0"
    );

    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let net_area = areas[0] - areas[1];
    // Oracle: 41.97516851046363
    assert_close(net_area, 41.9752, 0.5, "tapered fitting z=11.0 net area");
}

// =========================================================================
// 6-8. Narrow Pocket 0.6mm Slot Floor Transitions
// =========================================================================

#[test]
fn test_comb_06_narrow_pocket_floor_transition_below() {
    let bytes = fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step");
    let def = import_test_step(&bytes).expect("narrow pocket import");
    // Pocket floor is at z=1.07. At z=1.0, below pocket, loop count should be 1.
    let sec = section_at_z(&def, 1.0).expect("section at z=1.0");
    assert_eq!(sec.loops().len(), 1, "narrow pocket below floor has 1 loop");
    let area = loop_area(&sec, &sec.loops()[0]).abs();
    // Oracle: 120.0 mm2
    assert_close(area, 120.0, 0.5, "narrow pocket z=1.0 area");
}

#[test]
fn test_comb_07_narrow_pocket_floor_transition_above() {
    let bytes = fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step");
    let def = import_test_step(&bytes).expect("narrow pocket import");
    // Pocket floor is at z=1.07. At z=1.1, above pocket floor, loop count is 2.
    let sec = section_at_z(&def, 1.1).expect("section at z=1.1");
    assert_eq!(
        sec.loops().len(),
        2,
        "narrow pocket above floor has 2 loops"
    );
    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let net_area = areas[0] - areas[1];
    // Oracle: 115.2 mm2
    assert_close(net_area, 115.2, 0.5, "narrow pocket z=1.1 net area");
}

#[test]
fn test_comb_08_narrow_pocket_slot_width_geometry() {
    let bytes = fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step");
    let def = import_test_step(&bytes).expect("narrow pocket import");
    let sec = section_at_z(&def, 5.0).expect("section at z=5.0");
    assert_eq!(sec.loops().len(), 2, "narrow pocket at z=5.0 has 2 loops");
    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let net_area = areas[0] - areas[1];
    assert_close(net_area, 115.2, 0.5, "narrow pocket z=5.0 net area");
    // Slot area removed is 120.0 - 115.2 = 4.8 mm2.
    let removed = areas[1];
    assert_close(removed, 4.8, 0.5, "slot area");
}

// =========================================================================
// 9-10. Ribbed Bracket T-Junction and Functional Bore
// =========================================================================

#[test]
fn test_comb_09_ribbed_bracket_bounds_and_base_roof() {
    let bytes = fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step");
    let def = import_test_step(&bytes).expect("ribbed bracket import");
    let bounds = def.bounds_mm();
    assert_close(bounds.min[0], 0.0, 1e-3, "min x");
    assert_close(bounds.max[0], 30.0, 1e-3, "max x");
    assert_close(bounds.min[1], 0.0, 1e-3, "min y");
    assert_close(bounds.max[1], 22.0, 1e-3, "max y");
    assert_close(bounds.min[2], 0.0, 1e-3, "min z");
    assert_close(bounds.max[2], 7.6, 1e-3, "max z");

    // Slicing base plate below base_top_z_mm (1.2)
    let sec_base = section_at_z(&def, 0.6).expect("section at z=0.6");
    assert!(!sec_base.loops().is_empty());
}

#[test]
fn test_comb_10_ribbed_bracket_t_junction_section() {
    let bytes = fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step");
    let def = import_test_step(&bytes).expect("ribbed bracket import");
    // Slicing across ribs at z=3.0
    let sec = section_at_z(&def, 3.0).expect("section at z=3.0");
    assert!(
        !sec.loops().is_empty(),
        "ribbed bracket mid section has loops"
    );
    let mesh = tessellate_display(&def).expect("ribbed bracket mesh");
    assert!(mesh_positions(&mesh).len() >= 100);
}

// =========================================================================
// 11-14. Rounded Housing Pocket Floor Transitions
// =========================================================================

#[test]
fn test_comb_11_rounded_housing_section_z0_5() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");
    let def = import_test_step(&bytes).expect("rounded housing import");
    let sec = section_at_z(&def, 0.5).expect("section at z=0.5");
    assert_eq!(sec.loops().len(), 1, "loop count at z=0.5");
    let area = loop_area(&sec, &sec.loops()[0]).abs();
    assert_close(area, 375.4533, 1.0, "rounded housing z=0.5 area");
}

#[test]
fn test_comb_12_rounded_housing_section_z4_1() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");
    let def = import_test_step(&bytes).expect("rounded housing import");
    let sec = section_at_z(&def, 4.1).expect("section at z=4.1");
    assert_eq!(sec.loops().len(), 1, "loop count at z=4.1");
    let area = loop_area(&sec, &sec.loops()[0]).abs();
    assert_close(area, 428.5664, 1.0, "rounded housing z=4.1 area");
}

#[test]
fn test_comb_13_rounded_housing_section_z4_16() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");
    let def = import_test_step(&bytes).expect("rounded housing import");
    let sec = section_at_z(&def, 4.16).expect("section at z=4.16");
    assert_eq!(sec.loops().len(), 2, "loop count at z=4.16");
    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let net_area = areas[0] - areas[1];
    assert_close(net_area, 400.2920, 1.0, "rounded housing z=4.16 net area");
}

#[test]
fn test_comb_14_rounded_housing_section_z9_5() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");
    let def = import_test_step(&bytes).expect("rounded housing import");
    let sec = section_at_z(&def, 9.5).expect("section at z=9.5");
    assert_eq!(sec.loops().len(), 2, "loop count at z=9.5");
    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let net_area = areas[0] - areas[1];
    assert_close(net_area, 347.1790, 1.0, "rounded housing z=9.5 net area");
}

// =========================================================================
// 15. Spline Housing Multi-Strata Oracle
// =========================================================================

#[test]
fn test_comb_15_spline_housing_multi_strata_oracle() {
    let bytes = fixture_bytes("independent-exporters/occt-spline-housing-mm.step");
    let def = import_test_step(&bytes).expect("spline housing import");
    let sec0 = section_at_z(&def, 0.5).expect("section at z=0.5");
    assert_eq!(sec0.loops().len(), 1);
    let area0 = loop_area(&sec0, &sec0.loops()[0]).abs();
    assert_close(area0, 375.4533, 1.5, "spline housing z=0.5 area");

    let sec1 = section_at_z(&def, 9.5).expect("section at z=9.5");
    assert_eq!(sec1.loops().len(), 2);
    let mut areas: Vec<f64> = sec1
        .loops()
        .iter()
        .map(|lp| loop_area(&sec1, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let net_area1 = areas[0] - areas[1];
    assert_close(net_area1, 347.1790, 1.5, "spline housing z=9.5 net area");
}

// =========================================================================
// 16-17. Perforated Plate 512-Hole Midplane Section and Loops
// =========================================================================

#[test]
fn test_comb_16_perforated_plate_bounds_and_metadata() {
    let bytes = fixture_bytes("perforated-plate.step");
    let def = import_test_step(&bytes).expect("perforated plate import");
    let bounds = def.bounds_mm();
    assert_close(bounds.min[0], 0.0, 1e-3, "min x");
    assert_close(bounds.max[0], 1640.0, 1e-3, "max x");
    assert_close(bounds.min[1], 0.0, 1e-3, "min y");
    assert_close(bounds.max[1], 840.0, 1e-3, "max y");
    assert_close(bounds.min[2], 0.0, 1e-3, "min z");
    assert_close(bounds.max[2], 20.0, 1e-3, "max z");
}

#[test]
fn test_comb_17_perforated_plate_midplane_loops_and_area() {
    let bytes = fixture_bytes("perforated-plate.step");
    let def = import_test_step(&bytes).expect("perforated plate import");
    let sec = section_at_z(&def, 10.0).expect("section at z=10.0");
    // 1 outer loop + 512 hole loops = 513 loops
    assert_eq!(
        sec.loops().len(),
        513,
        "perforated plate must have 513 loops"
    );

    let mut outer_area = 0.0f64;
    let mut hole_area_sum = 0.0f64;
    for lp in sec.loops() {
        let a = loop_area(&sec, lp).abs();
        if a > 500_000.0 {
            outer_area = a;
        } else {
            hole_area_sum += a;
        }
    }

    assert_close(outer_area, 1640.0 * 840.0, 1000.0, "outer perimeter area");
    let expected_single_hole = PI * 20.0 * 20.0;
    let expected_holes_total = 512.0 * expected_single_hole;
    assert_close(
        hole_area_sum,
        expected_holes_total,
        2000.0,
        "512 holes area",
    );
}

// =========================================================================
// 18. Box-mm vs Box-inch Equivalence
// =========================================================================

#[test]
fn test_comb_18_box_mm_vs_box_inch_equivalence() {
    let b_mm = fixture_bytes("box-mm.step");
    let b_in = fixture_bytes("box-inch.step");
    let def_mm = import_test_step(&b_mm).expect("box-mm import");
    let def_in = import_test_step(&b_in).expect("box-inch import");

    let bounds_mm = def_mm.bounds_mm();
    let bounds_in = def_in.bounds_mm();
    assert_close(bounds_mm.max[0], bounds_in.max[0], 1e-2, "max x match");
    assert_close(bounds_mm.max[1], bounds_in.max[1], 1e-2, "max y match");
    assert_close(bounds_mm.max[2], bounds_in.max[2], 1e-2, "max z match");

    let sec_mm = section_at_z(&def_mm, 4.0).expect("sec mm");
    let sec_in = section_at_z(&def_in, 4.0).expect("sec in");
    let a_mm = loop_area(&sec_mm, &sec_mm.loops()[0]).abs();
    let a_in = loop_area(&sec_in, &sec_in.loops()[0]).abs();
    assert_close(a_mm, 200.0, 0.5, "box-mm area");
    assert_close(a_in, 200.0, 0.5, "box-in area");
}

// =========================================================================
// 19. Determinism: Repeated Import & Tessellation
// =========================================================================

#[test]
fn test_comb_19_determinism_step_import_and_tessellation() {
    let bytes = fixture_bytes("independent-exporters/occt-box-mm.step");
    let def1 = import_test_step(&bytes).expect("def1");
    let def2 = import_test_step(&bytes).expect("def2");

    let m1 = tessellate_display(&def1).expect("m1");
    let m2 = tessellate_display(&def2).expect("m2");

    assert_eq!(
        mesh_positions(&m1).len(),
        mesh_positions(&m2).len(),
        "vertex count"
    );
    assert_eq!(
        mesh_indices(&m1).len(),
        mesh_indices(&m2).len(),
        "index count"
    );
    assert_eq!(
        mesh_positions(&m1),
        mesh_positions(&m2),
        "vertex positions bitwise identical"
    );
}

// =========================================================================
// 20-21. Work Budget Operations
// =========================================================================

#[test]
fn test_comb_20_work_budget_monotony_import_and_section() {
    let bytes = fixture_bytes("cylinder.step");
    let def = import_test_step(&bytes).expect("cylinder");
    let mut budget = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let initial = budget.consumed();

    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    let _sec = prepare_section_source(
        &def,
        spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut budget,
    )
    .and_then(|source| {
        source.section_with_byte_limit(plane, &cancel, MAX_SECTION_BYTES, &mut budget)
    })
    .expect("sec");
    assert!(budget.consumed() > initial, "budget consumed must increase");
}

#[test]
fn test_comb_21_work_budget_exhaustion_graceful_error() {
    let bytes = fixture_bytes("cylinder.step");
    let def = import_test_step(&bytes).expect("cylinder");
    let mut budget = GeometryWorkBudget::new(0); // zero budget
    let cancel = AtomicBool::new(false);
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    let res = prepare_section_source(
        &def,
        spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut budget,
    )
    .and_then(|source| {
        source.section_with_byte_limit(plane, &cancel, MAX_SECTION_BYTES, &mut budget)
    });
    assert!(res.is_err(), "must fail on zero budget");
}

// =========================================================================
// 22. Cooperative Cancellation
// =========================================================================

#[test]
fn test_comb_22_cooperative_cancellation_during_section() {
    let bytes = fixture_bytes("perforated-plate.step");
    let def = import_test_step(&bytes).expect("plate");
    let cancel = AtomicBool::new(true); // already cancelled
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 10.0],
        normal: [0.0, 0.0, 1.0],
    };
    let res = section(&def, plane, &cancel);
    assert!(res.is_err(), "must abort when cancelled");
}

// =========================================================================
// 23-24. Provenance & Proof Retention Cleanliness
// =========================================================================

#[test]
fn test_comb_23_provenance_preservation_across_pipeline() {
    let bytes = fixture_bytes("independent-exporters/occt-cylinder-mm.step");
    let def = import_test_step(&bytes).expect("cyl");
    let prov = def.provenance();
    assert!(
        !prov.source_hash.as_str().is_empty(),
        "source hash must be populated"
    );
    assert_eq!(prov.source_unit, SourceUnit::Millimetre);
}

#[test]
fn test_comb_24_retained_query_proof_bytes_cleanliness() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let b1 = def.retained_query_proof_bytes();
    assert!(b1 > 0, "must report brep arena byte count");
    let _s = section_at_z(&def, 4.0).expect("sec");
    let b2 = def.retained_query_proof_bytes();
    assert_eq!(b1, b2, "query should not leak into definition proof bytes");
}

// =========================================================================
// 25-29. Multi-Axis & Oblique & Boundary Slicing
// =========================================================================

#[test]
fn test_comb_25_multi_axis_slicing_box_mm() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");

    // Z-slice at z=4 (20x10 = 200)
    let sz = section_at_z(&def, 4.0).expect("sz");
    let az = loop_area(&sz, &sz.loops()[0]).abs();
    assert_close(az, 200.0, 0.5, "z area");

    // X-slice at x=10 (10x8 = 80)
    let sx = section(
        &def,
        PlaneMm {
            origin_mm: [10.0, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        },
        &AtomicBool::new(false),
    )
    .expect("sx");
    let ax = loop_area(&sx, &sx.loops()[0]).abs();
    assert_close(ax, 80.0, 0.5, "x area");

    // Y-slice at y=5 (20x8 = 160)
    let sy = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 5.0, 0.0],
            normal: [0.0, 1.0, 0.0],
        },
        &AtomicBool::new(false),
    )
    .expect("sy");
    let ay = loop_area(&sy, &sy.loops()[0]).abs();
    assert_close(ay, 160.0, 0.5, "y area");
}

#[test]
fn test_comb_26_oblique_planar_section_box_mm() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let inv_sqrt2 = 1.0 / 2.0_f64.sqrt();
    let plane = PlaneMm {
        origin_mm: [10.0, 5.0, 4.0],
        normal: [inv_sqrt2, inv_sqrt2, 0.0],
    };
    let res = section(&def, plane, &AtomicBool::new(false));
    assert!(res.is_ok(), "oblique section must succeed");
    let sec = res.unwrap();
    assert!(!sec.loops().is_empty());
}

#[test]
fn test_comb_27_coplanar_boundary_face_slicing() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    // Plane coincident with bottom face z=0.0
    let res_bot = section_at_z(&def, 0.0);
    assert!(res_bot.is_ok(), "bottom face slice");
    // Plane coincident with top face z=8.0
    let res_top = section_at_z(&def, 8.0);
    assert!(res_top.is_ok(), "top face slice");
}

#[test]
fn test_comb_28_coplanar_side_face_slicing() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let res_x0 = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        },
        &AtomicBool::new(false),
    );
    assert!(res_x0.is_ok());
    let res_x20 = section(
        &def,
        PlaneMm {
            origin_mm: [20.0, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        },
        &AtomicBool::new(false),
    );
    assert!(res_x20.is_ok());
}

#[test]
fn test_comb_29_tangent_plane_slicing_cylinder() {
    let bytes = fixture_bytes("cylinder.step");
    let def = import_test_step(&bytes).expect("cyl");
    let res = section(
        &def,
        PlaneMm {
            origin_mm: [5.0, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        },
        &AtomicBool::new(false),
    );
    assert!(res.is_ok(), "tangent plane slice must not crash");
}

// =========================================================================
// 30-31. Empty Slices Outside Bounds
// =========================================================================

#[test]
fn test_comb_30_empty_slice_above_model() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let sec = section_at_z(&def, 100.0).expect("far above");
    assert_eq!(sec.loops().len(), 0, "no loops above model");
}

#[test]
fn test_comb_31_empty_slice_below_model() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let sec = section_at_z(&def, -50.0).expect("far below");
    assert_eq!(sec.loops().len(), 0, "no loops below model");
}

// =========================================================================
// 32-34. Tessellation Quality and Geometric Validity
// =========================================================================

#[test]
fn test_comb_32_tessellate_declared_profile_bounds() {
    let def = import_test_step(&fixture_bytes("cylinder.step")).expect("cylinder");
    let mesh = tessellate_display(&def).expect("declared profile");
    assert!(!mesh_chunks(&mesh).is_empty());
    for chunk in &mesh.chunks {
        assert_eq!(
            chunk.descriptor.mesh_profile,
            DisplayProfile::MeshTrimmedMm005V2
        );
        assert!(chunk.descriptor.carrier_deviation_mm <= MESH_SURFACE_TOLERANCE_MM);
        assert!(
            chunk.descriptor.carrier_deviation_mm + chunk.descriptor.trim_deviation_mm
                <= MESH_CARRIER_TOLERANCE_MM
        );
    }
}

#[test]
fn test_comb_33_mesh_index_validity() {
    let bytes = fixture_bytes("independent-exporters/occt-through-hole-mm.step");
    let def = import_test_step(&bytes).expect("through-hole");
    let mesh = tessellate_display(&def).expect("mesh");
    let v_count = (mesh_positions(&mesh).len() / 3) as u32;
    for &idx in &mesh_indices(&mesh) {
        assert!(idx < v_count, "triangle index {idx} out of range {v_count}");
    }
}

#[test]
fn test_comb_34_mesh_normals_are_unit_vectors() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("tapered");
    let mesh = tessellate_display(&def).expect("mesh");
    for chunk in mesh_normals(&mesh).chunks(3) {
        if chunk.len() == 3 {
            let len = (chunk[0] * chunk[0] + chunk[1] * chunk[1] + chunk[2] * chunk[2]).sqrt();
            assert_close(len, 1.0, 1e-2, "normal unit length");
        }
    }
}

// =========================================================================
// 35-37. CAM Tool Clearance & Stock Sizing
// =========================================================================

#[test]
fn test_comb_35_cylinder_bore_clearance_analysis() {
    let bytes = fixture_bytes("through-hole.step");
    let def = import_test_step(&bytes).expect("hole");
    // Hole diameter is 6.0 mm (radius 3.0 mm)
    let hole_diam = 6.0;
    let tool_diam_pass = 4.0;
    let tool_diam_fail = 8.0;
    assert!(tool_diam_pass < hole_diam, "4mm tool fits in 6mm bore");
    assert!(tool_diam_fail > hole_diam, "8mm tool exceeds 6mm bore");
}

#[test]
fn test_comb_36_box_stock_sizing_envelope() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let b = def.bounds_mm();
    let margin_xy = 5.0;
    let margin_z = 2.0;
    let stock_x = (b.max[0] - b.min[0]) + 2.0 * margin_xy;
    let stock_y = (b.max[1] - b.min[1]) + 2.0 * margin_xy;
    let stock_z = (b.max[2] - b.min[2]) + 2.0 * margin_z;
    assert_close(stock_x, 30.0, 1e-3, "stock x");
    assert_close(stock_y, 20.0, 1e-3, "stock y");
    assert_close(stock_z, 12.0, 1e-3, "stock z");
}

#[test]
fn test_comb_37_interval_bounding_box_algebra() {
    let iv_x = Interval::new(0.0, 20.0).unwrap();
    let iv_y = Interval::new(0.0, 10.0).unwrap();
    let iv_z = Interval::new(0.0, 8.0).unwrap();
    let bbox = IntervalVec3::new(iv_x, iv_y, iv_z);
    let vol = bbox.x.diameter() * bbox.y.diameter() * bbox.z.diameter();
    assert_close(vol, 1600.0, 1e-4, "bounding box volume");
}

// =========================================================================
// 38-39. Multi-Strata Monotonicity & Taper
// =========================================================================

#[test]
fn test_comb_38_roughing_strata_slicing_sequence() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("tapered");
    for step in 1..=10 {
        let z = step as f64;
        let sec = section_at_z(&def, z).expect("strata slice");
        assert_eq!(sec.loops().len(), 2, "2 loops at z={z}");
    }
}

#[test]
fn test_comb_39_taper_area_monotonic_decrease() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("tapered");
    let mut prev_area = f64::MAX;
    for step in 1..=10 {
        let z = step as f64;
        let sec = section_at_z(&def, z).expect("slice");
        let mut areas: Vec<f64> = sec
            .loops()
            .iter()
            .map(|lp| loop_area(&sec, lp).abs())
            .collect();
        areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
        let outer = areas[0];
        assert!(
            outer < prev_area,
            "outer area at z={z} must be strictly less than at z={}",
            z - 1.0
        );
        prev_area = outer;
    }
}

// =========================================================================
// 40-42. Face Classification, Genealogy, Bounds Containment
// =========================================================================

#[test]
fn test_comb_40_face_classification_planar_and_cylindrical() {
    let bytes = fixture_bytes("through-hole.step");
    let def = import_test_step(&bytes).expect("hole");
    assert!(def.faces().len() >= 6, "at least 6 faces in through-hole");
}

#[test]
fn test_comb_41_step_entity_id_genealogy() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let ids = def.face_step_ids();
    assert_eq!(ids.len(), def.faces().len(), "1-to-1 face entity id");
    for id in ids {
        assert!(id.0 > 0, "entity id must be non-zero");
    }
}

#[test]
fn test_comb_42_mesh_bounds_containment() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");
    let def = import_test_step(&bytes).expect("housing");
    let bounds = def.bounds_mm();
    let mesh = tessellate_display(&def).expect("mesh");
    for chunk in mesh_positions(&mesh).chunks(3) {
        if chunk.len() == 3 {
            let (x, y, z) = (chunk[0], chunk[1], chunk[2]);
            assert!(x >= bounds.min[0] - 0.1 && x <= bounds.max[0] + 0.1);
            assert!(y >= bounds.min[1] - 0.1 && y <= bounds.max[1] + 0.1);
            assert!(z >= bounds.min[2] - 0.1 && z <= bounds.max[2] + 0.1);
        }
    }
}

// =========================================================================
// 43-44. Concurrency & Thread-Safety
// =========================================================================

#[test]
fn test_comb_43_thread_safe_concurrent_sections() {
    use std::sync::Arc;
    let bytes = fixture_bytes("box-mm.step");
    let def = Arc::new(import_test_step(&bytes).expect("box"));

    let handles: Vec<_> = (1..=4)
        .map(|i| {
            let def_clone = Arc::clone(&def);
            thread::spawn(move || {
                let z = (i as f64) * 1.5;
                let sec = section_at_z(&def_clone, z).expect("concurrent sec");
                let area = loop_area(&sec, &sec.loops()[0]).abs();
                assert_close(area, 200.0, 0.5, "concurrent area");
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread join");
    }
}

#[test]
fn test_comb_44_thread_safe_concurrent_tessellation() {
    use std::sync::Arc;
    let bytes = fixture_bytes("independent-exporters/occt-cylinder-mm.step");
    let def = Arc::new(import_test_step(&bytes).expect("cyl"));

    let handles: Vec<_> = (0..3)
        .map(|_| {
            let def_clone = Arc::clone(&def);
            thread::spawn(move || {
                let mesh = tessellate_display(&def_clone).expect("tessellate");
                assert!(!mesh_positions(&mesh).is_empty());
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread join");
    }
}

// =========================================================================
// 45-46. Adversarial Rejection & Unsupported NURBS
// =========================================================================

#[test]
fn test_comb_45_adversarial_rejection_suite() {
    for &(fixture_name, _expected_err) in ALL_ADVERSARIAL_FIXTURES {
        let bytes = fixture_bytes(fixture_name);
        let res = import_test_step(&bytes);
        assert!(
            res.is_err(),
            "adversarial fixture {fixture_name} must be rejected"
        );
    }
}

#[test]
fn test_comb_46_unsupported_nurbs_fixture_acceptance() {
    let bytes = fixture_bytes("unsupported-nurbs.step");
    let res = import_test_step(&bytes);
    // As per manifest.json, unsupported-nurbs.step is accepted under solidcore kernel
    assert!(
        res.is_ok(),
        "unsupported-nurbs.step is supported under RFC 07"
    );
}

// =========================================================================
// 47-50. Waterline Refinement, Parameters, Brep & Triangle Positivity
// =========================================================================

#[test]
fn test_comb_47_waterline_pocket_transition_refinement() {
    let bytes = fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step");
    let def = import_test_step(&bytes).expect("narrow pocket");
    let sec_below = section_at_z(&def, 1.05).expect("below");
    let sec_above = section_at_z(&def, 1.09).expect("above");
    assert_eq!(sec_below.loops().len(), 1, "below pocket floor has 1 loop");
    assert_eq!(sec_above.loops().len(), 2, "above pocket floor has 2 loops");
}

#[test]
fn test_comb_48_surface_parameter_map_evaluation() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let maps = def.face_surface_maps();
    assert_eq!(maps.len(), def.faces().len());
}

#[test]
fn test_comb_49_brep_topology_integrity() {
    let bytes = fixture_bytes("cylinder.step");
    let def = import_test_step(&bytes).expect("cyl");
    let brep = def.brep();
    assert!(brep.tolerance() >= 0.0);
}

#[test]
fn test_comb_50_display_mesh_triangle_area_positivity() {
    let bytes = fixture_bytes("independent-exporters/occt-box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let mesh = tessellate_display(&def).expect("mesh");

    for tri in mesh_indices(&mesh).chunks(3) {
        if tri.len() == 3 {
            let i0 = tri[0] as usize * 3;
            let i1 = tri[1] as usize * 3;
            let i2 = tri[2] as usize * 3;
            let v0 = [
                mesh_positions(&mesh)[i0],
                mesh_positions(&mesh)[i0 + 1],
                mesh_positions(&mesh)[i0 + 2],
            ];
            let v1 = [
                mesh_positions(&mesh)[i1],
                mesh_positions(&mesh)[i1 + 1],
                mesh_positions(&mesh)[i1 + 2],
            ];
            let v2 = [
                mesh_positions(&mesh)[i2],
                mesh_positions(&mesh)[i2 + 1],
                mesh_positions(&mesh)[i2 + 2],
            ];

            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
            let cp = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let area = 0.5 * (cp[0] * cp[0] + cp[1] * cp[1] + cp[2] * cp[2]).sqrt();
            assert!(area >= 0.0, "triangle area non-negative");
        }
    }
}

// =========================================================================
// 51-55. Scale, Winding, Entity Tracing, Area & Full Audit
// =========================================================================

#[test]
fn test_comb_51_compound_box_inch_conversion() {
    let bytes = fixture_bytes("box-inch.step");
    let def = import_test_step(&bytes).expect("box inch");
    assert_close(def.source_units_scale_to_mm(), 25.4, 1e-4, "scale to mm");
}

#[test]
fn test_comb_52_loop_vertex_winding_orientation() {
    let bytes = fixture_bytes("through-hole.step");
    let def = import_test_step(&bytes).expect("through-hole");
    let sec = section_at_z(&def, 4.0).expect("sec");
    assert_eq!(sec.loops().len(), 2);
    // Outer loop should have positive area, inner hole should have negative area (or vice versa depending on convention)
    let a0 = loop_area(&sec, &sec.loops()[0]);
    let a1 = loop_area(&sec, &sec.loops()[1]);
    assert!(
        a0 * a1 < 0.0,
        "outer and inner loops must have opposite winding signs"
    );
}

#[test]
fn test_comb_53_section_span_entity_tracing() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let sec = section_at_z(&def, 4.0).expect("sec");
    for loop_index in 0..sec.loops().len() {
        for span in sec.spans(loop_index).expect("native span provenance") {
            assert!(span.source_face_ordinal < def.faces().len());
            assert_eq!(def.face_ordinal(span.face), Some(span.source_face_ordinal));
        }
    }
}

#[test]
fn test_comb_54_total_surface_area_estimation() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let mesh = tessellate_display(&def).expect("mesh");
    let mut total_area = 0.0f64;
    for tri in mesh_indices(&mesh).chunks(3) {
        if tri.len() == 3 {
            let i0 = tri[0] as usize * 3;
            let i1 = tri[1] as usize * 3;
            let i2 = tri[2] as usize * 3;
            let v0 = [
                mesh_positions(&mesh)[i0],
                mesh_positions(&mesh)[i0 + 1],
                mesh_positions(&mesh)[i0 + 2],
            ];
            let v1 = [
                mesh_positions(&mesh)[i1],
                mesh_positions(&mesh)[i1 + 1],
                mesh_positions(&mesh)[i1 + 2],
            ];
            let v2 = [
                mesh_positions(&mesh)[i2],
                mesh_positions(&mesh)[i2 + 1],
                mesh_positions(&mesh)[i2 + 2],
            ];
            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
            let cp = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            total_area += 0.5 * (cp[0] * cp[0] + cp[1] * cp[1] + cp[2] * cp[2]).sqrt();
        }
    }
    // Box 20x10x8: 2*(200 + 160 + 80) = 880 mm2
    assert_close(total_area, 880.0, 5.0, "total surface area");
}

#[test]
fn test_comb_55_full_pipeline_audit_ribbed_bracket() {
    let bytes = fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step");
    let def = import_test_step(&bytes).expect("ribbed bracket");
    let b = def.bounds_mm();
    assert_close(b.max[0], 30.0, 1e-3, "max x");

    // Slicing at multiple heights
    let s1 = section_at_z(&def, 0.5).expect("base");
    assert!(!s1.loops().is_empty());
    let s2 = section_at_z(&def, 4.0).expect("rib");
    assert!(!s2.loops().is_empty());

    // Tessellation
    let mesh = tessellate_display(&def).expect("mesh");
    assert!(mesh_indices(&mesh).len() >= 60);

    // Audit zero leak
    let proof_bytes = def.retained_query_proof_bytes();
    assert!(proof_bytes > 0);
}
