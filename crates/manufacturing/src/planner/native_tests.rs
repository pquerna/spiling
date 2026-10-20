// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
use crate::{CompilerInput, compile};
use geo::BooleanOps;
use spiling_contracts::geometry::{AabbMm, DefinitionId, OccurrenceId, OccurrenceRecord, PlaneMm};
use spiling_contracts::project::{ProjectId, ProjectRevision, StoredDefinition};
use std::collections::BTreeMap;

struct NativeScene {
    project: ProjectId,
    definitions: BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
    stored: Vec<StoredDefinition>,
    occurrences: Vec<OccurrenceRecord>,
    surface_overlays: spiling_contracts::surface::SurfaceOverlays,
    process_document: spiling_contracts::process::ProcessDocument,
    surface_images:
        BTreeMap<spiling_contracts::geometry::SourceHash, Arc<spiling_surface_fields::HeightImage>>,
}
impl NativeScene {
    fn new(bytes: &[u8], poses: &[RigidPoseMm]) -> Self {
        let native =
            Arc::new(spiling_geometry::import_step(bytes, &AtomicBool::new(false)).unwrap());
        let id = native.id().clone();
        Self {
            project: ProjectId::default(),
            surface_overlays: spiling_contracts::surface::SurfaceOverlays::empty(),
            process_document: spiling_contracts::process::ProcessDocument::empty(),
            surface_images: BTreeMap::new(),
            stored: vec![StoredDefinition {
                definition_id: id.clone(),
                provenance: native.provenance().clone(),
            }],
            definitions: BTreeMap::from([(id.clone(), native)]),
            occurrences: poses
                .iter()
                .enumerate()
                .map(|(i, pose)| OccurrenceRecord {
                    occurrence_id: OccurrenceId::new(i as u32 + 1).unwrap(),
                    definition_id: id.clone(),
                    pose: *pose,
                })
                .collect(),
        }
    }
    fn add(&mut self, bytes: &[u8], poses: &[RigidPoseMm]) {
        let native =
            Arc::new(spiling_geometry::import_step(bytes, &AtomicBool::new(false)).unwrap());
        let id = native.id().clone();
        if !self.definitions.contains_key(&id) {
            self.stored.push(StoredDefinition {
                definition_id: id.clone(),
                provenance: native.provenance().clone(),
            });
            self.definitions.insert(id.clone(), native);
        }
        for pose in poses {
            self.occurrences.push(OccurrenceRecord {
                occurrence_id: OccurrenceId::new(self.occurrences.len() as u32 + 1).unwrap(),
                definition_id: id.clone(),
                pose: *pose,
            });
        }
    }
    fn input<'a>(&'a self, intent: &'a ManufacturingIntent) -> CompilerInput<'a> {
        CompilerInput {
            project_id: &self.project,
            revision: ProjectRevision(19),
            definitions: &self.definitions,
            stored_definitions: &self.stored,
            occurrences: &self.occurrences,
            intent,
            surface_overlays: &self.surface_overlays,
            process_document: &self.process_document,
            surface_images: &self.surface_images,
        }
    }
}
fn intent(supports: bool) -> ManufacturingIntent {
    let mut recipe = super::tests::recipe(1.0);
    recipe.top_skin_thickness_mm = 0.8;
    recipe.bottom_skin_thickness_mm = 0.8;
    recipe.supports = if supports {
        SupportPolicy::Breakaway {
            density: 0.3,
            xy_clearance_mm: 0.2,
            z_gap_mm: 0.2,
            interface_thickness_mm: 0.8,
        }
    } else {
        SupportPolicy::Disabled
    };
    ManufacturingIntent {
        recipe,
        printer: PrinterSpecification {
            schema_version: MANUFACTURING_SCHEMA_VERSION,
            id: "native-operator-software-only".into(),
            revision: "3".into(),
            name: "Unqualified native operator regression".into(),
            coordinate_frame: PrinterCoordinateFrame::RightHandedMillimetres,
            build_envelope: AabbMm::new([-30.0, -30.0, 0.0], [100.0, 100.0, 100.0]).unwrap(),
            components: vec![],
            capabilities: PrinterCapabilities {
                motion_space: MotionSpace::CartesianXyz,
                extruders: 1,
                output_dialect: OutputDialect::CartesianAbsoluteGcodeV1,
                generated_supports: supports,
            },
            workflow: JobWorkflow::DiagnosticBodyOnly,
            nozzle_diameter_mm: 0.4,
            deposition_limits: DepositionLimits {
                min_bead_width_mm: 0.01,
                max_bead_width_mm: 2.0,
                min_bead_height_mm: 0.000001,
                max_bead_height_mm: 0.6,
            },
            max_feed_mm_s: 200.0,
            max_volumetric_flow_mm3_s: 100.0,
        },
    }
}
fn generated(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../fixtures/manufacturing/native/{name}.step")))
        .expect("parent must generate genuine native operator STEP fixtures before running native regressions")
}
const BOX: &[u8] = include_bytes!("../../../../fixtures/geometry/box-mm.step");
const HOLE: &[u8] = include_bytes!("../../../../fixtures/geometry/through-hole.step");
fn translated(x: f64, y: f64, z: f64) -> RigidPoseMm {
    RigidPoseMm {
        translation_mm: [x, y, z],
        ..RigidPoseMm::IDENTITY
    }
}

fn original_section_polygons(scene: &NativeScene, z: f64) -> MultiPolygon<f64> {
    assert_eq!(scene.definitions.len(), 1);
    assert_eq!(scene.occurrences[0].pose, RigidPoseMm::IDENTITY);
    let cancel = AtomicBool::new(false);
    let section = spiling_geometry::section(
        scene.definitions.values().next().unwrap(),
        PlaneMm {
            origin_mm: [0.0, 0.0, z],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    )
    .unwrap();
    assert!(section.is_native_intersection());
    region::from_native(&section, &RigidPoseMm::IDENTITY, z, &cancel).unwrap()
}

fn visit_fresh_replayed_object_segments(
    bundle: &ManufacturingBundle,
    mut visit: impl FnMut([f64; 3], [f64; 3]),
) {
    let encoded = serde_json::to_vec(bundle).unwrap();
    let reopened = decode_bundle(&encoded).unwrap();
    let hash = spiling_contracts::geometry::SourceHash::from_bytes(&encoded);
    let inspection = crate::inspect_bundle(
        &reopened,
        &hash,
        VerificationOutput::ReportAndMotion,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(inspection.report.verified && inspection.report.deposition_segments > 0);
    let replay = inspection.replay.unwrap();
    let mut previous: Option<([f64; 3], f64)> = None;
    let mut object_segments = 0;
    for record in replay.bytes[REPLAY_HEADER_BYTES..]
        .as_chunks::<REPLAY_RECORD_BYTES>()
        .0
    {
        let end = std::array::from_fn(|axis| {
            f64::from_le_bytes(record[axis * 8..axis * 8 + 8].try_into().unwrap())
        });
        let extrusion = f64::from_le_bytes(record[24..32].try_into().unwrap());
        if matches!(record[46], 1..=5 | 8) {
            let (start, prior_e) = previous.expect("deposition has a preceding emitted endpoint");
            assert!(
                extrusion > prior_e,
                "serialized object extrusion must remain positive"
            );
            visit(start, end);
            object_segments += 1;
        }
        previous = Some((end, extrusion));
    }
    assert!(object_segments > 0);
}

// These fixtures have one actual contact height. Read the declared program
// representation, not an assumed nominal/native floor or an exact stop value.
fn actual_fixture_contact_gap(bundle: &ManufacturingBundle, source_floor: f64) -> f64 {
    let xyz6 = |z: f64| format!("{z:.6}").parse::<f64>().unwrap();
    let first_object = bundle
        .plan
        .layers
        .iter()
        .find(|layer| {
            layer.section_z_mm > source_floor
                && layer.paths.iter().any(|path| {
                    !matches!(
                        path.kind,
                        DepositionKind::Support | DepositionKind::SupportInterface
                    )
                })
        })
        .unwrap();
    let object_bottom = first_object
        .paths
        .iter()
        .filter(|path| {
            !matches!(
                path.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            )
        })
        .flat_map(|path| &path.points_mm)
        .map(|point| xyz6(point[2]) - first_object.height_mm)
        .fold(f64::INFINITY, f64::min);
    let support_top = bundle
        .plan
        .layers
        .iter()
        .filter(|layer| layer.z_mm < source_floor)
        .flat_map(|layer| &layer.paths)
        .filter(|path| path.kind == DepositionKind::SupportInterface)
        .flat_map(|path| &path.points_mm)
        .map(|point| xyz6(point[2]))
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(object_bottom.is_finite() && support_top.is_finite());
    object_bottom - support_top
}

#[test]
fn original_box_reuses_certified_slabs_without_fabricating_layer_captures() {
    let cancel = AtomicBool::new(false);
    let scene = NativeScene::new(BOX, &[translated(100.0, 105.0, 0.0)]);
    let mut intent = intent(true);
    intent.printer.build_envelope = AabbMm::new([0.0, 0.0, 0.0], [220.0, 220.0, 100.0]).unwrap();
    intent.recipe.native_shape_error_mm = 0.1;
    intent.recipe.layer_height_mm = 0.2;
    intent.recipe.infill_fraction = 0.25;
    let metadata = preflight(&scene.input(&intent), &cancel).unwrap();
    assert_eq!(metadata.disposition, PreflightDisposition::Admitted);
    let bundle = compile(scene.input(&intent), &cancel).unwrap();
    let coverage = &bundle.plan.native_geometry;
    assert_eq!(coverage.requested_shape_error_mm, 0.1);
    assert_eq!(coverage.height_analysis_count, 1);
    assert_eq!(
        coverage.certified_layer_count,
        bundle.plan.layers.len() as u32
    );
    assert!(
        coverage.slab_count > 0 && coverage.slab_count < coverage.certified_layer_count,
        "invariant source slabs are real accepted captures, not one synthetic capture per reused layer: {coverage:?}"
    );
    assert!(coverage.achieved_shape_error_mm.is_finite());
    assert!((0.0..=0.1).contains(&coverage.achieved_shape_error_mm));
    assert!(coverage.geometry_work > 0);
    assert_eq!(coverage.evidence, PreflightEvidence::Certified);
    assert_eq!(bundle.summary().native_geometry, *coverage);
    assert_eq!(bundle.provenance.occurrences, scene.occurrences);
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .all(|layer| { (layer.region_coverage.source_section_area_mm2 - 200.0).abs() < 1e-6 })
    );
    let features = &coverage.source_feature_coverage;
    assert_eq!(
        features.disjoint_source_layer_count
            + features.invariant_source_union_layer_count
            + features.native_interface_complex_layer_count,
        coverage.certified_layer_count
    );
    assert!(features.owned_critical_slab_count <= coverage.slab_count);
    let reopened = decode_bundle(&serde_json::to_vec(&bundle).unwrap()).unwrap();
    assert_eq!(reopened.plan.native_geometry, *coverage);
    assert!(crate::verify_bundle(&reopened, &cancel).unwrap().verified);
}

#[test]
fn disjoint_mixed_height_sources_bind_actual_birth_and_terminal_contributors() {
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    scene.add(FULL_CYLINDER, &[translated(50.0, 0.0, 4.0)]);
    let intent = intent(true);
    let bundle = compile(scene.input(&intent), &cancel).unwrap();
    assert!((bundle.plan.layers.last().unwrap().z_mm - 12.0).abs() < 1e-6);
    assert_eq!(bundle.provenance.definitions, scene.stored);
    assert_eq!(bundle.provenance.occurrences, scene.occurrences);
    for layer in &bundle.plan.layers {
        let box_area = if layer.section_z_mm < 8.0 { 200.0 } else { 0.0 };
        let cylinder_area = if layer.section_z_mm > 4.0 {
            25.0 * std::f64::consts::PI
        } else {
            0.0
        };
        assert!(
            (layer.region_coverage.source_section_area_mm2 - box_area - cylinder_area).abs() < 0.2
        );
    }
    let coverage = &bundle.plan.native_geometry;
    assert_eq!(coverage.height_analysis_count, 2);
    assert!(coverage.slab_count >= coverage.height_analysis_count);
    assert!(coverage.slab_count <= coverage.height_analysis_count * coverage.certified_layer_count);
    assert_eq!(
        coverage.slab_count % coverage.height_analysis_count,
        0,
        "every accepted span captures both original sources, even while one is inactive"
    );
    let features = &coverage.source_feature_coverage;
    assert_eq!(
        features.disjoint_source_layer_count
            + features.invariant_source_union_layer_count
            + features.native_interface_complex_layer_count,
        coverage.certified_layer_count
    );
    assert_eq!(features.invariant_source_union_layer_count, 0);
    assert!(
        features.native_interface_complex_layer_count > 0,
        "inactive source caps own real adjacent material through native interface evidence"
    );
    assert!(coverage.source_feature_coverage.owned_critical_slab_count >= 2);
    let reopened = decode_bundle(&serde_json::to_vec(&bundle).unwrap()).unwrap();
    assert!(crate::verify_bundle(&reopened, &cancel).unwrap().verified);
}

#[test]
fn genuine_native_birth_near_bed_is_not_aliased_to_bed_contact() {
    let cancel = AtomicBool::new(false);
    let scene = NativeScene::new(BOX, &[translated(0.0, 0.0, LAYER_QUANTIZATION_MM * 0.5)]);
    let intent = intent(false);
    let report = preflight(&scene.input(&intent), &cancel).unwrap();
    assert_eq!(report.disposition, PreflightDisposition::Admitted);
    let failure = compile(scene.input(&intent), &cancel).unwrap_err();
    assert_eq!(failure.code, ManufacturingErrorCode::UnsupportedGeometry);
}

#[test]
fn native_nonintegral_top_and_non_grid_shelf_preserve_every_event() {
    for name in ["nonintegral-top", "non-grid-shelf"] {
        let scene = NativeScene::new(&generated(name), &[RigidPoseMm::IDENTITY]);
        let intent = intent(false);
        let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
        assert!(bundle.verification.verified);
        assert!((bundle.plan.layers.last().unwrap().z_mm - 7.85).abs() < 1e-9);
        assert!(
            bundle
                .plan
                .layers
                .iter()
                .all(|layer| layer.height_mm > 0.0
                    && layer.height_mm <= intent.recipe.layer_height_mm)
        );
        if name == "non-grid-shelf" {
            assert!(
                bundle
                    .plan
                    .layers
                    .iter()
                    .any(|layer| (layer.z_mm - 3.15).abs() < 1e-9)
            );
            let before = bundle
                .plan
                .layers
                .iter()
                .find(|layer| (layer.z_mm - 3.15).abs() < 1e-9)
                .unwrap();
            let after = bundle
                .plan
                .layers
                .iter()
                .find(|layer| layer.z_mm > 3.15 + 1e-9)
                .unwrap();
            assert!((before.region_coverage.section_area_mm2 - 200.0).abs() < 1e-6);
            assert!((after.region_coverage.section_area_mm2 - 120.0).abs() < 1e-6);
            assert!(after.height_mm > 0.0 && after.height_mm <= intent.recipe.layer_height_mm);
        }
        assert_eq!(bundle.provenance.definitions, scene.stored);
    }
}

#[test]
fn off_grid_native_tunnel_split_merge_and_near_tangency_preserve_replayed_material() {
    let cancel = AtomicBool::new(false);
    let baseline = NativeScene::new(&generated("nonintegral-top"), &[RigidPoseMm::IDENTITY]);
    let changed = NativeScene::new(
        &generated("off-grid-circular-tunnel"),
        &[RigidPoseMm::IDENTITY],
    );
    // Both native sources have exactly the same ordinary selected sections.
    // The changed original has a through-Y cylindrical void between them:
    // gap(z) = 2*sqrt(0.24^2 - (z-1.37)^2), with real split/merge at
    // 1.13/1.61. This is neither a component birth nor a static overlapping union.
    for z in [1.0, 2.0] {
        let a = original_section_polygons(&baseline, z);
        let b = original_section_polygons(&changed, z);
        assert!(a.xor(&b).unsigned_area() < 1e-6);
    }
    let middle = original_section_polygons(&changed, 1.37);
    assert_eq!(middle.0.len(), 2);
    assert!((middle.unsigned_area() - 195.2).abs() < 1e-5);
    // Approach each smooth cylindrical tangency from the split side. The
    // smallest true gap is below the manufacturing shape-error budget, but
    // that budget never licenses epsilon-merging the two native components.
    for event in [1.13, 1.61] {
        for delta in [0.06, 0.012, 0.003] {
            let z = if event < 1.37 {
                event + delta
            } else {
                event - delta
            };
            let section = original_section_polygons(&changed, z);
            let gap = 2.0 * (0.24f64.powi(2) - (z - 1.37).powi(2)).sqrt();
            assert_eq!(section.0.len(), 2, "event {event}, distance {delta}");
            assert!((section.unsigned_area() - (200.0 - 10.0 * gap)).abs() < 1e-5);
        }
        let outside = if event < 1.37 {
            event - 0.003
        } else {
            event + 0.003
        };
        assert_eq!(original_section_polygons(&changed, outside).0.len(), 1);
        let failure = spiling_geometry::section(
            changed.definitions.values().next().unwrap(),
            PlaneMm {
                origin_mm: [0.0, 0.0, event],
                normal: [0.0, 0.0, 1.0],
            },
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::DegenerateSection);
    }

    let intent = intent(false);
    let unchanged = compile(baseline.input(&intent), &cancel).unwrap();
    let bundle = compile(changed.input(&intent), &cancel).unwrap();
    assert_eq!(bundle.provenance.definitions, changed.stored);
    assert_eq!(
        bundle.plan.native_geometry.evidence,
        PreflightEvidence::Certified
    );
    assert!(
        bundle.plan.native_geometry.achieved_shape_error_mm <= intent.recipe.native_shape_error_mm
    );
    assert_eq!(
        bundle.plan.native_geometry.certified_layer_count,
        bundle.plan.layers.len() as u32
    );
    assert!((bundle.plan.layers.last().unwrap().z_mm - 7.85).abs() < 1e-6);
    for event in [1.13, 1.61] {
        assert!(
            bundle
                .plan
                .layers
                .iter()
                .any(|layer| (layer.z_mm - event).abs() < 1e-6),
            "the actual off-grid split/merge must partition the emitted schedule"
        );
    }
    assert_ne!(
        unchanged
            .plan
            .layers
            .iter()
            .map(|layer| layer.z_mm)
            .collect::<Vec<_>>(),
        bundle
            .plan
            .layers
            .iter()
            .map(|layer| layer.z_mm)
            .collect::<Vec<_>>()
    );
    let mut split_layers = 0;
    for layer in &bundle.plan.layers {
        let half_gap = if (1.13..1.61).contains(&layer.section_z_mm) {
            split_layers += 1;
            (0.24f64.powi(2) - (layer.section_z_mm - 1.37).powi(2)).sqrt()
        } else {
            0.0
        };
        assert!(
            (layer.region_coverage.source_section_area_mm2 - (200.0 - 20.0 * half_gap)).abs()
                < 1e-5
        );
    }
    assert!(
        split_layers > 0,
        "matching endpoint samples cannot hide the native tunnel"
    );
    let mut witnessed = [false; 4];
    visit_fresh_replayed_object_segments(&bundle, |start, end| {
        let layer = bundle
            .plan
            .layers
            .iter()
            .find(|layer| (layer.z_mm - end[2]).abs() <= 0.000001)
            .expect("emitted object stroke belongs to a scheduled layer");
        let sample = layer.section_z_mm;
        if (1.13..1.61).contains(&sample) {
            let half_gap = (0.24f64.powi(2) - (sample - 1.37).powi(2)).sqrt();
            let left = start[0] <= 10.0 - half_gap && end[0] <= 10.0 - half_gap;
            let right = start[0] >= 10.0 + half_gap && end[0] >= 10.0 + half_gap;
            assert!(
                left || right,
                "actual emitted segment crosses the native split void"
            );
            witnessed[1] |= left;
            witnessed[2] |= right;
        } else if start[0].min(end[0]) < 10.0 && start[0].max(end[0]) > 10.0 {
            witnessed[if sample < 1.13 { 0 } else { 3 }] = true;
        }
    });
    assert!(
        witnessed.into_iter().all(|present| present),
        "fresh replay must retain connected→split→connected material"
    );
    visit_fresh_replayed_object_segments(&unchanged, |start, end| {
        for point in [start, end] {
            assert!((0.0..=20.0).contains(&point[0]));
            assert!((0.0..=10.0).contains(&point[1]));
        }
    });
}

#[test]
fn genuine_inward_sloped_planar_ramp_passes_native_compile_and_replay() {
    let scene = NativeScene::new(&generated("inward-planar-ramp"), &[RigidPoseMm::IDENTITY]);
    let intent = intent(false);
    let report = preflight(&scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert_eq!(report.input_revision, ProjectRevision(19));
    assert!(report.disposition == PreflightDisposition::Admitted && report.software_only);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.face_id.is_some() && check.occurrence_id.is_some())
    );
    let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
    for layer in &bundle.plan.layers {
        let expected = (20.0 - 4.0 * layer.section_z_mm / 7.85) * 10.0;
        assert!(
            (layer.region_coverage.section_area_mm2 - expected).abs() < 1e-5,
            "native ramp layer {} sample {:.12} height {:.12}: actual area {:.12}, analytic {:.12}, delta {:.12}; perimeter {:?}",
            layer.index,
            layer.section_z_mm,
            layer.height_mm,
            layer.region_coverage.section_area_mm2,
            expected,
            layer.region_coverage.section_area_mm2 - expected,
            layer.paths.first()
        );
    }
    assert!(bundle.verification.verified);
    assert_eq!(bundle.verification.support_deposited_volume_mm3, 0.0);
    assert_eq!(bundle.provenance.definitions, scene.stored);
}

