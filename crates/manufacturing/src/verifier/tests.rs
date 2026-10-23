// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use spiling_contracts::geometry::AabbMm;
use spiling_contracts::manufacturing::{
    DepositionKind, DepositionPath, MotionSpace, OutputDialect, PlanarPrintRecipe, PrintLayer,
    PrinterCapabilities, PrinterCoordinateFrame, PrinterSpecification,
};

// This independent numeric golden is not emitter output. A circular bead of
// diameter 1 and filament of diameter 2 gives E = path length / 4 exactly.
const GOLDEN: &str = "\
; Independent software-only numeric fixture
G21
G90
M82
G92 E0
M106 S0
G0 X1 Y1 Z1 F1200
G1 X5 Y1 Z1 E1 F600
G1 X5 Y5 Z1 E2 F600
G1 X1 Y5 Z1 E3 F600
G1 X1 Y1 Z1 E4 F600
G0 X2 Y2 Z1 F1200
G1 X6 Y2 Z1 E5 F600
G0 X2 Y2 Z2 F1200
G1 X2 Y6 Z2 E6 F600
";

fn coverage() -> PlanarRegionCoverage {
    PlanarRegionCoverage {
        skin_bridge_domains: Vec::new(),
        process_diaphragms: Vec::new(),
        diaphragm_area_mm2: 0.0,
        section_area_mm2: 16.0,
        source_section_area_mm2: 16.0,
        surface_added_area_mm2: 0.0,
        separator_area_mm2: 0.0,
        process_interfaces: Vec::new(),
        wall_area_mm2: 16.0,
        bottom_skin_area_mm2: 0.0,
        top_skin_area_mm2: 0.0,
        solid_interior_area_mm2: 0.0,
        sparse_interior_area_mm2: 0.0,
        intentional_void_area_mm2: 0.0,
        protected_hole_area_mm2: 0.0,
        nominal_deposition_volume_mm3: 0.0,
        support_area_mm2: 0.0,
        support_nominal_deposition_volume_mm3: 0.0,
        process_regions: vec![],
    }
}
fn refresh_volumes(intent: &ManufacturingIntent, plan: &mut NormalizedPrintPlan) {
    plan.native_geometry.certified_layer_count = plan.layers.len() as u32;
    plan.native_geometry.slab_count = plan.layers.len() as u32;
    plan.native_geometry
        .source_feature_coverage
        .disjoint_source_layer_count = plan.layers.len() as u32;
    let r = &intent.recipe;
    for layer in &mut plan.layers {
        let mut object = 0.0;
        let mut support = 0.0;
        for path in &layer.paths {
            let height = path.height_mm();
            let flow = r.flow_multiplier
                * if layer.index == 0 {
                    r.process.first_layer_flow_factor
                } else {
                    1.0
                };
            let volume: f64 = path
                .checked_edges()
                .unwrap()
                .map(|edge| {
                    let area = ((edge.width_mm - height) * height
                        + std::f64::consts::PI * (height / 2.0).powi(2))
                        * flow;
                    distance(edge.points_mm[0], edge.points_mm[1])
                        * area
                        * if path.kind == DepositionKind::Bridge {
                            r.process.bridge_flow_factor
                        } else {
                            1.0
                        }
                })
                .sum();
            if matches!(
                path.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            ) {
                support += volume;
            } else {
                object += volume;
            }
        }
        layer.region_coverage.nominal_deposition_volume_mm3 = object;
        layer.region_coverage.support_nominal_deposition_volume_mm3 = support;
    }
}
fn fixture() -> (ManufacturingIntent, NormalizedPrintPlan) {
    let intent = ManufacturingIntent {
        printer: PrinterSpecification {
            schema_version: MANUFACTURING_SCHEMA_VERSION,
            id: "software-fixture".into(),
            revision: "1".into(),
            name: "Not a physical printer".into(),
            coordinate_frame: PrinterCoordinateFrame::RightHandedMillimetres,
            build_envelope: AabbMm {
                min: [0.0; 3],
                max: [100.0; 3],
            },
            components: vec![],
            capabilities: PrinterCapabilities {
                motion_space: MotionSpace::CartesianXyz,
                extruders: 1,
                output_dialect: OutputDialect::CartesianAbsoluteGcodeV1,
                generated_supports: false,
            },
            workflow: JobWorkflow::DiagnosticBodyOnly,
            nozzle_diameter_mm: 1.0,
            deposition_limits: DepositionLimits {
                min_bead_width_mm: 0.000001,
                max_bead_width_mm: 1.01,
                min_bead_height_mm: 0.000001,
                max_bead_height_mm: 1.0,
            },
            max_feed_mm_s: 100.0,
            max_volumetric_flow_mm3_s: 100.0,
        },
        recipe: PlanarPrintRecipe {
            native_shape_error_mm: 0.1,
            id: "recipe".into(),
            revision: "1".into(),
            material_id: "nominal-only".into(),
            material_revision: "1".into(),
            filament_diameter_mm: 2.0,
            layer_height_mm: 1.0,
            bead_width_mm: 1.0,
            sparse_infill: SparseInfillPolicy::Fine,
            perimeter_count: 1,
            seam_policy: SeamPolicy::Nearest,
            rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
            top_skin_axis: RectilinearAxisPolicy::AlternatingXY,
            infill_fraction: 1.0,
            top_skin_thickness_mm: 1.0,
            bottom_skin_thickness_mm: 1.0,
            print_speed_mm_s: 10.0,
            travel_speed_mm_s: 20.0,
            flow_multiplier: 1.0,
            process: ProcessPolicy {
                first_layer_speed_factor: 1.0,
                first_layer_flow_factor: 1.0,
                fan_first_layers: 0,
                fan_after_first_layers: 0,
                bridge_speed_factor: 0.5,
                bridge_flow_factor: 1.0,
                bridge_fan_pwm: 255,
                max_bridge_span_mm: 5.0,
                retraction: RetractionPolicy::Disabled,
            },
            supports: SupportPolicy::Disabled,
        },
    };
    let mut plan = NormalizedPrintPlan {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
        sparse_bands: vec![],
        process_resolution: Vec::new(),
        native_geometry: NativeGeometryCoverageSummary {
            requested_shape_error_mm: intent.recipe.native_shape_error_mm,
            achieved_shape_error_mm: 0.0,
            height_analysis_count: 1,
            slab_count: 1,
            certified_layer_count: 2,
            event_count: 2,
            geometry_work: 1,
            evidence: PreflightEvidence::Certified,
            skin_coverage: SkinCoverageScope::ScheduledMaterialDistance,
            support_clearance: SupportClearanceScope::ContinuousTargetOuterOccupancy,
            source_feature_coverage: NativeFeatureCoverageSummary {
                disjoint_source_layer_count: 2,
                invariant_source_union_layer_count: 0,
                native_interface_complex_layer_count: 0,
                owned_critical_slab_count: 0,
            },
        },
        surface_resolution: Vec::new(),
        sampling_tolerance_mm: 0.005,
        offset_tolerance_mm: 0.001,
        surface_tolerance_mm: 0.001,
        layer_quantization_mm: 0.000_001,
        layers: vec![
            PrintLayer {
                skin_command_fits: Vec::new(),
                index: 0,
                z_mm: 1.0,
                lower_z_mm: 0.0,
                height_mm: 1.0,
                section_z_mm: 0.5,
                region_coverage: coverage(),
                paths: vec![
                    DepositionPath {
                        kind: DepositionKind::Perimeter,
                        owner: Some(DepositionOwner::Wall),
                        width: WidthProfile::Constant { width_mm: 1.0 },
                        physical_z_mm: [0.0, 1.0],
                        points_mm: vec![
                            [1.0, 1.0, 1.0],
                            [5.0, 1.0, 1.0],
                            [5.0, 5.0, 1.0],
                            [1.0, 5.0, 1.0],
                            [1.0, 1.0, 1.0],
                        ],
                    },
                    DepositionPath {
                        kind: DepositionKind::SolidFill,
                        owner: None,
                        width: WidthProfile::Constant { width_mm: 1.0 },
                        physical_z_mm: [0.0, 1.0],
                        points_mm: vec![[2.0, 2.0, 1.0], [6.0, 2.0, 1.0]],
                    },
                ],
            },
            PrintLayer {
                skin_command_fits: Vec::new(),
                index: 1,
                z_mm: 2.0,
                lower_z_mm: 1.0,
                height_mm: 1.0,
                section_z_mm: 1.5,
                region_coverage: coverage(),
                paths: vec![DepositionPath {
                    kind: DepositionKind::SolidFill,
                    owner: None,
                    width: WidthProfile::Constant { width_mm: 1.0 },
                    physical_z_mm: [1.0, 2.0],
                    points_mm: vec![[2.0, 2.0, 2.0], [2.0, 6.0, 2.0]],
                }],
            },
        ],
    };
    refresh_volumes(&intent, &mut plan);
    (intent, plan)
}

fn replay(program: &str) -> Result<VerificationReport, ManufacturingError> {
    let (intent, plan) = fixture();
    verify(program, &plan, &intent, &AtomicBool::new(false))
}

#[test]
fn open_immutable_wall_survives_normalization_emission_and_fresh_replay() {
    let cancel = AtomicBool::new(false);
    for varying in [false, true] {
        let (intent, mut plan) = fixture();
        let wall = &mut plan.layers[0].paths[0];
        wall.points_mm.pop();
        if varying {
            wall.width = WidthProfile::PerEdge {
                widths_mm: vec![1.0, 1.005, 1.01],
            };
        }
        refresh_volumes(&intent, &mut plan);
        plan.validate(&intent).unwrap();
        let mut unowned = plan.clone();
        unowned.layers[0].paths[0].owner = None;
        assert!(
            unowned.validate(&intent).is_err(),
            "Open strokes cannot acquire wall authority from a Perimeter label"
        );
        crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
        let wall = plan.layers[0]
            .paths
            .iter()
            .find(|path| path.owner == Some(DepositionOwner::Wall))
            .unwrap();
        assert_eq!(wall.kind, DepositionKind::Perimeter);
        assert_ne!(
            wall.points_mm.first(),
            wall.points_mm.last(),
            "Realization must not invent a closing edge or relabel an open wall Bridge"
        );
        let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
        let mut sink = ReplaySink::new(&plan, &intent).unwrap();
        let report = verify_actual(&program, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
        assert!(report.verified);
        assert_eq!(report.deposition_segments, 5);
        let (bytes, bounds) = sink.finish().unwrap();
        validate_replay_display(
            &bytes,
            &packed_process_descriptor(&bytes, bounds, &program, &intent, &plan, &report),
        )
        .unwrap();
    }
}

#[test]
fn independent_numeric_golden_measures_actual_extrusion_and_paths() {
    let report = replay(GOLDEN).unwrap();
    assert!(report.verified);
    assert_eq!(report.deposition_segments, 6);
    assert_eq!(report.travel_segments, 3);
    assert_eq!(report.filament_length_mm, 6.0);
    assert!((report.deposited_volume_mm3 - 6.0 * std::f64::consts::PI).abs() < 1e-12);
    assert_eq!(report.max_position_error_mm, 0.0);
    assert_eq!(report.max_extrusion_error_mm, 0.0);
    report.validate().unwrap();
}

#[test]
fn replay_rejects_omitted_intermediate_and_final_fixed_height_layers() {
    let (intent, plan) = fixture();
    let cancel = AtomicBool::new(false);
    let baseline = verify(GOLDEN, &plan, &intent, &cancel).unwrap();
    assert_eq!(baseline.deposition_segments, 6);
    assert_eq!(baseline.travel_segments, 3);
    for empty_index in [1, 2] {
        let mut omitted = plan.clone();
        omitted.layers.insert(
            empty_index,
            PrintLayer {
                skin_command_fits: Vec::new(),
                index: empty_index as u32,
                z_mm: (empty_index + 1) as f64,
                lower_z_mm: empty_index as f64,
                height_mm: 1.0,
                section_z_mm: empty_index as f64 + 0.5,
                region_coverage: coverage(),
                paths: vec![],
            },
        );
        for (index, layer) in omitted.layers.iter_mut().enumerate() {
            layer.index = index as u32;
            layer.z_mm = (index + 1) as f64;
            layer.lower_z_mm = index as f64;
            layer.section_z_mm = index as f64 + 0.5;
            for path in &mut layer.paths {
                path.physical_z_mm = [layer.lower_z_mm, layer.z_mm];
                for point in &mut path.points_mm {
                    point[2] = layer.z_mm;
                }
            }
        }
        refresh_volumes(&intent, &mut omitted);
        // The remaining deposited paths are realized exactly by the numeric
        // golden; absence of moves for an empty layer cannot imply coverage.
        let program = if empty_index == 1 {
            GOLDEN.replace("Z2", "Z3")
        } else {
            GOLDEN.to_owned()
        };
        assert_eq!(
            verify(&program, &omitted, &intent, &cancel)
                .unwrap_err()
                .code,
            ManufacturingErrorCode::InvalidSpecification
        );
    }
}

#[test]
fn actual_backend_program_is_independently_replayed() {
    let (mut intent, mut plan) = fixture();
    intent.recipe.layer_height_mm = 0.2;
    intent.recipe.bead_width_mm = 0.45;
    intent.recipe.filament_diameter_mm = 1.75;
    intent.recipe.print_speed_mm_s = 10.123_456_789;
    intent.recipe.travel_speed_mm_s = 20.987_654_321;
    for layer in &mut plan.layers {
        layer.z_mm = f64::from(layer.index + 1) * 0.2;
        layer.lower_z_mm = f64::from(layer.index) * 0.2;
        layer.height_mm = 0.2;
        layer.section_z_mm = (f64::from(layer.index) + 0.5) * 0.2;
        for path in &mut layer.paths {
            path.width = WidthProfile::Constant { width_mm: 0.45 };
            path.physical_z_mm = [layer.lower_z_mm, layer.z_mm];
            for point in &mut path.points_mm {
                point[0] += 0.123_456_789;
                point[2] = layer.z_mm;
            }
        }
    }
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let report = verify(&program, &plan, &intent, &cancel).unwrap();
    assert_eq!(report.deposition_segments, 6);
    assert_eq!(report.travel_segments, 3);
    // Independent numerical value for 24 mm of rounded 0.45 x 0.2 bead.
    assert!((report.deposited_volume_mm3 - 1.953_982_236_861_550_5).abs() < 0.000_000_02);
    assert!(report.max_position_error_mm > 0.0);
    assert!(report.max_position_error_mm <= 3.0_f64.sqrt() * XYZ_HALF_STEP + 1e-12);
    assert!(report.max_extrusion_error_mm <= 2.0 * E_HALF_STEP + 1e-12);
    report.validate().unwrap();
    let second_program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert_eq!(program, second_program);
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::to_value(verify(&second_program, &plan, &intent, &cancel).unwrap()).unwrap()
    );
}

