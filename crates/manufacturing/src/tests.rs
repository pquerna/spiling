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
            native_shape_error_mm: 0.1,
            id: "solid".into(),
            revision: "1".into(),
            material_id: "nominal".into(),
            material_revision: "1".into(),
            filament_diameter_mm: 1.75,
            layer_height_mm: 0.4,
            bead_width_mm: 0.6,
            perimeter_count: 1,
            seam_policy: SeamPolicy::Nearest,
            rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
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
    surface_overlays: spiling_contracts::surface::SurfaceOverlays,
    surface_images:
        BTreeMap<spiling_contracts::geometry::SourceHash, Arc<spiling_surface_fields::HeightImage>>,
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
            surface_overlays: spiling_contracts::surface::SurfaceOverlays::empty(),
            surface_images: BTreeMap::new(),
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
            surface_overlays: &self.surface_overlays,
            surface_images: &self.surface_images,
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

fn actual_motion(bundle: &ManufacturingBundle) -> Vec<(u8, u32, [f64; 3])> {
    let bytes = serde_json::to_vec(bundle).unwrap();
    let inspection = inspect_bundle(
        bundle,
        &SourceHash::from_bytes(&bytes),
        VerificationOutput::ReportAndMotion,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(inspection.report.verified);
    inspection.replay.unwrap().bytes[REPLAY_HEADER_BYTES..]
        .as_chunks::<REPLAY_RECORD_BYTES>()
        .0
        .iter()
        .map(|record| {
            (
                record[46],
                u16::from_le_bytes(record[44..46].try_into().unwrap()) as u32,
                std::array::from_fn(|axis| {
                    f64::from_le_bytes(record[axis * 8..axis * 8 + 8].try_into().unwrap())
                }),
            )
        })
        .collect()
}

#[test]
fn authored_rectilinear_axes_change_actual_skin_and_interior_motion() {
    let scene = Scene::new(BOX, &[pose(30.0, 40.0, 0.0)]);
    for fraction in [0.25, 1.0] {
        for policy in [
            RectilinearAxisPolicy::X,
            RectilinearAxisPolicy::Y,
            RectilinearAxisPolicy::AlternatingXY,
            RectilinearAxisPolicy::AlternatingYX,
        ] {
            let mut intent = intent();
            intent.recipe.infill_fraction = fraction;
            intent.recipe.rectilinear_axis = policy;
            let bundle = scene.compile(&intent).unwrap();
            let motion = actual_motion(&bundle);
            let mut roles = std::collections::BTreeSet::new();
            for pair in motion.windows(2) {
                let (kind, index, end) = pair[1];
                if !(2..=5).contains(&kind) {
                    continue;
                }
                roles.insert(kind);
                let along_x = match policy {
                    RectilinearAxisPolicy::X => true,
                    RectilinearAxisPolicy::Y => false,
                    RectilinearAxisPolicy::AlternatingXY => index.is_multiple_of(2),
                    RectilinearAxisPolicy::AlternatingYX => !index.is_multiple_of(2),
                };
                let start = pair[0].2;
                let fixed = usize::from(along_x);
                assert_eq!(
                    start[fixed], end[fixed],
                    "{policy:?}, layer {index}, role {kind}"
                );
                assert_ne!(start[1 - fixed], end[1 - fixed]);
            }
            assert!(roles.contains(&3) && roles.contains(&4));
            assert!(roles.contains(&(if fraction == 1.0 { 2 } else { 5 })));
            assert!(
                bundle
                    .plan
                    .layers
                    .iter()
                    .all(|layer| { layer.region_coverage.surface_added_area_mm2 == 0.0 })
            );
        }
    }
}

#[test]
fn authored_housing_bridge_recovery_keeps_supported_fill_on_its_axis() {
    let scene = Scene::new(
        include_bytes!(
            "../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ),
        &[pose(98.0, 101.0, 0.0)],
    );
    let mut intent = decode_intent(include_bytes!(
        "../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::Y;
    intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [0.6, 0.8] };
    let bundle = scene.compile(&intent).unwrap();
    let motion = actual_motion(&bundle);
    let mut orthogonal_bridges = 0;
    let mut ordinary = 0;
    let mut aligned = std::collections::BTreeMap::<(u32, usize, u64), Vec<[f64; 2]>>::new();
    for pair in motion.windows(2) {
        let (kind, layer, end) = pair[1];
        let start = pair[0].2;
        if (2..=5).contains(&kind) {
            assert_eq!(
                start[0], end[0],
                "ordinary role {kind} ignored Y at layer {layer}"
            );
            ordinary += 1;
        } else if kind == 8 && start[1] == end[1] && start[0] != end[0] {
            orthogonal_bridges += 1;
        }
        if (1..=5).contains(&kind) || kind == 8 {
            let axis = if start[1] == end[1] {
                Some(0)
            } else if start[0] == end[0] {
                Some(1)
            } else {
                None
            };
            if let Some(axis) = axis.filter(|axis| start[*axis] != end[*axis]) {
                aligned
                    .entry((layer, axis, start[1 - axis].to_bits()))
                    .or_default()
                    .push([start[axis].min(end[axis]), start[axis].max(end[axis])]);
            }
        }
    }
    assert!(ordinary > 0 && orthogonal_bridges > 0);
    for ((layer, _, _), mut intervals) in aligned {
        intervals.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]));
        assert!(
            intervals.windows(2).all(|pair| pair[0][1] <= pair[1][0]),
            "duplicate aligned actual deposition at layer {layer}"
        );
    }
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .all(|layer| layer.region_coverage.surface_added_area_mm2 == 0.0)
    );
}