#[test]
fn tilted_native_cylinder_uses_analytic_extrema_and_native_sections() {
    let bytes = include_bytes!("../../../../fixtures/geometry/cylinder.step");
    let native = spiling_geometry::import_step(bytes, &AtomicBool::new(false)).unwrap();
    let angle = 1.0f64.to_radians();
    let component = (angle * 0.5).sin() / 2.0f64.sqrt();
    let mut pose = RigidPoseMm {
        translation_mm: [0.0; 3],
        rotation_xyzw: [component, component, 0.0, (angle * 0.5).cos()],
    };
    let mut work = spiling_geometry::GeometryWorkBudget::new(20_000_000);
    let bounds = spiling_geometry::projected_bounds(
        &native,
        rotate_inverse(&pose, [0.0, 0.0, 1.0]),
        spiling_geometry::BoundsPolicy {
            maximum_endpoint_uncertainty_mm: 0.000001,
        },
        &mut work,
        &AtomicBool::new(false),
    )
    .unwrap();
    let expected_min = -5.0 * angle.sin();
    let expected_max = 8.0 * angle.cos() + 5.0 * angle.sin();
    assert!(bounds.minimum[0] <= expected_min + 1e-12 && bounds.minimum[1] >= expected_min - 1e-12);
    assert!(bounds.maximum[0] <= expected_max + 1e-12 && bounds.maximum[1] >= expected_max - 1e-12);
    assert!(bounds.achieved_uncertainty_mm <= 0.000001);
    assert!(work.consumed() > 0);
    let extent = [bounds.minimum[0], bounds.maximum[1]];
    // Bed placement uses the safe lower bracket, never an assumed exact contact.
    pose.translation_mm[2] = -extent[0];
    let scene = NativeScene::new(bytes, &[pose]);
    let mut intent = intent(false);
    intent.recipe.layer_height_mm = (extent[1] - extent[0]) / 20.0;
    let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert!(
        (bundle.plan.layers.last().unwrap().z_mm - (expected_max - extent[0])).abs() <= 0.000001
    );
    assert!(bundle.verification.verified);
    assert_eq!(bundle.provenance.definitions, scene.stored);
}

#[test]
fn sparse_native_roof_is_a_bounded_bridge_not_full_material_support() {
    let scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut intent = intent(false);
    intent.recipe.infill_fraction = 0.2;
    intent.recipe.process.max_bridge_span_mm = 6.0;
    let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert!(bundle.plan.layers.iter().any(|layer| {
        layer.region_coverage.top_skin_area_mm2 > 0.0
            && layer
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::Bridge)
    }));
    assert_eq!(bundle.verification.support_deposited_volume_mm3, 0.0);
    intent.recipe.process.max_bridge_span_mm = 0.5;
    let error = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::UnsupportedGeometry);
}

#[test]
fn native_cantilever_emits_bed_anchored_support_and_separate_volume() {
    let scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY, translated(10.0, 0.0, 8.0)]);
    let intent = intent(true);
    let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert!(bundle.verification.support_deposited_volume_mm3 > 0.0);
    assert!(bundle.verification.support_deposition_segments > 0);
    let top = bundle
        .plan
        .layers
        .iter()
        .filter(|layer| {
            layer
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::SupportInterface)
        })
        .map(|layer| layer.z_mm)
        .fold(0.0f64, f64::max);
    assert!(top < 8.0);
    assert!(actual_fixture_contact_gap(&bundle, 8.0) >= 0.2);
    let interface_height: f64 = bundle
        .plan
        .layers
        .iter()
        .filter(|layer| {
            layer
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::SupportInterface)
        })
        .map(|layer| layer.height_mm)
        .sum();
    assert!((interface_height - 0.8).abs() < 1e-8);
    for layer in bundle.plan.layers.iter().filter(|layer| layer.z_mm <= 8.0) {
        for point in layer
            .paths
            .iter()
            .filter(|path| {
                matches!(
                    path.kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                )
            })
            .flat_map(|path| &path.points_mm)
        {
            // Nominal support bead is excluded from lower object plus XY gap.
            assert!(point[0] - intent.recipe.bead_width_mm * 0.5 >= 20.2 - 0.011);
        }
    }
    let support_nominal: f64 = bundle
        .plan
        .layers
        .iter()
        .map(|layer| layer.region_coverage.support_nominal_deposition_volume_mm3)
        .sum();
    assert!((bundle.verification.support_deposited_volume_mm3 - support_nominal).abs() < 0.02);
    let object_nominal: f64 = bundle
        .plan
        .layers
        .iter()
        .map(|layer| layer.region_coverage.nominal_deposition_volume_mm3)
        .sum();
    assert!((bundle.verification.deposited_volume_mm3 - object_nominal).abs() < 0.02);
    assert_eq!(bundle.provenance.definitions, scene.stored);
    let disabled = self::intent(false);
    assert_eq!(
        compile(scene.input(&disabled), &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
}

#[test]
fn authored_sparse_cantilever_partial_skin_uses_actual_endpoint_footprints() {
    let scene = NativeScene::new(
        BOX,
        &[translated(40.0, 40.0, 0.0), translated(50.0, 40.0, 8.0)],
    );
    let mut intent: ManufacturingIntent = serde_json::from_slice(include_bytes!(
        "../../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert!(bundle.verification.verified);
    assert!(bundle.plan.layers.iter().any(|layer| {
        layer.z_mm < 8.0
            && layer.region_coverage.top_skin_area_mm2 > 0.0
            && layer
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::Bridge)
    }));
    assert!(bundle.verification.support_deposited_volume_mm3 > 0.0);
    let interface: Vec<_> = bundle
        .plan
        .layers
        .iter()
        .filter(|layer| {
            layer
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::SupportInterface)
        })
        .collect();
    assert!(interface.last().unwrap().z_mm < 8.0);
    assert!(actual_fixture_contact_gap(&bundle, 8.0) >= 0.2);
    assert!((interface.iter().map(|layer| layer.height_mm).sum::<f64>() - 0.4).abs() < 1e-8);
    for point in bundle
        .plan
        .layers
        .iter()
        .flat_map(|layer| &layer.paths)
        .filter(|path| {
            matches!(
                path.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            )
        })
        .flat_map(|path| &path.points_mm)
    {
        assert!(point[0] - intent.recipe.bead_width_mm * 0.5 >= 60.3 - 0.011);
    }
    let nominal: f64 = bundle
        .plan
        .layers
        .iter()
        .map(|layer| layer.region_coverage.support_nominal_deposition_volume_mm3)
        .sum();
    assert!((bundle.verification.support_deposited_volume_mm3 - nominal).abs() < 0.02);
    assert_eq!(bundle.provenance.definitions, scene.stored);
    intent.recipe.supports = SupportPolicy::Disabled;
    assert_eq!(
        compile(scene.input(&intent), &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
}

#[test]
fn floating_native_body_has_real_support_only_layers_and_conservative_minimum_contact_gap() {
    let scene = NativeScene::new(BOX, &[translated(0.0, 0.0, 2.0)]);
    let enabled = intent(true);
    let bundle = compile(scene.input(&enabled), &AtomicBool::new(false)).unwrap();
    let first = &bundle.plan.layers[0];
    assert_eq!(first.region_coverage.section_area_mm2, 0.0);
    assert_eq!(first.region_coverage.nominal_deposition_volume_mm3, 0.0);
    assert!(first.region_coverage.support_nominal_deposition_volume_mm3 > 0.0);
    let interface_height: f64 = bundle
        .plan
        .layers
        .iter()
        .filter(|layer| {
            layer
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::SupportInterface)
        })
        .map(|layer| layer.height_mm)
        .sum();
    assert!((interface_height - 0.8).abs() < 1e-8);
    let empty: Vec<_> = bundle
        .plan
        .layers
        .iter()
        .filter(|layer| layer.paths.is_empty())
        .collect();
    assert!(empty.iter().map(|layer| layer.height_mm).sum::<f64>() >= 0.2);
    assert!(actual_fixture_contact_gap(&bundle, 2.0) >= 0.2);
    let report = preflight(&scene.input(&intent(false)), &AtomicBool::new(false)).unwrap();
    assert_eq!(report.disposition, PreflightDisposition::Refused);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "floating_without_support")
    );
}

#[test]
fn fractional_native_contact_preserves_minimum_gap_in_fresh_actual_xyz6_replay() {
    let cancel = AtomicBool::new(false);
    let scene = NativeScene::new(BOX, &[translated(0.0, 0.0, 2.0000004)]);
    let mut enabled = intent(true);
    enabled.recipe.layer_height_mm = 0.2;
    let SupportPolicy::Breakaway { z_gap_mm, .. } = &mut enabled.recipe.supports else {
        panic!()
    };
    // Preserve a non-micrometre minimum across independently rounded endpoints.
    *z_gap_mm = 0.2000006;
    let z_gap_mm = *z_gap_mm;
    let bundle = compile(scene.input(&enabled), &cancel).unwrap();
    let hash =
        spiling_contracts::geometry::SourceHash::from_bytes(&serde_json::to_vec(&bundle).unwrap());
    let inspection =
        crate::inspect_bundle(&bundle, &hash, VerificationOutput::ReportAndMotion, &cancel)
            .unwrap();
    assert!(inspection.report.verified);
    let replay = inspection.replay.unwrap();
    let mut object_bottom = f64::INFINITY;
    let mut support_top = f64::NEG_INFINITY;
    let mut interface_segments = 0usize;
    for record in replay.bytes[REPLAY_HEADER_BYTES..]
        .as_chunks::<REPLAY_RECORD_BYTES>()
        .0
    {
        let z = f64::from_le_bytes(record[16..24].try_into().unwrap());
        let role = record[46];
        if matches!(role, 9 | 10) {
            support_top = support_top.max(z);
            interface_segments += usize::from(role == 10);
        } else if matches!(role, 1..=5 | 8) {
            let layer = u16::from_le_bytes(record[44..46].try_into().unwrap()) as usize;
            object_bottom = object_bottom.min(z - bundle.plan.layers[layer].height_mm);
        }
    }
    assert!(interface_segments > 0);
    assert!(object_bottom.is_finite() && support_top.is_finite());
    let actual_gap = object_bottom - support_top;
    assert!(
        actual_gap >= z_gap_mm,
        "actual XYZ6 gap {actual_gap:.12} must not undershoot minimum {z_gap_mm:.12}"
    );
    assert!(
        actual_gap - z_gap_mm < 4.0 * LAYER_QUANTIZATION_MM,
        "emitter-aware mandatory contact must not unnecessarily lose a whole scheduled layer"
    );
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .any(|layer| layer.paths.is_empty())
    );
    assert!(bundle.plan.layers[0].paths.iter().any(|path| matches!(
        path.kind,
        DepositionKind::Support | DepositionKind::SupportInterface
    )));
}