#[test]
fn actual_half_step_xyz_cannot_choose_the_other_lattice_neighbor() {
    let cancel = AtomicBool::new(false);
    for offset in [0.0000005, -0.0000005] {
        let (mut intent, mut plan) = fixture();
        intent.printer.build_envelope.min[0] = -10.0;
        for layer in &mut plan.layers {
            for path in &mut layer.paths {
                for point in &mut path.points_mm {
                    point[0] = (point[0] - 1.0) + offset;
                }
            }
        }
        refresh_volumes(&intent, &mut plan);
        let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
        assert!(verify(&program, &plan, &intent, &cancel).unwrap().verified);
        let actual = if offset > 0.0 {
            "G0 X0.000001 "
        } else {
            "G0 X-0.000001 "
        };
        let alternate = program.replacen(actual, "G0 X0.000000 ", 1);
        assert_ne!(alternate, program);
        assert_eq!(
            verify(&alternate, &plan, &intent, &cancel)
                .unwrap_err()
                .code,
            ManufacturingErrorCode::VerificationFailed,
            "the alternate physical endpoint is raw-half-step-close but not the encoded coordinate"
        );
    }
}

#[test]
fn actual_output_refuses_coordinates_without_unique_xyz6_numeric_correspondence() {
    let (mut intent, mut plan) = fixture();
    let shift = program_coordinates::MAX_XYZ6_COORDINATE_MM + 32.0;
    intent.printer.build_envelope.max[0] = shift + 100.0;
    for layer in &mut plan.layers {
        for path in &mut layer.paths {
            for point in &mut path.points_mm {
                point[0] += shift;
            }
        }
    }
    refresh_volumes(&intent, &mut plan);
    assert_eq!(
        crate::backend::emit(&plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ProgramCoordinateConditioningFailure
    );
}

#[test]
fn actual_emitted_motion_rejects_independent_tampering() {
    let (intent, plan) = fixture();
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let first_move = program
        .lines()
        .find(|line| line.starts_with("G1 "))
        .unwrap();
    let e = first_move
        .split_ascii_whitespace()
        .find(|word| word.starts_with('E'))
        .unwrap();
    let x = first_move
        .split_ascii_whitespace()
        .find(|word| word.starts_with('X'))
        .unwrap();
    let f = first_move
        .split_ascii_whitespace()
        .find(|word| word.starts_with('F'))
        .unwrap();
    for tampered in [
        program.replacen(first_move, &first_move.replace(e, "E99999.00000000"), 1),
        program.replacen(first_move, &first_move.replace(x, "X99.000000"), 1),
        program.replacen(first_move, &first_move.replace(f, "F0.000000"), 1),
        program.replacen(&format!("{first_move}\n"), "", 1),
        program.replacen("G90", "G91", 1),
        format!("{program}{first_move}\n"),
        format!("{program}M104 S200\n"),
    ] {
        assert_eq!(
            verify(&tampered, &plan, &intent, &cancel).unwrap_err().code,
            ManufacturingErrorCode::VerificationFailed
        );
    }
}

#[test]
fn diagonal_numeric_golden_measures_euclidean_length() {
    let (intent, mut plan) = fixture();
    plan.layers.truncate(1);
    plan.layers[0].paths = vec![DepositionPath {
        kind: DepositionKind::Perimeter,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::Constant { width_mm: 1.0 },
        physical_z_mm: [0.0, 1.0],
        points_mm: vec![
            [1.0, 1.0, 1.0],
            [4.0, 5.0, 1.0],
            [4.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
    }];
    refresh_volumes(&intent, &mut plan);
    let program = "G21\nG90\nM82\nG92 E0\nM106 S0\nG0 X1 Y1 Z1 F1200\nG1 X4 Y5 Z1 E1.25 F600\nG1 X4 Y1 Z1 E2.25 F600\nG1 X1 Y1 Z1 E3 F600\n";
    let report = verify(program, &plan, &intent, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.filament_length_mm, 3.0);
    assert_eq!(report.deposition_segments, 3);
    assert_eq!(report.travel_segments, 1);
}

#[test]
fn command_mutations_cannot_hide_behind_a_preview_or_verified_flag() {
    let cases = [
        ("underextrusion", GOLDEN.replacen("E1 F600", "E0.9 F600", 1)),
        ("overextrusion", GOLDEN.replacen("E1 F600", "E1.1 F600", 1)),
        (
            "small E tampering",
            GOLDEN.replacen("E1 F600", "E1.00000001 F600", 1),
        ),
        ("retraction", GOLDEN.replacen("E2 F600", "E0.5 F600", 1)),
        ("zero extrusion", GOLDEN.replacen("E2 F600", "E1 F600", 1)),
        ("coordinate", GOLDEN.replacen("G1 X5 Y1", "G1 X5.001 Y1", 1)),
        ("Z", GOLDEN.replacen("G1 X5 Y1 Z1", "G1 X5 Y1 Z1.001", 1)),
        ("feed", GOLDEN.replacen("F600", "F601", 1)),
        (
            "small feed tampering",
            GOLDEN.replacen("F600", "F600.000001", 1),
        ),
        ("travel feed", GOLDEN.replacen("F1200", "F1201", 1)),
        ("zero feed", GOLDEN.replacen("F600", "F0", 1)),
        ("negative feed", GOLDEN.replacen("F600", "F-600", 1)),
        ("speed overflow", GOLDEN.replacen("F600", "F600000", 1)),
        ("relative XYZ", GOLDEN.replacen("G90", "G91", 1)),
        ("relative E", GOLDEN.replacen("M82", "M83", 1)),
        ("inch units", GOLDEN.replacen("G21", "G20", 1)),
        ("setup order", GOLDEN.replacen("G21\nG90", "G90\nG21", 1)),
        ("missing setup", GOLDEN.replacen("M82\n", "", 1)),
        ("nonzero reset", GOLDEN.replacen("G92 E0", "G92 E1", 1)),
        ("repeat setup", GOLDEN.replacen("G92 E0", "G92 E0\nG90", 1)),
        (
            "late reset",
            GOLDEN.replacen("G1 X5 Y5", "G92 E0\nG1 X5 Y5", 1),
        ),
        (
            "extrusion during travel",
            GOLDEN.replacen("G0 X1 Y1 Z1 F1200", "G0 X1 Y1 Z1 E0 F1200", 1),
        ),
        (
            "travel replacing deposit",
            GOLDEN.replacen("G1 X5 Y1 Z1 E1 F600", "G0 X5 Y1 Z1 F600", 1),
        ),
        (
            "missing move",
            GOLDEN.replacen("G1 X5 Y5 Z1 E2 F600\n", "", 1),
        ),
        ("extra final move", format!("{GOLDEN}G0 X2 Y6 Z2 F1200\n")),
        (
            "missing last move",
            GOLDEN.replacen("G1 X2 Y6 Z2 E6 F600\n", "", 1),
        ),
        ("unsupported startup", GOLDEN.replacen("G21", "G28\nG21", 1)),
        ("unsupported heat", format!("{GOLDEN}M104 S200\n")),
        ("arc", GOLDEN.replacen("G1 X5 Y1", "G2 X5 Y1", 1)),
        (
            "duplicate axis",
            GOLDEN.replacen("G1 X5 Y1", "G1 X5 X5 Y1", 1),
        ),
        ("duplicate E", GOLDEN.replacen("E1 F600", "E1 E1 F600", 1)),
        (
            "duplicate feed",
            GOLDEN.replacen("E1 F600", "E1 F600 F600", 1),
        ),
        ("missing axis", GOLDEN.replacen("G1 X5 Y1", "G1 Y1", 1)),
        ("missing E", GOLDEN.replacen("E1 F600", "F600", 1)),
        ("missing feed", GOLDEN.replacen("E1 F600", "E1", 1)),
        ("nonfinite", GOLDEN.replacen("G1 X5 Y1", "G1 XNaN Y1", 1)),
        ("infinity", GOLDEN.replacen("E1 F600", "Einf F600", 1)),
        (
            "numeric overflow",
            GOLDEN.replacen("E1 F600", &format!("E{} F600", "9".repeat(310)), 1),
        ),
        ("exponent", GOLDEN.replacen("E1 F600", "E1e0 F600", 1)),
        (
            "unbounded precision",
            GOLDEN.replacen("G1 X5 Y1", "G1 X5.0000000 Y1", 1),
        ),
        (
            "empty fractional",
            GOLDEN.replacen("E1 F600", "E1. F600", 1),
        ),
        (
            "malformed fractional",
            GOLDEN.replacen("E1 F600", "E1.0.0 F600", 1),
        ),
        ("checksums", GOLDEN.replacen("E1 F600", "E1 F600*42", 1)),
        ("line numbers", GOLDEN.replacen("G21", "N1 G21", 1)),
        (
            "unsupported field",
            GOLDEN.replacen("E1 F600", "E1 S1 F600", 1),
        ),
        (
            "parenthesis comment",
            GOLDEN.replacen("G21", "G21 (units)", 1),
        ),
        ("control byte", GOLDEN.replacen("G21", "\u{000b}G21", 1)),
    ];
    for (label, program) in cases {
        let error = replay(&program).expect_err(label);
        assert_eq!(
            error.code,
            ManufacturingErrorCode::VerificationFailed,
            "{label}: {error}"
        );
    }
}

#[test]
fn permitted_comments_whitespace_and_numeric_forms_do_not_change_semantics() {
    let program = GOLDEN
        .replace("G21", "\tG21 ; inert units comment")
        .replace("G92 E0", "G92 E+0.00000000")
        .replace("\n", "\r\n");
    let report = replay(&program).unwrap();
    assert_eq!(report.filament_length_mm, 6.0);
}

#[test]
fn replay_thresholds_do_not_expand_to_plan_approximation_budget() {
    let (intent, mut plan) = fixture();
    let cancel = AtomicBool::new(false);
    for layer in &mut plan.layers {
        for path in &mut layer.paths {
            for point in &mut path.points_mm {
                point[0] += 0.000_000_4;
            }
        }
    }
    let report = verify(GOLDEN, &plan, &intent, &cancel).unwrap();
    assert!((report.max_position_error_mm - 0.000_000_4).abs() < 1e-14);
    // The 5 um native sampling error does not authorize a 0.6 um replay error.
    for layer in &mut plan.layers {
        for path in &mut layer.paths {
            for point in &mut path.points_mm {
                point[0] += 0.000_000_2;
            }
        }
    }
    assert_eq!(
        verify(GOLDEN, &plan, &intent, &cancel).unwrap_err().code,
        ManufacturingErrorCode::VerificationFailed
    );
}

#[test]
fn decoded_coordinates_must_fit_even_when_quantization_matches_expected_point() {
    let (mut intent, mut plan) = fixture();
    intent.printer.build_envelope.min[0] = 1.000_000_4;
    for layer in &mut plan.layers {
        for path in &mut layer.paths {
            for point in &mut path.points_mm {
                point[0] += 0.000_000_4;
            }
        }
    }
    assert_eq!(
        verify(GOLDEN, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::VerificationFailed
    );
}

#[test]
fn zero_length_after_serialization_is_not_positive_deposition() {
    let (intent, mut plan) = fixture();
    plan.layers.truncate(1);
    plan.native_geometry.certified_layer_count = 1;
    plan.layers[0].paths = vec![DepositionPath {
        kind: DepositionKind::SolidFill,
        owner: None,
        width: WidthProfile::Constant { width_mm: 1.0 },
        physical_z_mm: [0.0, 1.0],
        points_mm: vec![[2.0, 2.0, 1.0], [2.000_000_1, 2.0, 1.0]],
    }];
    refresh_volumes(&intent, &mut plan);
    let program =
        "G21\nG90\nM82\nG92 E0\nM106 S0\nG0 X2 Y2 Z1 F1200\nG1 X2 Y2 Z1 E0.00000003 F600\n";
    assert_eq!(
        verify(program, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::VerificationFailed
    );
}

fn wall_command_fixture(path: DepositionPath) -> (ManufacturingIntent, NormalizedPrintPlan) {
    let (mut intent, mut plan) = fixture();
    intent.recipe.layer_height_mm = path.height_mm();
    intent.recipe.bead_width_mm = 0.45;
    plan.layers.truncate(1);
    let layer = &mut plan.layers[0];
    layer.lower_z_mm = path.physical_z_mm[0];
    layer.z_mm = path.physical_z_mm[1];
    layer.height_mm = path.height_mm();
    layer.section_z_mm = (layer.lower_z_mm + layer.z_mm) * 0.5;
    layer.paths = vec![path];
    refresh_volumes(&intent, &mut plan);
    (intent, plan)
}

#[test]
fn variable_width_wall_bridge_keeps_rotated_edge_associations_and_bridge_process() {
    let (mut intent, mut plan) = wall_command_fixture(DepositionPath {
        kind: DepositionKind::Bridge,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::PerEdge {
            widths_mm: vec![0.45, 0.46, 0.47, 0.48],
        },
        physical_z_mm: [0.0, 0.2],
        points_mm: vec![
            [1.0, 1.0, 0.2],
            [5.0, 1.0, 0.2],
            [5.0, 5.0, 0.2],
            [1.0, 5.0, 0.2],
            [1.0, 1.0, 0.2],
        ],
    });
    intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [1.0, 0.0] };
    intent.recipe.process.first_layer_flow_factor = 1.1;
    intent.recipe.process.bridge_flow_factor = 0.8;
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
    let path = &plan.layers[0].paths[0];
    assert_eq!(path.points_mm[0], [5.0, 1.0, 0.2]);
    assert_eq!(
        path.checked_edges()
            .unwrap()
            .map(|edge| edge.width_mm)
            .collect::<Vec<_>>(),
        vec![0.46, 0.47, 0.48, 0.45]
    );
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let report = verify(&program, &plan, &intent, &cancel).unwrap();
    let expected_volume =
        4.0 * (0.265 * 0.2 + std::f64::consts::PI * 0.1_f64.powi(2)) * 4.0 * 1.1 * 0.8;
    assert_eq!(report.deposition_segments, 4);
    assert!((report.deposited_volume_mm3 - expected_volume).abs() < 2e-8);
    assert!(program.contains("M106 S255\n"));
    assert!(
        program
            .lines()
            .filter(|line| line.starts_with("G1 "))
            .all(|line| line.ends_with("F300.000000"))
    );

    // Actual bytes retain their original edge-local widths, not whichever
    // width association a later in-memory consumer happens to attach.
    if let WidthProfile::PerEdge { widths_mm } = &mut plan.layers[0].paths[0].width {
        widths_mm.rotate_left(1);
    }
    refresh_volumes(&intent, &mut plan);
    assert_eq!(
        verify(&program, &plan, &intent, &cancel).unwrap_err().code,
        ManufacturingErrorCode::VerificationFailed
    );
}

#[test]
fn minimum_deposition_length_is_the_decoded_xyz6_segment_not_the_raw_edge() {
    let (intent, plan) = wall_command_fixture(DepositionPath {
        kind: DepositionKind::Bridge,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.2],
        points_mm: vec![[1.0, 1.0, 0.2], [1.011_999_49, 1.016_000_49, 0.2]],
    });
    let length = distance(
        plan.layers[0].paths[0].points_mm[0],
        plan.layers[0].paths[0].points_mm[1],
    );
    assert!(length > program_coordinates::MIN_DEPOSITION_COMMAND_MM);
    assert!(
        distance([1.0, 1.0, 0.2], [1.011_999, 1.016, 0.2])
            < program_coordinates::MIN_DEPOSITION_COMMAND_MM
    );
    let e = length * (0.25 * 0.2 + std::f64::consts::PI * 0.1_f64.powi(2)) / std::f64::consts::PI;
    let program = format!(
        "G21\nG90\nM82\nG92 E0\nM106 S255\nG0 X1 Y1 Z0.2 F1200\nG1 X1.011999 Y1.016 Z0.2 E{e:.8} F300\n"
    );
    assert_eq!(
        verify(&program, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::VerificationFailed
    );
}

#[test]
fn actual_e8_width_cannot_borrow_the_nominal_width_association() {
    let (intent, plan) = wall_command_fixture(DepositionPath {
        kind: DepositionKind::Bridge,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.000_01],
        points_mm: vec![[1.0, 1.0, 0.000_01], [1.02, 1.0, 0.000_01]],
    });
    // E=3e-8 is the ordinary E8 rounding of this nominal stroke, but its
    // independently inverted width is about 0.47124 rather than 0.45 mm.
    let program = "G21\nG90\nM82\nG92 E0\nM106 S255\nG0 X1 Y1 Z0.00001 F1200\nG1 X1.02 Y1 Z0.00001 E0.00000003 F300\n";
    let error = verify(program, &plan, &intent, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::VerificationFailed);
}

#[test]
fn wall_gradient_uses_actual_e8_widths_and_checks_the_closed_seam() {
    let cancel = AtomicBool::new(false);
    let (intent, plan) = wall_command_fixture(DepositionPath {
        kind: DepositionKind::Bridge,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::PerEdge {
            widths_mm: vec![0.451, 0.455],
        },
        physical_z_mm: [0.0, 0.2],
        points_mm: vec![[1.0, 1.0, 0.2], [1.02, 1.0, 0.2], [1.0, 1.0, 0.2]],
    });
    // Nominal widths meet 0.20/mm. Actual E8 commands invert to about
    // 0.450997545 and 0.455003076: their gradient exceeds the frozen limit.
    let program = "G21\nG90\nM82\nG92 E0\nM106 S255\nG0 X1 Y1 Z0.2 F1200\nG1 X1.02 Y1 Z0.2 E0.00051958 F300\nG1 X1 Y1 Z0.2 E0.00104426 F300\n";
    assert_eq!(
        verify(program, &plan, &intent, &cancel).unwrap_err().code,
        ManufacturingErrorCode::VerificationFailed
    );

    let (intent, plan) = wall_command_fixture(DepositionPath {
        kind: DepositionKind::Bridge,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::PerEdge {
            widths_mm: vec![0.45, 0.455, 0.46],
        },
        physical_z_mm: [0.0, 0.2],
        points_mm: vec![
            [1.0, 1.0, 0.2],
            [1.03, 1.0, 0.2],
            [1.015, 1.025_981, 0.2],
            [1.0, 1.0, 0.2],
        ],
    });
    // Both stored interior transitions are below 0.20/mm; only last→first
    // violates it. Emission matching the same bad plan is not independent proof.
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert_eq!(
        verify(&program, &plan, &intent, &cancel).unwrap_err().code,
        ManufacturingErrorCode::VerificationFailed
    );
}

#[test]
fn printer_limits_and_explicit_flow_multiplier_are_authoritative() {
    let (mut intent, plan) = fixture();
    intent.recipe.flow_multiplier = 2.0;
    assert_eq!(
        verify(GOLDEN, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::VerificationFailed
    );
    intent.recipe.flow_multiplier = 1.0;
    intent.printer.max_volumetric_flow_mm3_s = 7.86;
    assert!(verify(GOLDEN, &plan, &intent, &AtomicBool::new(false)).is_ok());
    intent.printer.max_volumetric_flow_mm3_s = 7.0;
    assert_eq!(
        verify(GOLDEN, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::InvalidSpecification
    );
    intent.printer.max_volumetric_flow_mm3_s = 100.0;
    intent.printer.max_feed_mm_s = 19.0;
    assert_eq!(
        verify(GOLDEN, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::InvalidSpecification
    );
}

#[test]
fn cancellation_and_parser_resources_are_bounded() {
    let (intent, plan) = fixture();
    assert_eq!(
        verify(GOLDEN, &plan, &intent, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::Cancelled
    );
    let long_line = format!(";{}", "x".repeat(MAX_LINE_BYTES));
    assert_eq!(
        verify(&long_line, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    let oversized = ";".repeat(MAX_MANUFACTURING_BUNDLE_BYTES as usize + 1);
    assert_eq!(
        verify(&oversized, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn packed_sink_admits_two_hundred_thousand_actual_events_once() {
    let (intent, mut plan) = fixture();
    plan.layers.truncate(1);
    plan.layers[0].paths = vec![
        spiling_contracts::manufacturing::DepositionPath {
            kind: spiling_contracts::manufacturing::DepositionKind::SolidFill,
            owner: None,
            width: WidthProfile::Constant { width_mm: 1.0 },
            physical_z_mm: [0.0, 1.0],
            points_mm: vec![[1.0, 1.0, 1.0], [5.0, 1.0, 1.0]],
        };
        spiling_contracts::manufacturing::MAX_DEPOSITION_SEGMENTS as usize
    ];
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    let report = verify_actual(&program, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
    assert_eq!(report.deposition_segments, 100_000);
    assert_eq!(report.travel_segments, 100_000);
    let (bytes, bounds) = sink.finish().unwrap();
    assert_eq!(
        bytes.len(),
        REPLAY_HEADER_BYTES + REPLAY_RECORD_BYTES * 200_000
    );
    assert!(
        bytes.capacity() <= spiling_contracts::manufacturing::MAX_REPLAY_DISPLAY_BYTES as usize
    );
    assert_eq!(bounds.min, [1.0, 1.0, 1.0]);
    assert_eq!(bounds.max, [5.0, 1.0, 1.0]);
    let excess_path = plan.layers[0].paths[0].clone();
    plan.layers[0].paths.push(excess_path);
    assert!(ReplaySink::new(&plan, &intent).is_err());
}

fn complete_fixture() -> (ManufacturingIntent, NormalizedPrintPlan) {
    let (mut intent, mut plan) = fixture();
    intent.printer.capabilities.output_dialect = OutputDialect::Marlin2SingleExtruderGcodeV1;
    intent.printer.workflow = JobWorkflow::CompleteMarlin {
        job: SingleExtruderJob {
            machine_id: "unqualified-software-test-machine".into(),
            firmware_version: "2.1.2.5".into(),
            configuration_hash: spiling_contracts::geometry::SourceHash::from_bytes(
                b"test configuration declaration",
            ),
            calibration_record_hash: spiling_contracts::geometry::SourceHash::from_bytes(
                b"test calibration declaration, not a measurement",
            ),
            homing_contract: HomingCoordinateContract::MarlinNativeCartesian,
            home_position_mm: [0.0; 3],
            nozzle_temperature_c: 200.0,
            bed_temperature_c: 60.0,
            minimum_extrusion_temperature_c: 170.0,
            maximum_nozzle_temperature_c: 250.0,
            maximum_bed_temperature_c: 100.0,
            prime_start_mm: [10.0, 10.0, 1.0],
            prime_end_mm: [20.0, 10.0, 1.0],
            prime_filament_length_mm: 1.0,
            prime_speed_mm_s: 5.0,
            park_position_mm: [0.0, 0.0, 10.0],
        },
    };
    crate::realization::canonicalize(&mut plan, &intent, &AtomicBool::new(false)).unwrap();
    (intent, plan)
}

#[test]
fn complete_job_accounts_priming_separately_and_streams_lifecycle() {
    let (intent, plan) = complete_fixture();
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    let report = verify_actual(&program, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
    assert_eq!(report.filament_length_mm, 6.0);
    assert_eq!(report.priming_filament_length_mm, 1.0);
    assert_eq!(report.lifecycle_motion_count, 4);
    let (bytes, bounds) = sink.finish().unwrap();
    assert_eq!(bytes.len(), REPLAY_HEADER_BYTES + REPLAY_RECORD_BYTES * 13);
    assert_eq!(bounds.max, [20.0, 10.0, 10.0]);
    assert_eq!(&bytes[28..32], &4u32.to_le_bytes());
    assert_eq!(bytes[96 + 46], 6);
    assert_eq!(bytes[96 + REPLAY_RECORD_BYTES + 46], 7);
    report.validate().unwrap();
    let hash = spiling_contracts::geometry::SourceHash::from_bytes(&bytes);
    let descriptor = ReplayDisplayDescriptor {
        info: ReplayDisplayInfo {
            schema_version: REPLAY_SCHEMA_VERSION,
            bundle_hash: spiling_contracts::geometry::SourceHash::from_bytes(b"test bundle"),
            input_hash: spiling_contracts::geometry::SourceHash::from_bytes(b"test intent"),
            program_hash: spiling_contracts::geometry::SourceHash::from_bytes(program.as_bytes()),
            verifier_version: "test".into(),
            frame: intent.printer.coordinate_frame,
            event_count: 13,
            deposition_count: 6,
            travel_count: 3,
            lifecycle_count: 4,
            process_count: 0,
            layer_count: 2,
            bounds_mm: bounds,
        },
        resource: spiling_contracts::ArtifactView {
            name: format!("artifacts/{}", hash.as_str()),
            sha256: hash.as_str().into(),
            size_bytes: bytes.len().to_string(),
            media_type: REPLAY_MEDIA_TYPE.into(),
        },
    };
    validate_replay_display(&bytes, &descriptor).unwrap();
    for offset in [
        96 + 44,
        96 + 46,
        96 + REPLAY_RECORD_BYTES + 46,
        96 + 11 * REPLAY_RECORD_BYTES + 16,
    ] {
        let mut corrupt = bytes.clone();
        corrupt[offset] ^= 1;
        let mut rebound = descriptor.clone();
        let hash = spiling_contracts::geometry::SourceHash::from_bytes(&corrupt);
        rebound.resource.name = format!("artifacts/{}", hash.as_str());
        rebound.resource.sha256 = hash.as_str().into();
        assert!(validate_replay_display(&corrupt, &rebound).is_err());
    }
}

#[test]
fn complete_job_rejects_lifecycle_modal_thermal_barrier_and_extrusion_mutations() {
    let (intent, plan) = complete_fixture();
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    for (from, to) in [
        ("M200 D0", "M200 D1.75"),
        ("M220 S100", "M220 S90"),
        ("M221 S100", "M221 S110"),
        ("G28\n", ""),
        ("M420 S0", "M420 S1"),
        ("M190 R60.000000", "M190 S60"),
        ("M109 R200.000000", "M109 R190"),
        ("M109 R200.000000\n", ""),
        ("M400\n", ""),
        ("M104 S0", "M104 S200"),
        ("M84\n", ""),
        ("M82\n", "M83\n"),
        ("G90\n", "G91\n"),
        ("Z10.000000", "Z1.000000"),
        ("E1.00000000", "E2.00000000"),
        ("G28", "START_PRINT"),
        ("M107", "M106 S255"),
    ] {
        let mutated = program.replacen(from, to, 1);
        assert_ne!(mutated, program, "{from}");
        assert!(
            verify(&mutated, &plan, &intent, &cancel).is_err(),
            "{from} -> {to}"
        );
    }
    assert!(verify(&(program + "G0 X0 Y0 Z0 F1200\n"), &plan, &intent, &cancel).is_err());
}

#[test]
fn complete_job_rejects_unsafe_or_incompatible_specifications() {
    let (intent, plan) = complete_fixture();
    for mutation in 0..5 {
        let mut bad = intent.clone();
        if let JobWorkflow::CompleteMarlin { job } = &mut bad.printer.workflow {
            match mutation {
                0 => job.prime_end_mm[0] = 101.0,
                1 => job.park_position_mm[2] = 2.0,
                2 => job.nozzle_temperature_c = 100.0,
                3 => job.prime_filament_length_mm = 101.0,
                _ => job.firmware_version = "unknown".into(),
            }
        }
        assert!(crate::backend::emit(&plan, &bad, &AtomicBool::new(false)).is_err());
    }
    let mut bad = intent;
    bad.printer.capabilities.output_dialect = OutputDialect::CartesianAbsoluteGcodeV1;
    assert!(bad.validate().is_err());
}

#[test]
fn region_nominal_measurement_mutation_is_not_trusted() {
    let (intent, mut plan) = fixture();
    plan.layers[0].region_coverage.nominal_deposition_volume_mm3 += 1.0;
    assert!(verify(GOLDEN, &plan, &intent, &AtomicBool::new(false)).is_err());
}

#[test]
fn complete_job_requires_integral_celsius_and_explicit_temperature_units() {
    let (intent, plan) = complete_fixture();
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    for replacement in [
        "",
        "M149 F\n",
        "M149 K\n",
        "M149\n",
        "M149 C F\n",
        "M149 C1\n",
    ] {
        let mutated = program.replacen("M149 C\n", replacement, 1);
        assert!(
            verify(&mutated, &plan, &intent, &cancel).is_err(),
            "{replacement:?}"
        );
    }
    for temperature in [-0.1, 0.5, 169.999999, 200.25, 249.999999, 250.000001] {
        let mut bad = intent.clone();
        if let JobWorkflow::CompleteMarlin { job } = &mut bad.printer.workflow {
            job.nozzle_temperature_c = temperature;
        }
        assert!(bad.validate().is_err(), "{temperature}");
        assert!(crate::backend::emit(&plan, &bad, &cancel).is_err());
    }
    for temperature in [170.0, 250.0] {
        let mut boundary = intent.clone();
        if let JobWorkflow::CompleteMarlin { job } = &mut boundary.printer.workflow {
            job.nozzle_temperature_c = temperature;
        }
        let program = crate::backend::emit(&plan, &boundary, &cancel).unwrap();
        verify(&program, &plan, &boundary, &cancel).unwrap();
    }
    let mut bad = intent;
    if let JobWorkflow::CompleteMarlin { job } = &mut bad.printer.workflow {
        job.bed_temperature_c = 60.5;
    }
    assert!(bad.validate().is_err());
}

#[test]
fn priming_excludes_whole_object_xy_even_above_object_and_across_gaps() {
    let (intent, plan) = complete_fixture();
    for (start, end) in [
        ([0.0, 3.0, 1.0], [8.0, 3.0, 1.0]),
        ([3.0, 3.0, 1.0], [4.0, 3.0, 1.0]),
        ([0.0, 3.0, 9.0], [8.0, 3.0, 9.0]),
        ([0.0, 0.5, 1.0], [8.0, 0.5, 1.0]),
    ] {
        let mut bad = intent.clone();
        if let JobWorkflow::CompleteMarlin { job } = &mut bad.printer.workflow {
            job.prime_start_mm = start;
            job.prime_end_mm = end;
        }
        assert!(bad.validate().is_ok()); // Individually bounded lifecycle data.
        assert!(plan.validate(&bad).is_err()); // Captured object ownership matters.
        assert!(crate::backend::emit(&plan, &bad, &AtomicBool::new(false)).is_err());
    }
}

#[test]
fn priming_exclusion_accounts_for_emitted_rounding_at_boundary() {
    let (intent, plan) = complete_fixture();
    let cancel = AtomicBool::new(false);
    for (y, admissible) in [(0.499998, true), (0.4999988, false), (0.5, false)] {
        let mut candidate = intent.clone();
        if let JobWorkflow::CompleteMarlin { job } = &mut candidate.printer.workflow {
            job.prime_start_mm = [0.0, y, 1.0];
            job.prime_end_mm = [8.0, y, 1.0];
        }
        assert_eq!(plan.validate(&candidate).is_ok(), admissible, "{y}");
        if admissible {
            let mut plan = plan.clone();
            crate::realization::canonicalize(&mut plan, &candidate, &cancel).unwrap();
            let program = crate::backend::emit(&plan, &candidate, &cancel).unwrap();
            verify(&program, &plan, &candidate, &cancel).unwrap();
            let mutated = program.replace("Y0.499998", "Y0.500000");
            assert!(verify(&mutated, &plan, &candidate, &cancel).is_err());
        }
    }
}

fn process_fixture() -> (ManufacturingIntent, NormalizedPrintPlan) {
    let (mut intent, mut plan) = fixture();
    intent.printer.capabilities.generated_supports = true;
    intent.recipe.supports = SupportPolicy::Breakaway {
        density: 0.5,
        xy_clearance_mm: 0.5,
        z_gap_mm: 0.25,
        interface_thickness_mm: 0.5,
    };
    intent.recipe.process = ProcessPolicy {
        first_layer_speed_factor: 0.5,
        first_layer_flow_factor: 1.2,
        fan_first_layers: 0,
        fan_after_first_layers: 120,
        bridge_speed_factor: 0.4,
        bridge_flow_factor: 0.8,
        bridge_fan_pwm: 220,
        max_bridge_span_mm: 5.0,
        retraction: RetractionPolicy::Enabled {
            distance_mm: 0.4,
            speed_mm_s: 15.0,
            minimum_travel_mm: 1.0,
            z_hop_mm: 0.25,
        },
    };
    plan.layers[0].paths = vec![
        DepositionPath {
            kind: DepositionKind::SolidFill,
            owner: None,
            width: WidthProfile::Constant { width_mm: 1.0 },
            physical_z_mm: [0.0, 1.0],
            points_mm: vec![[9.0, 1.0, 1.0], [11.0, 1.0, 1.0]],
        },
        DepositionPath {
            kind: DepositionKind::SupportInterface,
            owner: None,
            width: WidthProfile::Constant { width_mm: 1.0 },
            physical_z_mm: [0.0, 1.0],
            points_mm: vec![[5.0, 1.0, 1.0], [7.0, 1.0, 1.0]],
        },
        DepositionPath {
            kind: DepositionKind::Support,
            owner: None,
            width: WidthProfile::Constant { width_mm: 1.0 },
            physical_z_mm: [0.0, 1.0],
            points_mm: vec![[1.0, 1.0, 1.0], [3.0, 1.0, 1.0]],
        },
    ];
    plan.layers[0].region_coverage.support_area_mm2 = 4.0;
    plan.layers[1].height_mm = 0.5;
    plan.layers[1].z_mm = 1.5;
    plan.layers[1].section_z_mm = 1.25;
    plan.layers[1].paths = vec![DepositionPath {
        kind: DepositionKind::Bridge,
        owner: None,
        width: WidthProfile::Constant { width_mm: 1.0 },
        physical_z_mm: [1.0, 1.5],
        points_mm: vec![[11.0, 1.0, 1.5], [15.0, 1.0, 1.5]],
    }];
    refresh_volumes(&intent, &mut plan);
    crate::realization::canonicalize(&mut plan, &intent, &AtomicBool::new(false)).unwrap();
    (intent, plan)
}

// Written independently: support is 4*1.2*pi/4; object is 2*1.2*pi/4
// plus the half-height bridge 4*0.8*(0.25+pi/16). Recovery adds no material.
const PROCESS_GOLDEN: &str = "\
G21
G90
M82
G92 E0
M106 S0
G0 X1 Y1 Z1 F1200
G1 X3 Y1 Z1 E0.6 F300
G1 E0.2 F900
G0 X3 Y1 Z1.25 F1200
G0 X5 Y1 Z1.25 F1200
G0 X5 Y1 Z1 F1200
G1 E0.6 F900
G1 X7 Y1 Z1 E1.2 F300
G1 E0.8 F900
G0 X7 Y1 Z1.25 F1200
G0 X9 Y1 Z1.25 F1200
G0 X9 Y1 Z1 F1200
G1 E1.2 F900
G1 X11 Y1 Z1 E1.8 F300
M106 S220
G0 X11 Y1 Z1.5 F1200
G1 X15 Y1 Z1.5 E2.25464791 F240
";

fn packed_process_descriptor(
    bytes: &[u8],
    bounds: AabbMm,
    program: &str,
    intent: &ManufacturingIntent,
    plan: &NormalizedPrintPlan,
    report: &VerificationReport,
) -> ReplayDisplayDescriptor {
    let hash = spiling_contracts::geometry::SourceHash::from_bytes(bytes);
    ReplayDisplayDescriptor {
        info: ReplayDisplayInfo {
            schema_version: REPLAY_SCHEMA_VERSION,
            bundle_hash: spiling_contracts::geometry::SourceHash::from_bytes(
                b"standalone software fixture",
            ),
            input_hash: spiling_contracts::geometry::SourceHash::from_bytes(
                &serde_json::to_vec(intent).unwrap(),
            ),
            program_hash: spiling_contracts::geometry::SourceHash::from_bytes(program.as_bytes()),
            verifier_version: crate::VERIFIER_VERSION.into(),
            frame: intent.printer.coordinate_frame,
            event_count: report.deposition_segments
                + report.support_deposition_segments
                + report.travel_segments
                + report.retraction_count * 2
                + report.lifecycle_motion_count,
            deposition_count: report.deposition_segments + report.support_deposition_segments,
            travel_count: report.travel_segments,
            lifecycle_count: report.lifecycle_motion_count,
            process_count: report.retraction_count * 2,
            layer_count: plan.layers.len() as u32,
            bounds_mm: bounds,
        },
        resource: spiling_contracts::ArtifactView {
            name: format!("artifacts/{}", hash.as_str()),
            sha256: hash.as_str().into(),
            size_bytes: bytes.len().to_string(),
            media_type: REPLAY_MEDIA_TYPE.into(),
        },
    }
}

#[test]
fn independent_process_golden_separates_support_object_and_stationary_recovery() {
    let (intent, plan) = process_fixture();
    let cancel = AtomicBool::new(false);
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    let report = verify_actual(PROCESS_GOLDEN, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
    assert_eq!(report.support_deposition_segments, 2);
    assert_eq!(report.deposition_segments, 2);
    assert_eq!(report.travel_segments, 8);
    assert_eq!(report.retraction_count, 2);
    assert!((report.retracted_filament_mm - 0.8).abs() < 1e-12);
    assert!((report.support_filament_length_mm - 1.2).abs() < 1e-12);
    assert!((report.support_deposited_volume_mm3 - 1.2 * std::f64::consts::PI).abs() < 1e-12);
    assert!((report.deposited_volume_mm3 - (0.8 + 0.8 * std::f64::consts::PI)).abs() < 2e-8);
    let (bytes, bounds) = sink.finish().unwrap();
    validate_replay_display(
        &bytes,
        &packed_process_descriptor(&bytes, bounds, PROCESS_GOLDEN, &intent, &plan, &report),
    )
    .unwrap();
    assert_eq!(&bytes[88..92], &4u32.to_le_bytes());
    let records = bytes[96..].as_chunks::<REPLAY_RECORD_BYTES>().0;
    assert_eq!(records.iter().filter(|r| r[46] == 11).count(), 2);
    assert_eq!(records.iter().filter(|r| r[46] == 12).count(), 2);
    assert_eq!(
        records.iter().filter(|r| matches!(r[46], 8..=10)).count(),
        3
    );
    for (i, r) in records
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r[46], 11 | 12))
    {
        assert!(i > 0);
        assert_eq!(
            &r[..24],
            &records[i - 1][..24],
            "recovery must have actual stationary XYZ"
        );
    }
    report.validate().unwrap();
    let emitted = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let actual = verify(&emitted, &plan, &intent, &cancel).unwrap();
    assert_eq!(
        actual.support_deposition_segments,
        report.support_deposition_segments
    );
    assert!((actual.filament_length_mm - report.filament_length_mm).abs() < 1e-12);
}

#[test]
fn independently_rejects_cooling_recovery_first_layer_bridge_and_hop_mutations() {
    let (intent, plan) = process_fixture();
    let cancel = AtomicBool::new(false);
    for (from, to) in [
        ("M106 S0", "M106 S120"),
        ("M106 S220", "M106 S120"),
        ("M106 S220\n", ""),
        ("G1 E0.2 F900", "G1 E0.3 F900"),
        ("G1 E0.6 F900", "G1 E0.7 F900"),
        ("G1 E0.6 F900\n", ""),
        ("G1 E0.2 F900", "G1 X3 Y1 Z1 E0.2 F900"),
        ("G1 E0.2 F900", "G1 E0.2 F600"),
        ("Z1.25 F1200", "Z1.5 F1200"),
        ("E0.6 F300", "E0.6 F600"),
        ("E0.6 F300", "E0.5 F300"),
        ("E2.25464791 F240", "E2.25464791 F600"),
        ("E2.25464791 F240", "E2.36830989 F240"),
    ] {
        let mutated = PROCESS_GOLDEN.replacen(from, to, 1);
        assert_ne!(mutated, PROCESS_GOLDEN);
        assert!(
            verify(&mutated, &plan, &intent, &cancel).is_err(),
            "{from} -> {to}"
        );
    }
}

#[test]
fn unretraction_restores_actual_endpoint_without_decimal_tolerance() {
    let (mut intent, mut plan) = process_fixture();
    intent.recipe.filament_diameter_mm = 0.002;
    plan.layers.truncate(1);
    refresh_volumes(&intent, &mut plan);
    let program = "\
G21
G90
M82
G92 E0
M106 S0
G0 X1 Y1 Z1 F1200
G1 X3 Y1 Z1 E600000 F300
G1 E599999.6 F900
G0 X3 Y1 Z1.25 F1200
G0 X5 Y1 Z1.25 F1200
G0 X5 Y1 Z1 F1200
G1 E600000 F900
G1 X7 Y1 Z1 E1200000 F300
G1 E1199999.6 F900
G0 X7 Y1 Z1.25 F1200
G0 X9 Y1 Z1.25 F1200
G0 X9 Y1 Z1 F1200
G1 E1200000 F900
G1 X11 Y1 Z1 E1800000 F300
";
    let cancel = AtomicBool::new(false);
    let baseline = verify(program, &plan, &intent, &cancel).unwrap();
    assert_eq!(baseline.retraction_count, 2);
    let mutated = program.replacen("G1 E1200000 F900", "G1 E1200000.00000001 F900", 1);
    assert!(verify(&mutated, &plan, &intent, &cancel).is_err());
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    assert!(verify_actual(&mutated, &plan, &intent, &cancel, Some(&mut sink)).is_err());
}

#[test]
fn cooling_transition_and_variable_height_properties_use_actual_bytes() {
    let cancel = AtomicBool::new(false);
    for height in [0.1, 0.25, 0.5, 1.0] {
        for first_flow in [0.6, 1.0, 1.2] {
            let (mut intent, mut plan) = fixture();
            intent.recipe.process.first_layer_flow_factor = first_flow;
            intent.recipe.process.first_layer_speed_factor = 0.4;
            intent.recipe.process.fan_first_layers = 0;
            intent.recipe.process.fan_after_first_layers = 173;
            plan.layers[1].height_mm = height;
            plan.layers[1].z_mm = 1.0 + height;
            plan.layers[1].section_z_mm = 1.0 + height * 0.5;
            let z = plan.layers[1].z_mm;
            for path in &mut plan.layers[1].paths {
                path.physical_z_mm = [1.0, z];
                for p in &mut path.points_mm {
                    p[2] = z;
                }
            }
            refresh_volumes(&intent, &mut plan);
            let emitted = crate::backend::emit(&plan, &intent, &cancel).unwrap();
            assert!(emitted.contains("M106 S173\n"));
            let report = verify(&emitted, &plan, &intent, &cancel).unwrap();
            let expected = plan
                .layers
                .iter()
                .map(|l| l.region_coverage.nominal_deposition_volume_mm3)
                .sum::<f64>();
            assert!((report.deposited_volume_mm3 - expected).abs() < 2e-8);
            let wrong = emitted.replacen("M106 S173", "M106 S172", 1);
            assert!(verify(&wrong, &plan, &intent, &cancel).is_err());
        }
    }
}

#[test]
fn preferred_seam_rejects_a_matching_policy_ignoring_plan_and_program() {
    let (mut intent, mut plan) = fixture();
    intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [0.6, 0.8] };
    let cancel = AtomicBool::new(false);
    // General geometry validation deliberately accepts pre-canonical plans.
    plan.validate(&intent).unwrap();
    let error = verify(GOLDEN, &plan, &intent, &cancel).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::VerificationFailed);

    crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
    let canonical_program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert!(
        verify(&canonical_program, &plan, &intent, &cancel)
            .unwrap()
            .verified
    );
    assert_eq!(plan.layers[0].paths[0].points_mm[0], [5.0, 5.0, 1.0]);
    plan.layers[0].paths[0].rotate_closed(2).unwrap();
    plan.validate(&intent).unwrap();
    let policy_ignoring_program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert_ne!(policy_ignoring_program, canonical_program);
    let error = verify(&policy_ignoring_program, &plan, &intent, &cancel).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::VerificationFailed);
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    assert_eq!(
        verify_actual(
            &policy_ignoring_program,
            &plan,
            &intent,
            &cancel,
            Some(&mut sink)
        )
        .unwrap_err()
        .code,
        ManufacturingErrorCode::VerificationFailed,
        "Packed actual replay enforces the same authored seam boundary"
    );
}

#[test]
fn preferred_seam_ties_follow_the_previous_actual_contour_endpoint() {
    let (mut intent, mut plan) = fixture();
    intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [1.0, 0.0] };
    for point in &mut plan.layers[0].paths[0].points_mm {
        point[1] += 9.0;
    }
    plan.layers[0].paths.push(DepositionPath {
        kind: DepositionKind::Perimeter,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::Constant { width_mm: 1.0 },
        physical_z_mm: [0.0, 1.0],
        points_mm: vec![
            [7.0, 1.0, 1.0],
            [9.0, 1.0, 1.0],
            [9.0, 5.0, 1.0],
            [7.0, 5.0, 1.0],
            [7.0, 1.0, 1.0],
        ],
    });
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
    assert_eq!(plan.layers[0].paths[0].points_mm[0], [5.0, 10.0, 1.0]);
    assert_eq!(plan.layers[0].paths[1].points_mm[0], [9.0, 5.0, 1.0]);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert!(verify(&program, &plan, &intent, &cancel).unwrap().verified);
    plan.layers[0].paths[1].rotate_closed(3).unwrap();
    assert_eq!(plan.layers[0].paths[1].points_mm[0], [9.0, 1.0, 1.0]);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let error = verify(&program, &plan, &intent, &cancel).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::VerificationFailed);
}

#[test]
fn preferred_seam_exact_ties_use_projected_coordinates_and_prime_position() {
    let cancel = AtomicBool::new(false);
    for prime_y in [None, Some(10.0), Some(3.000_000_4)] {
        let (mut intent, mut plan) = if prime_y.is_some() {
            complete_fixture()
        } else {
            fixture()
        };
        intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [1.0, -0.0] };
        if let (Some(y), JobWorkflow::CompleteMarlin { job }) =
            (prime_y, &mut intent.printer.workflow)
        {
            job.prime_end_mm[1] = y;
        }
        // Both right vertices project to X5. Raw X must not defeat the exact
        // command-space tie: absent prior motion choose Y1, with high prime
        // choose Y5. A prime just above raw Y3 projects to Y3 and ties at Y1.
        plan.layers[0].paths[0].points_mm = vec![
            [1.0, 1.0, 1.0],
            [5.000_000_1, 1.0, 1.0],
            [5.000_000_4, 5.0, 1.0],
            [1.0, 5.0, 1.0],
            [1.0, 1.0, 1.0],
        ];
        refresh_volumes(&intent, &mut plan);
        crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
        let expected_y = if prime_y == Some(10.0) { 5.0 } else { 1.0 };
        assert_eq!(plan.layers[0].paths[0].points_mm[0][1], expected_y);
        let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
        assert!(verify(&program, &plan, &intent, &cancel).unwrap().verified);

        let other = plan.layers[0].paths[0].points_mm[..4]
            .iter()
            .position(|p| p[0] > 5.0 && p[1] != expected_y)
            .unwrap();
        plan.layers[0].paths[0].rotate_closed(other).unwrap();
        let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
        let error = verify(&program, &plan, &intent, &cancel).unwrap_err();
        assert_eq!(error.code, ManufacturingErrorCode::VerificationFailed);
    }
}

#[test]
fn preferred_seam_score_has_no_proximity_epsilon_for_wall_bridges() {
    let (mut intent, mut plan) = complete_fixture();
    intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [1.0, 0.0] };
    // A one-XYZ6-step score advantage wins even though the prime endpoint is
    // much closer to the lower-scoring upper vertex.
    for point in &mut plan.layers[0].paths[0].points_mm {
        if point[0] == 5.0 && point[1] == 1.0 {
            point[0] = 5.000_001;
        }
    }
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
    assert_eq!(plan.layers[0].paths[0].points_mm[0], [5.000_001, 1.0, 1.0]);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert!(verify(&program, &plan, &intent, &cancel).unwrap().verified);

    plan.layers[0].paths[0].rotate_closed(1).unwrap();
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let error = verify(&program, &plan, &intent, &cancel).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::VerificationFailed);

    // Relabelling construction to Bridge preserves the closed-wall seam
    // obligation while retaining its separate feed, flow and cooling policy.
    plan.layers[0].paths[0].kind = DepositionKind::Bridge;
    refresh_volumes(&intent, &mut plan);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert_eq!(
        verify(&program, &plan, &intent, &cancel).unwrap_err().code,
        ManufacturingErrorCode::VerificationFailed
    );
    crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
    assert_eq!(plan.layers[0].paths[0].points_mm[0], [5.000_001, 1.0, 1.0]);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let report = verify(&program, &plan, &intent, &cancel).unwrap();
    assert_eq!(report.deposition_segments, 6);
}

#[test]
fn nearest_seam_and_open_direction_are_useful_and_deterministic() {
    let (intent, mut plan) = fixture();
    plan.layers.truncate(1);
    // The second perimeter's stored seam is distant; the previous end is near
    // its third vertex. The following open stroke should enter its far endpoint.
    plan.layers[0].paths.push(DepositionPath {
        kind: DepositionKind::Perimeter,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::Constant { width_mm: 1.0 },
        physical_z_mm: [0.0, 1.0],
        points_mm: vec![
            [10.0, 10.0, 1.0],
            [10.0, 2.0, 1.0],
            [2.0, 2.0, 1.0],
            [2.0, 10.0, 1.0],
            [10.0, 10.0, 1.0],
        ],
    });
    plan.layers[0].paths[1].points_mm = vec![[20.0, 2.0, 1.0], [2.0, 2.0, 1.0]];
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
    assert_eq!(plan.layers[0].paths[1].points_mm[0], [2.0, 2.0, 1.0]);
    assert_eq!(plan.layers[0].paths[2].points_mm[0], [2.0, 2.0, 1.0]);
    let emitted = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert!(emitted.contains("G0 X2.000000 Y2.000000 Z1.000000"));
    assert!(!emitted.contains("G0 X20.000000 Y2.000000"));
    verify(&emitted, &plan, &intent, &cancel).unwrap();
    assert_eq!(
        emitted,
        crate::backend::emit(&plan, &intent, &cancel).unwrap()
    );
}

#[test]
fn support_only_layers_are_real_and_not_object_accounting() {
    let (mut intent, mut plan) = process_fixture();
    intent.recipe.process.retraction = RetractionPolicy::Disabled;
    plan.layers[0].paths.retain(|p| {
        matches!(
            p.kind,
            DepositionKind::Support | DepositionKind::SupportInterface
        )
    });
    let coverage = &mut plan.layers[0].region_coverage;
    coverage.section_area_mm2 = 0.0;
    coverage.source_section_area_mm2 = 0.0;
    coverage.wall_area_mm2 = 0.0;
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let report = verify(&program, &plan, &intent, &cancel).unwrap();
    assert_eq!(report.deposition_segments, 1);
    assert_eq!(report.support_deposition_segments, 2);
    assert!((report.deposited_volume_mm3 - (0.8 + 0.2 * std::f64::consts::PI)).abs() < 2e-8);
}

#[test]
fn real_contact_gap_admits_larger_than_minimum_but_rejects_insufficient_gap_or_missing_interface() {
    let (mut intent, mut plan) = process_fixture();
    intent.recipe.process.retraction = RetractionPolicy::Disabled;
    plan.layers[0].paths.retain(|p| {
        matches!(
            p.kind,
            DepositionKind::Support | DepositionKind::SupportInterface
        )
    });
    plan.layers[0].region_coverage.section_area_mm2 = 0.0;
    plan.layers[0].region_coverage.source_section_area_mm2 = 0.0;
    plan.layers[0].region_coverage.wall_area_mm2 = 0.0;
    let mut gap = PrintLayer {
        skin_command_fits: Vec::new(),
        index: 1,
        z_mm: 1.25,
        lower_z_mm: 1.0,
        height_mm: 0.25,
        section_z_mm: 1.125,
        paths: vec![],
        region_coverage: coverage(),
    };
    gap.region_coverage.section_area_mm2 = 0.0;
    gap.region_coverage.source_section_area_mm2 = 0.0;
    gap.region_coverage.wall_area_mm2 = 0.0;
    plan.layers[1].index = 2;
    plan.layers[1].height_mm = 0.25;
    plan.layers[1].lower_z_mm = 1.25;
    for path in &mut plan.layers[1].paths {
        path.physical_z_mm = [1.25, 1.5];
    }
    plan.layers[1].section_z_mm = 1.375;
    plan.layers.insert(1, gap);
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    let report = verify_actual(&program, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
    let (bytes, _) = sink.finish().unwrap();
    assert_eq!(&bytes[24..28], &3u32.to_le_bytes());
    for record in bytes[96..].as_chunks::<REPLAY_RECORD_BYTES>().0 {
        assert_ne!(u16::from_le_bytes(record[44..46].try_into().unwrap()), 1);
    }
    assert_eq!(report.support_deposition_segments, 2);
    assert_eq!(report.deposition_segments, 1);
    if let SupportPolicy::Breakaway { z_gap_mm, .. } = &mut intent.recipe.supports {
        *z_gap_mm = 0.1;
    }
    assert!(verify(&program, &plan, &intent, &cancel).is_ok());
    let mut detached = plan.clone();
    for path in &mut detached.layers[0].paths {
        if path.kind == DepositionKind::SupportInterface {
            path.kind = DepositionKind::Support;
        }
    }
    refresh_volumes(&intent, &mut detached);
    assert!(detached.validate(&intent).is_err());
    if let SupportPolicy::Breakaway { z_gap_mm, .. } = &mut intent.recipe.supports {
        *z_gap_mm = 0.3;
    }
    assert!(verify(&program, &plan, &intent, &cancel).is_err());
}

#[test]
fn threshold_and_zero_hop_recovery_preserve_absolute_extrusion_without_extra_xyz() {
    let cancel = AtomicBool::new(false);
    for minimum_travel_mm in [1.0, 10.0] {
        let (mut intent, mut plan) = fixture();
        intent.recipe.process.retraction = RetractionPolicy::Enabled {
            distance_mm: 10.0,
            speed_mm_s: 15.0,
            minimum_travel_mm,
            z_hop_mm: 0.0,
        };
        refresh_volumes(&intent, &mut plan);
        let emitted = crate::backend::emit(&plan, &intent, &cancel).unwrap();
        let mut sink = ReplaySink::new(&plan, &intent).unwrap();
        let report = verify_actual(&emitted, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
        let (bytes, bounds) = sink.finish().unwrap();
        validate_replay_display(
            &bytes,
            &packed_process_descriptor(&bytes, bounds, &emitted, &intent, &plan, &report),
        )
        .unwrap();
        assert_eq!(report.travel_segments, 3);
        assert_eq!(report.filament_length_mm, 6.0);
        if minimum_travel_mm == 1.0 {
            assert_eq!(report.retraction_count, 2);
            assert!(emitted.contains("G1 E-6.00000000 F900.000000\n"));
        } else {
            assert_eq!(report.retraction_count, 0);
        }
        assert!(
            emitted
                .lines()
                .filter(|line| line.starts_with("G1 E"))
                .all(|line| !line.contains('X') && !line.contains('Y') && !line.contains('Z'))
        );
    }
}

#[test]
fn complete_prime_to_first_body_can_retract_at_known_stationary_position() {
    let (mut intent, plan) = complete_fixture();
    intent.recipe.process.retraction = RetractionPolicy::Enabled {
        distance_mm: 0.4,
        speed_mm_s: 15.0,
        minimum_travel_mm: 1.0,
        z_hop_mm: 0.25,
    };
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    let report = verify_actual(&program, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
    let (bytes, bounds) = sink.finish().unwrap();
    assert_eq!(
        bytes[96 + 2 * REPLAY_RECORD_BYTES + 46],
        11,
        "first body event retracts after complete prime"
    );
    assert_eq!(
        &bytes[96 + REPLAY_RECORD_BYTES..96 + REPLAY_RECORD_BYTES + 24],
        &bytes[96 + 2 * REPLAY_RECORD_BYTES..96 + 2 * REPLAY_RECORD_BYTES + 24]
    );
    assert_eq!(report.priming_filament_length_mm, 1.0);
    assert!((report.filament_length_mm - 6.0).abs() < 1e-12);
    validate_replay_display(
        &bytes,
        &packed_process_descriptor(&bytes, bounds, &program, &intent, &plan, &report),
    )
    .unwrap();
}

#[test]
fn coarse_sparse_dimensions_are_verified_and_packed() {
    let (mut intent, mut plan) = fixture();
    intent.recipe.layer_height_mm = 0.1;
    intent.recipe.bead_width_mm = 0.45;
    intent.recipe.sparse_infill = SparseInfillPolicy::CombineEligible { max_height_mm: 0.3 };
    intent.printer.deposition_limits = DepositionLimits {
        min_bead_width_mm: 0.449,
        max_bead_width_mm: 0.451,
        min_bead_height_mm: 0.000001,
        max_bead_height_mm: 0.3,
    };
    plan.layers.truncate(1);
    plan.layers[0].paths.truncate(1);
    plan.layers[0].paths[0] = DepositionPath {
        kind: DepositionKind::SolidFill,
        owner: None,
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.1],
        points_mm: vec![[2.0, 2.0, 0.1], [6.0, 2.0, 0.1]],
    };
    plan.layers[0].lower_z_mm = 0.0;
    plan.layers[0].z_mm = 0.1;
    plan.layers[0].height_mm = 0.1;
    plan.layers[0].section_z_mm = 0.05;
    for index in 1..=2 {
        let mut layer = plan.layers[0].clone();
        layer.index = index;
        layer.lower_z_mm = if index == 1 { 0.1 } else { 0.2 };
        layer.z_mm = if index == 1 { 0.2 } else { 0.3 };
        layer.section_z_mm = (layer.lower_z_mm + layer.z_mm) * 0.5;
        let path = &mut layer.paths[0];
        path.physical_z_mm = [layer.lower_z_mm, layer.z_mm];
        for point in &mut path.points_mm {
            point[2] = layer.z_mm;
            if index == 1 {
                point[1] = 8.0;
            }
        }
        if index == 2 {
            path.kind = DepositionKind::SparseFill;
            path.physical_z_mm = [0.1, 0.3];
        }
        plan.layers.push(layer);
    }
    // The retained intermediate fine row also needs a genuine earlier anchor;
    // the later coarse row is never its support authority.
    let mut fine_anchor = plan.layers[0].paths[0].clone();
    for point in &mut fine_anchor.points_mm {
        point[1] = 8.0;
    }
    plan.layers[0].paths.push(fine_anchor);
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    plan.sparse_bands.push(SparseBandDecision {
        first_layer: 1,
        last_layer: 2,
        owner: None,
        disposition: SparseBandDisposition::Combined,
    });
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    let report = verify_actual(&program, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
    // Independently calculated capsule areas: three fine rows, one double-height row.
    let fine_area = 0.35 * 0.1 + std::f64::consts::PI * 0.05f64.powi(2);
    let coarse_area = 0.25 * 0.2 + std::f64::consts::PI * 0.1f64.powi(2);
    assert!((report.deposited_volume_mm3 - 4.0 * (3.0 * fine_area + coarse_area)).abs() < 1e-7);
    let (bytes, bounds) = sink.finish().unwrap();
    let descriptor = packed_process_descriptor(&bytes, bounds, &program, &intent, &plan, &report);
    validate_replay_display(&bytes, &descriptor).unwrap();
    let sparse = bytes[REPLAY_HEADER_BYTES..]
        .chunks_exact(REPLAY_RECORD_BYTES)
        .find(|record| record[46] == 5)
        .unwrap();
    assert_eq!(f64::from_le_bytes(sparse[48..56].try_into().unwrap()), 0.45);
    assert_eq!(f64::from_le_bytes(sparse[56..64].try_into().unwrap()), 0.1);
    assert_eq!(f64::from_le_bytes(sparse[64..72].try_into().unwrap()), 0.3);
    for mutation in 0..4 {
        let mut invalid = plan.clone();
        let path = &mut invalid.layers[2].paths[0];
        match mutation {
            0 => path.kind = DepositionKind::Bridge,
            1 => path.width = WidthProfile::Constant { width_mm: 0.46 },
            2 => path.physical_z_mm[0] = 0.11,
            _ => invalid.sparse_bands.clear(),
        }
        assert!(verify(&program, &invalid, &intent, &cancel).is_err());
    }
    let mut changed_height = plan.clone();
    changed_height.layers[2].paths[0].physical_z_mm = [0.2, 0.3];
    changed_height.sparse_bands.clear();
    refresh_volumes(&intent, &mut changed_height);
    assert!(verify(&program, &changed_height, &intent, &cancel).is_err());
    let mut limited = intent.clone();
    limited.printer.max_volumetric_flow_mm3_s = 0.6;
    assert!(
        limited.validate().is_ok(),
        "fine recipe remains within flow envelope"
    );
    assert!(
        verify(&program, &plan, &limited, &cancel).is_err(),
        "coarse emitted feed exceeds flow envelope"
    );
    let mut blocked_landing = plan.clone();
    for point in &mut blocked_landing.layers[0].paths[0].points_mm {
        point[1] = 20.0;
    }
    refresh_volumes(&intent, &mut blocked_landing);
    let unsupported = crate::backend::emit(&blocked_landing, &intent, &cancel).unwrap();
    assert!(verify(&unsupported, &blocked_landing, &intent, &cancel).is_err());
    use std::fmt::Write;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in &bytes {
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    let positions: Vec<[f64; 3]> = bytes[REPLAY_HEADER_BYTES..]
        .chunks_exact(REPLAY_RECORD_BYTES)
        .map(|record| {
            std::array::from_fn(|axis| {
                f64::from_le_bytes(record[axis * 8..axis * 8 + 8].try_into().unwrap())
            })
        })
        .collect();
    let fixture = serde_json::json!({ "encoding":"hex", "descriptor":descriptor,
        "data":hex, "actual_positions":positions, "program":program });
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/protocol/replay-sparse.json"
    );
    if std::env::var_os("SPILING_UPDATE_REPLAY_FIXTURE").is_some() {
        std::fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    } else {
        let mut committed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut actual = fixture.clone();
        // Producer freshness is validated against captured provenance elsewhere;
        // an ABI golden owns motion bytes, dimensions, association and layout,
        // not an incidental producer literal or its semantic-input fingerprint.
        for value in [&mut committed, &mut actual] {
            let info = value["descriptor"]["info"].as_object_mut().unwrap();
            info.remove("input_hash");
            info.remove("verifier_version");
        }
        assert_eq!(
            committed, actual,
            "combined sparse replay motion/dimension/association ABI drift"
        );
    }
    let mut regional = plan.clone();
    let mut regional_intent = intent.clone();
    regional_intent.recipe.infill_fraction = 0.2;
    for layer in &mut regional.layers {
        layer.region_coverage.wall_area_mm2 = 0.0;
        layer.region_coverage.sparse_interior_area_mm2 = 16.0;
        layer.region_coverage.intentional_void_area_mm2 = 12.8;
        layer.region_coverage.process_regions = vec![ProcessRegionCoverage {
            area_mm2: 16.0,
            fraction: 0.2,
            contributions: vec![],
        }];
    }
    let owner = Some(DepositionOwner::Regional { region_index: 0 });
    regional.sparse_bands[0].owner = owner;
    regional.layers[2].paths[0].owner = owner;
    regional.layers[2].paths[0].kind = DepositionKind::Bridge;
    let check = |mut candidate: NormalizedPrintPlan| {
        refresh_volumes(&regional_intent, &mut candidate);
        let bytes = crate::backend::emit(&candidate, &regional_intent, &cancel).unwrap();
        verify(&bytes, &candidate, &regional_intent, &cancel)
    };
    assert!(
        check(regional.clone()).is_err(),
        "supported coarse row cannot borrow a Bridge label"
    );
    // A genuine gap uses the band's BASE, not the retained intermediate fine
    // row at y=8 or a fabricated terminal-previous sparse owner.
    regional.layers[0].paths[0].points_mm[1][0] = 3.0;
    let mut right = regional.layers[0].paths[0].clone();
    right.points_mm = vec![[5.0, 2.0, 0.1], [6.0, 2.0, 0.1]];
    regional.layers[0].paths.push(right);
    check(regional.clone()).unwrap();
    let mut ordinary = regional.clone();
    ordinary.layers[2].paths[0].kind = DepositionKind::SparseFill;
    assert!(
        check(ordinary).is_err(),
        "coarse ordinary row cannot conceal a genuine base gap"
    );
    regional.layers[0].paths.pop();
    assert!(
        check(regional).is_err(),
        "coarse Bridge requires both actual direct base anchors"
    );
}

#[test]
fn deferred_sparse_bonds_cover_every_fine_interval_but_never_support_earlier_walls() {
    use spiling_contracts::geometry::{DefinitionId, OccurrenceId};
    use spiling_contracts::process::{ProcessLayerId, ProcessOperationId, ProcessProvenance};
    let (intent, mut plan) = fixture();
    let occurrence_id = OccurrenceId::new(1).unwrap();
    let definition_id = DefinitionId::from_source_sha256(&[7; 32]);
    let provenance = ProcessProvenance {
        layer_id: ProcessLayerId::new(1).unwrap(),
        operation_id: ProcessOperationId::new(1).unwrap(),
    };
    let region = ProcessRegionCoverage {
        area_mm2: 4.0,
        fraction: 0.2,
        contributions: vec![ProcessRegionContribution {
            occurrence_id,
            definition_id: definition_id.clone(),
            provenance: Some(provenance),
        }],
    };
    let mut other = region.clone();
    other.fraction = 0.4;
    let interface = ProcessInterfaceCoverage {
        occurrence_id,
        definition_id,
        provenance,
        region_indices: [0, 1],
        axis: 0,
        coordinate_mm: 3.0,
        thickness_mm: 0.45,
        bonding_overlap_mm: 0.225,
        area_mm2: 1.5975,
        centerlines_mm: vec![[[3.0, 1.225], [3.0, 4.775]]],
        endpoint_bonds: vec![[
            ProcessEndpointBond::Wall {
                wall_segment_mm: [[1.0, 1.0], [5.0, 1.0]],
            },
            ProcessEndpointBond::Wall {
                wall_segment_mm: [[1.0, 5.0], [5.0, 5.0]],
            },
        ]],
    };
    let template = plan.layers[0].clone();
    plan.layers = (0..4)
        .map(|index| {
            let mut layer = template.clone();
            layer.index = index;
            layer.lower_z_mm = f64::from(index) * 0.1;
            layer.z_mm = f64::from(index + 1) * 0.1;
            layer.height_mm = 0.1;
            layer.section_z_mm = (layer.lower_z_mm + layer.z_mm) * 0.5;
            layer.region_coverage.process_regions = vec![region.clone(), other.clone()];
            layer.region_coverage.process_interfaces = vec![interface.clone()];
            layer.paths = vec![DepositionPath {
                kind: DepositionKind::SeparatorWall,
                owner: None,
                width: WidthProfile::Constant { width_mm: 0.45 },
                physical_z_mm: [layer.lower_z_mm, layer.z_mm],
                points_mm: vec![[3.0, 1.225, layer.z_mm], [3.0, 4.775, layer.z_mm]],
            }];
            layer
        })
        .collect();
    // Effective region identity is preserved while terminal table indices move.
    plan.layers[3].region_coverage.process_regions.swap(0, 1);
    plan.layers[3].region_coverage.process_interfaces[0].region_indices = [1, 0];
    plan.layers[3].paths.push(DepositionPath {
        kind: DepositionKind::SparseFill,
        owner: Some(DepositionOwner::Regional { region_index: 1 }),
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.1, 0.4],
        points_mm: vec![[1.0, 3.0, 0.4], [2.775, 3.0, 0.4]],
    });
    plan.sparse_bands = vec![SparseBandDecision {
        first_layer: 1,
        last_layer: 3,
        owner: Some(DepositionOwner::Regional { region_index: 1 }),
        disposition: SparseBandDisposition::Combined,
    }];
    let cancel = AtomicBool::new(false);
    let contact = |plan: &NormalizedPrintPlan| {
        interfaces::deferred_sparse_contact(
            plan,
            1,
            &plan.layers[1].region_coverage.process_interfaces[0],
            0,
            &mut 0,
            &cancel,
        )
    };
    assert!(
        contact(&plan).unwrap(),
        "strict .025mm nominal inner-core margin"
    );
    interfaces::verify_earlier_fine_paths(
        &plan,
        1,
        &intent.recipe,
        &mut 0,
        &mut crate::planner::support::AnchorWorkspace::default(),
        &cancel,
    )
    .unwrap();
    let mut future_only = plan.clone();
    future_only.layers[0].paths.clear();
    assert!(contact(&future_only).unwrap(), "eventual bond still exists");
    assert!(
        interfaces::verify_earlier_fine_paths(
            &future_only,
            1,
            &intent.recipe,
            &mut 0,
            &mut crate::planner::support::AnchorWorkspace::default(),
            &cancel
        )
        .is_err(),
        "future coarse deposition cannot support an earlier fine wall"
    );
    for mutation in 0..6 {
        let mut broken = plan.clone();
        match mutation {
            0 => broken.layers[3].paths[1].physical_z_mm[0] = 0.11,
            1 => broken.layers[2].region_coverage.process_regions[0].fraction = 0.3,
            2 => {
                broken.layers[3].region_coverage.process_regions[1].contributions[0].occurrence_id =
                    OccurrenceId::new(2).unwrap()
            }
            3 => broken.layers[3].paths[1].points_mm[1][0] = 2.5,
            4 => {
                // Preserve a contiguous .3mm band but make one fine interval
                // .15mm: its core is only tangent to the coarse core.
                broken.layers[1].z_mm = 0.15;
                broken.layers[1].height_mm = 0.05;
                broken.layers[1].section_z_mm = 0.125;
                broken.layers[1].paths[0].physical_z_mm = [0.1, 0.15];
                for point in &mut broken.layers[1].paths[0].points_mm {
                    point[2] = 0.15;
                }
                broken.layers[2].lower_z_mm = 0.15;
                broken.layers[2].z_mm = 0.3;
                broken.layers[2].height_mm = 0.15;
                broken.layers[2].section_z_mm = 0.225;
                broken.layers[2].paths[0].physical_z_mm = [0.15, 0.3];
                for point in &mut broken.layers[2].paths[0].points_mm {
                    point[2] = 0.3;
                }
            }
            _ => broken.layers[3].paths[1].width = WidthProfile::Constant { width_mm: 0.3 },
        }
        assert!(
            !contact(&broken).unwrap_or(false),
            "broken full-height/core/source bond {mutation}"
        );
    }
    // Insert a positive nominal end interval without changing the combined
    // physical extent or any XY/source witness. XYZ6 has no guaranteed vertical
    // interior for this tiny interval, so nominal containment alone cannot bond.
    let mut tiny_end = plan.clone();
    let mut tiny = tiny_end.layers[1].clone();
    tiny.z_mm = tiny.lower_z_mm.next_up();
    tiny.height_mm = tiny.z_mm - tiny.lower_z_mm;
    tiny.section_z_mm = tiny.lower_z_mm;
    tiny.paths[0].physical_z_mm = [tiny.lower_z_mm, tiny.z_mm];
    for point in &mut tiny.paths[0].points_mm {
        point[2] = tiny.z_mm;
    }
    tiny_end.layers[1].lower_z_mm = tiny.z_mm;
    tiny_end.layers[1].paths[0].physical_z_mm[0] = tiny.z_mm;
    tiny_end.layers.insert(1, tiny);
    for (index, layer) in tiny_end.layers.iter_mut().enumerate() {
        layer.index = index as u32;
    }
    tiny_end.sparse_bands[0].last_layer = 4;
    let interval = [tiny_end.layers[1].lower_z_mm, tiny_end.layers[1].z_mm];
    assert!(
        interval[0] < interval[1],
        "nominal interval is strictly positive"
    );
    let inner = program_coordinates::project_xyz6_bead_inner_height_mm(interval).unwrap();
    assert!(inner[0] >= inner[1], "no guaranteed XYZ6 vertical interior");
    assert!(
        !contact(&tiny_end).unwrap(),
        "tiny end interval cannot borrow outer-hull overlap"
    );
}

#[test]
fn emitted_top_skin_touching_fractional_separator_boundary_is_not_longitudinal_overlap() {
    use spiling_contracts::geometry::{DefinitionId, OccurrenceId};
    use spiling_contracts::process::{ProcessLayerId, ProcessOperationId, ProcessProvenance};
    let (mut intent, mut plan) = fixture();
    intent.recipe.layer_height_mm = 0.1;
    intent.recipe.bead_width_mm = 0.45;
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    intent.recipe.top_skin_axis = RectilinearAxisPolicy::Y;
    let start_y = 1.224_999_950_008_16;
    let end_y = 4.775;
    let occurrence_id = OccurrenceId::new(1).unwrap();
    let definition_id = DefinitionId::from_source_sha256(&[9; 32]);
    let provenance = ProcessProvenance {
        layer_id: ProcessLayerId::new(1).unwrap(),
        operation_id: ProcessOperationId::new(1).unwrap(),
    };
    let interface = ProcessInterfaceCoverage {
        occurrence_id,
        definition_id: definition_id.clone(),
        provenance,
        region_indices: [0, 1],
        axis: 0,
        coordinate_mm: 3.0,
        thickness_mm: 0.45,
        bonding_overlap_mm: 0.225,
        area_mm2: 0.45 * (end_y - start_y),
        centerlines_mm: vec![[[3.0, start_y], [3.0, end_y]]],
        endpoint_bonds: vec![[
            ProcessEndpointBond::Wall {
                wall_segment_mm: [[1.0, 1.0], [5.0, 1.0]],
            },
            ProcessEndpointBond::Wall {
                wall_segment_mm: [[1.0, 5.0], [5.0, 5.0]],
            },
        ]],
    };
    for layer in &mut plan.layers {
        layer.lower_z_mm = f64::from(layer.index) * 0.1;
        layer.z_mm = f64::from(layer.index + 1) * 0.1;
        layer.height_mm = 0.1;
        layer.section_z_mm = (layer.lower_z_mm + layer.z_mm) * 0.5;
        layer.paths = vec![DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [layer.lower_z_mm, layer.z_mm],
            points_mm: vec![
                [1.0, 1.0, layer.z_mm],
                [5.0, 1.0, layer.z_mm],
                [5.0, 5.0, layer.z_mm],
                [1.0, 5.0, layer.z_mm],
                [1.0, 1.0, layer.z_mm],
            ],
        }];
    }
    // The lower separator anchor is actual earlier deposition, not a future
    // fill or a synthetic declared band. Keep its explicit closed traversal.
    plan.layers[0].paths.push(DepositionPath {
        kind: DepositionKind::Perimeter,
        owner: Some(DepositionOwner::Wall),
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.1],
        points_mm: vec![[3.0, 1.0, 0.1], [3.0, 5.0, 0.1], [3.0, 1.0, 0.1]],
    });
    plan.layers[0].paths.push(DepositionPath {
        kind: DepositionKind::SolidFill,
        owner: None,
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.1],
        points_mm: vec![[1.0, 3.0, 0.1], [5.0, 3.0, 0.1]],
    });
    let fine = &mut plan.layers[1];
    fine.paths.extend([
        DepositionPath {
            kind: DepositionKind::TopSkin,
            owner: Some(DepositionOwner::MandatorySkin {
                skin: ProcessSkinKind::TopSkin,
            }),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.1, 0.2],
            points_mm: vec![[3.0, 1.0, 0.2], [3.0, start_y, 0.2]],
        },
        DepositionPath {
            kind: DepositionKind::SeparatorWall,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.1, 0.2],
            points_mm: vec![[3.0, start_y, 0.2], [3.0, end_y, 0.2]],
        },
        DepositionPath {
            kind: DepositionKind::SparseFill,
            owner: Some(DepositionOwner::Regional { region_index: 0 }),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.1, 0.2],
            points_mm: vec![[1.225, 3.0, 0.2], [2.775, 3.0, 0.2]],
        },
        DepositionPath {
            kind: DepositionKind::SparseFill,
            owner: Some(DepositionOwner::Regional { region_index: 1 }),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.1, 0.2],
            points_mm: vec![[3.225, 3.0, 0.2], [4.775, 3.0, 0.2]],
        },
    ]);
    fine.region_coverage.process_regions = [0.2, 0.4]
        .map(|fraction| ProcessRegionCoverage {
            area_mm2: 4.0,
            fraction,
            contributions: vec![ProcessRegionContribution {
                occurrence_id,
                definition_id: definition_id.clone(),
                provenance: None,
            }],
        })
        .to_vec();
    fine.region_coverage.separator_area_mm2 = interface.area_mm2;
    fine.region_coverage.top_skin_area_mm2 = 0.5;
    fine.region_coverage.sparse_interior_area_mm2 = 8.0;
    fine.region_coverage.intentional_void_area_mm2 = 5.6;
    fine.region_coverage.wall_area_mm2 = 16.0 - 8.0 - 0.5 - interface.area_mm2;
    fine.region_coverage.process_interfaces = vec![interface];
    plan.process_resolution = vec![ProcessRegionResolution {
        occurrence_id,
        definition_id,
        provenance,
        status: ProcessRegionStatus::Applied,
    }];
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert!(verify(&program, &plan, &intent, &cancel).unwrap().verified);
    let actual_touch = project_xyz6_coordinate_mm(start_y).unwrap();
    assert!(
        actual_touch > start_y,
        "raw/XYZ6 mixing would invent a positive overlap"
    );
    // Both mutations still match newly emitted bytes. Admission must therefore
    // distinguish projected tangency from actual longitudinal ownership.
    for intrusion in [0.000_001, 0.001] {
        let mut overlapping = plan.clone();
        let path = overlapping.layers[1]
            .paths
            .iter_mut()
            .find(|path| path.kind == DepositionKind::TopSkin)
            .unwrap();
        let endpoint = path
            .points_mm
            .iter_mut()
            .max_by(|a, b| a[1].total_cmp(&b[1]))
            .unwrap();
        endpoint[1] = start_y + intrusion;
        refresh_volumes(&intent, &mut overlapping);
        let changed_program = crate::backend::emit(&overlapping, &intent, &cancel).unwrap();
        let error = verify(&changed_program, &overlapping, &intent, &cancel).unwrap_err();
        assert_eq!(error.code, ManufacturingErrorCode::VerificationFailed);
        assert!(
            error
                .message
                .contains("mandatory skin/Bridge duplicates longitudinal separator ownership")
        );
    }
}

