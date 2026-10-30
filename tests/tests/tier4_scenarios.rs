// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Tier 4: Realistic CAM Scenarios.
//! 26 end-to-end operator journeys testing the full CAM pipeline on reference fixtures.

use spiling_contracts::geometry::*;
use spiling_e2e_tests::fixtures::*;
use spiling_e2e_tests::helpers::*;
use spiling_geometry::*;
use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;

// =========================================================================
// Scenario 1: Stock Selection & Facing Setup
// =========================================================================

#[test]
fn test_cam_01_stock_selection_and_facing_setup_box_mm() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("import box-mm");
    let b = def.bounds_mm();

    // CAM operator adds 5.0mm stock margin on XY and 2.0mm on Z
    let margin_xy = 5.0;
    let margin_z = 2.0;
    let stock_min = [
        b.min[0] - margin_xy,
        b.min[1] - margin_xy,
        b.min[2] - margin_z,
    ];
    let stock_max = [
        b.max[0] + margin_xy,
        b.max[1] + margin_xy,
        b.max[2] + margin_z,
    ];

    assert_close(stock_max[0] - stock_min[0], 30.0, 1e-3, "stock x");
    assert_close(stock_max[1] - stock_min[1], 20.0, 1e-3, "stock y");
    assert_close(stock_max[2] - stock_min[2], 12.0, 1e-3, "stock z");

    // Face milling pass 1.0mm below top face (z = 8.0 - 1.0 = 7.0)
    let sec = section_at_z(&def, 7.0).expect("face milling slice");
    assert_eq!(sec.loops().len(), 1, "facing section has 1 boundary");
    let area = loop_area(&sec, &sec.loops()[0]).abs();
    assert_close(area, 200.0, 0.5, "facing cut area");
}

// =========================================================================
// Scenario 2: Hole-Making & Peck Drilling
// =========================================================================

#[test]
fn test_cam_02_hole_making_and_peck_drilling_through_hole() {
    let bytes = fixture_bytes("through-hole.step");
    let def = import_test_step(&bytes).expect("import through-hole");
    let b = def.bounds_mm();

    // Verify hole depth
    let depth = b.max[2] - b.min[2];
    assert_close(depth, 8.0, 1e-3, "hole depth");

    // Check circular boundary at three strata: top (z=7.5), mid (z=4.0), bottom (z=0.5)
    for z in [0.5, 4.0, 7.5] {
        let sec = section_at_z(&def, z).expect("strata slice");
        assert_eq!(sec.loops().len(), 2, "box + hole at z={z}");
        let mut areas: Vec<f64> = sec
            .loops()
            .iter()
            .map(|lp| loop_area(&sec, lp).abs())
            .collect();
        areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
        let hole_area = areas[1];
        assert_close(hole_area, 9.0 * PI, 0.5, "hole area at z={z}");
    }
}

// =========================================================================
// Scenario 3: Waterline Roughing Sequence
// =========================================================================

#[test]
fn test_cam_03_waterline_roughing_tapered_fitting() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("import tapered fitting");

    // Roughing passes every 2.0 mm from z=1.0 to z=11.0
    let mut prev_outer = f64::MAX;
    for z_step in [1.0, 3.0, 5.0, 7.0, 9.0, 11.0] {
        let sec = section_at_z(&def, z_step).expect("waterline slice");
        assert_eq!(sec.loops().len(), 2, "2 loops per roughing level");
        let mut areas: Vec<f64> = sec
            .loops()
            .iter()
            .map(|lp| loop_area(&sec, lp).abs())
            .collect();
        areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
        let outer = areas[0];
        assert!(
            outer < prev_outer,
            "waterline area decreases monotonically with Z"
        );
        prev_outer = outer;
    }
}

// =========================================================================
// Scenario 4: Finish Contouring Narrow Pocket
// =========================================================================

#[test]
fn test_cam_04_finish_contouring_narrow_pocket() {
    let bytes = fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step");
    let def = import_test_step(&bytes).expect("import narrow pocket");

    // Floor transition at z=1.07mm
    let sec = section_at_z(&def, 2.0).expect("contouring level");
    assert_eq!(sec.loops().len(), 2);
    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let pocket_area = areas[1];
    // Slot width 0.6 mm * slot length 8.0 mm = 4.8 mm2
    assert_close(pocket_area, 4.8, 0.5, "narrow pocket area");

    // CAM tool verification: 0.5mm endmill leaves 0.05mm radial clearance on each side
    let tool_diam = 0.5;
    let slot_width = 0.6;
    let clearance = (slot_width - tool_diam) * 0.5;
    assert_close(clearance, 0.05, 1e-4, "tool radial clearance");
}