#[test]
fn native_floating_through_hole_keeps_actual_emitted_capsules_outside_the_unchanged_gap() {
    let cancel = AtomicBool::new(false);
    let shift = [0.00000037, 0.00000019];
    let mut scene = NativeScene::new(HOLE, &[translated(shift[0], shift[1], 2.0)]);
    // Another source really reaches the bed: global scene Z bounds cannot
    // stand in for this floating source's endpoint-sheet occupancy scope.
    scene.add(BOX, &[translated(30.0, 0.0, 0.0)]);
    let enabled = intent(true);
    let bundle = compile(scene.input(&enabled), &cancel).unwrap();
    assert!(bundle.verification.support_deposition_segments > 0);
    let hash =
        spiling_contracts::geometry::SourceHash::from_bytes(&serde_json::to_vec(&bundle).unwrap());
    let inspection =
        crate::inspect_bundle(&bundle, &hash, VerificationOutput::ReportAndMotion, &cancel)
            .unwrap();
    assert!(inspection.report.verified);
    let replay = inspection.replay.unwrap();
    let SupportPolicy::Breakaway {
        xy_clearance_mm,
        z_gap_mm,
        ..
    } = enabled.recipe.supports
    else {
        panic!()
    };
    let required = 3.0 + enabled.recipe.bead_width_mm * 0.5 + xy_clearance_mm;
    let mut previous: Option<[f64; 3]> = None;
    let mut checked = 0usize;
    let mut supports = Vec::new();
    let mut objects = Vec::new();
    for record in replay.bytes[REPLAY_HEADER_BYTES..]
        .as_chunks::<REPLAY_RECORD_BYTES>()
        .0
    {
        let end: [f64; 3] = std::array::from_fn(|axis| {
            f64::from_le_bytes(record[axis * 8..axis * 8 + 8].try_into().unwrap())
        });
        let role = record[46];
        let layer = u16::from_le_bytes(record[44..46].try_into().unwrap()) as usize;
        if matches!(role, 9 | 10) {
            let start =
                previous.expect("actual support deposition has an actual preceding endpoint");
            supports.push((start, end, layer));
            let a = [start[0] - 10.0 - shift[0], start[1] - 10.0 - shift[1]];
            let d = [end[0] - start[0], end[1] - start[1]];
            let squared_length = d[0] * d[0] + d[1] * d[1];
            let t = if squared_length == 0.0 {
                0.0
            } else {
                (-(a[0] * d[0] + a[1] * d[1]) / squared_length).clamp(0.0, 1.0)
            };
            let distance = (a[0] + t * d[0]).hypot(a[1] + t * d[1]);
            assert!(
                distance >= required - 1e-10,
                "actual XYZ6 support capsule violates native hole radius + nominal bead radius + unchanged XY gap"
            );
            checked += 1;
        }
        if matches!(role, 1..=5 | 8) {
            objects.push((
                previous.expect("actual object deposition has an actual preceding endpoint"),
                end,
                layer,
            ));
        }
        previous = Some(end);
    }
    assert!(checked > 0);
    let mut checked_pairs = 0usize;
    for (a, b, support_layer) in supports {
        let support_bottom = a[2].min(b[2]) - bundle.plan.layers[support_layer].height_mm;
        for &(c, d, object_layer) in &objects {
            let object_bottom = c[2].min(d[2]) - bundle.plan.layers[object_layer].height_mm;
            if object_bottom - a[2].max(b[2]) >= z_gap_mm
                || support_bottom - c[2].max(d[2]) >= z_gap_mm
            {
                continue;
            }
            // This fixture's object inside the required Z-gap neighborhood is
            // the separate bed box, including non-overlapping bead intervals.
            // Prove segment disjointness independently before using the four
            // endpoint-to-segment minima as an exact full-segment oracle.
            assert!(a[0].max(b[0]) < c[0].min(d[0]) || c[0].max(d[0]) < a[0].min(b[0]));
            let point_segment = |p: [f64; 3], a: [f64; 3], b: [f64; 3]| {
                let delta = [b[0] - a[0], b[1] - a[1]];
                let length = delta[0] * delta[0] + delta[1] * delta[1];
                let t = if length == 0.0 {
                    0.0
                } else {
                    (((p[0] - a[0]) * delta[0] + (p[1] - a[1]) * delta[1]) / length).clamp(0.0, 1.0)
                };
                (p[0] - a[0] - t * delta[0]).hypot(p[1] - a[1] - t * delta[1])
            };
            let distance = point_segment(a, c, d)
                .min(point_segment(b, c, d))
                .min(point_segment(c, a, b))
                .min(point_segment(d, a, b));
            assert!(
                distance >= enabled.recipe.bead_width_mm + xy_clearance_mm - 1e-10,
                "actual emitted support/object nominal capsules violate both radii + unchanged XY gap"
            );
            checked_pairs += 1;
        }
    }
    assert!(checked_pairs > 0);
}

#[test]
fn preflight_has_bounded_actionable_refs_pending_native_status_and_cancellation() {
    let scene = NativeScene::new(BOX, &[translated(1000.0, 0.0, 0.0)]);
    let intent = intent(false);
    let report = preflight(&scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert!(report.disposition == PreflightDisposition::Refused && report.software_only);
    assert!(report.checks.len() <= 256);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "outside_envelope" && check.occurrence_id.is_some())
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "native_analysis_pending")
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "physical_qualification_absent")
    );
    assert_eq!(
        preflight(&scene.input(&intent), &AtomicBool::new(true))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::Cancelled
    );
}

#[test]
fn mixed_anchor_native_perimeter_remains_closed_under_bridge_process() {
    let scene = NativeScene::new(
        BOX,
        &[
            RigidPoseMm::IDENTITY,
            translated(23.0, 0.0, 0.0),
            translated(11.0, 0.0, 8.0),
        ],
    );
    let bundle = compile(scene.input(&intent(true)), &AtomicBool::new(false)).unwrap();
    assert_eq!(bundle.verification.support_deposited_volume_mm3, 0.0);
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .flat_map(|layer| &layer.paths)
            .any(|path| path.kind == DepositionKind::Bridge
                && path.points_mm.first() == path.points_mm.last()),
        "first upper-layer object contours: {:?}",
        bundle
            .plan
            .layers
            .iter()
            .filter(|layer| layer.z_mm > 8.0 && layer.z_mm <= 8.5)
            .flat_map(|layer| layer
                .paths
                .iter()
                .filter(|path| !matches!(
                    path.kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                ))
                .filter(|path| path.points_mm.first() == path.points_mm.last())
                .map(move |path| (
                    layer.index,
                    path.kind,
                    &path.points_mm[..path.points_mm.len().min(12)]
                )))
            .take(12)
            .collect::<Vec<_>>()
    );
    for path in bundle
        .plan
        .layers
        .iter()
        .flat_map(|layer| &layer.paths)
        .filter(|path| path.kind == DepositionKind::Perimeter)
    {
        assert_eq!(path.points_mm.first(), path.points_mm.last());
    }
}

#[test]
fn blocked_native_component_is_not_certified_by_nearby_valid_support_column() {
    let mut scene = NativeScene::new(HOLE, &[RigidPoseMm::IDENTITY]);
    scene.add(
        BOX,
        &[translated(0.0, 0.0, 10.0), translated(30.0, 0.0, 10.0)],
    );
    let error = compile(scene.input(&intent(true)), &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::UnsupportedGeometry);
}

#[test]
fn affine_rational_native_source_is_admitted_and_replayed_without_family_fallback() {
    use spiling_contracts::geometry::SurfaceFamily;
    let bytes = include_bytes!("../../../../fixtures/geometry/unsupported-nurbs.step");
    let scene = NativeScene::new(bytes, &[RigidPoseMm::IDENTITY]);
    assert!(
        scene
            .definitions
            .values()
            .flat_map(|native| native.faces())
            .any(|face| face.geometry.family == SurfaceFamily::Nurbs)
    );
    let intent = intent(false);
    let cancel = AtomicBool::new(false);
    assert_eq!(
        preflight(&scene.input(&intent), &cancel)
            .unwrap()
            .disposition,
        PreflightDisposition::Admitted
    );
    let bundle = compile(scene.input(&intent), &cancel).unwrap();
    assert_eq!(bundle.provenance.definitions, scene.stored);
    assert!(!bundle.plan.layers.is_empty());
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .any(|layer| !layer.paths.is_empty())
    );
    assert!(crate::verify_bundle(&bundle, &cancel).unwrap().verified);
}

#[test]
fn balanced_native_top_is_preserved_and_layer_resource_limits_are_explicit() {
    let scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut intent = intent(false);
    intent.recipe.layer_height_mm = 0.399995;
    let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert_eq!(bundle.plan.layers.last().unwrap().z_mm, 8.0);
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .all(|layer| layer.height_mm <= 0.399995)
    );
    intent.recipe.layer_height_mm = 0.0001;
    assert_eq!(
        compile(scene.input(&intent), &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn genuine_native_cantilever_clearance_and_floating_gap_limits_are_explicit() {
    let scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY, translated(10.0, 0.0, 8.0)]);
    let mut enabled = intent(true);
    enabled.recipe.supports = SupportPolicy::Breakaway {
        density: 0.3,
        xy_clearance_mm: 10.0,
        z_gap_mm: 0.2,
        interface_thickness_mm: 0.8,
    };
    let unresolved = compile(scene.input(&enabled), &AtomicBool::new(false)).unwrap_err();
    assert_eq!(
        unresolved.code,
        ManufacturingErrorCode::NativeAnalysisIndeterminate
    );
    let floating = NativeScene::new(BOX, &[translated(0.0, 0.0, 2.0)]);
    enabled.recipe.supports = SupportPolicy::Breakaway {
        density: 0.3,
        xy_clearance_mm: 0.2,
        z_gap_mm: 3.0,
        interface_thickness_mm: 0.8,
    };
    assert!(compile(floating.input(&enabled), &AtomicBool::new(false)).is_err());
}

fn rib_layer(scene: &NativeScene, id: u32) -> spiling_contracts::surface::SurfaceLayer {
    use spiling_contracts::surface::{SurfaceFinish, SurfaceLayer, SurfaceLayerId, SurfaceScope};
    let occurrence = &scene.occurrences[0];
    let native = &scene.definitions[&occurrence.definition_id];
    let face = native
        .faces()
        .iter()
        .find(|face| {
            native
                .vertical_wall_face(&face.face_id)
                .is_ok_and(|cert| cert.width_mm >= 8.0)
        })
        .expect("fixture must contain a genuinely certified full-height rectangular exterior face");
    SurfaceLayer {
        layer_id: SurfaceLayerId::new(id).unwrap(),
        enabled: true,
        scope: SurfaceScope::Definition {
            definition_id: occurrence.definition_id.clone(),
        },
        face_id: face.face_id.clone(),
        finish: SurfaceFinish::VerticalRibs {
            depth_mm: 0.1,
            pitch_mm: 2.4,
            edge_margin_mm: 1.2,
            phase_mm: 0.0,
        },
    }
}

fn assert_native_surface_retention(
    source: &MultiPolygon<f64>,
    derived: &MultiPolygon<f64>,
    native: &spiling_geometry::Definition,
    pose: &RigidPoseMm,
    sample: f64,
    layers: &[spiling_contracts::surface::SurfaceLayer],
) {
    use geo::{Intersects, Point};
    use spiling_contracts::surface::SurfaceFinish;
    let certificates = layers
        .iter()
        .map(|layer| {
            (
                native.vertical_wall_face(&layer.face_id).unwrap(),
                &layer.finish,
            )
        })
        .collect::<Vec<_>>();
    let mut profile_vertices = 0usize;
    for (original, finished) in source.0.iter().zip(&derived.0) {
        assert_eq!(
            original.interiors(),
            finished.interiors(),
            "native hole coordinates changed"
        );
        for point in &original.exterior().0 {
            assert!(
                finished.intersects(&Point(*point)),
                "original native material point was excluded"
            );
            let local = inverse(pose, [point.x, point.y, sample]);
            let active = certificates.iter().any(|(cert, finish)| {
                let SurfaceFinish::VerticalRibs { edge_margin_mm, .. } = finish else {
                    return false;
                };
                let delta = [local[0] - cert.origin_mm[0], local[1] - cert.origin_mm[1]];
                let s = delta[0] * cert.horizontal_direction[0]
                    + delta[1] * cert.horizontal_direction[1];
                let outward = delta[0] * cert.outward_normal[0] + delta[1] * cert.outward_normal[1];
                outward.abs() <= 1e-7 && s > *edge_margin_mm && s < cert.width_mm - edge_margin_mm
            });
            if !active {
                assert!(
                    finished.exterior().0.contains(point),
                    "untargeted/protected native vertex changed"
                );
            }
        }
        for point in &finished.exterior().0 {
            if original.exterior().0.contains(point) {
                continue;
            }
            let local = inverse(pose, [point.x, point.y, sample]);
            assert!(
                certificates.iter().any(|(cert, finish)| {
                    let SurfaceFinish::VerticalRibs {
                        depth_mm,
                        pitch_mm,
                        edge_margin_mm,
                        phase_mm,
                    } = finish
                    else {
                        return false;
                    };
                    let delta = [local[0] - cert.origin_mm[0], local[1] - cert.origin_mm[1]];
                    let s = delta[0] * cert.horizontal_direction[0]
                        + delta[1] * cert.horizontal_direction[1];
                    let outward =
                        delta[0] * cert.outward_normal[0] + delta[1] * cert.outward_normal[1];
                    let edge = (s - edge_margin_mm).min(cert.width_mm - edge_margin_mm - s);
                    let taper = if edge <= 0.0 {
                        0.0
                    } else if edge >= pitch_mm / 2.0 {
                        1.0
                    } else {
                        0.5 * (1.0 - (std::f64::consts::TAU * edge / pitch_mm).cos())
                    };
                    let expected = depth_mm
                        * 0.5
                        * (1.0 - (std::f64::consts::TAU * (s - phase_mm) / pitch_mm).cos())
                        * taper;
                    s >= edge_margin_mm - 1e-5
                        && s <= cert.width_mm - edge_margin_mm + 1e-5
                        && outward >= -1e-5
                        && (outward - expected).abs() <= 1e-5
                }),
                "constructed chain sample changed the source-local waveform/phase"
            );
            profile_vertices += 1;
        }
        // Vertex agreement alone cannot detect an under-sampled curve. Check
        // the manufactured polygon's chords between samples against the
        // requested source-local relief, including both taper transitions.
        for edge in finished.exterior().0.windows(2) {
            for (cert, finish) in &certificates {
                let SurfaceFinish::VerticalRibs {
                    depth_mm,
                    pitch_mm,
                    edge_margin_mm,
                    phase_mm,
                } = finish
                else {
                    continue;
                };
                let local_edge = [edge[0], edge[1]].map(|point| {
                    let local = inverse(pose, [point.x, point.y, sample]);
                    let delta = [local[0] - cert.origin_mm[0], local[1] - cert.origin_mm[1]];
                    (
                        delta[0] * cert.horizontal_direction[0]
                            + delta[1] * cert.horizontal_direction[1],
                        delta[0] * cert.outward_normal[0] + delta[1] * cert.outward_normal[1],
                    )
                });
                if local_edge.iter().any(|(s, outward)| {
                    *s < -1e-5
                        || *s > cert.width_mm + 1e-5
                        || *outward < -1e-5
                        || *outward > depth_mm + 1e-5
                }) {
                    continue;
                }
                for fraction in [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875] {
                    let s = local_edge[0].0 + fraction * (local_edge[1].0 - local_edge[0].0);
                    let outward = local_edge[0].1 + fraction * (local_edge[1].1 - local_edge[0].1);
                    let end_distance = (s - edge_margin_mm).min(cert.width_mm - edge_margin_mm - s);
                    let taper = if end_distance <= 0.0 {
                        0.0
                    } else if end_distance >= pitch_mm / 2.0 {
                        1.0
                    } else {
                        0.5 * (1.0 - (std::f64::consts::TAU * end_distance / pitch_mm).cos())
                    };
                    let requested = depth_mm
                        * 0.5
                        * (1.0 - (std::f64::consts::TAU * (s - phase_mm) / pitch_mm).cos())
                        * taper;
                    assert!(
                        (outward - requested).abs() <= 0.001,
                        "realized rib chord exceeds its declared 0.001 mm approximation budget"
                    );
                }
            }
        }
    }
    assert!(profile_vertices > 8, "no genuine sampled rib profile");
}