#[test]
fn affected_fine_regional_rows_require_actual_support_or_genuine_bounded_bridge() {
    let (mut intent, mut plan) = fixture();
    intent.recipe.infill_fraction = 0.2;
    let following = &mut plan.layers[1];
    following.paths[0].kind = DepositionKind::Bridge;
    following.paths[0].owner = Some(DepositionOwner::Regional { region_index: 0 });
    following.paths[0].points_mm = vec![[2.0, 2.0, 2.0], [2.0, 5.0, 2.0]];
    following.region_coverage.wall_area_mm2 = 0.0;
    following.region_coverage.sparse_interior_area_mm2 = 16.0;
    following.region_coverage.intentional_void_area_mm2 = 12.8;
    following.region_coverage.process_regions = vec![ProcessRegionCoverage {
        area_mm2: 16.0,
        fraction: 0.2,
        contributions: vec![],
    }];
    let cancel = AtomicBool::new(false);
    let check = |intent: &ManufacturingIntent, mut plan: NormalizedPrintPlan| {
        refresh_volumes(intent, &mut plan);
        // Establish actual-byte/path correspondence first. Exercise the
        // affected-landing consumer directly without inventing a Combined band.
        let program = crate::backend::emit(&plan, intent, &cancel).unwrap();
        verify(&program, &plan, intent, &cancel).unwrap();
        interfaces::verify_earlier_fine_paths(
            &plan,
            1,
            &intent.recipe,
            &mut 0,
            &mut crate::planner::support::AnchorWorkspace::default(),
            &cancel,
        )
    };
    check(&intent, plan.clone()).unwrap();
    let mut mislabeled = plan.clone();
    mislabeled.layers[1].paths[0].kind = DepositionKind::SparseFill;
    assert!(
        check(&intent, mislabeled).is_err(),
        "ordinary row cannot hide a genuine lower gap"
    );
    let mut fully_supported = plan.clone();
    fully_supported.layers[1].paths[0].points_mm = vec![[2.0, 2.0, 2.0], [2.0, 2.25, 2.0]];
    assert!(
        check(&intent, fully_supported).is_err(),
        "Bridge must have a genuine bounded span"
    );
    let mut no_anchors = plan.clone();
    for path in &mut no_anchors.layers[0].paths {
        for point in &mut path.points_mm {
            point[0] += 40.0;
        }
    }
    assert!(
        check(&intent, no_anchors).is_err(),
        "actual immediate-prior anchors are mandatory"
    );
    let mut too_short = intent.clone();
    too_short.recipe.process.max_bridge_span_mm = 0.25;
    assert!(
        check(&too_short, plan).is_err(),
        "actual span must meet explicit bridge limit"
    );
}