#[test]
fn preferred_seams_use_build_sides_and_keep_actual_protected_holes() {
    let scene = Scene::new(HOLE, &[pose(30.0, 40.0, 0.0)]);
    let mut programs = Vec::new();
    for x in [1.0, -1.0] {
        let mut intent = intent();
        intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [x, 0.0] };
        let bundle = scene.compile(&intent).unwrap();
        let motion = actual_motion(&bundle);
        let first_wall = &bundle.plan.layers[0].paths[0];
        let extremum = first_wall
            .points_mm
            .iter()
            .map(|p| p[0] * x)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((motion[0].2[0] * x - extremum).abs() <= 0.0000005);
        assert_eq!(motion[0].0, 0);
        for pair in motion.windows(2) {
            if !matches!(pair[1].0, 1..=5 | 8) {
                continue;
            }
            let a = [pair[0].2[0] - 40.0, pair[0].2[1] - 50.0];
            let d = [pair[1].2[0] - pair[0].2[0], pair[1].2[1] - pair[0].2[1]];
            let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0.0, 1.0);
            assert!((a[0] + t * d[0]).hypot(a[1] + t * d[1]) >= 3.0 - 0.01);
        }
        assert_eq!(
            bundle.plan.layers[0]
                .paths
                .iter()
                .filter(|p| p.kind == DepositionKind::Perimeter)
                .count(),
            2
        );
        programs.push(bundle.program);
    }
    assert_ne!(programs[0], programs[1]);
}