#[test]
fn certified_native_ribs_change_closed_perimeters_actual_replay_and_added_accounting() {
    use crate::{inspect_bundle, verify_bundle};
    use spiling_contracts::geometry::SourceHash;
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let intent = intent(false);
    let original = compile(scene.input(&intent), &cancel).unwrap();
    let layer = rib_layer(&scene, 1);
    let face_id = layer.face_id.clone();
    scene.surface_overlays.layers.push(layer);
    let finished = compile(scene.input(&intent), &cancel).unwrap();
    assert_ne!(original.program, finished.program);
    assert_eq!(
        finished.provenance.definitions,
        original.provenance.definitions
    );
    assert_eq!(finished.provenance.surface_overlays, scene.surface_overlays);
    let native = &scene.definitions[&scene.occurrences[0].definition_id];
    let cert = native.vertical_wall_face(&face_id).unwrap();
    let (admitted, _) = preflight::admit(&scene.input(&intent), &cancel).unwrap();
    let sample = finished.plan.layers[0].section_z_mm;
    let native_section = spiling_geometry::section_with_byte_limit(
        native,
        PlaneMm {
            origin_mm: [0.0, 0.0, sample],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
        4 * 1024 * 1024,
    )
    .unwrap();
    let source =
        region::from_native(&native_section, &RigidPoseMm::IDENTITY, sample, &cancel).unwrap();
    let derived = surface::realize(
        source.clone(),
        &admitted.surface_fields[&scene.occurrences[0].occurrence_id],
        sample,
        0,
        &mut 0,
        &cancel,
    )
    .unwrap();
    assert_native_surface_retention(
        &source,
        &derived,
        native,
        &RigidPoseMm::IDENTITY,
        sample,
        &scene.surface_overlays.layers,
    );
    assert_eq!(source.0.len(), derived.0.len());
    let added = derived.difference(&source);
    assert!(added.unsigned_area() > 0.0);
    for point in added.0.iter().flat_map(|polygon| &polygon.exterior().0) {
        let dx = point.x - cert.origin_mm[0];
        let dy = point.y - cert.origin_mm[1];
        let s = dx * cert.horizontal_direction[0] + dy * cert.horizontal_direction[1];
        let outward = dx * cert.outward_normal[0] + dy * cert.outward_normal[1];
        assert!(
            s >= 1.2 - 1e-6 && s <= cert.width_mm - 1.2 + 1e-6,
            "protected corner band was filled"
        );
        assert!(
            (-1e-6..=0.1 + 1e-6).contains(&outward),
            "addition exceeded the outward-only profile bounds"
        );
    }
    let projected = |point: [f64; 3]| {
        (point[0] - cert.origin_mm[0]) * cert.outward_normal[0]
            + (point[1] - cert.origin_mm[1]) * cert.outward_normal[1]
    };
    for (plain, textured) in original.plan.layers.iter().zip(&finished.plan.layers) {
        let coverage = &textured.region_coverage;
        assert!((coverage.source_section_area_mm2 - 200.0).abs() < 1e-7);
        assert!(coverage.surface_added_area_mm2 > 0.0);
        assert!(
            (coverage.section_area_mm2
                - coverage.source_section_area_mm2
                - coverage.surface_added_area_mm2)
                .abs()
                < 1e-9
        );
        let perimeter = textured
            .paths
            .iter()
            .find(|p| p.kind == DepositionKind::Perimeter)
            .unwrap();
        assert_eq!(perimeter.points_mm.first(), perimeter.points_mm.last());
        assert_ne!(
            perimeter.points_mm,
            plain
                .paths
                .iter()
                .find(|p| p.kind == DepositionKind::Perimeter)
                .unwrap()
                .points_mm
        );
        let peak = perimeter
            .points_mm
            .iter()
            .copied()
            .map(projected)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((peak - (0.1 - intent.recipe.bead_width_mm / 2.0)).abs() <= 0.006);
        let area = (intent.recipe.bead_width_mm - textured.height_mm) * textured.height_mm
            + std::f64::consts::PI * (textured.height_mm / 2.0).powi(2);
        let volume = textured
            .paths
            .iter()
            .flat_map(|path| path.points_mm.windows(2))
            .map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]) * area)
            .sum::<f64>();
        assert!((coverage.nominal_deposition_volume_mm3 - volume).abs() < 1e-7);
    }
    let bytes = serde_json::to_vec(&finished).unwrap();
    let hash = SourceHash::from_bytes(&bytes);
    let inspection = inspect_bundle(
        &finished,
        &hash,
        VerificationOutput::ReportAndMotion,
        &cancel,
    )
    .unwrap();
    let replay = inspection.replay.unwrap();
    let peak = replay.bytes[REPLAY_HEADER_BYTES..]
        .as_chunks::<REPLAY_RECORD_BYTES>()
        .0
        .iter()
        .filter(|record| record[46] == 1)
        .map(|record| {
            projected(std::array::from_fn(|axis| {
                f64::from_le_bytes(record[axis * 8..axis * 8 + 8].try_into().unwrap())
            }))
        })
        .fold(f64::NEG_INFINITY, f64::max);
    assert!((peak - (0.1 - intent.recipe.bead_width_mm / 2.0)).abs() <= 0.006001);
    let decoded = decode_bundle(&bytes).unwrap();
    assert_eq!(decoded.plan, finished.plan);
    assert_eq!(
        verify_bundle(&decoded, &cancel).unwrap(),
        finished.verification
    );
    for field in ['X', 'E'] {
        let mut mutated = decoded.clone();
        let line = mutated
            .program
            .lines()
            .find(|line| line.starts_with("G1 ") && line.contains(" X"))
            .unwrap();
        let altered = line
            .split_whitespace()
            .map(|token| {
                if let Some(value) = token.strip_prefix(field) {
                    format!("{field}{:.8}", value.parse::<f64>().unwrap() + 0.02)
                } else {
                    token.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        mutated.program = mutated.program.replacen(line, &altered, 1);
        assert_eq!(
            verify_bundle(&mutated, &cancel).unwrap_err().code,
            ManufacturingErrorCode::VerificationFailed
        );
    }
}

#[test]
fn native_surface_scope_smooth_disable_and_expanded_bounds_are_real_consumers() {
    use spiling_contracts::surface::{SurfaceFinish, SurfaceScope};
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let intent = intent(false);
    let original = compile(scene.input(&intent), &cancel).unwrap();
    let definition_ribs = rib_layer(&scene, 2);
    let mut smooth = rib_layer(&scene, 1);
    smooth.scope = SurfaceScope::Occurrence {
        occurrence_id: scene.occurrences[0].occurrence_id,
        expected_definition_id: scene.occurrences[0].definition_id.clone(),
    };
    smooth.finish = SurfaceFinish::Smooth;
    // Occurrence precedence is independent of relative stored array position.
    scene.surface_overlays.layers = vec![smooth, definition_ribs];
    let overridden = compile(scene.input(&intent), &cancel).unwrap();
    assert_eq!(overridden.plan.layers, original.plan.layers);
    assert_ne!(
        overridden.provenance.input_hash,
        original.provenance.input_hash
    );
    scene.surface_overlays.layers[0].enabled = false;
    let revealed = compile(scene.input(&intent), &cancel).unwrap();
    assert!(
        revealed.plan.layers[0]
            .region_coverage
            .surface_added_area_mm2
            > 0.0
    );
    scene.surface_overlays.layers.remove(0);
    let removed = compile(scene.input(&intent), &cancel).unwrap();
    assert_eq!(removed.plan.layers, revealed.plan.layers);
    assert_ne!(
        removed.provenance.input_hash,
        revealed.provenance.input_hash
    );
    let mut exact_envelope = intent.clone();
    exact_envelope.printer.build_envelope =
        AabbMm::new([0.0, 0.0, 0.0], [20.0, 10.0, 8.0]).unwrap();
    let report = preflight(&scene.input(&exact_envelope), &cancel).unwrap();
    assert_eq!(report.disposition, PreflightDisposition::Refused);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "outside_envelope")
    );
    assert_eq!(
        compile(scene.input(&exact_envelope), &cancel)
            .unwrap_err()
            .code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
    scene.surface_overlays.layers[0].finish = SurfaceFinish::VerticalRibs {
        depth_mm: 0.1,
        pitch_mm: 1.0,
        edge_margin_mm: 1.2,
        phase_mm: 0.0,
    };
    assert_eq!(
        compile(scene.input(&intent), &cancel).unwrap_err().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
}

#[test]
fn native_surface_pose_holes_and_other_occurrence_exclusion_preserve_authority() {
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(HOLE, &[RigidPoseMm::IDENTITY]);
    scene.surface_overlays.layers.push(rib_layer(&scene, 1));
    let intent = intent(false);
    let finished = compile(scene.input(&intent), &cancel).unwrap();
    assert!(
        finished.plan.layers[0]
            .region_coverage
            .protected_hole_area_mm2
            > 0.0
    );
    let source_area = finished.plan.layers[0]
        .region_coverage
        .source_section_area_mm2;
    scene.occurrences[0].pose = translated(23.0, 17.0, 0.0);
    let moved = compile(scene.input(&intent), &cancel).unwrap();
    assert!(
        (moved.plan.layers[0].region_coverage.source_section_area_mm2 - source_area).abs() < 1e-7
    );
    assert!(
        (moved.plan.layers[0].region_coverage.surface_added_area_mm2
            - finished.plan.layers[0]
                .region_coverage
                .surface_added_area_mm2)
            .abs()
            < 1e-5
    );
    // Compare inverse-translated contour vertices, not source-only path counts.
    for path in &finished.plan.layers[0].paths {
        if path.kind != DepositionKind::Perimeter {
            continue;
        }
        let counterpart = moved.plan.layers[0]
            .paths
            .iter()
            .find(|p| {
                p.kind == path.kind
                    && p.points_mm.len() == path.points_mm.len()
                    && (p.points_mm[0][0] - 23.0 - path.points_mm[0][0]).abs() < 1e-5
                    && (p.points_mm[0][1] - 17.0 - path.points_mm[0][1]).abs() < 1e-5
            })
            .unwrap();
        for (a, b) in path.points_mm.iter().zip(&counterpart.points_mm) {
            assert!((a[0] - (b[0] - 23.0)).abs() < 1e-5 && (a[1] - (b[1] - 17.0)).abs() < 1e-5);
        }
    }
    let quarter = std::f64::consts::FRAC_1_SQRT_2;
    scene.occurrences[0].pose.rotation_xyzw = [0.0, 0.0, quarter, quarter];
    let rotated = compile(scene.input(&intent), &cancel).unwrap();
    assert!(
        (rotated.plan.layers[0]
            .region_coverage
            .surface_added_area_mm2
            - finished.plan.layers[0]
                .region_coverage
                .surface_added_area_mm2)
            .abs()
            < 1e-5
    );
    scene.occurrences[0].pose.rotation_xyzw = [0.0, 0.0, 0.0, 1.0];
    scene.occurrences[0].pose.translation_mm[2] = 1.0;
    assert_eq!(
        preflight(&scene.input(&intent), &cancel)
            .unwrap()
            .disposition,
        PreflightDisposition::Refused
    );
    let mut overlapping =
        NativeScene::new(BOX, &[RigidPoseMm::IDENTITY, translated(10.0, 0.0, 0.0)]);
    overlapping
        .surface_overlays
        .layers
        .push(rib_layer(&overlapping, 1));
    let report = preflight(&overlapping.input(&intent), &cancel).unwrap();
    assert!(
        report.disposition == PreflightDisposition::Refused
            && report
                .checks
                .iter()
                .any(|check| check.code == "surface_occurrence_exclusion")
    );
}

#[test]
fn finished_native_cantilever_keeps_ribs_through_actual_anchor_support_realization() {
    use geo::{Contains, Point};
    use spiling_contracts::surface::SurfaceScope;
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY, translated(10.0, 0.0, 8.0)]);
    let mut layer = rib_layer(&scene, 1);
    layer.scope = SurfaceScope::Occurrence {
        occurrence_id: scene.occurrences[0].occurrence_id,
        expected_definition_id: scene.occurrences[0].definition_id.clone(),
    };
    let face_id = layer.face_id.clone();
    scene.surface_overlays.layers.push(layer);
    let enabled = intent(true);
    let bundle = compile(scene.input(&enabled), &cancel).unwrap();
    assert!(bundle.verification.support_deposited_volume_mm3 > 0.0);
    let native = &scene.definitions[&scene.occurrences[0].definition_id];
    let cert = native.vertical_wall_face(&face_id).unwrap();
    let (admitted, _) = preflight::admit(&scene.input(&enabled), &cancel).unwrap();
    let source = region::from_native(
        &spiling_geometry::section_with_byte_limit(
            native,
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            &cancel,
            4 * 1024 * 1024,
        )
        .unwrap(),
        &RigidPoseMm::IDENTITY,
        4.0,
        &cancel,
    )
    .unwrap();
    let derived = surface::realize(
        source,
        &admitted.surface_fields[&scene.occurrences[0].occurrence_id],
        4.0,
        0,
        &mut 0,
        &cancel,
    )
    .unwrap();
    let exclusion = region::offset(&derived, 0.2, &cancel).unwrap();
    for layer in &bundle.plan.layers {
        if layer.section_z_mm < 8.0 {
            assert!(layer.region_coverage.surface_added_area_mm2 > 0.0);
            let peak = layer
                .paths
                .iter()
                .filter(|p| p.kind == DepositionKind::Perimeter)
                .flat_map(|p| &p.points_mm)
                .map(|point| {
                    (point[0] - cert.origin_mm[0]) * cert.outward_normal[0]
                        + (point[1] - cert.origin_mm[1]) * cert.outward_normal[1]
                })
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                (peak - (0.1 - enabled.recipe.bead_width_mm / 2.0)).abs() <= 0.006,
                "actual-anchor regeneration lost the lower occurrence's ribs"
            );
            for point in layer
                .paths
                .iter()
                .filter(|p| {
                    matches!(
                        p.kind,
                        DepositionKind::Support | DepositionKind::SupportInterface
                    )
                })
                .flat_map(|p| &p.points_mm)
            {
                assert!(
                    !exclusion.contains(&Point::new(point[0], point[1])),
                    "support entered the realized native-plus-rib clearance domain"
                );
            }
        } else {
            assert_eq!(layer.region_coverage.surface_added_area_mm2, 0.0);
        }
    }
}