fn skin_fit_box(x: [f64; 2], y: [f64; 2]) -> DiaphragmPolygonMm {
    DiaphragmPolygonMm {
        outer: vec![
            [x[0], y[0]],
            [x[1], y[0]],
            [x[1], y[1]],
            [x[0], y[1]],
            [x[0], y[0]],
        ],
        holes: Vec::new(),
    }
}

fn minimum_skin_fixture() -> (ManufacturingIntent, NormalizedPrintPlan) {
    use spiling_contracts::geometry::{DefinitionId, OccurrenceId};
    let (mut intent, mut plan) = fixture();
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    intent.recipe.top_skin_axis = RectilinearAxisPolicy::X;
    plan.layers.truncate(1);
    let layer = &mut plan.layers[0];
    let region = ProcessRegionCoverage {
        area_mm2: 15.98,
        fraction: 1.0,
        contributions: vec![ProcessRegionContribution {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: DefinitionId::from_source_sha256(&[13; 32]),
            provenance: None,
        }],
    };
    let fit = SkinCommandFit {
        required_mm: [[2.0, 2.0, 1.0], [2.01, 2.0, 1.0]],
        fitted_mm: [[2.0, 2.0, 1.0], [2.020003, 2.0, 1.0]],
    };
    layer.paths = vec![DepositionPath {
        kind: DepositionKind::BottomSkin,
        owner: Some(DepositionOwner::MandatorySkin {
            skin: ProcessSkinKind::BottomSkin,
        }),
        width: WidthProfile::Constant { width_mm: 1.0 },
        physical_z_mm: [0.0, 1.0],
        points_mm: fit.fitted_mm.to_vec(),
    }];
    layer.skin_command_fits = vec![SkinCommandFitDomain {
        region_index: 0,
        region: region.clone(),
        skin: ProcessSkinKind::BottomSkin,
        regional: vec![skin_fit_box([2.01, 5.0], [0.0, 4.0])],
        mandatory: vec![skin_fit_box([2.0, 2.01], [1.0, 3.0])],
        source_owner: vec![skin_fit_box([1.0, 5.0], [0.0, 4.0])],
        fits: vec![fit],
    }];
    layer.region_coverage.wall_area_mm2 = 0.0;
    layer.region_coverage.bottom_skin_area_mm2 = 0.02;
    layer.region_coverage.solid_interior_area_mm2 = 15.98;
    layer.region_coverage.process_regions = vec![region];
    refresh_volumes(&intent, &mut plan);
    (intent, plan)
}

