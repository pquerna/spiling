// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::geometry::{AabbMm, OccurrenceId, RigidPoseMm, SourceHash};

fn intent() -> ManufacturingIntent {
    ManufacturingIntent {
        printer: PrinterSpecification {
            schema_version: MANUFACTURING_SCHEMA_VERSION,
            id: "software-cartesian".into(),
            revision: "1".into(),
            name: "Software validation only".into(),
            coordinate_frame: PrinterCoordinateFrame::RightHandedMillimetres,
            build_envelope: AabbMm::new([-20.0, -20.0, 0.0], [200.0, 200.0, 100.0]).unwrap(),
            components: vec![],
            capabilities: PrinterCapabilities {
                motion_space: MotionSpace::CartesianXyz,
                extruders: 1,
                output_dialect: OutputDialect::CartesianAbsoluteGcodeV1,
                generated_supports: false,
            },
            workflow: JobWorkflow::DiagnosticBodyOnly,
            nozzle_diameter_mm: 0.4,
            max_feed_mm_s: 200.0,
            max_volumetric_flow_mm3_s: 100.0,
        },
        recipe: PlanarPrintRecipe {
            id: "solid".into(),
            revision: "1".into(),
            material_id: "nominal".into(),
            material_revision: "1".into(),
            filament_diameter_mm: 1.75,
            layer_height_mm: 0.4,
            bead_width_mm: 0.6,
            perimeter_count: 1,
            infill_fraction: 1.0,
            top_skin_thickness_mm: 0.8,
            bottom_skin_thickness_mm: 0.8,
            print_speed_mm_s: 20.0,
            travel_speed_mm_s: 100.0,
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
    }
}
struct Scene {
    id: ProjectId,
    definitions: BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
    stored: Vec<StoredDefinition>,
    occurrences: Vec<OccurrenceRecord>,
}
impl Scene {
    fn new(bytes: &[u8], poses: &[RigidPoseMm]) -> Self {
        let native =
            Arc::new(spiling_geometry::import_step(bytes, &AtomicBool::new(false)).unwrap());
        let id = native.id().clone();
        let mut provenance = native.provenance().clone();
        provenance.source_name = "operator captured source.step".into();
        Self {
            id: ProjectId::default(),
            stored: vec![StoredDefinition {
                definition_id: id.clone(),
                provenance,
            }],
            occurrences: poses
                .iter()
                .enumerate()
                .map(|(i, p)| OccurrenceRecord {
                    occurrence_id: OccurrenceId::new(i as u32 + 1).unwrap(),
                    definition_id: id.clone(),
                    pose: *p,
                })
                .collect(),
            definitions: BTreeMap::from([(id, native)]),
        }
    }
    fn compile(
        &self,
        intent: &ManufacturingIntent,
    ) -> Result<ManufacturingBundle, ManufacturingError> {
        compile(self.input(intent), &AtomicBool::new(false))
    }
    fn input<'a>(&'a self, intent: &'a ManufacturingIntent) -> CompilerInput<'a> {
        CompilerInput {
            project_id: &self.id,
            revision: ProjectRevision(7),
            definitions: &self.definitions,
            stored_definitions: &self.stored,
            occurrences: &self.occurrences,
            intent,
        }
    }
}
const BOX: &[u8] = include_bytes!("../../../fixtures/geometry/box-mm.step");
const CYLINDER: &[u8] = include_bytes!("../../../fixtures/geometry/cylinder.step");
const HOLE: &[u8] = include_bytes!("../../../fixtures/geometry/through-hole.step");
fn pose(x: f64, y: f64, z: f64) -> RigidPoseMm {
    RigidPoseMm {
        translation_mm: [x, y, z],
        ..RigidPoseMm::IDENTITY
    }
}

#[test]
fn native_box_has_pinned_geometry_nominal_volume_and_deterministic_program() {
    let scene = Scene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let intent = intent();
    let bundle = scene.compile(&intent).unwrap();
    assert_eq!(bundle.plan.layers.len(), 20);
    assert_eq!(bundle.plan.layers[0].z_mm, 0.4);
    assert_eq!(bundle.plan.layers[0].section_z_mm, 0.2);
    let ring = &bundle.plan.layers[0].paths[0];
    assert_eq!(ring.kind, DepositionKind::Perimeter);
    assert!((ring.points_mm[0][0] - 0.3).abs() < 1e-7);
    assert!((ring.points_mm[0][1] - 0.3).abs() < 1e-7);
    // Independent rectangular geometry/count calculation, not a planner preview echo.
    let h: f64 = 0.4;
    let w: f64 = 0.6;
    let area = (w - h) * h + std::f64::consts::PI * (h * 0.5).powi(2);
    let spacing = area / h;
    let wall_length = 2.0 * ((20.0 - w) + (10.0 - w));
    let x_rows = ((10.0 - 2.0 * w) / spacing).ceil();
    let y_rows = ((20.0 - 2.0 * w) / spacing).ceil();
    let expected =
        10.0 * (2.0 * wall_length + x_rows * (20.0 - 2.0 * w) + y_rows * (10.0 - 2.0 * w)) * area;
    assert!((bundle.verification.deposited_volume_mm3 - expected).abs() < 0.001);
    assert!(
        (bundle.verification.filament_length_mm
            - expected / (std::f64::consts::PI * 0.875f64.powi(2)))
        .abs()
            < 0.001
    );
    let again = scene.compile(&intent).unwrap();
    assert_eq!(
        SourceHash::from_bytes(bundle.program.as_bytes()),
        SourceHash::from_bytes(again.program.as_bytes())
    );
    assert_eq!(
        serde_json::to_vec(&bundle.plan).unwrap(),
        serde_json::to_vec(&again.plan).unwrap()
    );
    assert!(
        verify_bundle(&bundle, &AtomicBool::new(false))
            .unwrap()
            .verified
    );
    assert_eq!(bundle.provenance.definitions, scene.stored);
}

#[test]
fn native_cylinder_and_through_hole_replay_keep_nominal_material_and_voids() {
    for (bytes, expected_volume) in [
        (CYLINDER, 200.0 * std::f64::consts::PI),
        (HOLE, 8.0 * (400.0 - 9.0 * std::f64::consts::PI)),
    ] {
        let scene = Scene::new(bytes, &[RigidPoseMm::IDENTITY]);
        let bundle = scene.compile(&intent()).unwrap();
        assert!(
            (bundle.verification.deposited_volume_mm3 - expected_volume).abs() / expected_volume
                < 0.05
        );
        if bytes == HOLE {
            assert_eq!(
                bundle.plan.layers[0]
                    .paths
                    .iter()
                    .filter(|p| p.kind == DepositionKind::Perimeter)
                    .count(),
                2
            );
            for layer in &bundle.plan.layers {
                for path in &layer.paths {
                    for edge in path.points_mm.windows(2) {
                        let a = [edge[0][0] - 10.0, edge[0][1] - 10.0];
                        let d = [edge[1][0] - edge[0][0], edge[1][1] - edge[0][1]];
                        let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1]))
                            .clamp(0.0, 1.0);
                        assert!((a[0] + t * d[0]).hypot(a[1] + t * d[1]) >= 3.0 - 0.01);
                    }
                }
            }
        }
    }
}

