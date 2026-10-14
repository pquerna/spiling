// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
use crate::{CompilerInput, compile};
use spiling_contracts::geometry::{AabbMm, DefinitionId, OccurrenceId, OccurrenceRecord};
use spiling_contracts::project::{ProjectId, ProjectRevision, StoredDefinition};
use std::collections::BTreeMap;

struct NativeScene {
    project: ProjectId,
    definitions: BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
    stored: Vec<StoredDefinition>,
    occurrences: Vec<OccurrenceRecord>,
    surface_overlays: spiling_contracts::surface::SurfaceOverlays,
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

#[test]
fn native_nonintegral_top_and_non_grid_shelf_preserve_every_event() {
    for name in ["nonintegral-top", "non-grid-shelf"] {
        let scene = NativeScene::new(&generated(name), &[RigidPoseMm::IDENTITY]);
        let intent = intent(false);
        let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
        assert!(bundle.verification.verified);
        assert!((bundle.plan.layers.last().unwrap().z_mm - 7.85).abs() < 1e-9);
        assert!((bundle.plan.layers.last().unwrap().height_mm - 0.25).abs() < 1e-9);
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
                .find(|layer| (layer.z_mm - 3.2).abs() < 1e-9)
                .unwrap();
            assert!((before.region_coverage.section_area_mm2 - 200.0).abs() < 1e-6);
            assert!((after.region_coverage.section_area_mm2 - 120.0).abs() < 1e-6);
            assert!((after.height_mm - 0.05).abs() < 1e-9);
        }
        assert_eq!(bundle.provenance.definitions, scene.stored);
    }
}

#[test]
fn genuine_inward_sloped_planar_ramp_passes_native_compile_and_replay() {
    let scene = NativeScene::new(&generated("inward-planar-ramp"), &[RigidPoseMm::IDENTITY]);
    let intent = intent(false);
    let report = preflight(&scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert_eq!(report.input_revision, ProjectRevision(19));
    assert!(report.eligible && report.software_only);
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
    let extent = spiling_geometry::projected_extent(
        &native,
        rotate_inverse(&pose, [0.0, 0.0, 1.0]),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!((extent[0] + 5.0 * angle.sin()).abs() < 1e-10);
    assert!((extent[1] - (8.0 * angle.cos() + 5.0 * angle.sin())).abs() < 1e-10);
    pose.translation_mm[2] = -extent[0];
    let scene = NativeScene::new(bytes, &[pose]);
    let mut intent = intent(false);
    intent.recipe.layer_height_mm = (extent[1] - extent[0]) / 20.0;
    let bundle = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert!((bundle.plan.layers.last().unwrap().z_mm - (extent[1] - extent[0])).abs() < 1e-8);
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
    assert!((top - 7.8).abs() < 1e-8);
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
    assert!((interface.last().unwrap().z_mm - 7.8).abs() < 1e-8);
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
fn floating_native_body_has_real_support_only_layers_and_exact_contact_gap() {
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
    assert!((empty.iter().map(|layer| layer.height_mm).sum::<f64>() - 0.2).abs() < 1e-8);
    let report = preflight(&scene.input(&intent(false)), &AtomicBool::new(false)).unwrap();
    assert!(!report.eligible);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "floating_without_support")
    );
}

#[test]
fn native_floating_through_hole_never_receives_generated_support() {
    let scene = NativeScene::new(HOLE, &[translated(0.0, 0.0, 2.0)]);
    let bundle = compile(scene.input(&intent(true)), &AtomicBool::new(false)).unwrap();
    for edge in bundle
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
        .flat_map(|path| path.points_mm.windows(2))
    {
        let a = [edge[0][0] - 10.0, edge[0][1] - 10.0];
        let d = [edge[1][0] - edge[0][0], edge[1][1] - edge[0][1]];
        let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0.0, 1.0);
        assert!((a[0] + t * d[0]).hypot(a[1] + t * d[1]) >= 3.2 + 0.3 - 0.011);
    }
}