fn replay_skin_fit(
    intent: &ManufacturingIntent,
    mut plan: NormalizedPrintPlan,
) -> Result<VerificationReport, ManufacturingError> {
    let cancel = AtomicBool::new(false);
    refresh_volumes(intent, &mut plan);
    let program = crate::backend::emit(&plan, intent, &cancel)?;
    verify(&program, &plan, intent, &cancel)
}

#[test]
fn minimum_skin_command_preserves_required_material_and_replays_actual_extra_volume() {
    let (intent, mut plan) = minimum_skin_fixture();
    // Independent circular-bead numeric golden: E = commanded length / 4.
    let program =
        "G21\nG90\nM82\nG92 E0\nM106 S0\nG0 X2 Y2 Z1 F1200\nG1 X2.020003 Y2 Z1 E0.00500075 F600\n";
    let report = verify(program, &plan, &intent, &AtomicBool::new(false)).unwrap();
    assert!((report.deposited_volume_mm3 - 0.020003 * std::f64::consts::PI / 4.0).abs() < 1e-12);
    assert!(report.deposited_volume_mm3 > 0.01 * std::f64::consts::PI / 4.0);
    assert_eq!(plan.layers[0].region_coverage.bottom_skin_area_mm2, 0.02);
    plan.layers[0].paths[0].points_mm.reverse();
    replay_skin_fit(&intent, plan).unwrap();
}