#[test]
fn multiple_certified_finishes_preserve_exact_native_hole_rings_across_pose_and_occurrences() {
    use spiling_contracts::surface::SurfaceLayerId;
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(HOLE, &[RigidPoseMm::IDENTITY, translated(50.0, 0.0, 0.0)]);
    let mut first = rib_layer(&scene, 1);
    first.finish = spiling_contracts::surface::SurfaceFinish::VerticalRibs {
        depth_mm: 0.02,
        pitch_mm: 2.4,
        edge_margin_mm: 1.2,
        phase_mm: 0.37,
    };
    let native = scene.definitions[&scene.occurrences[0].definition_id].clone();
    let second_face = native
        .faces()
        .iter()
        .find(|face| {
            face.face_id != first.face_id
                && native
                    .vertical_wall_face(&face.face_id)
                    // Left-face native traversal crosses the canonical ring closure.
                    .is_ok_and(|cert| cert.width_mm >= 8.0 && cert.outward_normal[0] < -0.9)
        })
        .unwrap();
    let mut second = first.clone();
    second.layer_id = SurfaceLayerId::new(2).unwrap();
    second.face_id = second_face.face_id.clone();
    scene.surface_overlays.layers = vec![first, second];
    let intent = intent(false);
    let bundle = compile(scene.input(&intent), &cancel).unwrap();
    assert!(bundle.plan.layers[0].region_coverage.surface_added_area_mm2 > 0.0);
    assert!(
        bundle.plan.layers[0]
            .region_coverage
            .protected_hole_area_mm2
            > 0.0
    );
    let quarter = std::f64::consts::FRAC_1_SQRT_2;
    scene.occurrences[0].pose.rotation_xyzw = [0.0, 0.0, quarter, quarter];
    let (admitted, _) = preflight::admit(&scene.input(&intent), &cancel).unwrap();
    assert_eq!(admitted.report.disposition, PreflightDisposition::Admitted);
    for occurrence in &scene.occurrences {
        let source = region::from_native(
            &spiling_geometry::section_with_byte_limit(
                &native,
                PlaneMm {
                    origin_mm: [0.0, 0.0, 2.0],
                    normal: [0.0, 0.0, 1.0],
                },
                &cancel,
                4 * 1024 * 1024,
            )
            .unwrap(),
            &occurrence.pose,
            2.0,
            &cancel,
        )
        .unwrap();
        let derived = surface::realize(
            source.clone(),
            &admitted.surface_fields[&occurrence.occurrence_id],
            2.0,
            0,
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert_eq!(source.0.len(), derived.0.len());
        assert_native_surface_retention(
            &source,
            &derived,
            &native,
            &occurrence.pose,
            2.0,
            &scene.surface_overlays.layers,
        );
        assert!(derived.unsigned_area() > source.unsigned_area());
    }
    let posed = compile(scene.input(&intent), &cancel).unwrap();
    assert!(posed.verification.verified);
    assert!(
        (posed.plan.layers[0].region_coverage.surface_added_area_mm2
            - bundle.plan.layers[0].region_coverage.surface_added_area_mm2)
            .abs()
            < 1e-5
    );
}

const FULL_CYLINDER: &[u8] =
    include_bytes!("../../../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step");

fn cylinder_layer(scene: &NativeScene, phase: f64) -> spiling_contracts::surface::SurfaceLayer {
    use spiling_contracts::surface::{SurfaceFinish, SurfaceLayer, SurfaceLayerId, SurfaceScope};
    let occurrence = &scene.occurrences[0];
    let native = &scene.definitions[&occurrence.definition_id];
    let face = native
        .faces()
        .iter()
        .find(|face| native.vertical_cylinder_face(&face.face_id).is_ok())
        .expect("independent native full-turn cylindrical source face");
    SurfaceLayer {
        layer_id: SurfaceLayerId::new(1).unwrap(),
        enabled: true,
        scope: SurfaceScope::Definition {
            definition_id: occurrence.definition_id.clone(),
        },
        face_id: face.face_id.clone(),
        finish: SurfaceFinish::CylindricalRibs {
            depth_mm: 0.12,
            rib_count: 12,
            phase_fraction: phase,
        },
    }
}

#[test]
fn independent_cylindrical_ribs_change_ordinary_walls_accounting_and_actual_replay() {
    use crate::{inspect_bundle, verify_bundle};
    use spiling_contracts::geometry::SourceHash;
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(FULL_CYLINDER, &[RigidPoseMm::IDENTITY]);
    let intent = intent(false);
    let original = compile(scene.input(&intent), &cancel).unwrap();
    scene
        .surface_overlays
        .layers
        .push(cylinder_layer(&scene, 0.25));
    let finished = compile(scene.input(&intent), &cancel).unwrap();
    assert_ne!(original.program, finished.program);
    assert_eq!(finished.provenance.surface_overlays, scene.surface_overlays);
    assert_eq!(
        finished.provenance.definitions,
        original.provenance.definitions
    );
    for (plain, textured) in original.plan.layers.iter().zip(&finished.plan.layers) {
        let coverage = &textured.region_coverage;
        assert!(
            (coverage.source_section_area_mm2 - plain.region_coverage.section_area_mm2).abs()
                < 1e-8
        );
        assert!(coverage.surface_added_area_mm2 > 0.0);
        assert!(
            (coverage.section_area_mm2
                - coverage.source_section_area_mm2
                - coverage.surface_added_area_mm2)
                .abs()
                < 1e-9
        );
        let wall = textured
            .paths
            .iter()
            .find(|p| p.kind == DepositionKind::Perimeter)
            .unwrap();
        let old = plain
            .paths
            .iter()
            .find(|p| p.kind == DepositionKind::Perimeter)
            .unwrap();
        assert_eq!(wall.points_mm.first(), wall.points_mm.last());
        assert_ne!(wall.points_mm, old.points_mm);
        let peak = wall
            .points_mm
            .iter()
            .map(|p| p[0].hypot(p[1]))
            .fold(0.0f64, f64::max);
        assert!((peak - (5.0 + 0.12 - intent.recipe.bead_width_mm / 2.0)).abs() <= 0.011);
        let area = (intent.recipe.bead_width_mm - textured.height_mm) * textured.height_mm
            + std::f64::consts::PI * (textured.height_mm / 2.0).powi(2);
        let volume = textured
            .paths
            .iter()
            .flat_map(|p| p.points_mm.windows(2))
            .map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]) * area)
            .sum::<f64>();
        assert!((coverage.nominal_deposition_volume_mm3 - volume).abs() < 1e-7);
    }
    let hash = SourceHash::from_bytes(&serde_json::to_vec(&finished).unwrap());
    let inspection = inspect_bundle(
        &finished,
        &hash,
        VerificationOutput::ReportAndMotion,
        &cancel,
    )
    .unwrap();
    assert!(inspection.report.verified);
    assert!(inspection.replay.is_some());
    assert!(verify_bundle(&finished, &cancel).unwrap().verified);
    let mut stale = finished;
    stale.provenance.planner_version = "4".into();
    assert!(
        verify_bundle(&stale, &cancel).is_err(),
        "old planner artifacts must not retain inspection authority"
    );
}

#[test]
fn cylindrical_source_phase_pose_and_flipped_z_preserve_native_closed_material() {
    use geo::{Intersects, Point};
    let cancel = AtomicBool::new(false);
    let intent = intent(false);
    let poses = [
        RigidPoseMm::IDENTITY,
        RigidPoseMm {
            translation_mm: [17.0, 11.0, 0.0],
            rotation_xyzw: [
                0.0,
                0.0,
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ],
        },
        RigidPoseMm {
            translation_mm: [17.0, 11.0, 8.0],
            rotation_xyzw: [1.0, 0.0, 0.0, 0.0],
        },
    ];
    for pose in poses {
        for phase in [0.0, 0.25, 0.999] {
            let mut scene = NativeScene::new(FULL_CYLINDER, &[pose]);
            scene
                .surface_overlays
                .layers
                .push(cylinder_layer(&scene, phase));
            let native = &scene.definitions[&scene.occurrences[0].definition_id];
            let (admitted, _) = preflight::admit(&scene.input(&intent), &cancel).unwrap();
            assert_eq!(admitted.report.disposition, PreflightDisposition::Admitted);
            let sample = 2.0;
            let local_z = inverse(&pose, [0.0, 0.0, sample])[2];
            let section = spiling_geometry::section_with_byte_limit(
                native,
                PlaneMm {
                    origin_mm: [0.0, 0.0, local_z],
                    normal: [0.0, 0.0, 1.0],
                },
                &cancel,
                4 * 1024 * 1024,
            )
            .unwrap();
            let source = region::from_native(&section, &pose, sample, &cancel).unwrap();
            let derived = surface::realize(
                source.clone(),
                &admitted.surface_fields[&scene.occurrences[0].occurrence_id],
                sample,
                0,
                &mut 0,
                &cancel,
            )
            .unwrap();
            assert_eq!(source.0.len(), derived.0.len());
            for (old, new) in source.0.iter().zip(&derived.0) {
                assert_eq!(old.interiors(), new.interiors());
                assert_eq!(new.exterior().0.first(), new.exterior().0.last());
                assert!(old.exterior().0.iter().all(|p| new.intersects(&Point(*p))));
                let mut observed_peak = 0.0f64;
                for p in &new.exterior().0 {
                    let local = inverse(&pose, [p.x, p.y, sample]);
                    let theta = local[1].atan2(local[0]);
                    let radius = local[0].hypot(local[1]);
                    let expected =
                        0.06 * (1.0 - (12.0 * theta - std::f64::consts::TAU * phase).cos());
                    // Native polygon's radial deviation remains a separate bound.
                    assert!(
                        (radius - 5.0 - expected).abs() <= SECTION_SAMPLING_TOLERANCE_MM + 0.00001
                    );
                    observed_peak = observed_peak.max(expected);
                }
                assert!(observed_peak > 0.11999);
            }
        }
    }
}

#[test]
fn cylindrical_native_partial_resource_precision_and_cancel_are_explicit_refusals() {
    use spiling_contracts::surface::SurfaceFinish;
    let cancel = AtomicBool::new(false);
    let intent = intent(false);
    let mut partial = NativeScene::new(
        include_bytes!("../../../../fixtures/geometry/cylinder.step"),
        &[RigidPoseMm::IDENTITY],
    );
    let native = &partial.definitions[&partial.occurrences[0].definition_id];
    let face = native
        .faces()
        .iter()
        .find(|f| {
            matches!(
                f.geometry.analytic,
                Some(spiling_contracts::geometry::AnalyticSurfaceDescription::Cylinder { .. })
            )
        })
        .unwrap();
    let mut layer = cylinder_layer(
        &NativeScene::new(FULL_CYLINDER, &[RigidPoseMm::IDENTITY]),
        0.0,
    );
    layer.face_id = face.face_id.clone();
    layer.scope = spiling_contracts::surface::SurfaceScope::Definition {
        definition_id: native.id().clone(),
    };
    partial.surface_overlays.layers.push(layer);
    assert_eq!(
        preflight(&partial.input(&intent), &cancel)
            .unwrap()
            .disposition,
        PreflightDisposition::Refused,
    );
    let mut scene = NativeScene::new(FULL_CYLINDER, &[RigidPoseMm::IDENTITY]);
    scene
        .surface_overlays
        .layers
        .push(cylinder_layer(&scene, 0.0));
    assert_eq!(
        surface::metadata(
            &scene.input(&intent),
            &scene.occurrences[0],
            AabbMm::new([-5.0, -5.0, 0.0], [5.0, 5.0, 8.0]).unwrap(),
            1,
            &cancel
        )
        .err()
        .unwrap()
        .code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert_eq!(
        compile(scene.input(&intent), &AtomicBool::new(true))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::Cancelled
    );
    for depth in [0.022, 0.301] {
        scene.surface_overlays.layers[0].finish = SurfaceFinish::CylindricalRibs {
            depth_mm: depth,
            rib_count: 12,
            phase_fraction: 0.0,
        };
        assert_eq!(
            preflight(&scene.input(&intent), &cancel)
                .unwrap()
                .disposition,
            PreflightDisposition::Refused
        );
    }
    scene.surface_overlays.layers[0].finish = SurfaceFinish::CylindricalRibs {
        depth_mm: 0.12,
        rib_count: 12,
        phase_fraction: 0.0,
    };
    scene.occurrences[0].pose.translation_mm[0] = 1e10;
    assert_eq!(
        preflight(&scene.input(&intent), &cancel)
            .unwrap()
            .disposition,
        PreflightDisposition::Refused
    );
}

fn image_parameters(depth: f64, margin: f64) -> spiling_contracts::surface::ImageReliefParameters {
    spiling_contracts::surface::ImageReliefParameters {
        depth_mm: depth,
        u_repeats: 1,
        u_phase_fraction: 0.17,
        z_period_mm: 8.0,
        z_phase_mm: 0.13,
        edge_margin_mm: margin,
        vertical_error_mm: 0.03,
    }
}

fn install_image(
    scene: &mut NativeScene,
    cylindrical: bool,
    width: u32,
    height: u32,
    pixels: &[u8],
    parameters: spiling_contracts::surface::ImageReliefParameters,
) {
    use spiling_contracts::{
        geometry::SourceHash,
        surface::{SurfaceFinish, SurfaceImageRecord},
    };
    assert_eq!(pixels.len(), (width * height) as usize);
    let mut bytes = format!("P5\n{width} {height}\n255\n").into_bytes();
    bytes.extend_from_slice(pixels);
    let image_hash = SourceHash::from_bytes(&bytes);
    let record = SurfaceImageRecord {
        image_hash: image_hash.clone(),
        width,
        height,
        byte_count: bytes.len() as u64,
        source_hash: image_hash.clone(),
        source_name: "independent image samples.pgm".into(),
    };
    let image = Arc::new(
        spiling_surface_fields::HeightImage::from_canonical(record.clone(), Arc::new(bytes))
            .unwrap(),
    );
    let mut layer = if cylindrical {
        cylinder_layer(scene, 0.0)
    } else {
        rib_layer(scene, 1)
    };
    layer.finish = SurfaceFinish::ImageRelief {
        image_hash: image_hash.clone(),
        parameters,
    };
    scene.surface_overlays.layers = vec![layer];
    scene.surface_overlays.images = vec![record];
    scene.surface_images = BTreeMap::from([(image_hash, image)]);
}

fn object_xy(layer: &PrintLayer, kind: DepositionKind) -> Vec<Vec<[f64; 2]>> {
    layer
        .paths
        .iter()
        .filter(|path| path.kind == kind)
        .map(|path| path.points_mm.iter().map(|p| [p[0], p[1]]).collect())
        .collect()
}

#[test]
fn mapped_images_vary_native_planar_and_cylindrical_walls_fill_skin_and_actual_program() {
    use crate::{inspect_bundle, verify_bundle};
    use spiling_contracts::geometry::SourceHash;
    let pixels = [
        255, 32, 0, 96, 0, 0, 0, 0, 128, 16, 64, 192, 32, 160, 255, 8,
    ];
    let cancel = AtomicBool::new(false);
    for (source, cylindrical) in [(BOX, false), (FULL_CYLINDER, true)] {
        let mut scene = NativeScene::new(source, &[RigidPoseMm::IDENTITY]);
        let intent = intent(false);
        let original = compile(scene.input(&intent), &cancel).unwrap();
        install_image(
            &mut scene,
            cylindrical,
            4,
            4,
            &pixels,
            image_parameters(0.08, if cylindrical { 0.0 } else { 1.2 }),
        );
        let bundle = compile(scene.input(&intent), &cancel).unwrap();
        assert_eq!(
            bundle.provenance.definitions,
            original.provenance.definitions
        );
        assert_eq!(bundle.provenance.surface_images.len(), 1);
        let image = scene.surface_images.values().next().unwrap();
        assert!(
            Arc::ptr_eq(&bundle.provenance.surface_images[0].bytes, image.bytes()),
            "provenance must share canonical content, not copy the raster per capture"
        );
        assert_eq!(bundle.provenance.surface_overlays, scene.surface_overlays);
        assert_ne!(bundle.program, original.program);
        for layer in &bundle.plan.layers {
            let coverage = &layer.region_coverage;
            assert!(
                (coverage.section_area_mm2
                    - coverage.source_section_area_mm2
                    - coverage.surface_added_area_mm2)
                    .abs()
                    < 1e-8
            );
        }
        assert!(
            bundle.plan.layers.windows(2).any(|pair| object_xy(
                &pair[0],
                DepositionKind::Perimeter
            ) != object_xy(
                &pair[1],
                DepositionKind::Perimeter
            )),
            "native invariant M must not imply constant realized wall paths"
        );
        assert!(bundle.plan.layers.windows(2).any(|pair| {
            (pair[0].region_coverage.surface_added_area_mm2
                - pair[1].region_coverage.surface_added_area_mm2)
                .abs()
                > 1e-4
        }));
        assert!(
            bundle
                .plan
                .layers
                .iter()
                .any(|layer| !object_xy(layer, DepositionKind::SolidFill).is_empty())
        );
        assert!(
            bundle
                .plan
                .layers
                .iter()
                .any(|layer| layer.region_coverage.top_skin_area_mm2 > 0.0
                    || layer.region_coverage.bottom_skin_area_mm2 > 0.0)
        );
        assert!(!bundle.plan.surface_resolution.is_empty());
        assert!(
            bundle
                .plan
                .surface_resolution
                .iter()
                .all(|record| record.image_hash == image.record().image_hash
                    && record.face_id == scene.surface_overlays.layers[0].face_id
                    && record.layer_index < bundle.plan.layers.len() as u32)
        );
        let hash = SourceHash::from_bytes(&serde_json::to_vec(&bundle).unwrap());
        let inspected =
            inspect_bundle(&bundle, &hash, VerificationOutput::ReportAndMotion, &cancel).unwrap();
        assert!(inspected.report.verified && inspected.replay.is_some());
        assert!(verify_bundle(&bundle, &cancel).unwrap().verified);
        let mut corrupt = bundle;
        let bytes = Arc::make_mut(&mut corrupt.provenance.surface_images[0].bytes);
        *bytes.last_mut().unwrap() ^= 1;
        assert!(
            verify_bundle(&corrupt, &cancel).is_err(),
            "fresh inspection must validate image content before trusting saved replay"
        );
    }
}

#[test]
fn blank_image_interval_returns_exact_native_material_and_no_invented_added_area() {
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(HOLE, &[RigidPoseMm::IDENTITY]);
    let mut parameters = image_parameters(0.06, 1.2);
    parameters.u_phase_fraction = 0.0;
    parameters.z_phase_mm = 0.0;
    install_image(
        &mut scene,
        false,
        2,
        4,
        &[0, 0, 255, 255, 0, 0, 0, 0],
        parameters,
    );
    let intent = intent(false);
    let (admitted, _) = preflight::admit(&scene.input(&intent), &cancel).unwrap();
    assert_eq!(admitted.report.disposition, PreflightDisposition::Admitted);
    let occurrence = &scene.occurrences[0];
    let native = &scene.definitions[&occurrence.definition_id];
    let source = region::from_native(
        &spiling_geometry::section_with_byte_limit(
            native,
            PlaneMm {
                origin_mm: [0.0, 0.0, 2.0],
                normal: [0.0, 0.0, 1.0],
            },
            &cancel,
            4 * 1024 * 1024,
        )
        .unwrap(),
        &occurrence.pose,
        2.0,
        &cancel,
    )
    .unwrap();
    let derived = surface::realize(
        source.clone(),
        &admitted.surface_fields[&occurrence.occurrence_id],
        2.0,
        0,
        &mut 0,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        source, derived,
        "blank rows must retain original native vertices and holes exactly"
    );
    let bundle = compile(scene.input(&intent), &cancel).unwrap();
    for layer in bundle
        .plan
        .layers
        .iter()
        .filter(|layer| layer.section_z_mm < 4.0)
    {
        assert_eq!(layer.region_coverage.surface_added_area_mm2, 0.0);
        assert!(layer.region_coverage.protected_hole_area_mm2 > 0.0);
    }
}

// Deliberately independent of HeightImage/ImageField: the frozen pixel-node
// formula is the oracle for interval fidelity, including reversed image rows.
fn independent_pixel_height(
    pixels: &[u8],
    width: usize,
    height: usize,
    u: f64,
    z: f64,
    parameters: &spiling_contracts::surface::ImageReliefParameters,
) -> f64 {
    let x = width as f64
        * (parameters.u_repeats as f64 * u + parameters.u_phase_fraction).rem_euclid(1.0);
    let y =
        -(height as f64) * ((z + parameters.z_phase_mm) / parameters.z_period_mm).rem_euclid(1.0);
    let i = x.floor() as isize;
    let j = y.floor() as isize;
    let a = x - x.floor();
    let b = y - y.floor();
    let node = |column: isize, row: isize| {
        pixels[row.rem_euclid(height as isize) as usize * width
            + column.rem_euclid(width as isize) as usize] as f64
            / 255.0
    };
    parameters.depth_mm
        * ((node(i, j) * (1.0 - a) + node(i + 1, j) * a) * (1.0 - b)
            + (node(i, j + 1) * (1.0 - a) + node(i + 1, j + 1) * a) * b)
}

#[test]
fn non_grid_narrow_bright_stripe_cannot_alias_between_native_layer_midpoints() {
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut pixels = vec![0; 2 * 64];
    pixels[61 * 2..62 * 2].fill(255);
    let mut parameters = image_parameters(0.18, 1.2);
    parameters.u_phase_fraction = 0.0;
    parameters.z_phase_mm = 0.037;
    parameters.vertical_error_mm = 0.015;
    assert_eq!(
        independent_pixel_height(&pixels, 2, 64, 0.5, 0.2, &parameters),
        0.0
    );
    assert_eq!(
        independent_pixel_height(&pixels, 2, 64, 0.5, 0.6, &parameters),
        0.0
    );
    install_image(&mut scene, false, 2, 64, &pixels, parameters.clone());
    let intent = intent(false);
    let bundle = compile(scene.input(&intent), &cancel).unwrap();
    assert!(bundle.plan.layers.len() > 20);
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .any(|layer| layer.section_z_mm > 0.213
                && layer.section_z_mm < 0.463
                && layer.region_coverage.surface_added_area_mm2 > 0.5),
        "a genuine bright stripe invisible to nominal midpoint samples must survive"
    );
    for layer in &bundle.plan.layers {
        let bottom = layer.z_mm - layer.height_mm;
        let midpoint =
            independent_pixel_height(&pixels, 2, 64, 0.5, layer.section_z_mm, &parameters);
        for z in [
            bottom,
            (bottom + layer.section_z_mm) * 0.5,
            (layer.section_z_mm + layer.z_mm) * 0.5,
            layer.z_mm,
        ] {
            assert!(
                (independent_pixel_height(&pixels, 2, 64, 0.5, z, &parameters) - midpoint).abs()
                    <= parameters.vertical_error_mm + 1e-10
            );
        }
    }
    for required in [0.213, 0.338, 0.463] {
        assert!(
            bundle
                .plan
                .layers
                .iter()
                .any(|layer| (layer.z_mm - required).abs() < 1e-10)
        );
    }
}