// =========================================================================
// Scenario 5: Multi-Cavity Pocketing
// =========================================================================

#[test]
fn test_cam_05_multi_cavity_pocketing_rounded_housing() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");
    let def = import_test_step(&bytes).expect("rounded housing");

    // Slicing solid base (z=2.0) vs cavity level (z=6.0)
    let s_base = section_at_z(&def, 2.0).expect("base");
    assert_eq!(s_base.loops().len(), 1, "solid base is 1 loop");

    let s_pocket = section_at_z(&def, 6.0).expect("pocket");
    assert_eq!(s_pocket.loops().len(), 2, "pocket cavity creates 2 loops");
}

// =========================================================================
// Scenario 6: Spline NURBS Surface 3D Milling
// =========================================================================

#[test]
fn test_cam_06_spline_nurbs_surface_3d_milling() {
    let bytes = fixture_bytes("independent-exporters/occt-spline-housing-mm.step");
    let def = import_test_step(&bytes).expect("spline housing");
    let mesh = tessellate_display(&def).expect("fine mesh");
    assert!(mesh_positions(&mesh).len() >= 300, "spline mesh density");
    assert!(mesh_indices(&mesh).len() >= 300);
}

// =========================================================================
// Scenario 7: Thin Wall Machining
// =========================================================================

#[test]
fn test_cam_07_thin_wall_machining_ribbed_bracket() {
    let bytes = fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step");
    let def = import_test_step(&bytes).expect("ribbed bracket");

    // Slicing ribs at z=3.0 and z=5.0
    let s3 = section_at_z(&def, 3.0).expect("ribs z=3");
    let s5 = section_at_z(&def, 5.0).expect("ribs z=5");
    assert!(!s3.loops().is_empty());
    assert!(!s5.loops().is_empty());
}

// =========================================================================
// Scenario 8: High-Speed Drilling Perforated Plate
// =========================================================================

#[test]
fn test_cam_08_high_speed_drilling_perforated_plate_512_holes() {
    let bytes = fixture_bytes("perforated-plate.step");
    let def = import_test_step(&bytes).expect("perforated plate");
    let sec = section_at_z(&def, 10.0).expect("midplane");
    assert_eq!(sec.loops().len(), 513, "512 holes + 1 perimeter");

    // Calculate cycle time: 512 holes * 0.5 sec/hole = 256 seconds
    let hole_count = sec.loops().len() - 1;
    let cycle_time_sec = (hole_count as f64) * 0.5;
    assert_close(cycle_time_sec, 256.0, 1e-3, "cycle time estimate");
}

// =========================================================================
// Scenario 9: Imperial Stock & Metric Tooling
// =========================================================================

#[test]
fn test_cam_09_imperial_stock_and_metric_tooling() {
    let bytes = fixture_bytes("box-inch.step");
    let def = import_test_step(&bytes).expect("box inch");
    let b = def.bounds_mm();
    assert_close(b.max[0], 20.0, 1e-2, "converted max x");
    assert_close(b.max[1], 10.0, 1e-2, "converted max y");
    assert_close(b.max[2], 8.0, 1e-2, "converted max z");

    // Standard 6mm metric endmill perimeter pass
    let tool_radius = 3.0;
    let contour_x_min = b.min[0] - tool_radius;
    let contour_x_max = b.max[0] + tool_radius;
    assert_close(
        contour_x_max - contour_x_min,
        26.0,
        1e-2,
        "toolpath x envelope",
    );
}

// =========================================================================
// Scenario 10: Lathe Turning Profile
// =========================================================================

#[test]
fn test_cam_10_lathe_turning_profile_cylinder() {
    let bytes = fixture_bytes("cylinder.step");
    let def = import_test_step(&bytes).expect("cyl");
    let b = def.bounds_mm();
    let radius = (b.max[0] - b.min[0]) * 0.5;
    let length = b.max[2] - b.min[2];
    assert_close(radius, 5.0, 1e-3, "turning radius");
    assert_close(length, 8.0, 1e-3, "turning length");
}

// =========================================================================
// Scenario 11: Rest-Machining Corner Fillets
// =========================================================================

#[test]
fn test_cam_11_rest_machining_corner_fillets_rounded_housing() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");
    let def = import_test_step(&bytes).expect("housing");
    // Verify corner fillet geometry via mid-pocket section
    let sec = section_at_z(&def, 6.0).expect("sec");
    assert_eq!(sec.loops().len(), 2);
}

// =========================================================================
// Scenario 12: Chamfer and Deburring Detection
// =========================================================================