#[test]
fn minimum_skin_replay_rejects_lost_requirement_holes_winner_escape_and_missing_construction() {
    let (intent, plan) = minimum_skin_fixture();
    for mutation in 0..8 {
        let mut bad = plan.clone();
        let layer = &mut bad.layers[0];
        match mutation {
            0 => layer.skin_command_fits[0].fits[0].required_mm[0][0] = 2.001,
            1 => layer.skin_command_fits[0].source_owner[0]
                .holes
                .push(skin_fit_box([2.015, 2.016], [1.5, 2.5]).outer),
            2 => layer.skin_command_fits[0].regional = vec![skin_fit_box([2.015, 5.0], [0.0, 4.0])],
            3 => {
                layer.paths[0].points_mm[0][0] += 0.001;
                layer.paths[0].points_mm[1][0] += 0.001;
            }
            4 => layer.skin_command_fits[0].region_index = 1,
            5 => layer.paths[0].owner = Some(DepositionOwner::Regional { region_index: 0 }),
            6 => {
                layer.skin_command_fits[0].fits[0].fitted_mm[1][0] = 2.03;
                layer.paths[0].points_mm[1][0] = 2.03;
            }
            7 => {
                layer.region_coverage.process_regions[0].area_mm2 = 7.99;
                layer.skin_command_fits[0].region =
                    layer.region_coverage.process_regions[0].clone();
                let mut other = layer.region_coverage.process_regions[0].clone();
                other.contributions[0].occurrence_id =
                    spiling_contracts::geometry::OccurrenceId::new(2).unwrap();
                layer.region_coverage.process_regions.push(other);
                layer.skin_command_fits[0].region_index = 1;
            }
            _ => unreachable!(),
        }
        assert!(
            replay_skin_fit(&intent, bad).is_err(),
            "minimum-command construction must not authorize lost skin, void/winner escape or a missing actual stroke: {mutation}"
        );
    }
}

