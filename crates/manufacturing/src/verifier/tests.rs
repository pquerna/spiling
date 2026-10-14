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
        section_area_mm2: 16.0,
        source_section_area_mm2: 16.0,
        surface_added_area_mm2: 0.0,
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
        requested_infill_fraction: 1.0,
    }
}
fn refresh_volumes(intent: &ManufacturingIntent, plan: &mut NormalizedPrintPlan) {
    let r = &intent.recipe;
    for layer in &mut plan.layers {
        let height = layer.height_mm;
        let area = ((r.bead_width_mm - height) * height
            + std::f64::consts::PI * (height / 2.0).powi(2))
            * r.flow_multiplier
            * if layer.index == 0 {
                r.process.first_layer_flow_factor
            } else {
                1.0
            };
        let mut object = 0.0;
        let mut support = 0.0;
        for path in &layer.paths {
            let volume: f64 = path
                .points_mm
                .windows(2)
                .map(|e| {
                    distance(e[0], e[1])
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
            max_feed_mm_s: 100.0,
            max_volumetric_flow_mm3_s: 100.0,
        },
        recipe: PlanarPrintRecipe {
            id: "recipe".into(),
            revision: "1".into(),
            material_id: "nominal-only".into(),
            material_revision: "1".into(),
            filament_diameter_mm: 2.0,
            layer_height_mm: 1.0,
            bead_width_mm: 1.0,
            perimeter_count: 1,
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
        sampling_tolerance_mm: 0.005,
        offset_tolerance_mm: 0.001,
        surface_tolerance_mm: 0.001,
        layer_quantization_mm: 0.000_001,
        layers: vec![
            PrintLayer {
                index: 0,
                z_mm: 1.0,
                height_mm: 1.0,
                section_z_mm: 0.5,
                region_coverage: coverage(),
                paths: vec![
                    DepositionPath {
                        kind: DepositionKind::Perimeter,
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
                        points_mm: vec![[2.0, 2.0, 1.0], [6.0, 2.0, 1.0]],
                    },
                ],
            },
            PrintLayer {
                index: 1,
                z_mm: 2.0,
                height_mm: 1.0,
                section_z_mm: 1.5,
                region_coverage: coverage(),
                paths: vec![DepositionPath {
                    kind: DepositionKind::SolidFill,
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
                index: empty_index as u32,
                z_mm: (empty_index + 1) as f64,
                height_mm: 1.0,
                section_z_mm: empty_index as f64 + 0.5,
                region_coverage: coverage(),
                paths: vec![],
            },
        );
        for (index, layer) in omitted.layers.iter_mut().enumerate() {
            layer.index = index as u32;
            layer.z_mm = (index + 1) as f64;
            layer.section_z_mm = index as f64 + 0.5;
            for path in &mut layer.paths {
                for point in &mut path.points_mm {
                    point[2] = layer.z_mm;
                }
            }
        }
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
        layer.height_mm = 0.2;
        layer.section_z_mm = (f64::from(layer.index) + 0.5) * 0.2;
        for path in &mut layer.paths {
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
        kind: DepositionKind::SolidFill,
        points_mm: vec![[1.0, 1.0, 1.0], [4.0, 5.0, 1.0]],
    }];
    refresh_volumes(&intent, &mut plan);
    let program = "G21\nG90\nM82\nG92 E0\nM106 S0\nG0 X1 Y1 Z1 F1200\nG1 X4 Y5 Z1 E1.25 F600\n";
    let report = verify(program, &plan, &intent, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.filament_length_mm, 1.25);
    assert_eq!(report.deposition_segments, 1);
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
    plan.layers[0].paths = vec![DepositionPath {
        kind: DepositionKind::SolidFill,
        points_mm: vec![[2.0, 2.0, 1.0], [2.000_000_1, 2.0, 1.0]],
    }];
    let program =
        "G21\nG90\nM82\nG92 E0\nM106 S0\nG0 X2 Y2 Z1 F1200\nG1 X2 Y2 Z1 E0.00000003 F600\n";
    assert_eq!(
        verify(program, &plan, &intent, &AtomicBool::new(false))
            .unwrap_err()
            .code,
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
    assert_eq!(bytes.len(), 96 + 48 * 200_000);
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
    let (mut intent, plan) = fixture();
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
    assert_eq!(bytes.len(), 96 + 48 * 13);
    assert_eq!(bounds.max, [20.0, 10.0, 10.0]);
    assert_eq!(&bytes[28..32], &4u32.to_le_bytes());
    assert_eq!(bytes[96 + 46], 6);
    assert_eq!(bytes[96 + 48 + 46], 7);
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
    for offset in [96 + 44, 96 + 46, 96 + 48 + 46, 96 + 11 * 48 + 16] {
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
            points_mm: vec![[9.0, 1.0, 1.0], [11.0, 1.0, 1.0]],
        },
        DepositionPath {
            kind: DepositionKind::SupportInterface,
            points_mm: vec![[5.0, 1.0, 1.0], [7.0, 1.0, 1.0]],
        },
        DepositionPath {
            kind: DepositionKind::Support,
            points_mm: vec![[1.0, 1.0, 1.0], [3.0, 1.0, 1.0]],
        },
    ];
    plan.layers[0].region_coverage.support_area_mm2 = 4.0;
    plan.layers[1].height_mm = 0.5;
    plan.layers[1].z_mm = 1.5;
    plan.layers[1].section_z_mm = 1.25;
    plan.layers[1].paths = vec![DepositionPath {
        kind: DepositionKind::Bridge,
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
    let records = bytes[96..].as_chunks::<48>().0;
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
fn nearest_seam_and_open_direction_are_useful_and_deterministic() {
    let (intent, mut plan) = fixture();
    plan.layers.truncate(1);
    // The second perimeter's stored seam is distant; the previous end is near
    // its third vertex. The following open stroke should enter its far endpoint.
    plan.layers[0].paths.push(DepositionPath {
        kind: DepositionKind::Perimeter,
        points_mm: vec![
            [10.0, 10.0, 1.0],
            [10.0, 2.0, 1.0],
            [2.0, 2.0, 1.0],
            [2.0, 10.0, 1.0],
            [10.0, 10.0, 1.0],
        ],
    });
    plan.layers[0].paths[1].points_mm = vec![[20.0, 20.0, 1.0], [2.0, 2.0, 1.0]];
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    crate::realization::canonicalize(&mut plan, &intent, &cancel).unwrap();
    assert_eq!(plan.layers[0].paths[1].points_mm[0], [2.0, 2.0, 1.0]);
    assert_eq!(plan.layers[0].paths[2].points_mm[0], [2.0, 2.0, 1.0]);
    let emitted = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    assert!(emitted.contains("G0 X2.000000 Y2.000000 Z1.000000"));
    assert!(!emitted.contains("G0 X20.000000 Y20.000000"));
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
fn contact_gap_layer_has_no_fake_motion_and_is_physically_bounded_in_plan() {
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
        index: 1,
        z_mm: 1.25,
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
    plan.layers[1].section_z_mm = 1.375;
    plan.layers.insert(1, gap);
    refresh_volumes(&intent, &mut plan);
    let cancel = AtomicBool::new(false);
    let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
    let mut sink = ReplaySink::new(&plan, &intent).unwrap();
    let report = verify_actual(&program, &plan, &intent, &cancel, Some(&mut sink)).unwrap();
    let (bytes, _) = sink.finish().unwrap();
    assert_eq!(&bytes[24..28], &3u32.to_le_bytes());
    for record in bytes[96..].as_chunks::<48>().0 {
        assert_ne!(u16::from_le_bytes(record[44..46].try_into().unwrap()), 1);
    }
    assert_eq!(report.support_deposition_segments, 2);
    assert_eq!(report.deposition_segments, 1);
    if let SupportPolicy::Breakaway { z_gap_mm, .. } = &mut intent.recipe.supports {
        *z_gap_mm = 0.1;
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
        bytes[96 + 2 * 48 + 46],
        11,
        "first body event retracts after complete prime"
    );
    assert_eq!(
        &bytes[96 + 48..96 + 48 + 24],
        &bytes[96 + 2 * 48..96 + 2 * 48 + 24]
    );
    assert_eq!(report.priming_filament_length_mm, 1.0);
    assert!((report.filament_length_mm - 6.0).abs() < 1e-12);
    validate_replay_display(
        &bytes,
        &packed_process_descriptor(&bytes, bounds, &program, &intent, &plan, &report),
    )
    .unwrap();
}