#[test]
fn build_direction_and_prime_endpoint_resolve_actual_seam_ties() {
    use spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;
    let scene = Scene::new(BOX, &[pose(30.0, 40.0, 0.0)]);
    let mut diagnostic = intent();
    diagnostic.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [1.0, -0.0] };
    let without_prior = scene.compile(&diagnostic).unwrap();
    let first = &without_prior.plan.layers[0].paths[0];
    let max_x = first
        .points_mm
        .iter()
        .map(|point| point[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = first
        .points_mm
        .iter()
        .map(|point| point[1])
        .fold(f64::INFINITY, f64::min);
    assert_eq!(
        actual_motion(&without_prior)[0].2,
        project_xyz6_point_mm([max_x, min_y, first.points_mm[0][2]]).unwrap()
    );

    let mut complete = decode_intent(include_bytes!(
        "../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    let JobWorkflow::CompleteMarlin { job } = &mut complete.printer.workflow else {
        panic!("complete-job fixture must declare its lifecycle");
    };
    job.prime_start_mm = [25.0, 70.0, 0.2];
    job.prime_end_mm = [50.0, 70.0, 0.2];
    complete.recipe.seam_policy = SeamPolicy::Nearest;
    let nearest = scene.compile(&complete).unwrap();
    complete.recipe.seam_policy = diagnostic.recipe.seam_policy;
    let preferred = scene.compile(&complete).unwrap();
    for bundle in [&nearest, &preferred] {
        let wall = &bundle.plan.layers[0].paths[0];
        let max_x = wall
            .points_mm
            .iter()
            .map(|point| point[0])
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = wall
            .points_mm
            .iter()
            .map(|point| point[1])
            .fold(f64::NEG_INFINITY, f64::max);
        let motion = actual_motion(bundle);
        let body_entry = motion.windows(2).find(|pair| pair[1].0 == 1).unwrap()[0].2;
        assert_eq!(
            body_entry,
            project_xyz6_point_mm([max_x, max_y, wall.points_mm[0][2]]).unwrap()
        );
    }
    let rotated = Scene::new(
        BOX,
        &[RigidPoseMm {
            translation_mm: [70.0, 70.0, 0.0],
            rotation_xyzw: [0.0, 0.0, 1.0, 0.0],
        }],
    );
    let bundle = rotated.compile(&diagnostic).unwrap();
    let start = actual_motion(&bundle)[0].2;
    assert!((start[0] - 69.7).abs() < 0.000001);
    assert!((start[1] - 60.3).abs() < 0.000001);
}

#[test]
fn saved_verification_cannot_authorize_an_incompatible_planner() {
    let mut bundle = Scene::new(BOX, &[RigidPoseMm::IDENTITY])
        .compile(&intent())
        .unwrap();
    assert!(bundle.verification.verified);
    bundle.provenance.planner_version = "unsupported-source-semantics".into();
    let bytes = serde_json::to_vec(&bundle).unwrap();
    let decoded = decode_bundle(&bytes).unwrap();
    for output in [
        VerificationOutput::ReportOnly,
        VerificationOutput::ReportAndMotion,
    ] {
        assert_eq!(
            inspect_bundle(
                &decoded,
                &SourceHash::from_bytes(&bytes),
                output,
                &AtomicBool::new(false)
            )
            .err()
            .unwrap()
            .code,
            ManufacturingErrorCode::UnsupportedCapability,
        );
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
    assert_eq!(bundle.plan.layers, again.plan.layers);
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
        overlap
            .plan
            .native_geometry
            .source_feature_coverage
            .disjoint_source_layer_count,
        0
    );
    assert_eq!(
        overlap
            .plan
            .native_geometry
            .source_feature_coverage
            .invariant_source_union_layer_count,
        overlap.plan.native_geometry.certified_layer_count,
    );
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
            .into_loops()
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
    bundle.plan.native_geometry.certified_layer_count = 1;
    bundle.plan.native_geometry.slab_count = 1;
    bundle.plan.native_geometry.source_feature_coverage = NativeFeatureCoverageSummary {
        disjoint_source_layer_count: 1,
        invariant_source_union_layer_count: 0,
        native_interface_complex_layer_count: 0,
        owned_critical_slab_count: 0,
    };
    // ABI fixture identity excludes incidental native traversal work.
    bundle.plan.native_geometry.geometry_work = 1;
    bundle.plan.layers[0].paths = vec![
        DepositionPath {
            kind: DepositionKind::SolidFill,
            points_mm: vec![[1.000_000_4, 1.0, 0.4], [5.0, 1.0, 0.4]],
        },
        DepositionPath {
            kind: DepositionKind::SolidFill,
            points_mm: vec![[5.0, 1.0, 0.4], [9.0, 1.0, 0.4]],
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