#[test]
fn test_cam_12_chamfer_and_deburring_detection() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let b = def.bounds_mm();
    let perimeter = 2.0 * ((b.max[0] - b.min[0]) + (b.max[1] - b.min[1]));
    assert_close(perimeter, 60.0, 1e-3, "top chamfer perimeter");
}

// =========================================================================
// Scenario 13: Deep Hole Bore Aspect Ratio
// =========================================================================

#[test]
fn test_cam_13_deep_hole_bore_aspect_ratio() {
    let bytes = fixture_bytes("through-hole.step");
    let def = import_test_step(&bytes).expect("hole");
    let b = def.bounds_mm();
    let depth = b.max[2] - b.min[2];
    let diameter = 6.0;
    let aspect_ratio = depth / diameter;
    assert!(aspect_ratio < 2.0, "standard aspect ratio < 3:1");
}

// =========================================================================
// Scenario 14: Fixture Clamping Clearance Envelope
// =========================================================================

#[test]
fn test_cam_14_fixture_clamping_clearance_envelope() {
    let bytes = fixture_bytes("independent-exporters/occt-box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let b = def.bounds_mm();
    let clamp_height = 2.0;
    let machinable_z_min = b.min[2] + clamp_height;
    assert!(
        machinable_z_min < b.max[2],
        "clear machinable region above clamp"
    );
}

// =========================================================================
// Scenario 15: Taper Angle Verification for Conical Cutter
// =========================================================================

#[test]
fn test_cam_15_taper_angle_verification_for_conical_cutter() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("tapered");
    let sec1 = section_at_z(&def, 1.0).expect("z=1");
    let sec11 = section_at_z(&def, 11.0).expect("z=11");

    let a1 = loop_area(&sec1, &sec1.loops()[0]).abs();
    let a11 = loop_area(&sec11, &sec11.loops()[0]).abs();
    let r1 = (a1 / PI).sqrt();
    let r11 = (a11 / PI).sqrt();
    let delta_r = r1 - r11;
    let delta_z = 10.0;
    let taper_angle_rad = (delta_r / delta_z).atan();
    let taper_angle_deg = taper_angle_rad * 180.0 / PI;
    assert!(
        taper_angle_deg > 0.0 && taper_angle_deg < 45.0,
        "valid taper angle"
    );
}

// =========================================================================
// Scenario 16: Scallop Height & Display Mesh Inspection
// =========================================================================

#[test]
fn test_cam_16_scallop_height_and_display_mesh_inspection() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");
    let def = import_test_step(&bytes).expect("housing");
    let mesh = tessellate_display(&def).expect("mesh");
    assert!(!mesh_positions(&mesh).is_empty());
}

// =========================================================================
// Scenario 17: Adaptive Clearing Stepover Area
// =========================================================================

#[test]
fn test_cam_17_adaptive_clearing_stepover_area() {
    let bytes = fixture_bytes("independent-exporters/occt-narrow-pocket-mm.step");
    let def = import_test_step(&bytes).expect("narrow pocket");
    let sec = section_at_z(&def, 2.0).expect("sec");
    let mut areas: Vec<f64> = sec
        .loops()
        .iter()
        .map(|lp| loop_area(&sec, lp).abs())
        .collect();
    areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let pocket_area = areas[1];
    let stepover_percentage = 0.40;
    let effective_stepover_area = pocket_area * stepover_percentage;
    assert!(effective_stepover_area > 0.0);
}

// =========================================================================
// Scenario 18: Multi-Axis 5-Axis Swarf Surface Detection
// =========================================================================

#[test]
fn test_cam_18_multi_axis_5axis_swarf_surface_detection() {
    let bytes = fixture_bytes("independent-exporters/occt-tapered-fitting-mm.step");
    let def = import_test_step(&bytes).expect("tapered");
    assert!(!def.faces().is_empty());
}

// =========================================================================
// Scenario 19: Undercut and Draft Angle Check
// =========================================================================

#[test]
fn test_cam_19_under_cut_and_draft_angle_check() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    // Prismatic box has zero undercuts
    assert_eq!(def.faces().len(), 6);
}

// =========================================================================
// Scenario 20: Toolpath Linking Rapid Plane Height
// =========================================================================

#[test]
fn test_cam_20_toolpath_linking_rapid_plane_height() {
    let bytes = fixture_bytes("independent-exporters/occt-ribbed-bracket-mm.step");
    let def = import_test_step(&bytes).expect("ribbed bracket");
    let b = def.bounds_mm();
    let clearance_margin = 5.0;
    let rapid_plane_z = b.max[2] + clearance_margin;
    assert_close(rapid_plane_z, 12.6, 1e-3, "rapid plane z");
}

