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

fn fixture() -> (ManufacturingIntent, NormalizedPrintPlan) {
    let intent = ManufacturingIntent {
        printer: PrinterSpecification {
            schema_version: 1,
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
            print_speed_mm_s: 10.0,
            travel_speed_mm_s: 20.0,
            flow_multiplier: 1.0,
        },
    };
    let plan = NormalizedPrintPlan {
        schema_version: 1,
        sampling_tolerance_mm: 0.005,
        offset_tolerance_mm: 0.001,
        layer_quantization_mm: 0.000_001,
        layers: vec![
            PrintLayer {
                index: 0,
                z_mm: 1.0,
                section_z_mm: 0.5,
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
                section_z_mm: 1.5,
                paths: vec![DepositionPath {
                    kind: DepositionKind::SolidFill,
                    points_mm: vec![[2.0, 2.0, 2.0], [2.0, 6.0, 2.0]],
                }],
            },
        ],
    };
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
                section_z_mm: empty_index as f64 + 0.5,
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
        layer.section_z_mm = (f64::from(layer.index) + 0.5) * 0.2;
        for path in &mut layer.paths {
            for point in &mut path.points_mm {
                point[0] += 0.123_456_789;
                point[2] = layer.z_mm;
            }
        }
    }
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
    let program = "G21\nG90\nM82\nG92 E0\nG0 X1 Y1 Z1 F1200\nG1 X4 Y5 Z1 E1.25 F600\n";
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
    let program = "G21\nG90\nM82\nG92 E0\nG0 X2 Y2 Z1 F1200\nG1 X2 Y2 Z1 E0.00000003 F600\n";
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