fn wall_peak_in_u_band(
    layer: &PrintLayer,
    cert: &spiling_geometry::VerticalWallFace,
    margin: f64,
    low: f64,
    high: f64,
) -> f64 {
    let projected = |point: &[f64; 3]| {
        let delta = [point[0] - cert.origin_mm[0], point[1] - cert.origin_mm[1]];
        let s = delta[0] * cert.horizontal_direction[0] + delta[1] * cert.horizontal_direction[1];
        (
            (s - margin) / (cert.width_mm - 2.0 * margin),
            delta[0] * cert.outward_normal[0] + delta[1] * cert.outward_normal[1],
        )
    };
    let mut peak = f64::NEG_INFINITY;
    for path in layer.paths.iter().filter(|path| {
        path.kind == DepositionKind::Perimeter
            || (path.kind == DepositionKind::Bridge
                && path.points_mm.first() == path.points_mm.last())
    }) {
        for edge in path.points_mm.windows(2) {
            let a = projected(&edge[0]);
            let b = projected(&edge[1]);
            for point in [a, b] {
                if point.0 >= low && point.0 <= high {
                    peak = peak.max(point.1);
                }
            }
            if a.0 != b.0 {
                for boundary in [low, high] {
                    let t = (boundary - a.0) / (b.0 - a.0);
                    if (0.0..=1.0).contains(&t) {
                        peak = peak.max(a.1 + t * (b.1 - a.1));
                    }
                }
            }
        }
    }
    peak
}

#[test]
fn asymmetric_image_phase_moves_actual_first_wall_lobe_despite_identical_extrema() {
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut parameters = image_parameters(0.12, 1.2);
    parameters.z_phase_mm = 0.0;
    parameters.u_phase_fraction = 0.25;
    let pixels = [255, 0, 0, 0, 255, 0, 0, 0];
    install_image(&mut scene, false, 4, 2, &pixels, parameters.clone());
    let cert = scene.definitions[&scene.occurrences[0].definition_id]
        .vertical_wall_face(&scene.surface_overlays.layers[0].face_id)
        .unwrap();
    let intent = intent(false);
    let first = compile(scene.input(&intent), &cancel).unwrap();
    parameters.u_phase_fraction = 0.75;
    install_image(&mut scene, false, 4, 2, &pixels, parameters);
    let shifted = compile(scene.input(&intent), &cancel).unwrap();
    let a = &first.plan.layers[0];
    let b = &shifted.plan.layers[0];
    assert!(
        (wall_peak_in_u_band(a, &cert, 1.2, 0.1, 0.9)
            - wall_peak_in_u_band(b, &cert, 1.2, 0.1, 0.9))
        .abs()
            < 0.006
    );
    assert!(
        wall_peak_in_u_band(a, &cert, 1.2, 0.7, 0.8) - wall_peak_in_u_band(b, &cert, 1.2, 0.7, 0.8)
            > 0.06
    );
    assert!(
        wall_peak_in_u_band(b, &cert, 1.2, 0.2, 0.3) - wall_peak_in_u_band(a, &cert, 1.2, 0.2, 0.3)
            > 0.06
    );
    assert_ne!(first.program, shifted.program);
}

#[test]
fn ordinary_erosion_lost_image_feature_is_attributed_advisory_not_fake_wall_survival() {
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut pixels = vec![0; 256 * 2];
    pixels[128] = 255;
    pixels[256 + 128] = 255;
    let mut parameters = image_parameters(0.12, 1.2);
    parameters.u_phase_fraction = 0.0;
    parameters.z_phase_mm = 0.0;
    install_image(&mut scene, false, 256, 2, &pixels, parameters);
    let intent = intent(false);
    let bundle = compile(scene.input(&intent), &cancel).unwrap();
    let cert = scene.definitions[&scene.occurrences[0].definition_id]
        .vertical_wall_face(&scene.surface_overlays.layers[0].face_id)
        .unwrap();
    let peak = wall_peak_in_u_band(&bundle.plan.layers[0], &cert, 1.2, 0.45, 0.55)
        + intent.recipe.bead_width_mm * 0.5;
    assert!(
        peak < 0.09,
        "erosion must not invent survival of a sub-bead narrow peak"
    );
    assert!(
        bundle.plan.surface_resolution.iter().any(
            |record| record.below_nominal_resolution > 0 && record.max_residual_lower_mm > 0.02
        )
    );
    assert!(bundle.verification.verified);
}

#[test]
fn steep_image_support_succeeds_from_real_bed_corridor_and_lower_image_rebuild_blocks_it() {
    use geo::{Contains, Point};
    let cancel = AtomicBool::new(false);
    for (source, sparse) in [(BOX, false), (HOLE, true)] {
        let mut scene = NativeScene::new(source, &[RigidPoseMm::IDENTITY]);
        let mut parameters = image_parameters(5.0, 1.2);
        parameters.u_phase_fraction = 0.0;
        // Two-millimetre cells bound nominal midpoint error by 0.75 mm.
        // Source Z rises from 4.6 to 6.6, then stays at its peak through top 8.
        parameters.z_period_mm = 32.0;
        parameters.z_phase_mm = 1.4;
        parameters.vertical_error_mm = 0.8;
        let mut pixels = [0; 2 * 16];
        pixels[11 * 2..13 * 2].fill(255);
        install_image(&mut scene, false, 2, 16, &pixels, parameters.clone());
        let mut disabled = intent(false);
        disabled.recipe.layer_height_mm = 0.6;
        if sparse {
            disabled.recipe.infill_fraction = 0.2;
        }
        let first_growth = independent_pixel_height(&pixels, 2, 16, 0.5, 4.7, &parameters);
        assert!(
            (first_growth - 0.25).abs() < 1e-10
                && first_growth < disabled.recipe.bead_width_mm * 0.5,
            "first growing slab must have genuine previous native-bead anchoring"
        );
        let next_growth = independent_pixel_height(&pixels, 2, 16, 0.5, 5.1, &parameters);
        assert!(
            (next_growth - 1.25).abs() < 1e-10 && next_growth - 0.2 > disabled.recipe.bead_width_mm,
            "unbridgeable growth must leave a printable bed corridor after XY clearance"
        );
        assert_eq!(
            compile(scene.input(&disabled), &cancel).unwrap_err().code,
            ManufacturingErrorCode::UnsupportedGeometry,
            "steep mapped geometry must demand actual anchors rather than fabricate native support"
        );
        let mut enabled = disabled.clone();
        enabled.printer.capabilities.generated_supports = true;
        enabled.recipe.supports = SupportPolicy::Breakaway {
            density: 1.0,
            xy_clearance_mm: 0.2,
            z_gap_mm: 0.2,
            interface_thickness_mm: 0.8,
        };
        let bundle = compile(scene.input(&enabled), &cancel).unwrap();
        assert!(
            bundle.verification.verified && bundle.verification.support_deposited_volume_mm3 > 0.0
        );
        assert!(
            bundle.plan.layers[0]
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::Support),
            "successful support must genuinely begin at the bed"
        );
        assert!(bundle.plan.layers.iter().any(|layer| {
            layer
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::SupportInterface)
        }));
        let (admitted, _) = preflight::admit(&scene.input(&enabled), &cancel).unwrap();
        let occurrence = &scene.occurrences[0];
        let native = &scene.definitions[&occurrence.definition_id];
        let cert = native
            .vertical_wall_face(&scene.surface_overlays.layers[0].face_id)
            .unwrap();
        for layer in &bundle.plan.layers {
            let source = region::from_native(
                &spiling_geometry::section_with_byte_limit(
                    native,
                    PlaneMm {
                        origin_mm: [0.0, 0.0, layer.section_z_mm],
                        normal: [0.0, 0.0, 1.0],
                    },
                    &cancel,
                    4 * 1024 * 1024,
                )
                .unwrap(),
                &occurrence.pose,
                layer.section_z_mm,
                &cancel,
            )
            .unwrap();
            let derived = surface::realize(
                source,
                &admitted.surface_fields[&occurrence.occurrence_id],
                layer.section_z_mm,
                0,
                &mut 0,
                &cancel,
            )
            .unwrap();
            let clearance = region::offset(&derived, 0.2, &cancel).unwrap();
            for point in layer
                .paths
                .iter()
                .filter(|path| {
                    matches!(
                        path.kind,
                        DepositionKind::Support | DepositionKind::SupportInterface
                    )
                })
                .flat_map(|path| &path.points_mm)
            {
                assert!(
                    !clearance.contains(&Point::new(point[0], point[1])),
                    "all lower supports must honor that layer's final mapped D, not native M"
                );
            }
            if sparse {
                assert!(layer.region_coverage.protected_hole_area_mm2 > 0.0);
            }
            let intended =
                independent_pixel_height(&pixels, 2, 16, 0.5, layer.section_z_mm, &parameters);
            let actual = wall_peak_in_u_band(layer, &cert, 1.2, 0.3, 0.7);
            assert!(
                (actual - (intended - enabled.recipe.bead_width_mm * 0.5)).abs() <= 0.006,
                "actual-support regeneration must retain the same mapped first wall at Z {}",
                layer.section_z_mm
            );
        }
        // Same upper field and native source, but the earlier image rows now
        // occupy every formerly usable vertical corridor all the way to the bed.
        let mut blocked_pixels = pixels;
        blocked_pixels[0..2].fill(255);
        blocked_pixels[14 * 2..16 * 2].fill(255);
        for z in [4.6, 5.1, 6.3, 6.8, 7.8, 8.0] {
            assert_eq!(
                independent_pixel_height(&pixels, 2, 16, 0.5, z, &parameters),
                independent_pixel_height(&blocked_pixels, 2, 16, 0.5, z, &parameters),
                "the independent blocker must leave the entire upper image field unchanged",
            );
        }
        install_image(&mut scene, false, 2, 16, &blocked_pixels, parameters);
        let blocked = compile(scene.input(&enabled), &cancel).unwrap_err();
        assert!(
            matches!(
                blocked.code,
                ManufacturingErrorCode::UnsupportedGeometry
                    | ManufacturingErrorCode::NativeAnalysisIndeterminate
            ),
            "{blocked:?}"
        );
    }
}

#[test]
fn image_capture_disabled_references_metadata_and_event_budgets_are_not_bypassed() {
    let cancel = AtomicBool::new(false);
    let mut scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    install_image(
        &mut scene,
        false,
        2,
        2,
        &[0, 255, 255, 0],
        image_parameters(0.06, 1.2),
    );
    scene.surface_overlays.layers[0].enabled = false;
    let intent = intent(false);
    let valid = compile(scene.input(&intent), &cancel).unwrap();
    assert_eq!(valid.provenance.surface_images.len(), 1);
    scene.surface_images.clear();
    assert_eq!(
        preflight(&scene.input(&intent), &cancel)
            .unwrap()
            .disposition,
        PreflightDisposition::Refused
    );
    assert_eq!(
        compile(scene.input(&intent), &cancel).unwrap_err().code,
        ManufacturingErrorCode::InvalidSpecification
    );
    let mut parameters = image_parameters(0.06, 1.2);
    parameters.z_period_mm = 0.001;
    parameters.z_phase_mm = 0.0;
    install_image(&mut scene, false, 2, 2, &[0, 255, 255, 0], parameters);
    let report = preflight(&scene.input(&intent), &cancel).unwrap();
    assert_eq!(report.disposition, PreflightDisposition::Admitted);
    assert!(
        report
            .checks
            .iter()
            .any(|check| !check.required && check.evidence == PreflightEvidence::NotEvaluated)
    );
    assert_eq!(
        compile(scene.input(&intent), &cancel).unwrap_err().code,
        ManufacturingErrorCode::ResourceLimit
    );
}

fn parity_source(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../fixtures/geometry/native-parity-{name}.step")),
    )
    .expect("parent must generate the genuine native parity corpus before integration regressions")
}