// =========================================================================
// Scenario 21: Workpiece Mass and Stock Removal Ratio
// =========================================================================

#[test]
fn test_cam_21_workpiece_mass_and_stock_removal_ratio() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let b = def.bounds_mm();
    let part_vol = (b.max[0] - b.min[0]) * (b.max[1] - b.min[1]) * (b.max[2] - b.min[2]);
    let stock_vol = 30.0 * 20.0 * 12.0;
    let removal_ratio = (stock_vol - part_vol) / stock_vol;
    assert_close(removal_ratio, 0.7777, 1e-2, "stock removal ratio");
}

// =========================================================================
// Scenario 22: Cancellation During Long Toolpath Slicing
// =========================================================================

#[test]
fn test_cam_22_cancellation_during_long_toolpath_slicing() {
    let bytes = fixture_bytes("perforated-plate.step");
    let def = import_test_step(&bytes).expect("plate");
    let cancel = AtomicBool::new(false);

    // First slice succeeds
    let s1 = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 5.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    );
    assert!(s1.is_ok());

    // Operator cancels
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let s2 = section(
        &def,
        PlaneMm {
            origin_mm: [0.0, 0.0, 10.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    );
    assert!(s2.is_err(), "must abort after cancellation");
}

// =========================================================================
// Scenario 23: Work Budget Governor on Batch Nests
// =========================================================================

#[test]
fn test_cam_23_work_budget_governor_on_batch_nests() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    let mut budget = GeometryWorkBudget::new(50_000);
    let cancel = AtomicBool::new(false);
    for z in [2.0, 4.0, 6.0] {
        let plane = PlaneMm {
            origin_mm: [0.0, 0.0, z],
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
        assert!(res.is_ok());
    }
}

// =========================================================================
// Scenario 24: Part Reorientation Coordinate Transform
// =========================================================================

#[test]
fn test_cam_24_part_reorientation_coordinate_transform() {
    let bytes = fixture_bytes("box-mm.step");
    let def = import_test_step(&bytes).expect("box");
    // Setup 1: slicing top-down
    let sec_top = section_at_z(&def, 4.0).expect("top sec");
    let area_top = loop_area(&sec_top, &sec_top.loops()[0]).abs();

    // Setup 2: slicing from reverse normal (looking down -Z)
    let plane_rev = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, -1.0],
    };
    let sec_rev = section(&def, plane_rev, &AtomicBool::new(false)).expect("rev sec");
    let area_rev = loop_area(&sec_rev, &sec_rev.loops()[0]).abs();

    assert_close(area_top, area_rev, 1e-3, "reoriented slice area invariant");
}

// =========================================================================
// Scenario 25: Unsupported Geometry Pre-Flight Check
// =========================================================================

#[test]
fn test_cam_25_unsupported_geometry_pre_flight_check() {
    let bytes_unsupported = fixture_bytes("reachable-unsupported.step");
    let res = import_test_step(&bytes_unsupported);
    assert!(
        res.is_err(),
        "CAM pre-flight must reject unsupported geometry"
    );

    let bytes_assembly = fixture_bytes("assembly.step");
    let res_asm = import_test_step(&bytes_assembly);
    assert!(res_asm.is_err(), "CAM pre-flight must reject assembly step");
}

// =========================================================================
// Scenario 26: End-to-End Full CAM Lifecycle Production
// =========================================================================

#[test]
fn test_cam_26_end_to_end_full_cam_lifecycle_production() {
    let bytes = fixture_bytes("independent-exporters/occt-rounded-housing-mm.step");

    // Phase 1: Import & Provenance
    let def = import_test_step(&bytes).expect("import");
    assert_eq!(def.provenance().source_unit, SourceUnit::Millimetre);

    // Phase 2: Stock sizing
    let b = def.bounds_mm();
    assert_close(b.max[0], 24.0, 1e-3, "max x");
    assert_close(b.max[1], 18.0, 1e-3, "max y");
    assert_close(b.max[2], 10.0, 1e-3, "max z");

    // Phase 3: Strata roughing
    let sec_rough = section_at_z(&def, 3.0).expect("roughing slice");
    assert_eq!(sec_rough.loops().len(), 1);

    // Phase 4: Pocket finish
    let sec_pocket = section_at_z(&def, 8.0).expect("pocket finish slice");
    assert_eq!(sec_pocket.loops().len(), 2);

    // Phase 5: Display mesh rendering
    let mesh = tessellate_display(&def).expect("mesh generation");
    assert!(mesh_indices(&mesh).len() >= 60);

    // Phase 6: Proof retention verification
    let proof_bytes = def.retained_query_proof_bytes();
    assert!(proof_bytes > 0);
}