#[test]
fn repeated_definition_placements_and_overlap_union_are_material_not_duplicates() {
    let data = intent();
    let one = Scene::new(BOX, &[RigidPoseMm::IDENTITY])
        .compile(&data)
        .unwrap();
    let separated = Scene::new(
        BOX,
        &[
            pose(0.0, 0.0, 0.0),
            pose(30.0, 0.0, 0.0),
            pose(60.0, 0.0, 0.0),
        ],
    )
    .compile(&data)
    .unwrap();
    assert_eq!(separated.provenance.definitions.len(), 1);
    assert_eq!(separated.provenance.occurrences.len(), 3);
    assert_eq!(
        separated.plan.layers[0]
            .paths
            .iter()
            .filter(|p| p.kind == DepositionKind::Perimeter)
            .count(),
        3
    );
    assert!(
        (separated.verification.deposited_volume_mm3 - 3.0 * one.verification.deposited_volume_mm3)
            .abs()
            < 0.001
    );
    let duplicate = Scene::new(BOX, &[RigidPoseMm::IDENTITY, RigidPoseMm::IDENTITY])
        .compile(&data)
        .unwrap();
    assert!(
        (duplicate.verification.deposited_volume_mm3 - one.verification.deposited_volume_mm3).abs()
            < 0.001
    );
    let overlap = Scene::new(BOX, &[RigidPoseMm::IDENTITY, pose(10.0, 0.0, 0.0)])
        .compile(&data)
        .unwrap();
    assert_eq!(
        overlap.plan.layers[0]
            .paths
            .iter()
            .filter(|p| p.kind == DepositionKind::Perimeter)
            .count(),
        1
    );
    assert!(overlap.verification.deposited_volume_mm3 > one.verification.deposited_volume_mm3);
    assert!(
        overlap.verification.deposited_volume_mm3 < 2.0 * one.verification.deposited_volume_mm3
    );
}