#[test]
fn equivalent_analytic_and_rational_sources_preserve_sections_material_and_fresh_replay() {
    let cancel = AtomicBool::new(false);
    for (name, analytic, rational, perimeter, hole_radius) in [
        ("box", BOX.to_vec(), parity_source("nurbs-box"), 60.0, 0.0),
        (
            "cylinder",
            FULL_CYLINDER.to_vec(),
            parity_source("nurbs-cylinder"),
            10.0 * std::f64::consts::PI,
            0.0,
        ),
        (
            "pocket",
            parity_source("blind-pocket"),
            parity_source("blind-pocket-nurbs-floor"),
            80.0 + 6.0 * std::f64::consts::PI,
            3.0,
        ),
    ] {
        let a = NativeScene::new(&analytic, &[RigidPoseMm::IDENTITY]);
        let b = NativeScene::new(&rational, &[RigidPoseMm::IDENTITY]);
        assert_ne!(
            a.occurrences[0].definition_id,
            b.occurrences[0].definition_id
        );
        assert_ne!(
            a.stored, b.stored,
            "equivalent geometry must not erase original provenance"
        );
        // These recipe pairs have matching physical dimensions, not merely
        // matching family labels. Compare actual sections on both sides of
        // the pocket floor; two independent chord tubes bound symmetric loss.
        let chord = spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM;
        let area_tube = perimeter * chord + std::f64::consts::PI * chord * chord;
        for z in [1.0, 3.9, 4.1, 7.0] {
            let left = original_section_polygons(&a, z);
            let right = original_section_polygons(&b, z);
            assert_eq!(left.0.len(), right.0.len(), "{name} at {z}");
            assert_eq!(
                left.0
                    .iter()
                    .map(|polygon| polygon.interiors().len())
                    .sum::<usize>(),
                right
                    .0
                    .iter()
                    .map(|polygon| polygon.interiors().len())
                    .sum::<usize>(),
                "{name} at {z}"
            );
            assert!(
                left.xor(&right).unsigned_area() <= 2.0 * area_tube,
                "{name} at {z}: changed source encoding must preserve physical material"
            );
        }
        let intent = intent(false);
        let bundles = [
            compile(a.input(&intent), &cancel).unwrap(),
            compile(b.input(&intent), &cancel).unwrap(),
        ];
        assert_eq!(bundles[0].provenance.definitions, a.stored);
        assert_eq!(bundles[1].provenance.definitions, b.stored);
        let mut volumes = [0.0; 2];
        for (index, bundle) in bundles.iter().enumerate() {
            let coverage = &bundle.plan.native_geometry;
            assert_eq!(coverage.evidence, PreflightEvidence::Certified);
            assert_eq!(
                coverage.certified_layer_count,
                bundle.plan.layers.len() as u32
            );
            assert!(coverage.achieved_shape_error_mm <= intent.recipe.native_shape_error_mm);
            assert!((bundle.plan.layers.last().unwrap().z_mm - 8.0).abs() < 1e-6);
            for layer in &bundle.plan.layers {
                let expected = match name {
                    "box" => 200.0,
                    "cylinder" => 25.0 * std::f64::consts::PI,
                    _ if layer.section_z_mm > 4.0 => 400.0 - 9.0 * std::f64::consts::PI,
                    _ => 400.0,
                };
                assert!(
                    (layer.region_coverage.source_section_area_mm2 - expected).abs() <= area_tube,
                    "{name}: original-material area must agree with the separate analytic oracle"
                );
                volumes[index] += layer.region_coverage.source_section_area_mm2 * layer.height_mm;
            }
            if hole_radius > 0.0 {
                assert!(
                    bundle
                        .plan
                        .layers
                        .iter()
                        .any(|layer| (layer.z_mm - 4.0).abs() < 1e-6)
                );
            }
            visit_fresh_replayed_object_segments(bundle, |start, end| {
                if name == "cylinder" {
                    assert!(start[0].hypot(start[1]) <= 5.0 && end[0].hypot(end[1]) <= 5.0);
                } else {
                    let depth = if name == "box" { 10.0 } else { 20.0 };
                    for point in [start, end] {
                        assert!((0.0..=20.0).contains(&point[0]));
                        assert!((0.0..=depth).contains(&point[1]));
                    }
                    let layer = bundle
                        .plan
                        .layers
                        .iter()
                        .find(|layer| (layer.z_mm - end[2]).abs() <= 0.000001)
                        .unwrap();
                    if hole_radius > 0.0 && layer.section_z_mm > 4.0 {
                        let offset = [start[0] - 10.0, start[1] - 10.0];
                        let delta = [end[0] - start[0], end[1] - start[1]];
                        let squared = delta[0] * delta[0] + delta[1] * delta[1];
                        let t = if squared == 0.0 {
                            0.0
                        } else {
                            (-(offset[0] * delta[0] + offset[1] * delta[1]) / squared)
                                .clamp(0.0, 1.0)
                        };
                        assert!(
                            (offset[0] + t * delta[0]).hypot(offset[1] + t * delta[1])
                                >= hole_radius,
                            "fresh emitted bytes must not fill the equivalently encoded native pocket"
                        );
                    }
                }
            });
        }
        assert!(
            (volumes[0] - volumes[1]).abs() <= 16.0 * area_tube,
            "{name}: representation changes cannot change scheduled native material volume"
        );
    }
}

#[test]
fn new_native_families_use_one_material_planner_and_independent_replay() {
    use spiling_contracts::geometry::SurfaceFamily;
    let cancel = AtomicBool::new(false);
    for (name, family, bed_translation, top) in [
        ("cone-frustum", SurfaceFamily::Cone, 0.0, 8.0),
        ("sphere-belt", SurfaceFamily::Sphere, 3.0, 6.0),
        ("full-sphere", SurfaceFamily::Sphere, 5.0, 10.0),
        ("ring-torus", SurfaceFamily::Torus, 2.0, 4.0),
        ("nurbs-box", SurfaceFamily::Nurbs, 0.0, 8.0),
        ("nurbs-cylinder", SurfaceFamily::Nurbs, 0.0, 8.0),
        ("nurbs-torus", SurfaceFamily::Nurbs, 2.0, 4.0),
        (
            "nurbs-torus-independent-parameters",
            SurfaceFamily::Nurbs,
            2.0,
            4.0,
        ),
        ("bspline-box", SurfaceFamily::BSpline, 0.0, 8.0),
    ] {
        let source = parity_source(name);
        let scene = NativeScene::new(&source, &[translated(0.0, 0.0, bed_translation)]);
        let native = &scene.definitions[&scene.occurrences[0].definition_id];
        assert!(
            native
                .faces()
                .iter()
                .any(|face| face.geometry.family == family),
            "{name}"
        );
        let mut intent = intent(true);
        intent.recipe.layer_height_mm = 0.2;
        intent.recipe.native_shape_error_mm = 0.1;
        let metadata = preflight(&scene.input(&intent), &cancel).unwrap();
        assert_eq!(
            metadata.disposition,
            PreflightDisposition::Admitted,
            "{name}: {metadata:?}"
        );
        assert!(
            metadata
                .checks
                .iter()
                .any(|check| !check.required && check.evidence == PreflightEvidence::NotEvaluated)
        );
        let bundle = compile(scene.input(&intent), &cancel).unwrap();
        assert_eq!(bundle.provenance.definitions, scene.stored, "{name}");
        assert!(
            (bundle.plan.layers.last().unwrap().z_mm - top).abs() <= 0.000001,
            "{name}"
        );
        let coverage = &bundle.plan.native_geometry;
        assert_eq!(coverage.evidence, PreflightEvidence::Certified);
        assert_eq!(
            coverage.certified_layer_count,
            bundle.plan.layers.len() as u32
        );
        assert_eq!(
            coverage.height_analysis_count,
            scene.occurrences.len() as u32
        );
        assert!(coverage.achieved_shape_error_mm <= coverage.requested_shape_error_mm);
        assert!(coverage.slab_count > 0 && coverage.geometry_work > 0);
        assert_eq!(
            coverage.skin_coverage,
            SkinCoverageScope::ScheduledMaterialDistance
        );
        assert_eq!(
            coverage.support_clearance,
            SupportClearanceScope::ContinuousTargetOuterOccupancy
        );
        let features = &coverage.source_feature_coverage;
        assert_eq!(
            features.disjoint_source_layer_count
                + features.invariant_source_union_layer_count
                + features.native_interface_complex_layer_count,
            coverage.certified_layer_count,
            "{name}"
        );
        assert_eq!(features.invariant_source_union_layer_count, 0, "{name}");
        if matches!(
            family,
            SurfaceFamily::Cone | SurfaceFamily::Sphere | SurfaceFamily::Torus
        ) {
            assert!(
                features.native_interface_complex_layer_count > 0,
                "{name}: non-product critical cofaces require real native interface evidence"
            );
        }
        assert!(
            bundle
                .plan
                .layers
                .iter()
                .flat_map(|layer| &layer.paths)
                .any(|path| path.kind == DepositionKind::Perimeter)
        );
        assert!(
            bundle
                .plan
                .layers
                .iter()
                .flat_map(|layer| &layer.paths)
                .any(|path| matches!(
                    path.kind,
                    DepositionKind::BottomSkin | DepositionKind::TopSkin
                ))
        );
        let encoded = serde_json::to_vec(&bundle).unwrap();
        let reopened = decode_bundle(&encoded).unwrap();
        let replay = crate::verify_bundle(&reopened, &cancel).unwrap();
        assert!(replay.verified && replay.deposition_segments > 0, "{name}");
        assert!(bundle.plan.layers.iter().all(|layer| {
            (layer.region_coverage.section_area_mm2 - layer.region_coverage.source_section_area_mm2)
                .abs()
                < 1e-7
        }));
    }
}

#[test]
fn sphere_source_does_not_broaden_finish_certificates_and_smooth_keeps_precedence() {
    use spiling_contracts::geometry::SurfaceFamily;
    use spiling_contracts::surface::{SurfaceFinish, SurfaceLayer, SurfaceLayerId, SurfaceScope};
    let source = parity_source("sphere-belt");
    let mut scene = NativeScene::new(&source, &[translated(0.0, 0.0, 3.0)]);
    let native = &scene.definitions[&scene.occurrences[0].definition_id];
    let face = native
        .faces()
        .iter()
        .find(|face| face.geometry.family == SurfaceFamily::Sphere)
        .unwrap();
    let face_id = face.face_id.clone();
    let definition_id = native.id().clone();
    scene.surface_overlays.layers.push(SurfaceLayer {
        layer_id: SurfaceLayerId::new(1).unwrap(),
        scope: SurfaceScope::Definition {
            definition_id: definition_id.clone(),
        },
        face_id: face_id.clone(),
        enabled: true,
        finish: SurfaceFinish::VerticalRibs {
            depth_mm: 0.06,
            pitch_mm: 1.0,
            edge_margin_mm: 1.0,
            phase_mm: 0.0,
        },
    });
    let mut intent = intent(true);
    intent.recipe.layer_height_mm = 0.2;
    let cancel = AtomicBool::new(false);
    let refused = preflight(&scene.input(&intent), &cancel).unwrap();
    assert_eq!(refused.disposition, PreflightDisposition::Refused);
    assert!(
        refused
            .checks
            .iter()
            .any(|check| check.required && check.code == "unsupported_surface")
    );
    assert_eq!(
        compile(scene.input(&intent), &cancel).unwrap_err().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
    scene.surface_overlays.layers.push(SurfaceLayer {
        layer_id: SurfaceLayerId::new(2).unwrap(),
        scope: SurfaceScope::Definition { definition_id },
        face_id,
        enabled: true,
        finish: SurfaceFinish::Smooth,
    });
    let smooth = compile(scene.input(&intent), &cancel).unwrap();
    assert_eq!(smooth.provenance.surface_overlays, scene.surface_overlays);
    assert!(smooth.verification.verified);
}

#[test]
fn spline_extrusion_bore_keeps_native_holes_through_ordinary_paths_and_reopen_replay() {
    let source = parity_source("bezier-extrusion-bore");
    let scene = NativeScene::new(&source, &[RigidPoseMm::IDENTITY]);
    let mut intent = intent(false);
    intent.recipe.layer_height_mm = 0.2;
    let cancel = AtomicBool::new(false);
    let report = preflight(&scene.input(&intent), &cancel).unwrap();
    assert_eq!(
        report.disposition,
        PreflightDisposition::Admitted,
        "{report:?}"
    );
    let bundle = compile(scene.input(&intent), &cancel).unwrap();
    assert_eq!(bundle.provenance.definitions, scene.stored);
    assert!((bundle.plan.layers.last().unwrap().z_mm - 8.0).abs() <= 0.000001);
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .all(|layer| layer.region_coverage.protected_hole_area_mm2 > 12.0)
    );
    for layer in &bundle.plan.layers {
        for path in &layer.paths {
            for endpoints in path.points_mm.windows(2) {
                let a = [endpoints[0][0] - 10.0, endpoints[0][1] - 5.0];
                let b = [endpoints[1][0] - 10.0, endpoints[1][1] - 5.0];
                let delta = [b[0] - a[0], b[1] - a[1]];
                let length_squared = delta[0] * delta[0] + delta[1] * delta[1];
                let t = if length_squared == 0.0 {
                    0.0
                } else {
                    (-(a[0] * delta[0] + a[1] * delta[1]) / length_squared).clamp(0.0, 1.0)
                };
                let nearest = [a[0] + t * delta[0], a[1] + t * delta[1]];
                assert!(
                    nearest[0].hypot(nearest[1]) >= 2.0 - 0.01,
                    "emitted source-object centerline crosses protected native bore"
                );
            }
        }
    }
    let reopened = decode_bundle(&serde_json::to_vec(&bundle).unwrap()).unwrap();
    assert!(crate::verify_bundle(&reopened, &cancel).unwrap().verified);
    assert_eq!(
        reopened.plan.native_geometry.evidence,
        PreflightEvidence::Certified
    );
}

#[test]
fn independent_mechanical_sources_preserve_native_pocket_events_and_replay() {
    let cancel = AtomicBool::new(false);
    for (name, top, pocket_floor) in [
        ("occt-tapered-fitting-mm.step", 12.0, None),
        ("occt-rounded-housing-mm.step", 10.0, Some(4.13)),
        ("occt-spline-housing-mm.step", 10.0, Some(4.13)),
        ("occt-narrow-pocket-mm.step", 6.0, Some(1.07)),
    ] {
        let source = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/geometry/independent-exporters")
                .join(name),
        )
        .unwrap();
        let scene = NativeScene::new(&source, &[RigidPoseMm::IDENTITY]);
        let mut intent = intent(false);
        intent.recipe.layer_height_mm = 0.2;
        intent.recipe.native_shape_error_mm = 0.1;
        let metadata = preflight(&scene.input(&intent), &cancel).unwrap();
        assert_eq!(
            metadata.disposition,
            PreflightDisposition::Admitted,
            "{name}: {metadata:?}"
        );
        let bundle = compile(scene.input(&intent), &cancel).unwrap();
        assert!(
            bundle.plan.native_geometry.achieved_shape_error_mm <= 0.1,
            "{name}"
        );
        if let Some(floor) = pocket_floor {
            // A literal source-F floor need not be a selected-G slice plane.
            // Probe ordinary material on each side; closed critical ownership
            // remains a separate geometry obligation.
            let below = bundle
                .plan
                .layers
                .iter()
                .rev()
                .find(|layer| layer.section_z_mm <= floor * 0.5)
                .expect("material below the pocket");
            let above = bundle
                .plan
                .layers
                .iter()
                .find(|layer| layer.section_z_mm >= (floor + top) * 0.5)
                .expect("material above the pocket");
            assert!(
                below.region_coverage.protected_hole_area_mm2 < 1e-5,
                "{name}"
            );
            assert!(
                above.region_coverage.protected_hole_area_mm2 > 0.1,
                "{name}"
            );
        }
        let reopened = decode_bundle(&serde_json::to_vec(&bundle).unwrap()).unwrap();
        assert!(
            crate::verify_bundle(&reopened, &cancel).unwrap().verified,
            "{name}"
        );
    }
}

#[test]
fn blind_native_pocket_floor_remains_owned_while_ordinary_material_becomes_a_ring() {
    let cancel = AtomicBool::new(false);
    for name in ["blind-pocket", "blind-pocket-nurbs-floor"] {
        let source = parity_source(name);
        let scene = NativeScene::new(&source, &[RigidPoseMm::IDENTITY]);
        let intent = intent(false);
        let bundle = compile(scene.input(&intent), &cancel).unwrap();
        let floor = bundle
            .plan
            .layers
            .iter()
            .position(|layer| (layer.z_mm - 4.0).abs() < 1e-6)
            .expect("native pocket floor must be a real layer boundary");
        assert!(
            (bundle.plan.layers[floor]
                .region_coverage
                .source_section_area_mm2
                - 400.0)
                .abs()
                < 0.01,
            "{name}"
        );
        assert!(
            bundle.plan.layers[floor]
                .region_coverage
                .protected_hole_area_mm2
                < 0.01,
            "{name}"
        );
        assert!(
            (bundle.plan.layers[floor + 1]
                .region_coverage
                .source_section_area_mm2
                - (400.0 - 9.0 * std::f64::consts::PI))
                .abs()
                < 0.1,
            "{name}"
        );
        assert!(
            bundle.plan.layers[floor + 1]
                .region_coverage
                .protected_hole_area_mm2
                > 28.0,
            "{name}"
        );
        let coverage = &bundle.plan.native_geometry;
        assert!(
            coverage
                .source_feature_coverage
                .native_interface_complex_layer_count
                > 0,
            "{name}"
        );
        assert!(
            coverage.source_feature_coverage.owned_critical_slab_count > 0,
            "{name}"
        );
        assert!(
            coverage.achieved_shape_error_mm <= coverage.requested_shape_error_mm,
            "{name}"
        );
        assert_eq!(bundle.provenance.definitions, scene.stored, "{name}");
        let reopened = decode_bundle(&serde_json::to_vec(&bundle).unwrap()).unwrap();
        assert!(
            crate::verify_bundle(&reopened, &cancel).unwrap().verified,
            "{name}"
        );
    }
}

