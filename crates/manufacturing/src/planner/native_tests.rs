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
}
impl NativeScene {
    fn new(bytes: &[u8], poses: &[RigidPoseMm]) -> Self {
        let native =
            Arc::new(spiling_geometry::import_step(bytes, &AtomicBool::new(false)).unwrap());
        let id = native.id().clone();
        Self {
            project: ProjectId::default(),
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
    assert!(
        (bundle
            .plan
            .layers
            .iter()
            .find(|layer| layer.region_coverage.section_area_mm2 > 0.0)
            .unwrap()
            .height_mm
            - 0.4)
            .abs()
            < 1e-8
    );
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
    let error = compile(scene.input(&enabled), &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::UnsupportedGeometry);
    let floating = NativeScene::new(BOX, &[translated(0.0, 0.0, 2.0)]);
    enabled.recipe.supports = SupportPolicy::Breakaway {
        density: 0.3,
        xy_clearance_mm: 0.2,
        z_gap_mm: 3.0,
        interface_thickness_mm: 0.8,
    };
    let error = compile(floating.input(&enabled), &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::UnsupportedGeometry);
}
