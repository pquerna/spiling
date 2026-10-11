// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::geometry::{AabbMm, OccurrenceId, RigidPoseMm, SourceHash};

fn intent() -> ManufacturingIntent {
    ManufacturingIntent {
        printer: PrinterSpecification {
            schema_version: 1,
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
            print_speed_mm_s: 20.0,
            travel_speed_mm_s: 100.0,
            flow_multiplier: 1.0,
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
fn unsupported_thin_support_partial_top_envelope_and_invalid_profiles_are_errors() {
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
    assert_eq!(
        scene.compile(&data).err().unwrap().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
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
            _ => data.recipe.infill_fraction = 0.5,
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