#[test]
fn floating_blind_native_pocket_does_not_project_its_closed_floor_down_to_bed() {
    let cancel = AtomicBool::new(false);
    for name in ["blind-pocket", "blind-pocket-nurbs-floor"] {
        let source = parity_source(name);
        let scene = NativeScene::new(&source, &[translated(0.0, 0.0, 2.0)]);
        let bundle = compile(scene.input(&intent(true)), &cancel).unwrap();
        let inside_floor_projection = bundle
            .plan
            .layers
            .iter()
            .filter(|layer| layer.z_mm < 2.0)
            .flat_map(|layer| &layer.paths)
            .filter(|path| {
                matches!(
                    path.kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                )
            })
            .flat_map(|path| path.points_mm.windows(2))
            .any(|edge| {
                let a = [edge[0][0] - 10.0, edge[0][1] - 10.0];
                let d = [edge[1][0] - edge[0][0], edge[1][1] - edge[0][1]];
                let t =
                    (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0.0, 1.0);
                (a[0] + t * d[0]).hypot(a[1] + t * d[1]) < 1.0
            });
        assert!(
            inside_floor_projection,
            "{name}: a closed pocket floor is not a bottom-open through-hole"
        );
        assert_eq!(bundle.provenance.definitions, scene.stored, "{name}");
        assert_eq!(bundle.provenance.occurrences, scene.occurrences, "{name}");
        let reopened = decode_bundle(&serde_json::to_vec(&bundle).unwrap()).unwrap();
        assert!(
            crate::verify_bundle(&reopened, &cancel).unwrap().verified,
            "{name}"
        );
    }
}

#[test]
fn non_grid_overlapping_native_material_binds_actual_overlay_and_replays() {
    let cancel = AtomicBool::new(false);
    let dx = 10.123456789;
    let dy = 0.234567891;
    let scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY, translated(dx, dy, 0.0)]);
    let bundle = compile(scene.input(&intent(false)), &cancel).unwrap();
    let expected = 400.0 - (20.0 - dx) * (10.0 - dy);
    for layer in &bundle.plan.layers {
        assert!((layer.region_coverage.source_section_area_mm2 - expected).abs() < 0.001);
    }
    let coverage = &bundle.plan.native_geometry;
    assert!(
        coverage.achieved_shape_error_mm > 0.0,
        "Actual common-frame overlay conversion cannot be reported as an exact zero-error source model"
    );
    assert!(coverage.achieved_shape_error_mm <= coverage.requested_shape_error_mm);
    assert_eq!(
        coverage
            .source_feature_coverage
            .invariant_source_union_layer_count,
        coverage.certified_layer_count
    );
    assert_eq!(bundle.provenance.occurrences, scene.occurrences);
    let reopened = decode_bundle(&serde_json::to_vec(&bundle).unwrap()).unwrap();
    assert!(crate::verify_bundle(&reopened, &cancel).unwrap().verified);
}

#[test]
fn overlapping_native_boxes_and_separated_varying_cone_share_actual_union_and_fresh_replay() {
    let cancel = AtomicBool::new(false);
    let dx = 10.123456789;
    let dy = 0.234567891;
    let mut intent = intent(false);
    intent.recipe.layer_height_mm = 0.2;
    intent.recipe.native_shape_error_mm = 0.1;
    let box_union_area = 400.0 - (20.0 - dx) * (10.0 - dy);
    let cone_source = parity_source("cone-frustum");
    for (cone_first, [cone_x, cone_y]) in [
        (false, [60.0, 0.0]),
        (true, [60.0, 0.0]),
        // True curved separation (~0.46 mm), despite overlapping raw XY AABBs.
        (false, [34.5, 13.5]),
        (true, [34.5, 13.5]),
    ] {
        // Original IDs, not just Vec order, determine native composition order.
        // In the second case the cone is already in M when the boxes overlap.
        let scene = if cone_first {
            let mut scene = NativeScene::new(&cone_source, &[translated(cone_x, cone_y, 0.0)]);
            scene.add(BOX, &[RigidPoseMm::IDENTITY, translated(dx, dy, 0.0)]);
            scene.occurrences.reverse();
            scene
        } else {
            let mut scene =
                NativeScene::new(BOX, &[RigidPoseMm::IDENTITY, translated(dx, dy, 0.0)]);
            scene.add(&cone_source, &[translated(cone_x, cone_y, 0.0)]);
            scene
        };
        // Genuine corpus profile (r,z)=(5,0)..(3,8); its full native tube is
        // clearly separated from the overlapping box subgroup.
        assert!(scene.definitions.values().any(|definition| {
            definition.faces().iter().any(|face| {
                face.geometry.family == spiling_contracts::geometry::SurfaceFamily::Cone
            })
        }));
        let bundle = compile(scene.input(&intent), &cancel).unwrap();
        assert_eq!(bundle.provenance.occurrences, scene.occurrences);
        assert_eq!(bundle.provenance.definitions, scene.stored);
        let coverage = &bundle.plan.native_geometry;
        assert_eq!(coverage.height_analysis_count, 3);
        let features = &coverage.source_feature_coverage;
        assert_eq!(
            features.disjoint_source_layer_count
                + features.invariant_source_union_layer_count
                + features.native_interface_complex_layer_count,
            coverage.certified_layer_count
        );
        assert_eq!(
            features.native_interface_complex_layer_count, coverage.certified_layer_count,
            "an invariant overlapping subgroup cannot label the varying whole layer invariant or disjoint"
        );
        assert_eq!(features.invariant_source_union_layer_count, 0);
        assert!(coverage.achieved_shape_error_mm <= intent.recipe.native_shape_error_mm);
        assert!((bundle.plan.layers.last().unwrap().z_mm - 8.0).abs() < 0.000001);
        let mut cone_source_volume = 0.0;
        let mut volume_error = 0.0;
        for layer in &bundle.plan.layers {
            // Analytic fixture oracle at accepted samples, not an all-Z proof:
            // interval/leaf/joint native evidence remains the Compile authority.
            let radius = 5.0 - layer.section_z_mm * 0.25;
            let expected = std::f64::consts::PI * radius * radius;
            let xy_error = intent.recipe.native_shape_error_mm * 1.25;
            let area_error = 0.01
                + 2.0 * std::f64::consts::PI * radius * xy_error
                + std::f64::consts::PI * xy_error * xy_error;
            let cone_area = layer.region_coverage.source_section_area_mm2 - box_union_area;
            assert!(
                (cone_area - expected).abs() <= area_error,
                "cone-first IDs={cone_first}, XY=({cone_x},{cone_y}), layer {}: source area must retain both boxes and the cone",
                layer.index
            );
            assert!(cone_area > 20.0);
            cone_source_volume += cone_area * layer.height_mm;
            volume_error += area_error * layer.height_mm
                + std::f64::consts::PI * layer.height_mm.powi(3) / 192.0;
        }
        let exact_cone_volume = std::f64::consts::PI * 8.0 * (25.0 + 15.0 + 9.0) / 3.0;
        assert!((cone_source_volume - exact_cone_volume).abs() <= volume_error);
        let encoded = serde_json::to_vec(&bundle).unwrap();
        let reopened = decode_bundle(&encoded).unwrap();
        let hash = spiling_contracts::geometry::SourceHash::from_bytes(&encoded);
        let inspection = crate::inspect_bundle(
            &reopened,
            &hash,
            VerificationOutput::ReportAndMotion,
            &cancel,
        )
        .unwrap();
        assert!(inspection.report.verified);
        let replay = inspection.replay.unwrap();
        let filament_area =
            std::f64::consts::PI * (intent.recipe.filament_diameter_mm * 0.5).powi(2);
        let mut actual_volumes = [0.0f64; 3];
        let mut previous: Option<([f64; 3], f64)> = None;
        for record in replay.bytes[REPLAY_HEADER_BYTES..]
            .as_chunks::<REPLAY_RECORD_BYTES>()
            .0
        {
            let end: [f64; 3] = std::array::from_fn(|axis| {
                f64::from_le_bytes(record[axis * 8..axis * 8 + 8].try_into().unwrap())
            });
            let extrusion = f64::from_le_bytes(record[24..32].try_into().unwrap());
            if matches!(record[46], 1..=5 | 8) {
                let (start, prior_e) =
                    previous.expect("actual object stroke has a preceding actual endpoint");
                let volume = (extrusion - prior_e) * filament_area;
                assert!(volume > 0.0);
                // Entire actual segments in exclusive source regions, not
                // preview labels, counters or travel into another contributor.
                let source = if start[0] < 8.0 && end[0] < 8.0 {
                    Some(0)
                } else if start[0] > 22.0 && end[0] > 22.0 && start[0] < 28.0 && end[0] < 28.0 {
                    Some(1)
                } else if start[0] > cone_x + 1.0 && end[0] > cone_x + 1.0 {
                    Some(2)
                } else {
                    None
                };
                if let Some(source) = source {
                    actual_volumes[source] += volume;
                }
            }
            previous = Some((end, extrusion));
        }
        assert!(
            actual_volumes.iter().all(|volume| *volume > 10.0),
            "fresh actual deposited volume must include each original contributor: {actual_volumes:?}"
        );
    }
}

#[test]
fn overlay_lost_native_notch_cannot_be_certified_merely_by_source_input_receipts() {
    let cancel = AtomicBool::new(false);
    let scene = NativeScene::new(
        BOX,
        &[
            RigidPoseMm::IDENTITY,
            translated(20.0 + 1e-10, 0.0, 0.0),
            translated(10.0, 5.0, 0.0),
        ],
    );
    let intent = intent(false);
    assert_eq!(
        preflight(&scene.input(&intent), &cancel)
            .unwrap()
            .disposition,
        PreflightDisposition::Admitted
    );
    // The third source forces the common-frame fixed-grid union. The genuine
    // source-union notch between the first two bodies is below that grid, even
    // though every original body and source-M input is well formed.
    let failure = compile(scene.input(&intent), &cancel).unwrap_err();
    assert!(
        matches!(
            failure.code,
            ManufacturingErrorCode::NativeAnalysisIndeterminate
                | ManufacturingErrorCode::NativeConditioningFailure
        ),
        "{failure}"
    );
}

#[test]
fn previous_planner_identity_cannot_acquire_current_fresh_inspection_authority() {
    let cancel = AtomicBool::new(false);
    let scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut bundle = compile(scene.input(&intent(false)), &cancel).unwrap();
    assert!(crate::verify_bundle(&bundle, &cancel).unwrap().verified);
    bundle.provenance.planner_version = "7".into();
    // The unchanged storage schema still admits this otherwise valid bundle.
    // Its saved report and current-coordinate program cannot certify that the
    // previous producer performed the current native geometry obligations.
    bundle.validate().unwrap();
    assert_eq!(
        crate::verify_bundle(&bundle, &cancel).unwrap_err().code,
        ManufacturingErrorCode::UnsupportedCapability
    );
    let hash =
        spiling_contracts::geometry::SourceHash::from_bytes(&serde_json::to_vec(&bundle).unwrap());
    for output in [
        VerificationOutput::ReportOnly,
        VerificationOutput::ReportAndMotion,
    ] {
        let failure = crate::inspect_bundle(&bundle, &hash, output, &cancel)
            .err()
            .expect("previous planner output must not receive current inspection authority");
        assert_eq!(failure.code, ManufacturingErrorCode::UnsupportedCapability);
    }
}

#[test]
fn housing_terminal_native_source_refinement_preserves_closed_material_authority() {
    let intent: ManufacturingIntent = serde_json::from_slice(include_bytes!(
        "../../../../fixtures/manufacturing/housing-ordinary.intent.json",
    ))
    .unwrap();
    let pose = RigidPoseMm {
        translation_mm: [98.0, 100.99999999999977, 1.225587472778531e-9],
        rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
    };
    let mut scene = NativeScene::new(
        include_bytes!("../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"),
        &[pose],
    );
    scene.process_document = serde_json::from_value(serde_json::json!({
        "schema_version": 3,
        "layers": [
            {"layer_id": 1, "enabled": true,
                "scope": {"kind": "definition", "definition_id": scene.occurrences[0].definition_id},
                "operations": [{"operation_id": 1,
                    "selector": {"kind": "whole_owned_volume"},
                    "action": {"kind": "replace", "value": {"kind": "sparse_fraction", "fraction": 0.60}},
                    "interface": null}]},
            {"layer_id": 2, "enabled": true,
                "scope": {"kind": "definition", "definition_id": scene.occurrences[0].definition_id},
                "operations": [{"operation_id": 2,
                    "selector": {"kind": "source_box_mm", "min": [-1.0,-1.0,-1.0], "max": [25.0,19.0,5.03]},
                    "action": {"kind": "replace", "value": {"kind": "sparse_fraction", "fraction": 0.20}},
                    "interface": {"kind": "horizontal_diaphragm", "thickness_mm": 0.23}}]},
        ],
    }))
    .unwrap();
    let input = scene.input(&intent);
    let cancel = AtomicBool::new(false);
    let (mut admission, geometry) = preflight::admit(&input, &cancel).unwrap();
    let mut proposal_work = 0;
    preflight::resolve_native_events(&input, &mut admission, &geometry, &mut proposal_work, &cancel)
        .unwrap();
    let mut captures = Vec::new();
    for occurrence in input.occurrences {
        captures.push(zones::capture(
            &input,
            &admission,
            occurrence.occurrence_id,
            &mut proposal_work,
            &cancel,
        ).unwrap());
    }
    let sources = prepare_sources(
        geometry,
        admission.native_capture_proposal_bytes(),
        &mut admission.geometry_work,
        &cancel,
    )
    .unwrap();
    // Prepare genuine accepted lower material in the ordinary producer before
    // making the captured terminal request. A state starting only at that
    // terminal height lacks the production lower-leaf precondition.
    let interval = [9.98333333474894857, 10.0000000924533996];
    let mut schedule = scheduled_layers(&input, &admission, &captures, &cancel).unwrap();
    while schedule.last().unwrap().lower_z >= interval[0] {
        schedule.pop();
    }
    let last = schedule.last_mut().unwrap();
    assert!(last.lower_z < interval[0]);
    last.z = interval[0];
    last.height = last.z - last.lower_z;
    last.sample = last.lower_z + last.height * 0.5;
    let occurrences = input.occurrences.iter().collect::<Vec<_>>();
    let full_interval = [intent.printer.build_envelope.min[2], interval[1]];
    let mut state = native::NativeStageState::new(full_interval).unwrap();
    let mut index = 0;
    let mut covered_upper = full_interval[0];
    let mut terminal_appended = false;
    loop {
        if index as usize == schedule.len() {
            if terminal_appended {
                break;
            }
            assert_eq!(covered_upper, interval[0]);
            schedule.push(LayerElevation {
                lower_z: interval[0],
                z: interval[1],
                sample: 9.99166671360117320,
                height: interval[1] - interval[0],
            });
            terminal_appended = true;
        }
        let retained = schedule.capacity() * std::mem::size_of::<LayerElevation>()
            + occurrences.capacity() * std::mem::size_of::<&OccurrenceRecord>();
        let span = native::collect_span(
            &native::NativeSourceInputs {
                input: &input,
                occurrences: &occurrences,
                sources: &sources,
            },
            &mut admission,
            &mut schedule,
            &native::NativeSpanRequest {
                index,
                retained_points: 0,
                retained_output_bytes: retained,
            },
            &mut state,
            &mut proposal_work,
            &cancel,
        )
        .unwrap();
        assert!(span.end > index);
        assert_eq!(schedule[index as usize].lower_z, covered_upper);
        assert!(!span.material.0.is_empty());
        covered_upper = schedule[span.end as usize - 1].z;
        index = span.end;
    }
    assert_eq!(covered_upper, interval[1]);
    state.finalize(&mut admission.geometry_work, &cancel).unwrap();
}