#[test]
fn minimum_skin_replay_rejects_duplicate_witnesses_and_positive_collinear_extrusion() {
    let (intent, plan) = minimum_skin_fixture();
    let mut repeated = plan.clone();
    let fit = repeated.layers[0].skin_command_fits[0].fits[0].clone();
    repeated.layers[0].skin_command_fits[0].fits.push(fit);
    assert!(replay_skin_fit(&intent, repeated).is_err());
    let mut duplicate = plan.clone();
    let mut other = duplicate.layers[0].paths[0].clone();
    other.kind = DepositionKind::SolidFill;
    other.owner = Some(DepositionOwner::Regional { region_index: 0 });
    other.points_mm = vec![[2.015, 2.0, 1.0], [2.035003, 2.0, 1.0]];
    duplicate.layers[0].paths.push(other);
    assert!(replay_skin_fit(&intent, duplicate).is_err());
}

#[test]
fn fitted_skin_bridge_still_requires_genuine_direct_lower_anchors_and_bounded_span() {
    let (intent, mut plan) = minimum_skin_fixture();
    let mut upper = plan.layers[0].clone();
    upper.index = 1;
    upper.lower_z_mm = 1.0;
    upper.z_mm = 2.0;
    upper.section_z_mm = 1.5;
    let domain = &mut upper.skin_command_fits[0];
    domain.mandatory = vec![skin_fit_box([4.0, 4.01], [1.0, 3.0])];
    domain.regional = vec![
        skin_fit_box([1.0, 4.0], [0.0, 4.0]),
        skin_fit_box([4.01, 9.0], [0.0, 4.0]),
    ];
    domain.source_owner = vec![skin_fit_box([1.0, 9.0], [0.0, 4.0])];
    domain.fits[0] = SkinCommandFit {
        required_mm: [[4.0, 2.0, 2.0], [4.01, 2.0, 2.0]],
        fitted_mm: [[4.0, 2.0, 2.0], [4.020003, 2.0, 2.0]],
    };
    upper.paths[0].kind = DepositionKind::Bridge;
    upper.paths[0].physical_z_mm = [1.0, 2.0];
    upper.paths[0].points_mm = vec![[2.0, 2.0, 2.0], [6.0, 2.0, 2.0]];
    let lower = &mut plan.layers[0];
    lower.skin_command_fits.clear();
    lower.paths[0].kind = DepositionKind::SolidFill;
    lower.paths[0].owner = Some(DepositionOwner::Regional { region_index: 0 });
    lower.paths[0].points_mm = vec![[2.0, 2.0, 1.0], [2.1, 2.0, 1.0]];
    let mut right = lower.paths[0].clone();
    right.points_mm = vec![[5.9, 2.0, 1.0], [6.0, 2.0, 1.0]];
    lower.paths.push(right);
    plan.layers.push(upper);
    replay_skin_fit(&intent, plan.clone()).unwrap();
    let mut supported = plan.clone();
    supported.layers[0].paths[0].points_mm[1][0] = 6.0;
    supported.layers[0].paths.pop();
    assert!(
        replay_skin_fit(&intent, supported).is_err(),
        "a fit does not excuse a false Bridge label"
    );
    let mut floating = plan.clone();
    for path in &mut floating.layers[0].paths {
        for point in &mut path.points_mm {
            point[1] = 40.0;
        }
    }
    assert!(
        replay_skin_fit(&intent, floating).is_err(),
        "a fit is never lower-anchor authority"
    );
    let mut short_span = intent.clone();
    short_span.recipe.process.max_bridge_span_mm = 1.0;
    assert!(
        replay_skin_fit(&short_span, plan).is_err(),
        "a fit never relaxes maximum bridge span"
    );
}