#[test]
fn preflight_has_bounded_actionable_refs_pending_native_status_and_cancellation() {
    let scene = NativeScene::new(BOX, &[translated(1000.0, 0.0, 0.0)]);
    let intent = intent(false);
    let report = preflight(&scene.input(&intent), &AtomicBool::new(false)).unwrap();
    assert!(!report.eligible && report.software_only);
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
fn unadmitted_freeform_native_source_is_explicitly_refused() {
    let bytes = include_bytes!("../../../../fixtures/geometry/unsupported-nurbs.step");
    let error = spiling_geometry::import_step(bytes, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::UnsupportedGeometry);
}

#[test]
fn native_unresolved_top_and_layer_resource_limits_refuse_before_sections() {
    let scene = NativeScene::new(BOX, &[RigidPoseMm::IDENTITY]);
    let mut intent = intent(false);
    intent.recipe.layer_height_mm = 0.399995;
    let error = compile(scene.input(&intent), &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::UnsupportedGeometry);
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
    assert!(compile(scene.input(&enabled), &AtomicBool::new(false)).is_err());
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
    let admitted = preflight::admit(&scene.input(&intent), &cancel).unwrap();
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
        region::from_native(native_section, &RigidPoseMm::IDENTITY, sample, &cancel).unwrap();
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
            outward >= -1e-6 && outward <= 0.1 + 1e-6,
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
    assert_eq!(overridden.plan, original.plan);
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
    assert_eq!(removed.plan, revealed.plan);
    assert_ne!(
        removed.provenance.input_hash,
        revealed.provenance.input_hash
    );
    let mut exact_envelope = intent.clone();
    exact_envelope.printer.build_envelope =
        AabbMm::new([0.0, 0.0, 0.0], [20.0, 10.0, 8.0]).unwrap();
    let report = preflight(&scene.input(&exact_envelope), &cancel).unwrap();
    assert!(!report.eligible);
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
    assert!(!preflight(&scene.input(&intent), &cancel).unwrap().eligible);
    let mut overlapping =
        NativeScene::new(BOX, &[RigidPoseMm::IDENTITY, translated(10.0, 0.0, 0.0)]);
    overlapping
        .surface_overlays
        .layers
        .push(rib_layer(&overlapping, 1));
    let report = preflight(&overlapping.input(&intent), &cancel).unwrap();
    assert!(
        !report.eligible
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
    let admitted = preflight::admit(&scene.input(&enabled), &cancel).unwrap();
    let source = region::from_native(
        spiling_geometry::section_with_byte_limit(
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
    let admitted = preflight::admit(&scene.input(&intent), &cancel).unwrap();
    assert!(admitted.report.eligible);
    for occurrence in &scene.occurrences {
        let source = region::from_native(
            spiling_geometry::section_with_byte_limit(
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
            let admitted = preflight::admit(&scene.input(&intent), &cancel).unwrap();
            assert!(admitted.report.eligible);
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
            let source = region::from_native(section, &pose, sample, &cancel).unwrap();
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
                f.carrier,
                spiling_contracts::geometry::FaceCarrier::Cylinder { .. }
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
    assert!(
        !preflight(&partial.input(&intent), &cancel)
            .unwrap()
            .eligible
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
        assert!(!preflight(&scene.input(&intent), &cancel).unwrap().eligible);
    }
    scene.surface_overlays.layers[0].finish = SurfaceFinish::CylindricalRibs {
        depth_mm: 0.12,
        rib_count: 12,
        phase_fraction: 0.0,
    };
    scene.occurrences[0].pose.translation_mm[0] = 1e10;
    assert!(!preflight(&scene.input(&intent), &cancel).unwrap().eligible);
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
        let first_area = original.plan.layers[0]
            .region_coverage
            .source_section_area_mm2;
        for layer in &bundle.plan.layers {
            let coverage = &layer.region_coverage;
            assert!((coverage.source_section_area_mm2 - first_area).abs() < 1e-8);
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
    let admitted = preflight::admit(&scene.input(&intent), &cancel).unwrap();
    assert!(admitted.report.eligible);
    let occurrence = &scene.occurrences[0];
    let native = &scene.definitions[&occurrence.definition_id];
    let source = region::from_native(
        spiling_geometry::section_with_byte_limit(
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
        let admitted = preflight::admit(&scene.input(&enabled), &cancel).unwrap();
        let occurrence = &scene.occurrences[0];
        let native = &scene.definitions[&occurrence.definition_id];
        let cert = native
            .vertical_wall_face(&scene.surface_overlays.layers[0].face_id)
            .unwrap();
        for layer in &bundle.plan.layers {
            let source = region::from_native(
                spiling_geometry::section_with_byte_limit(
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
        assert_eq!(blocked.code, ManufacturingErrorCode::UnsupportedGeometry);
        assert!(
            blocked
                .message
                .contains("blocks every bed-anchored support corridor"),
            "failure must specifically reflect lower realized corridor blockage: {}",
            blocked.message
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
    assert!(!preflight(&scene.input(&intent), &cancel).unwrap().eligible);
    assert_eq!(
        compile(scene.input(&intent), &cancel).unwrap_err().code,
        ManufacturingErrorCode::InvalidSpecification
    );
    let mut parameters = image_parameters(0.06, 1.2);
    parameters.z_period_mm = 0.001;
    parameters.z_phase_mm = 0.0;
    install_image(&mut scene, false, 2, 2, &[0, 255, 255, 0], parameters);
    let report = preflight(&scene.input(&intent), &cancel).unwrap();
    assert!(!report.eligible);
    assert_eq!(
        compile(scene.input(&intent), &cancel).unwrap_err().code,
        ManufacturingErrorCode::ResourceLimit
    );
}