#[test]
fn rigid_rotated_sections_and_nonzero_bed_use_declared_world_frame() {
    let mut data = intent();
    let original = Scene::new(BOX, &[RigidPoseMm::IDENTITY])
        .compile(&data)
        .unwrap();
    data.printer.build_envelope.min[2] = 5.0;
    let angle = std::f64::consts::FRAC_PI_4;
    let rotated = RigidPoseMm {
        translation_mm: [30.0, 0.0, 5.0],
        rotation_xyzw: [0.0, 0.0, angle.sin(), angle.cos()],
    };
    let scene = Scene::new(BOX, &[rotated]);
    let bundle = scene.compile(&data).unwrap();
    assert!((bundle.plan.layers[0].z_mm - 5.4).abs() < 1e-12);
    assert!((bundle.plan.layers[0].section_z_mm - 5.2).abs() < 1e-12);
    assert!(
        (bundle.verification.deposited_volume_mm3 - original.verification.deposited_volume_mm3)
            .abs()
            < 0.001
    );
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .flat_map(|p| &p.points_mm)
            .all(|p| p[0] >= 20.0 - 1e-6
                && p[0] <= 30.0 + 1e-6
                && p[1] >= -1e-6
                && p[1] <= 20.0 + 1e-6)
    );
}

#[test]
fn unsupported_thin_support_envelope_and_invalid_profiles_are_errors() {
    let scene = Scene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut data = intent();
    data.recipe.bead_width_mm = 12.0;
    data.recipe.print_speed_mm_s = 1.0;
    assert_eq!(
        scene.compile(&data).err().unwrap().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
    let elevated = Scene::new(BOX, &[pose(0.0, 0.0, 0.0), pose(10.0, 0.0, 8.0)]);
    assert_eq!(
        elevated.compile(&intent()).err().unwrap().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
    data = intent();
    data.recipe.layer_height_mm = 0.3;
    let variable = scene.compile(&data).unwrap();
    let last = variable.plan.layers.last().unwrap();
    assert!((last.z_mm - 8.0).abs() < 1e-6);
    assert!(last.height_mm > 0.0 && last.height_mm < data.recipe.layer_height_mm);
    assert!(variable.verification.verified);
    data = intent();
    data.printer.build_envelope.max[0] = 19.0;
    assert_eq!(
        scene.compile(&data).err().unwrap().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
    for case in 0..4 {
        data = intent();
        match case {
            0 => data.recipe.print_speed_mm_s = 201.0,
            1 => data.printer.max_volumetric_flow_mm3_s = 0.01,
            2 => data.recipe.flow_multiplier = f64::NAN,
            _ => data.recipe.infill_fraction = 0.0,
        }
        assert!(scene.compile(&data).is_err());
    }
    data = intent();
    data.recipe.layer_height_mm = 0.001;
    assert_eq!(
        scene.compile(&data).err().unwrap().code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert_eq!(
        compile(scene.input(&intent()), &AtomicBool::new(true))
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::Cancelled
    );
}

#[test]
fn replay_rejects_tampered_program_and_fingerprint_even_with_verified_flag() {
    let scene = Scene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut bundle = scene.compile(&intent()).unwrap();
    bundle.program.push_str("G28\n");
    assert!(verify_bundle(&bundle, &AtomicBool::new(false)).is_err());
    let mut bundle = scene.compile(&intent()).unwrap();
    bundle.provenance.intent.recipe.material_revision = "changed".into();
    assert!(verify_bundle(&bundle, &AtomicBool::new(false)).is_err());
}

#[test]
fn vertical_slab_transition_keeps_upper_material_inside_fresh_native_sections() {
    use geo::{Contains, LineString, Point, Polygon};
    use spiling_contracts::geometry::PlaneMm;

    let mut scene = Scene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let upper = Arc::new(spiling_geometry::import_step(CYLINDER, &AtomicBool::new(false)).unwrap());
    let upper_id = upper.id().clone();
    scene.stored.push(StoredDefinition {
        definition_id: upper_id.clone(),
        provenance: upper.provenance().clone(),
    });
    scene.occurrences.push(OccurrenceRecord {
        occurrence_id: OccurrenceId::new(2).unwrap(),
        definition_id: upper_id.clone(),
        pose: pose(10.0, 5.0, 8.0),
    });
    scene.definitions.insert(upper_id, upper.clone());
    let bundle = scene.compile(&intent()).unwrap();
    assert_eq!(bundle.plan.layers.len(), 40);
    for layer in &bundle.plan.layers[20..] {
        for point in layer.paths.iter().flat_map(|path| &path.points_mm) {
            assert!((point[0] - 10.0).hypot(point[1] - 5.0) <= 4.71);
            assert_eq!(point[2], layer.z_mm);
        }
    }
    for index in [20, 39] {
        let layer = &bundle.plan.layers[index];
        let native = spiling_geometry::section_with_byte_limit(
            &upper,
            PlaneMm {
                origin_mm: [0.0, 0.0, layer.section_z_mm - 8.0],
                normal: [0.0, 0.0, 1.0],
            },
            &AtomicBool::new(false),
            4 * 1024 * 1024,
        )
        .unwrap();
        let boundary = native
            .loops
            .into_iter()
            .find(|boundary| !boundary.is_hole)
            .unwrap();
        let polygon = Polygon::new(
            LineString::from(
                boundary
                    .points_mm
                    .into_iter()
                    .map(|p| (p[0], p[1]))
                    .collect::<Vec<_>>(),
            ),
            Vec::new(),
        );
        for edge in layer
            .paths
            .iter()
            .flat_map(|path| path.points_mm.windows(2))
        {
            let midpoint = Point::new(
                (edge[0][0] + edge[1][0]) * 0.5 - 10.0,
                (edge[0][1] + edge[1][1]) * 0.5 - 5.0,
            );
            assert!(polygon.contains(&midpoint));
        }
    }
}

#[test]
fn repeated_slab_paths_charge_every_layer_against_segment_budget() {
    let scene = Scene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut intent = intent();
    intent.recipe.layer_height_mm = 0.002;
    assert_eq!(
        scene.compile(&intent).unwrap_err().code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn actual_motion_inspection_rounding_coincident_travel_and_fixture() {
    let mut scene = Scene::new(BOX, &[RigidPoseMm::IDENTITY]);
    scene.id = ProjectId::parse("550e8400-e29b-41d4-a716-446655440000").unwrap();
    let intent = intent();
    let cancel = AtomicBool::new(false);
    let mut bundle = scene.compile(&intent).unwrap();
    // A supported two-path plan retains actual source provenance; only the first
    // endpoint has an intentionally visible serialization-rounding deviation.
    bundle.plan.layers.truncate(1);
    bundle.plan.layers[0].paths = vec![
        DepositionPath {
            kind: DepositionKind::SolidFill,
            points_mm: vec![[1.000_000_4, 1.0, 0.4], [5.0, 1.0, 0.4]],
        },
        DepositionPath {
            kind: DepositionKind::SolidFill,
            points_mm: vec![[5.0, 1.0, 0.4], [5.0, 5.0, 0.4]],
        },
    ];
    let recipe = &intent.recipe;
    let bead_area = ((recipe.bead_width_mm - recipe.layer_height_mm) * recipe.layer_height_mm
        + std::f64::consts::PI * (recipe.layer_height_mm * 0.5).powi(2))
        * recipe.flow_multiplier;
    bundle.plan.layers[0]
        .region_coverage
        .nominal_deposition_volume_mm3 = bundle.plan.layers[0]
        .paths
        .iter()
        .flat_map(|path| path.points_mm.windows(2))
        .map(|edge| {
            (edge[1][0] - edge[0][0])
                .hypot(edge[1][1] - edge[0][1])
                .hypot(edge[1][2] - edge[0][2])
                * bead_area
        })
        .sum();
    bundle.program = backend::emit(&bundle.plan, &intent, &cancel).unwrap();
    bundle.verification =
        verifier::verify(&bundle.program, &bundle.plan, &intent, &cancel).unwrap();
    let bundle_bytes = serde_json::to_vec(&bundle).unwrap();
    let hash = SourceHash::from_bytes(&bundle_bytes);
    let inspection =
        inspect_bundle(&bundle, &hash, VerificationOutput::ReportAndMotion, &cancel).unwrap();
    assert!(inspection.report.max_position_error_mm > 0.0);
    assert_eq!(
        inspection.provenance,
        ManufacturingInspectionProvenance::from(&bundle.provenance)
    );
    let replay = inspection.replay.unwrap();
    assert_eq!(replay.info.event_count, 4);
    assert_eq!(replay.info.travel_count, 2);
    assert_eq!(replay.bytes[REPLAY_HEADER_BYTES + 47], 0);
    assert_eq!(
        replay.bytes[REPLAY_HEADER_BYTES + 2 * REPLAY_RECORD_BYTES + 47],
        1
    );
    let positions: Vec<[f64; 3]> = replay.bytes[REPLAY_HEADER_BYTES..]
        .as_chunks::<REPLAY_RECORD_BYTES>()
        .0
        .iter()
        .map(|r| {
            std::array::from_fn(|axis| {
                f64::from_le_bytes(r[axis * 8..axis * 8 + 8].try_into().unwrap())
            })
        })
        .collect();
    assert_eq!(positions[0], [1.0, 1.0, 0.4]);
    assert_ne!(positions[0], bundle.plan.layers[0].paths[0].points_mm[0]);
    assert_eq!(positions[1], positions[2]);
    let replay_hash = SourceHash::from_bytes(&replay.bytes);
    let descriptor = ReplayDisplayDescriptor {
        info: replay.info,
        resource: spiling_contracts::ArtifactView {
            name: format!("artifacts/{}", replay_hash.as_str()),
            size_bytes: replay.bytes.len().to_string(),
            sha256: replay_hash.as_str().into(),
            media_type: REPLAY_MEDIA_TYPE.into(),
        },
    };
    descriptor
        .validate_for_record(&ManufacturingArtifactRecord {
            hash: hash.clone(),
            input_hash: bundle.provenance.input_hash.clone(),
            byte_count: bundle_bytes.len() as u32,
            summary: bundle.summary(),
        })
        .unwrap();
    validate_replay_display(&replay.bytes, &descriptor).unwrap();
    let mut corrupted = replay.bytes.clone();
    corrupted[REPLAY_HEADER_BYTES + 47] = 1;
    let changed_hash = SourceHash::from_bytes(&corrupted);
    let mut corrupted_descriptor = descriptor.clone();
    corrupted_descriptor.resource.name = format!("artifacts/{}", changed_hash.as_str());
    corrupted_descriptor.resource.sha256 = changed_hash.as_str().into();
    assert!(validate_replay_display(&corrupted, &corrupted_descriptor).is_err());
    assert!(
        inspect_bundle(&bundle, &hash, VerificationOutput::ReportOnly, &cancel)
            .unwrap()
            .replay
            .is_none()
    );
    let mut invalid_bundle = bundle.clone();
    invalid_bundle.program = invalid_bundle.program.replacen("G1 ", "G0 ", 1);
    assert!(
        inspect_bundle(
            &invalid_bundle,
            &hash,
            VerificationOutput::ReportAndMotion,
            &cancel
        )
        .is_err()
    );
    assert_eq!(
        inspect_bundle(
            &bundle,
            &hash,
            VerificationOutput::ReportAndMotion,
            &AtomicBool::new(true)
        )
        .err()
        .unwrap()
        .code,
        ManufacturingErrorCode::Cancelled
    );
    // Fixed fixture identity makes the independently decoded ABI golden reproducible.
    use std::fmt::Write;
    let mut hex = String::with_capacity(replay.bytes.len() * 2);
    for byte in &replay.bytes {
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    let fixture = serde_json::json!({ "encoding":"hex", "descriptor":descriptor,
        "data":hex, "actual_positions":positions, "program":bundle.program });
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/protocol/replay.json"
    );
    if std::env::var_os("SPILING_UPDATE_REPLAY_FIXTURE").is_some() {
        std::fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    } else {
        let committed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(committed, fixture, "native replay ABI golden drift");
    }
}