#[test]
fn ordinary_fitted_skin_uses_actual_lower_strokes_not_source_material_as_support() {
    let (intent, mut plan) = minimum_skin_fixture();
    let mut upper = plan.layers[0].clone();
    upper.index = 1;
    upper.lower_z_mm = 1.0;
    upper.z_mm = 2.0;
    upper.section_z_mm = 1.5;
    upper.paths[0].physical_z_mm = [1.0, 2.0];
    for point in &mut upper.paths[0].points_mm {
        point[2] = 2.0;
    }
    let fit = &mut upper.skin_command_fits[0].fits[0];
    for point in fit.required_mm.iter_mut().chain(&mut fit.fitted_mm) {
        point[2] = 2.0;
    }
    plan.layers[0].skin_command_fits.clear();
    plan.layers[0].paths[0].kind = DepositionKind::SolidFill;
    plan.layers[0].paths[0].owner = Some(DepositionOwner::Regional { region_index: 0 });
    plan.layers.push(upper);
    replay_skin_fit(&intent, plan.clone()).unwrap();
    for point in &mut plan.layers[0].paths[0].points_mm {
        point[1] = 40.0;
    }
    assert!(
        replay_skin_fit(&intent, plan).is_err(),
        "captured source material cannot replace an earlier deposited anchor"
    );
}

#[test]
fn projected_skin_just_below_command_minimum_still_receives_complete_fitted_material() {
    let (intent, mut plan) = minimum_skin_fixture();
    let layer = &mut plan.layers[0];
    let domain = &mut layer.skin_command_fits[0];
    domain.fits[0] = SkinCommandFit {
        required_mm: [[4.0, 2.0, 1.0], [4.02, 2.0, 1.0]],
        fitted_mm: [[4.0, 2.0, 1.0], [4.020003, 2.0, 1.0]],
    };
    domain.mandatory = vec![skin_fit_box([4.0, 4.02], [1.0, 3.0])];
    domain.regional = vec![skin_fit_box([4.02, 9.0], [0.0, 4.0])];
    domain.source_owner = vec![skin_fit_box([1.0, 9.0], [0.0, 4.0])];
    layer.paths[0].points_mm = domain.fits[0].fitted_mm.to_vec();
    layer.region_coverage.bottom_skin_area_mm2 = 0.04;
    layer.region_coverage.solid_interior_area_mm2 = 15.96;
    layer.region_coverage.process_regions[0].area_mm2 = 15.96;
    domain.region = layer.region_coverage.process_regions[0].clone();
    let required = domain.fits[0].required_mm;
    let a = project_xyz6_point_mm(required[0]).unwrap();
    let b = project_xyz6_point_mm(required[1]).unwrap();
    assert!((b[0] - a[0]).abs() < program_coordinates::MIN_DEPOSITION_COMMAND_MM);
    let mut unfitted = plan.clone();
    unfitted.layers[0].skin_command_fits.clear();
    unfitted.layers[0].paths[0].points_mm = required.to_vec();
    assert!(
        replay_skin_fit(&intent, unfitted).is_err(),
        "the original projected stroke must still fail the unchanged command minimum"
    );
    replay_skin_fit(&intent, plan).unwrap();
}

#[test]
fn minimum_skin_replay_rejects_off_lattice_rows_inside_unchanged_captured_masks() {
    let (intent, mut plan) = minimum_skin_fixture();
    let layer = &mut plan.layers[0];
    let fit = &mut layer.skin_command_fits[0].fits[0];
    // The rectangle's centered three-row lattice includes Y=2, not Y=2.001.
    // Required material, added corridor and source masks all still contain the
    // shifted interval; only the independent authored-phase proof can veto it.
    for point in fit.required_mm.iter_mut().chain(&mut fit.fitted_mm) {
        point[1] += 0.001;
    }
    for point in &mut layer.paths[0].points_mm {
        point[1] += 0.001;
    }
    assert!(
        replay_skin_fit(&intent, plan).is_err(),
        "jointly shifting the requirement, fit and actual construction must not invent a new hatch phase"
    );
}

#[test]
fn minimum_skin_projects_the_authored_slanted_interval_but_rechecks_actual_source_ownership() {
    let (intent, mut plan) = minimum_skin_fixture();
    let row = 2.0000004;
    let y = [row - 0.2, row + 0.2];
    let layer = &mut plan.layers[0];
    let domain = &mut layer.skin_command_fits[0];
    domain.mandatory = vec![DiaphragmPolygonMm {
        outer: vec![
            [1.4, y[0]],
            [1.41, y[0]],
            [2.61, y[1]],
            [2.6, y[1]],
            [1.4, y[0]],
        ],
        holes: Vec::new(),
    }];
    domain.regional = vec![DiaphragmPolygonMm {
        outer: vec![
            [1.41, y[0]],
            [5.0, y[0]],
            [5.0, y[1]],
            [2.61, y[1]],
            [1.41, y[0]],
        ],
        holes: Vec::new(),
    }];
    let fit = &mut domain.fits[0];
    for point in fit.required_mm.iter_mut().chain(&mut fit.fitted_mm) {
        point[1] = row;
    }
    for point in &mut layer.paths[0].points_mm {
        point[1] = row;
    }
    layer.region_coverage.bottom_skin_area_mm2 = 0.004;
    layer.region_coverage.solid_interior_area_mm2 = 15.996;
    layer.region_coverage.process_regions[0].area_mm2 = 15.996;
    domain.region = layer.region_coverage.process_regions[0].clone();
    // Q(Y) is below the authored row: re-sectioning the slanted mandatory
    // mask there loses a micron from Q(required), despite correct projection.
    replay_skin_fit(&intent, plan.clone()).unwrap();
    let source = &mut plan.layers[0].skin_command_fits[0].source_owner;
    *source = vec![DiaphragmPolygonMm {
        outer: vec![
            [1.0, y[0]],
            [1.420003, y[0]],
            [2.620003, y[1]],
            [1.0, y[1]],
            [1.0, y[0]],
        ],
        holes: Vec::new(),
    }];
    assert!(
        replay_skin_fit(&intent, plan).is_err(),
        "authored classification projection cannot authorize escape from the independently evaluated actual source row"
    );
}

fn mixed_skin_fixture() -> (ManufacturingIntent, NormalizedPrintPlan) {
    let (mut intent, mut plan) = fixture();
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    let row = 2.0000004;
    let y = [row - 0.2, row + 0.2];
    let boundary = 3.4 + (row - y[0]) * (4.6 - 3.4) / (y[1] - y[0]);
    plan.layers[0].paths = [[2.0, 2.1], [5.9, 6.0]]
        .into_iter()
        .map(|x| DepositionPath {
            kind: DepositionKind::SolidFill,
            owner: None,
            width: WidthProfile::Constant { width_mm: 1.0 },
            physical_z_mm: [0.0, 1.0],
            points_mm: vec![[x[0], row, 1.0], [x[1], row, 1.0]],
        })
        .collect();
    let layer = &mut plan.layers[1];
    layer.paths = vec![DepositionPath {
        kind: DepositionKind::Bridge,
        owner: Some(DepositionOwner::RegionalSkinBridge {
            region_index: 0,
            skin: ProcessSkinKind::BottomSkin,
            skin_first: false,
        }),
        width: WidthProfile::Constant { width_mm: 1.0 },
        physical_z_mm: [1.0, 2.0],
        points_mm: vec![[2.0, row, 2.0], [boundary, row, 2.0], [5.4, row, 2.0]],
    }];
    layer.region_coverage.wall_area_mm2 = 14.0;
    layer.region_coverage.bottom_skin_area_mm2 = 1.2;
    layer.region_coverage.solid_interior_area_mm2 = 0.8;
    layer.region_coverage.process_regions = vec![ProcessRegionCoverage {
        area_mm2: 0.8,
        fraction: 1.0,
        contributions: Vec::new(),
    }];
    layer.region_coverage.skin_bridge_domains = vec![SkinBridgeDomain {
        region_index: 0,
        skin: ProcessSkinKind::BottomSkin,
        regional: vec![DiaphragmPolygonMm {
            outer: vec![
                [2.0, y[0]],
                [3.4, y[0]],
                [4.6, y[1]],
                [2.0, y[1]],
                [2.0, y[0]],
            ],
            holes: Vec::new(),
        }],
        mandatory: vec![DiaphragmPolygonMm {
            outer: vec![
                [3.4, y[0]],
                [7.0, y[0]],
                [7.0, y[1]],
                [4.6, y[1]],
                [3.4, y[0]],
            ],
            holes: Vec::new(),
        }],
        source_owner: vec![skin_fit_box([1.0, 8.0], [0.0, 4.0])],
    }];
    refresh_volumes(&intent, &mut plan);
    (intent, plan)
}

#[test]
fn mixed_skin_projects_original_classified_intervals_without_rehatching_the_rounded_row() {
    let (intent, plan) = mixed_skin_fixture();
    // Q(Y) is below the authored row. Re-sectioning these original slanted
    // masks at Q(Y) moves their shared boundary to Q(X)=3.999999, although
    // the complete captured regional interval correctly projects to [2, 4].
    replay_skin_fit(&intent, plan.clone()).unwrap();
    let mut reversed = plan.clone();
    reversed.layers[1].paths[0].reverse();
    replay_skin_fit(&intent, reversed).unwrap();
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let alternate = program.replacen("Y2.000000 ", "Y2.000001 ", 1);
    assert_ne!(alternate, program);
    assert!(
        verify(&alternate, &plan, &intent, &cancel).is_err(),
        "captured-row projection never authorizes an alternate actual XYZ6 neighbor"
    );
    let mut escaped = plan;
    let row = escaped.layers[1].paths[0].points_mm[0][1];
    let y = [row - 0.2, row + 0.2];
    escaped.layers[1].region_coverage.skin_bridge_domains[0].source_owner =
        vec![DiaphragmPolygonMm {
            outer: vec![
                [1.0, y[0]],
                [4.8, y[0]],
                [6.0, y[1]],
                [1.0, y[1]],
                [1.0, y[0]],
            ],
            holes: Vec::new(),
        }];
    assert!(
        replay_skin_fit(&intent, escaped).is_err(),
        "physical source confinement must still use the actual emitted row, not its authored preimage"
    );
}

#[test]
fn mixed_skin_replay_keeps_complete_regional_rows_mandatory_masks_and_direct_lower_anchors() {
    let (intent, plan) = mixed_skin_fixture();
    for mutation in 0..7 {
        let mut bad = plan.clone();
        match mutation {
            0 => bad.layers[1].paths[0].points_mm[0][0] = 2.01,
            1 => bad.layers[1].region_coverage.skin_bridge_domains[0].mandatory[0]
                .holes
                .push(skin_fit_box([4.5, 4.6], [1.9, 2.1]).outer),
            2 => bad.layers[1].region_coverage.skin_bridge_domains[0].source_owner[0]
                .holes
                .push(skin_fit_box([3.0, 3.1], [1.9, 2.1]).outer),
            3 => {
                for path in &mut bad.layers[0].paths {
                    for point in &mut path.points_mm {
                        point[1] = 40.0;
                    }
                }
            }
            4 => bad.layers[1].paths[0].points_mm[2][0] += 0.05,
            5 => bad.layers[1].paths[0].points_mm[1][1] += 0.0000001,
            6 => {
                for point in &mut bad.layers[1].paths[0].points_mm {
                    point[2] += 0.0000001;
                }
            }
            _ => unreachable!(),
        }
        assert!(
            replay_skin_fit(&intent, bad).is_err(),
            "authored-row projection must not excuse missing material, holes, changed raw row/height or false anchors: {mutation}"
        );
    }
    let mut wrong_axis = intent.clone();
    wrong_axis.recipe.rectilinear_axis = RectilinearAxisPolicy::Y;
    assert!(
        replay_skin_fit(&wrong_axis, plan).is_err(),
        "a mixed regional row must retain its declared authored hatch axis"
    );
}
