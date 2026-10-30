// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::geometry::{AabbMm, OccurrenceId, RigidPoseMm, SourceHash};
use spiling_contracts::process::{
    ProcessAction, ProcessLayer, ProcessLayerId, ProcessOperation, ProcessOperationId,
    ProcessSelector, ProcessValue,
};
use spiling_contracts::surface::SurfaceScope;

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
            deposition_limits: spiling_contracts::manufacturing::DepositionLimits {
                min_bead_width_mm: 0.3,
                max_bead_width_mm: 0.75,
                min_bead_height_mm: 0.000001,
                // Test-only envelope leaves room for actual commanded width
                // and height enclosures; this is not a process-boundary test.
                max_bead_height_mm: 0.5,
            },
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
            sparse_infill: spiling_contracts::manufacturing::SparseInfillPolicy::Fine,
            perimeter_count: 1,
            seam_policy: SeamPolicy::Nearest,
            rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
            top_skin_axis: RectilinearAxisPolicy::AlternatingXY,
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
    process_document: spiling_contracts::process::ProcessDocument,
    surface_images:
        BTreeMap<spiling_contracts::geometry::SourceHash, Arc<spiling_surface_fields::HeightImage>>,
}
impl Scene {
    fn new(bytes: &[u8], poses: &[RigidPoseMm]) -> Self {
        let native = Arc::new(
            spiling_geometry::from_step_bytes_with_cancel(bytes, &AtomicBool::new(false)).unwrap(),
        );
        let id = native.id().clone();
        let mut provenance = native.provenance().clone();
        provenance.source_name = "operator captured source.step".into();
        Self {
            id: ProjectId::default(),
            surface_overlays: spiling_contracts::surface::SurfaceOverlays::empty(),
            process_document: spiling_contracts::process::ProcessDocument::empty(),
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
    fn set_process_fractions(&mut self, fractions: &[f64]) {
        assert_eq!(fractions.len(), self.occurrences.len());
        self.process_document.layers = self
            .occurrences
            .iter()
            .zip(fractions)
            .enumerate()
            .map(|(index, (occurrence, &fraction))| ProcessLayer {
                layer_id: ProcessLayerId::new(index as u32 + 1).unwrap(),
                scope: SurfaceScope::Occurrence {
                    occurrence_id: occurrence.occurrence_id,
                    expected_definition_id: occurrence.definition_id.clone(),
                },
                enabled: true,
                operations: vec![ProcessOperation {
                    operation_id: ProcessOperationId::new(index as u32 + 1).unwrap(),
                    selector: ProcessSelector::WholeOwnedVolume,
                    interface: None,
                    action: ProcessAction::Replace {
                        value: ProcessValue::SparseFraction { fraction },
                    },
                }],
            })
            .collect();
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
            process_document: &self.process_document,
            surface_images: &self.surface_images,
        }
    }
}
const BOX: &[u8] = include_bytes!("../../../fixtures/geometry/box-mm.step");
const CYLINDER: &[u8] = include_bytes!("../../../fixtures/geometry/cylinder.step");
const HOLE: &[u8] = include_bytes!("../../../fixtures/geometry/through-hole.step");

fn surface_color_intent(scene: &Scene) -> ManufacturingIntent {
    let mut intent: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../fixtures/manufacturing/fixed-dual.intent.json"
    ))
    .unwrap();
    let native = scene.definitions.values().next().unwrap();
    let occurrence = &scene.occurrences[0];
    let face = native
        .faces()
        .iter()
        .find(|face| {
            native
                .vertical_wall_face(&face.face_id)
                .is_ok_and(|f| f.outward_normal[1] < -0.5)
        })
        .unwrap();
    let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
        unreachable!()
    };
    job.assignments.truncate(1);
    job.assignments[0].material = OccurrenceMaterial::Uniform {
        material_id: job.tools[1].material.id.clone(),
    };
    let bounds = native.bounds_mm();
    job.assignments[0].work_envelope_mm = AabbMm::new(
        std::array::from_fn(|i| bounds.min[i] + occurrence.pose.translation_mm[i] - 1.0),
        std::array::from_fn(|i| bounds.max[i] + occurrence.pose.translation_mm[i] + 1.0),
    )
    .unwrap();
    job.surface_constructions = vec![SurfaceColorConstruction {
        optical_origin: None,
        occurrence_id: occurrence.occurrence_id,
        definition_id: occurrence.definition_id.clone(),
        source_hash: native.provenance().source_hash.clone(),
        faces: vec![SurfaceColorFace {
            mask: None,
            face_id: face.face_id.clone(),
            skin_depth_mm: 1.2,
            backing_depth_mm: 1.2,
        }],
        shell: OccurrenceMaterial::Uniform {
            material_id: job.tools[0].material.id.clone(),
        },
        white_material_id: job.tools[1].material.id.clone(),
    }];
    intent
}

#[test]
fn native_face_color_constructs_owned_skin_backing_core_and_replays_protected_hole() {
    for bytes in [BOX, HOLE] {
        let scene = Scene::new(bytes, &[pose(20.0, 20.0, 0.0)]);
        let mut intent = surface_color_intent(&scene);
        intent.recipe.infill_fraction = 0.25;
        let bundle = scene.compile(&intent).unwrap();
        assert!(!bundle.plan.material_domains.is_empty());
        for role in [
            MaterialConstructionRole::Shell,
            MaterialConstructionRole::Backing,
            MaterialConstructionRole::Core,
        ] {
            assert!(
                bundle
                    .plan
                    .layers
                    .iter()
                    .flat_map(|l| &l.paths)
                    .any(|p| p.material_domain.is_some_and(|r| r.role == role))
            );
        }
        assert!(
            bundle
                .plan
                .layers
                .iter()
                .all(|l| l.paths.iter().all(|p| p.material_domain.is_some()))
        );
        if bytes == HOLE {
            assert!(
                bundle
                    .plan
                    .layers
                    .iter()
                    .all(|l| l.region_coverage.protected_hole_area_mm2 > 0.0)
            );
        }
        let saved = serde_json::to_vec(&bundle).unwrap();
        let restored = decode_bundle(&saved).unwrap();
        assert_eq!(bundle.program, restored.program);
        assert!(
            verify_bundle(&restored, &AtomicBool::new(false))
                .unwrap()
                .verified
        );
        let mut wrong = restored.clone();
        let path = wrong
            .plan
            .layers
            .iter_mut()
            .flat_map(|l| &mut l.paths)
            .find(|p| p.material_domain.unwrap().role == MaterialConstructionRole::Shell)
            .unwrap();
        path.material_domain.as_mut().unwrap().role = MaterialConstructionRole::Core;
        assert!(
            verify_bundle(&wrong, &AtomicBool::new(false)).is_err(),
            "matching geometry cannot authorize the wrong material"
        );
    }
}

#[test]
fn native_face_color_cycle_keeps_waste_only_groups_and_physical_partial_intervals() {
    use spiling_contracts::optical::{LayerCycle, OpticalSlot};
    let scene = Scene::new(BOX, &[pose(20.0, 20.0, 0.0)]);
    let mut intent = surface_color_intent(&scene);
    let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
        unreachable!()
    };
    let material_ids = job.tools.each_ref().map(|tool| tool.material.id.clone());
    job.surface_constructions[0].shell = OccurrenceMaterial::LayerCycle {
        recipe: LayerCycle {
            slots: vec![
                OpticalSlot {
                    material: 0,
                    height_mm: 0.2,
                },
                OpticalSlot {
                    material: 1,
                    height_mm: 0.2,
                },
            ],
            phase_slot: 0,
            origin_z_mm: 0.03,
        },
        material_ids,
    };
    let bundle = scene.compile(&intent).unwrap();
    let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
        unreachable!()
    };
    let mut empty_groups = 0;
    let motion = actual_motion(&bundle);
    for layer in &bundle.plan.layers {
        let active = layer
            .paths
            .iter()
            .map(|p| job.tool_for_path(p).unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        if !active.contains(&0) {
            empty_groups += 1;
        }
        assert_eq!(
            motion
                .iter()
                .filter(|(kind, index, _)| *kind == 14 && *index == layer.index)
                .count(),
            2
        );
        assert_eq!(
            motion
                .iter()
                .filter(|(kind, index, _)| *kind == 15 && *index == layer.index)
                .count(),
            2
        );
        let expected = job.surface_constructions[0]
            .shell
            .material_id_for_interval([layer.lower_z_mm, layer.z_mm])
            .unwrap();
        for path in &layer.paths {
            if path.material_domain.unwrap().role == MaterialConstructionRole::Shell {
                assert_eq!(
                    job.tools[usize::from(job.tool_for_path(path).unwrap())]
                        .material
                        .id,
                    expected
                );
            } else {
                assert_eq!(job.tool_for_path(path).unwrap(), 1);
            }
        }
    }
    assert!(empty_groups > 0);
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .any(|layer| layer.z_mm - layer.lower_z_mm < 0.19)
    );
    let routed = bundle.verification.routing.as_ref().unwrap();
    assert_eq!(
        routed.selection_count as usize,
        bundle.plan.layers.len() * 2
    );
    for tool in &routed.tools {
        assert_eq!(
            tool.waste_deposition_segments as usize,
            bundle.plan.layers.len()
        );
    }
}

#[test]
fn native_face_color_rejects_lost_core_wrong_source_and_unprintable_skin() {
    let scene = Scene::new(BOX, &[pose(20.0, 20.0, 0.0)]);
    for kind in 0..3 {
        let mut intent = surface_color_intent(&scene);
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        match kind {
            0 => {
                job.surface_constructions[0].source_hash =
                    SourceHash::from_bytes(b"different source")
            }
            1 => job.surface_constructions[0].faces[0].backing_depth_mm = 100.0,
            _ => job.surface_constructions[0].faces[0].skin_depth_mm = 0.01,
        }
        assert!(scene.compile(&intent).is_err());
    }
}

#[test]
fn finite_surface_window_preserves_white_inheritance_hole_and_source_frame() {
    for bytes in [BOX, HOLE] {
        let scene = Scene::new(bytes, &[pose(20.0, 20.0, 0.0)]);
        let mut intent = surface_color_intent(&scene);
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        job.surface_constructions[0].faces[0].mask = Some(SurfaceColorMask {
            min_u_mm: 4.0,
            max_u_mm: 16.0,
        });
        let bundle = scene.compile(&intent).unwrap();
        assert_eq!(bundle.plan.material_frames[0].selected_u_mm, [4.0, 16.0]);
        assert_eq!(bundle.plan.material_frames[0].origin_mm, [0.0, 0.0, 0.0]);
        assert_eq!(
            bundle.plan.material_frames[0].horizontal_direction,
            [1.0, 0.0, 0.0]
        );
        let first = &bundle.plan.layers[0];
        assert!(
            first
                .paths
                .iter()
                .filter(|p| p.material_domain.unwrap().role == MaterialConstructionRole::Shell)
                .flat_map(|p| &p.points_mm)
                .all(|p| p[0] > 24.0 && p[0] < 36.0)
        );
        assert_eq!(
            first.region_coverage.source_section_area_mm2,
            first.region_coverage.section_area_mm2
        );
        assert!(
            verify_bundle(&bundle, &AtomicBool::new(false))
                .unwrap()
                .verified
        );
        if bytes == HOLE {
            assert!(
                bundle
                    .plan
                    .layers
                    .iter()
                    .all(|l| l.region_coverage.protected_hole_area_mm2 > 0.0)
            );
        }
    }
}

#[test]
fn regional_surface_cycles_resolve_opposite_phases_at_common_z_with_shared_white_backing() {
    use spiling_contracts::optical::{LayerCycle, OpticalSlot};
    for bytes in [BOX, HOLE] {
        let scene = Scene::new(bytes, &[pose(20.0, 20.0, 0.0)]);
        let mut intent = surface_color_intent(&scene);
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        let first = job.surface_constructions[0].clone();
        job.surface_constructions.push(first);
        let material_ids = job.tools.each_ref().map(|t| t.material.id.clone());
        for (index, construction) in job.surface_constructions.iter_mut().enumerate() {
            construction.faces[0].mask = Some(SurfaceColorMask {
                min_u_mm: [2.0, 12.0][index],
                max_u_mm: [8.0, 18.0][index],
            });
            construction.shell = OccurrenceMaterial::LayerCycle {
                recipe: LayerCycle {
                    slots: vec![
                        OpticalSlot {
                            material: 0,
                            height_mm: 0.2,
                        },
                        OpticalSlot {
                            material: 1,
                            height_mm: 0.2,
                        },
                    ],
                    phase_slot: index as u16,
                    origin_z_mm: 0.03,
                },
                material_ids: material_ids.clone(),
            };
        }
        let bundle = scene.compile(&intent).unwrap();
        let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
            unreachable!()
        };
        assert_eq!(bundle.plan.material_frames.len(), 2);
        for layer in &bundle.plan.layers {
            let mut tools = [None; 2];
            for path in &layer.paths {
                let owner = path.material_domain.unwrap();
                let tool = job.tool_for_path(path).unwrap();
                if owner.role == MaterialConstructionRole::Shell {
                    tools[owner.construction_index as usize] = Some(tool);
                } else {
                    assert_eq!(owner.construction_index, 0);
                    assert_eq!(tool, 1);
                }
            }
            assert!(tools[0].is_some() && tools[1].is_some());
            assert_ne!(tools[0], tools[1]);
        }
        actual_motion(&bundle);
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        job.surface_constructions[1].faces[0].mask =
            job.surface_constructions[0].faces[0].mask.clone();
        assert!(
            scene
                .compile(&intent)
                .unwrap_err()
                .message
                .contains("overlap")
        );
    }
}

#[test]
fn adjoining_equal_surface_bands_union_once_and_conflicting_depths_refuse() {
    for source in [BOX, HOLE] {
        let scene = Scene::new(source, &[pose(20.0, 20.0, 0.0)]);
        let native = scene.definitions.values().next().unwrap();
        let side = native
            .faces()
            .iter()
            .find(|face| {
                native
                    .vertical_wall_face(&face.face_id)
                    .is_ok_and(|f| f.outward_normal[0] > 0.5)
            })
            .unwrap();
        let mut intent = surface_color_intent(&scene);
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        let mut next = job.surface_constructions[0].faces[0].clone();
        next.face_id = side.face_id.clone();
        job.surface_constructions[0].faces.push(next);
        let bundle = scene.compile(&intent).unwrap();
        assert_eq!(bundle.plan.material_frames.len(), 2);
        if source == HOLE {
            assert!(
                bundle
                    .plan
                    .layers
                    .iter()
                    .all(|layer| layer.region_coverage.protected_hole_area_mm2 > 0.0)
            );
        }

        assert!(
            verify_bundle(&bundle, &AtomicBool::new(false))
                .unwrap()
                .verified
        );
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        for face in &mut job.surface_constructions[0].faces {
            face.skin_depth_mm = 1.35;
        }
        let edited = scene.compile(&intent).unwrap();
        assert_ne!(bundle.program, edited.program);
        actual_motion(&edited);
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        job.surface_constructions[0].faces[1].skin_depth_mm = 1.5;
        let failure = scene.compile(&intent).unwrap_err();
        assert!(
            failure.message.contains("intersecting face constructions"),
            "{failure}"
        );
    }
}
fn pose(x: f64, y: f64, z: f64) -> RigidPoseMm {
    RigidPoseMm {
        translation_mm: [x, y, z],
        ..RigidPoseMm::IDENTITY
    }
}

fn actual_motion(bundle: &ManufacturingBundle) -> Vec<(u8, u32, [f64; 3])> {
    let bytes = serde_json::to_vec(bundle).unwrap();
    let reopened = decode_bundle(&bytes).unwrap();
    let inspection = inspect_bundle(
        &reopened,
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

fn assert_actual_sparse_row_pitch(
    motion: &[(u8, u32, [f64; 3])],
    source_translation: [f64; 3],
    window: [[f64; 2]; 3],
    bead_width: f64,
    fraction: f64,
) {
    use std::collections::{BTreeMap, BTreeSet};
    let micrometres = |value: f64| (value * 1_000_000.0).round() as i64;
    let levels: BTreeSet<_> = motion
        .iter()
        .filter(|(kind, _, _)| matches!(kind, 1..=5 | 8 | 13))
        .map(|(_, _, end)| micrometres(end[2]))
        .collect();
    let mut rows = BTreeMap::<i64, BTreeSet<i64>>::new();
    for pair in motion.windows(2) {
        let (kind, _, end) = pair[1];
        if kind != 5 {
            continue;
        }
        let start = pair[0].2;
        let a: [f64; 3] = std::array::from_fn(|axis| start[axis] - source_translation[axis]);
        let b: [f64; 3] = std::array::from_fn(|axis| end[axis] - source_translation[axis]);
        if !(window[2][0]..=window[2][1]).contains(&b[2])
            || !(window[1][0]..=window[1][1]).contains(&b[1])
            || a[0].min(b[0]) > window[0][0]
            || a[0].max(b[0]) < window[0][1]
        {
            continue;
        }
        assert!(
            (a[1] - b[1]).abs() < 0.000002,
            "the authored X raster must remain X"
        );
        rows.entry(micrometres(end[2]))
            .or_default()
            .insert(micrometres(end[1]));
    }
    assert!(
        !rows.is_empty(),
        "the source-relative interior window must contain actual sparse rows"
    );
    for (z, coordinates) in rows {
        let lower = *levels.range(..z).next_back().unwrap();
        // Recover actual bead height from emitted deposition levels. Native
        // event-balanced layers need not have the recipe's nominal height.
        let height = (z - lower) as f64 / 1_000_000.0;
        let solid_pitch = bead_width - height + std::f64::consts::PI * height * 0.25;
        let expected = solid_pitch / fraction;
        let coordinates: Vec<_> = coordinates.into_iter().collect();
        assert!(
            coordinates.len() > 1,
            "the window must witness adjacent emitted rows"
        );
        for pair in coordinates.windows(2) {
            let actual = (pair[1] - pair[0]) as f64 / 1_000_000.0;
            assert!(
                (actual - expected).abs() < 0.000005,
                "actual row pitch {actual} at Z={} must be W/rho={expected}, rho={fraction}",
                z as f64 / 1_000_000.0,
            );
        }
    }
}

fn actual_object_geometry(
    motion: &[(u8, u32, [f64; 3])],
    translation: [f64; 3],
    mut keep: impl FnMut(u8, [f64; 3], [f64; 3]) -> bool,
) -> Vec<(u8, [i64; 3], [i64; 3])> {
    let mut geometry = Vec::new();
    for pair in motion.windows(2) {
        let (kind, _, end) = pair[1];
        if !matches!(kind, 1..=5 | 8 | 13) {
            continue;
        }
        let start = std::array::from_fn(|axis| pair[0].2[axis] - translation[axis]);
        let end = std::array::from_fn(|axis| end[axis] - translation[axis]);
        if !keep(kind, start, end) {
            continue;
        }
        let xyz6 = |point: [f64; 3]| point.map(|value| (value * 1_000_000.0).round() as i64);
        let mut a = xyz6(start);
        let mut b = xyz6(end);
        if b < a {
            std::mem::swap(&mut a, &mut b);
        }
        geometry.push((kind, a, b));
    }
    geometry.sort_unstable();
    geometry
}

#[test]
fn native_process_occurrences_keep_actual_row_cadence_skins_and_protected_bore() {
    let placements = [[40.0, 40.0, 0.0], [100.0, 40.0, 0.0]];
    let mut scene = Scene::new(HOLE, &placements.map(|p| pose(p[0], p[1], p[2])));
    scene.set_process_fractions(&[0.20, 0.60]);
    let mut intent = decode_intent(include_bytes!(
        "../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    intent.recipe.infill_fraction = 0.4;
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    intent.recipe.top_skin_axis = RectilinearAxisPolicy::X;
    intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [1.0, 0.0] };
    assert_eq!(
        preflight(&scene.input(&intent), &AtomicBool::new(false))
            .unwrap()
            .disposition,
        PreflightDisposition::Admitted,
    );
    let bundle = scene.compile(&intent).unwrap();
    let motion = actual_motion(&bundle);
    // The unchanged native source is 20x20x8 with an R3 through-bore
    // at (10,10). This source-relative window stays outside the bore
    // and the physical cap-skin horizons.
    for (translation, fraction) in placements.into_iter().zip([0.20, 0.60]) {
        assert_actual_sparse_row_pitch(
            &motion,
            translation,
            [[2.0, 6.0], [3.0, 17.0], [2.2, 3.0]],
            intent.recipe.bead_width_mm,
            fraction,
        );
    }
    let unchanged = placements.map(|translation| {
        actual_object_geometry(&motion, translation, |kind, a, b| {
            let in_source = |p: [f64; 3]| {
                (-0.001..=20.001).contains(&p[0]) && (-0.001..=20.001).contains(&p[1])
            };
            in_source(a)
                && in_source(b)
                && (kind == 1 || kind == 3 && b[2] <= 0.6 || kind == 4 && b[2] >= 7.6)
        })
    });
    for role in [1, 3, 4] {
        assert!(unchanged[0].iter().any(|segment| segment.0 == role));
    }
    assert_eq!(
        unchanged[0], unchanged[1],
        "local sparse fractions must not move actual perimeters or physical cap skins",
    );
    for translation in placements {
        let mut pocket_segments = 0;
        for pair in motion.windows(2) {
            let (kind, _, end) = pair[1];
            if !matches!(kind, 1..=5 | 8 | 13) {
                continue;
            }
            let a = [
                pair[0].2[0] - translation[0] - 10.0,
                pair[0].2[1] - translation[1] - 10.0,
            ];
            let b = [
                end[0] - translation[0] - 10.0,
                end[1] - translation[1] - 10.0,
            ];
            if a[0].abs().max(b[0].abs()) > 10.001 || a[1].abs().max(b[1].abs()) > 10.001 {
                continue;
            }
            let d = [b[0] - a[0], b[1] - a[1]];
            let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0.0, 1.0);
            let closest = (a[0] + t * d[0]).hypot(a[1] + t * d[1]);
            assert!(
                closest >= 3.0 + intent.recipe.bead_width_mm * 0.5 - 0.011,
                "actual emitted bead must preserve the unchanged native through-bore",
            );
            pocket_segments += 1;
        }
        assert!(pocket_segments > 0);
    }
}

#[test]
fn native_process_noop_intent_preserves_actual_commands_and_hatch_phase() {
    let mut scene = Scene::new(HOLE, &[pose(40.0, 40.0, 0.0), pose(100.0, 40.0, 0.0)]);
    let mut intent = decode_intent(include_bytes!(
        "../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    intent.recipe.top_skin_axis = RectilinearAxisPolicy::X;
    assert_eq!(
        preflight(&scene.input(&intent), &AtomicBool::new(false))
            .unwrap()
            .disposition,
        PreflightDisposition::Admitted,
    );
    let baseline = scene.compile(&intent).unwrap();
    let baseline_motion = actual_motion(&baseline);
    assert!(baseline_motion.iter().any(|event| event.0 == 5));
    // Retain all real commands, including E/feed/lifecycle controls and inline
    // process roles; omit only standalone provenance/header comments.
    let baseline_commands: Vec<_> = baseline
        .program
        .lines()
        .filter(|line| !line.trim_start().starts_with(';'))
        .collect();
    let occurrence_scope = SurfaceScope::Occurrence {
        occurrence_id: scene.occurrences[0].occurrence_id,
        expected_definition_id: scene.occurrences[0].definition_id.clone(),
    };
    let unchanged_fraction = ProcessAction::Replace {
        value: ProcessValue::SparseFraction {
            fraction: intent.recipe.infill_fraction,
        },
    };
    for (scope, action) in [
        (occurrence_scope.clone(), ProcessAction::Erase),
        (
            SurfaceScope::Definition {
                definition_id: scene.occurrences[0].definition_id.clone(),
            },
            unchanged_fraction,
        ),
        (occurrence_scope, unchanged_fraction),
    ] {
        scene.process_document.layers = vec![ProcessLayer {
            layer_id: ProcessLayerId::new(1).unwrap(),
            scope,
            enabled: true,
            operations: vec![ProcessOperation {
                operation_id: ProcessOperationId::new(1).unwrap(),
                selector: ProcessSelector::WholeOwnedVolume,
                interface: None,
                action,
            }],
        }];
        let unchanged = scene.compile(&intent).unwrap();
        assert_eq!(
            actual_motion(&unchanged),
            baseline_motion,
            "effective no-op process intent must preserve actual decoded motion and row phase",
        );
        assert_eq!(
            unchanged
                .program
                .lines()
                .filter(|line| !line.trim_start().starts_with(';'))
                .collect::<Vec<_>>(),
            baseline_commands,
            "effective no-op intent must preserve actual machine commands",
        );
    }
}

#[test]
fn native_process_overlap_conflicts_and_admitted_compatible_union_keeps_contributors() {
    let placements = [RigidPoseMm::IDENTITY, pose(10.123456789, 0.234567891, 0.0)];
    let mut incompatible = Scene::new(BOX, &placements);
    incompatible.set_process_fractions(&[0.20, 0.60]);
    let conflict = incompatible.compile(&intent()).unwrap_err();
    assert_eq!(
        conflict.code,
        ManufacturingErrorCode::InvalidSpecification,
        "{conflict}"
    );
    // Coalescing retains the existing admitted separated material union; it
    // grants no additional contacting/overlapping-source native capability.
    let placements = [RigidPoseMm::IDENTITY, pose(40.0, 0.0, 0.0)];
    let mut duplicate = Scene::new(BOX, &placements);
    duplicate.set_process_fractions(&[0.20, 0.20]);
    let baseline = Scene::new(BOX, &placements);
    let mut recipe = intent();
    recipe.recipe.infill_fraction = 0.20;
    let one = baseline.compile(&recipe).unwrap();
    let coalesced = duplicate.compile(&recipe).unwrap();
    assert_eq!(
        actual_object_geometry(&actual_motion(&one), [0.0; 3], |_, _, _| true),
        actual_object_geometry(&actual_motion(&coalesced), [0.0; 3], |_, _, _| true),
        "compatible admitted source union must not duplicate actual fill",
    );
    assert!(coalesced.plan.layers.iter().any(|layer| {
        layer.region_coverage.process_regions.iter().any(|region| {
            region
                .contributions
                .iter()
                .all(|contribution| contribution.provenance.is_some())
                && region
                    .contributions
                    .iter()
                    .map(|contribution| contribution.occurrence_id)
                    .collect::<std::collections::BTreeSet<_>>()
                    == duplicate
                        .occurrences
                        .iter()
                        .map(|occurrence| occurrence.occurrence_id)
                        .collect()
        })
    }));
}

fn author_end_zone(scene: &mut Scene, bounds: [[f64; 3]; 2], authorized: bool) {
    let layer = scene
        .process_document
        .layers
        .iter()
        .map(|l| l.layer_id.get())
        .max()
        .unwrap_or(0)
        + 1;
    let operation = scene
        .process_document
        .layers
        .iter()
        .flat_map(|l| &l.operations)
        .map(|o| o.operation_id.get())
        .max()
        .unwrap_or(0)
        + 1;
    scene.process_document.layers.push(ProcessLayer {
        layer_id: ProcessLayerId::new(layer).unwrap(),
        scope: SurfaceScope::Definition {
            definition_id: scene.occurrences[0].definition_id.clone(),
        },
        enabled: true,
        operations: vec![ProcessOperation {
            operation_id: ProcessOperationId::new(operation).unwrap(),
            selector: ProcessSelector::SourceBoxMm {
                min: bounds[0],
                max: bounds[1],
            },
            interface: authorized
                .then_some(spiling_contracts::process::ProcessInterfacePermission::SeparatorWall),
            action: ProcessAction::Replace {
                value: ProcessValue::SparseFraction { fraction: 0.60 },
            },
        }],
    });
}

#[test]
fn housing_horizontal_plate_is_supported_permanent_construction_with_exact_actual_thickness() {
    use spiling_contracts::process::ProcessInterfacePermission;
    let placed: OccurrenceRecord = serde_json::from_slice(include_bytes!(
        "../../../fixtures/manufacturing/rounded-housing-bed-occurrence.json"
    ))
    .unwrap();
    let mut scene = Scene::new(
        include_bytes!(
            "../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ),
        &[placed.pose],
    );
    scene.set_process_fractions(&[0.60]);
    scene.process_document.layers[0].scope = SurfaceScope::Definition {
        definition_id: placed.definition_id,
    };
    author_end_zone(&mut scene, [[-1., -1., -1.], [25., 19., 5.03]], false);
    let operation = &mut scene.process_document.layers.last_mut().unwrap().operations[0];
    operation.action = ProcessAction::Replace {
        value: ProcessValue::SparseFraction { fraction: 0.20 },
    };
    operation.interface =
        Some(ProcessInterfacePermission::HorizontalDiaphragm { thickness_mm: 0.23 });
    let recipe = decode_intent(include_bytes!(
        "../../../fixtures/manufacturing/housing-ordinary.intent.json"
    ))
    .unwrap();
    let bundle = scene.compile(&recipe).unwrap();
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .flat_map(|layer| &layer.paths)
            .filter(|path| matches!(path.owner, Some(DepositionOwner::RegionalSkinBridge { .. })))
            .count()
            >= 2
    );
    assert_eq!(
        recipe.recipe.rectilinear_axis,
        RectilinearAxisPolicy::AlternatingXY
    );
    let (mixed_layer, mixed_path) = bundle
        .plan
        .layers
        .iter()
        .enumerate()
        .find_map(|(index, layer)| {
            layer
                .paths
                .iter()
                .position(|path| {
                    matches!(path.owner, Some(DepositionOwner::RegionalSkinBridge { .. }))
                })
                .map(|path| (index, path))
        })
        .expect("ordinary alternating interior needs an explicit skin-terminal Bridge");
    for mutation in 0..3 {
        let mut changed = bundle.plan.clone();
        let path = &mut changed.layers[mixed_layer].paths[mixed_path];
        if mutation == 1 {
            let along = usize::from(path.points_mm[0][0] == path.points_mm[2][0]);
            path.points_mm[1][along] = (path.points_mm[1][along] + path.points_mm[0][along]) * 0.5;
        } else if mutation == 2 {
            let Some(DepositionOwner::RegionalSkinBridge { skin_first, .. }) = path.owner else {
                unreachable!()
            };
            let endpoint = if skin_first { 0 } else { 2 };
            let along = usize::from(path.points_mm[0][0] == path.points_mm[2][0]);
            path.points_mm[endpoint][along] +=
                0.05 * (path.points_mm[endpoint][along] - path.points_mm[1][along]).signum();
        } else if let Some(DepositionOwner::RegionalSkinBridge { skin_first, .. }) = &mut path.owner
        {
            *skin_first = !*skin_first;
        }
        assert_reemitted_interface_refuses(changed, &recipe);
    }
    for stripped_owner in [
        None,
        Some(DepositionOwner::Regional { region_index: 0 }),
        Some(DepositionOwner::MandatorySkin {
            skin: ProcessSkinKind::TopSkin,
        }),
    ] {
        let mut stripped = bundle.plan.clone();
        stripped.layers[mixed_layer].paths[mixed_path].owner = stripped_owner;
        assert!(
            verifier::verify_actual(
                &bundle.program,
                &stripped,
                &recipe,
                &AtomicBool::new(false),
                None
            )
            .is_err(),
            "same actual program must not lose one mixed purpose while other mixed paths retain the domain"
        );
    }
    let plates = bundle
        .plan
        .layers
        .iter()
        .filter(|layer| !layer.region_coverage.process_diaphragms.is_empty())
        .collect::<Vec<_>>();
    assert!(plates.len() > 1);
    let first = plates[0];
    let last = plates.last().unwrap();
    let requested = &first.region_coverage.process_diaphragms[0];
    assert_eq!(first.lower_z_mm, requested.physical_z_mm[0]);
    assert_eq!(last.z_mm, requested.physical_z_mm[1]);
    assert!(
        plates
            .iter()
            .all(|layer| layer.region_coverage.diaphragm_area_mm2 > 0.0
                && layer.region_coverage.solid_interior_area_mm2 == 0.0)
    );
    assert!(
        first
            .paths
            .iter()
            .any(|path| path.kind == DepositionKind::Bridge
                && path.owner
                    == Some(DepositionOwner::Regional {
                        region_index: requested.region_index
                    }))
    );
    assert!(bundle.plan.layers.iter().any(|layer| {
        layer
            .region_coverage
            .process_regions
            .iter()
            .any(|r| r.fraction == 0.20)
    }));
    assert!(bundle.plan.layers.iter().any(|layer| {
        layer
            .region_coverage
            .process_regions
            .iter()
            .any(|r| r.fraction == 0.60)
    }));
    let first_index = first.index as usize;
    let last_index = last.index as usize;
    for remove_permission in [true, false] {
        let mut changed = bundle.clone();
        if remove_permission {
            changed
                .provenance
                .process_document
                .layers
                .last_mut()
                .unwrap()
                .operations[0]
                .interface = None;
        } else {
            // Occurrence scope wins over a later definition box. A valid
            // rehashed document must not lend its old plate the former winner.
            changed.provenance.process_document.layers[0].scope = SurfaceScope::Occurrence {
                occurrence_id: scene.occurrences[0].occurrence_id,
                expected_definition_id: scene.occurrences[0].definition_id.clone(),
            };
        }
        let provenance = &mut changed.provenance;
        provenance.input_hash = input_fingerprint(
            &provenance.project_id,
            &provenance.definitions,
            &provenance.occurrences,
            &provenance.intent,
            &provenance.surface_overlays,
            &provenance.process_document,
        )
        .unwrap();
        provenance.validate().unwrap();
        assert!(changed.validate().is_err());
    }
    let mut missing_interval = bundle.plan.clone();
    let missing = &mut missing_interval.layers[first_index + 1].region_coverage;
    missing.process_diaphragms.clear();
    missing.solid_interior_area_mm2 += missing.diaphragm_area_mm2;
    missing.diaphragm_area_mm2 = 0.0;
    assert!(missing_interval.validate(&recipe).is_err());
    let mut hole_crossing = bundle.plan.clone();
    let bridge = hole_crossing.layers[first_index]
        .paths
        .iter_mut()
        .find(|path| {
            path.kind == DepositionKind::Bridge
                && path.owner
                    == Some(DepositionOwner::Regional {
                        region_index: requested.region_index,
                    })
        })
        .unwrap();
    let shift = scene.occurrences[0].pose.translation_mm;
    bridge.points_mm[0][0] = shift[0] + 12.;
    bridge.points_mm[1][0] = shift[0] + 12.;
    bridge.points_mm[0][1] = shift[1] + 1.;
    bridge.points_mm[1][1] = shift[1] + 17.;
    assert_reemitted_interface_refuses(hole_crossing, &recipe);
    let mut unowned = bundle.plan.clone();
    unowned.layers[first_index]
        .paths
        .iter_mut()
        .find(|path| {
            path.kind == DepositionKind::Bridge
                && path.owner
                    == Some(DepositionOwner::Regional {
                        region_index: requested.region_index,
                    })
        })
        .unwrap()
        .owner = None;
    assert_reemitted_interface_refuses(unowned, &recipe);
    let mut unanchored_plate = bundle.plan.clone();
    unanchored_plate.layers[first_index - 1]
        .paths
        .retain(|path| path.kind == DepositionKind::Perimeter);
    assert_reemitted_interface_refuses(unanchored_plate, &recipe);
    let mut unanchored_upper = bundle.plan.clone();
    let upper = &mut unanchored_upper.layers[last_index + 1];
    let row = upper
        .paths
        .iter_mut()
        .find(|path| {
            matches!(
                path.kind,
                DepositionKind::SparseFill | DepositionKind::Bridge
            ) && matches!(path.owner, Some(DepositionOwner::Regional { .. }))
        })
        .unwrap();
    row.points_mm[0][0] = shift[0] + 11.;
    row.points_mm[1][0] = shift[0] + 13.;
    row.points_mm[0][1] = shift[1] + 9.;
    row.points_mm[1][1] = shift[1] + 9.;
    assert_reemitted_interface_refuses(unanchored_upper, &recipe);
    let mut combined_intent = recipe.clone();
    combined_intent.recipe.sparse_infill = SparseInfillPolicy::CombineEligible {
        max_height_mm: 0.30,
    };
    // Separate fixed-axis whole-width control: no optimization claim is made
    // from changing the ordinary alternating job's bridge/anchor geometry.
    combined_intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    let combined = scene.compile(&combined_intent).unwrap();
    let first_plate = combined
        .plan
        .layers
        .iter()
        .position(|layer| !layer.region_coverage.process_diaphragms.is_empty())
        .unwrap();
    let last_plate = combined
        .plan
        .layers
        .iter()
        .rposition(|layer| !layer.region_coverage.process_diaphragms.is_empty())
        .unwrap();
    let touching = combined
        .plan
        .sparse_bands
        .iter()
        .filter(|band| {
            band.first_layer as usize <= last_plate + 1
                && band.last_layer as usize + 1 >= first_plate
        })
        .collect::<Vec<_>>();
    assert!(!touching.is_empty());
    assert!(touching.iter().all(|band| matches!(
        band.disposition,
        SparseBandDisposition::RetainedFine {
            reason: SparseBandVeto::ChangingDomain | SparseBandVeto::MandatoryNeighborhood
        }
    )));
    assert!(
        combined
            .plan
            .sparse_bands
            .iter()
            .any(|band| band.disposition
                == SparseBandDisposition::RetainedFine {
                    reason: SparseBandVeto::MissingLowerAnchor
                })
    );
    let mut crossing = combined.plan.clone();
    crossing.sparse_bands.push(SparseBandDecision {
        first_layer: first_plate as u32 - 1,
        last_layer: first_plate as u32,
        owner: Some(DepositionOwner::Regional {
            region_index: crossing.layers[first_plate]
                .region_coverage
                .process_diaphragms[0]
                .region_index,
        }),
        disposition: SparseBandDisposition::Combined,
    });
    assert!(crossing.validate(&combined_intent).is_err());
    scene.process_document.layers.last_mut().unwrap().operations[0].interface =
        Some(ProcessInterfacePermission::HorizontalDiaphragm {
            thickness_mm: 0.0001,
        });
    assert_eq!(
        scene.compile(&recipe).unwrap_err().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
}

#[test]
fn pocketed_housing_end_zone_emits_two_actual_cadences_and_one_bonded_open_separator() {
    // Captured actual native center-and-bed result, not a fabricated Z reserve.
    // The child-service regression independently exercises its derivation.
    let placed: OccurrenceRecord = serde_json::from_slice(include_bytes!(
        "../../../fixtures/manufacturing/rounded-housing-bed-occurrence.json"
    ))
    .unwrap();
    let translation = placed.pose.translation_mm;
    let mut scene = Scene::new(
        include_bytes!(
            "../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ),
        &[placed.pose],
    );
    assert_eq!(scene.occurrences[0].definition_id, placed.definition_id);
    scene.set_process_fractions(&[0.20]);
    scene.process_document.layers[0].scope = SurfaceScope::Definition {
        definition_id: placed.definition_id,
    };
    author_end_zone(&mut scene, [[-1., -1., -1.], [8., 19., 11.]], true);
    let recipe = decode_intent(include_bytes!(
        "../../../fixtures/manufacturing/housing-two-zone.intent.json"
    ))
    .unwrap();
    let bundle = scene.compile(&recipe).unwrap();
    let motion = actual_motion(&bundle);
    for (window, fraction) in [
        ([[2., 6.], [2., 16.], [2.2, 3.]], 0.60),
        ([[17., 21.], [2., 16.], [2.2, 3.]], 0.20),
    ] {
        assert_actual_sparse_row_pitch(
            &motion,
            translation,
            window,
            recipe.recipe.bead_width_mm,
            fraction,
        );
    }
    let mut actual_separators = std::collections::BTreeSet::new();
    for pair in motion.windows(2).filter(|p| p[1].0 == 13) {
        let (_, layer, b) = pair[1];
        let a = pair[0].2;
        assert_eq!(a[0], translation[0] + 8.);
        assert_eq!(b[0], translation[0] + 8.);
        assert!(a[1] != b[1]);
        assert!(
            actual_separators.insert((
                layer,
                (a[1].min(b[1]) * 1e6).round() as i64,
                (a[1].max(b[1]) * 1e6).round() as i64
            )),
            "no duplicate separator extrusion"
        );
    }
    assert!(!actual_separators.is_empty());
    for layer in &bundle.plan.layers {
        let c = &layer.region_coverage;
        assert_eq!(
            c.separator_area_mm2,
            c.process_interfaces.iter().map(|i| i.area_mm2).sum::<f64>()
        );
        if !c.process_interfaces.is_empty() {
            assert!(c.process_regions.iter().any(|r| r.fraction == 0.20));
            assert!(c.process_regions.iter().any(|r| r.fraction == 0.60));
        }
    }
    // Actual machine motion above the blind floor must not enter the R3 pocket.
    for pair in motion
        .windows(2)
        .filter(|p| matches!(p[1].0, 1..=5 | 8 | 13) && p[1].2[2] > 4.2)
    {
        let a = [
            pair[0].2[0] - (translation[0] + 12.),
            pair[0].2[1] - (translation[1] + 9.),
        ];
        let b = [
            pair[1].2[0] - (translation[0] + 12.),
            pair[1].2[1] - (translation[1] + 9.),
        ];
        let d = [b[0] - a[0], b[1] - a[1]];
        let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0., 1.);
        let native_xy = if pair[1].0 == 1 {
            bundle.plan.sampling_tolerance_mm
                + bundle.plan.offset_tolerance_mm
                + 0.0000005 * std::f64::consts::SQRT_2
        } else {
            0.
        };
        assert!(
            (a[0] + t * d[0]).hypot(a[1] + t * d[1])
                >= 3. + recipe.recipe.bead_width_mm * 0.5 - native_xy,
            "actual byte bead violates bounded native contour or strict regional pocket exclusion"
        );
    }
    let first = bundle
        .plan
        .layers
        .iter()
        .position(|layer| !layer.region_coverage.process_interfaces.is_empty())
        .unwrap();
    let indices = bundle.plan.layers[first].region_coverage.process_interfaces[0].region_indices;
    let mut wrong_owner = bundle.plan.clone();
    let row = wrong_owner.layers[first]
        .paths
        .iter_mut()
        .find(|p| {
            p.owner
                == Some(DepositionOwner::Regional {
                    region_index: indices[1],
                })
        })
        .unwrap();
    row.owner = Some(DepositionOwner::Regional {
        region_index: indices[0],
    });
    assert_reemitted_interface_refuses(wrong_owner, &recipe);
    let mut excessive = bundle.plan.clone();
    let row = excessive.layers[first]
        .paths
        .iter_mut()
        .find(|p| {
            p.owner
                == Some(DepositionOwner::Regional {
                    region_index: indices[1],
                })
                && p.points_mm.len() == 2
        })
        .unwrap();
    let end = usize::from(row.points_mm[0][0] > row.points_mm[1][0]);
    row.points_mm[end][0] = translation[0] + 8. + recipe.recipe.bead_width_mm * 0.5 - 0.000001;
    assert_reemitted_interface_refuses(excessive, &recipe);
    let mut floating = bundle.plan.clone();
    floating.layers[first - 1]
        .paths
        .retain(|path| path.kind == DepositionKind::Perimeter);
    assert_reemitted_interface_refuses(floating, &recipe);
    // A skin label/witness alone is not a one-hop connection to actual wall
    // deposition. Move both skin ends off walls but keep the real rib contact.
    let mut island = bundle.plan.clone();
    let interface = &island.layers[first].region_coverage.process_interfaces[0];
    let c = interface.coordinate_mm;
    let (skin_kind, old) = interface
        .endpoint_bonds
        .iter()
        .flatten()
        .find_map(|bond| match bond {
            ProcessEndpointBond::Skin {
                skin_kind,
                skin_segment_mm,
                ..
            } => Some((*skin_kind, *skin_segment_mm)),
            _ => None,
        })
        .expect("filleted housing's initial rib has an actual mandatory skin witness");
    let kind = match skin_kind {
        ProcessSkinKind::BottomSkin => DepositionKind::BottomSkin,
        ProcessSkinKind::TopSkin => DepositionKind::TopSkin,
    };
    let path = island.layers[first]
        .paths
        .iter_mut()
        .find(|p| {
            p.kind == kind && p.points_mm.len() == 2 && {
                let xy = [
                    [p.points_mm[0][0], p.points_mm[0][1]],
                    [p.points_mm[1][0], p.points_mm[1][1]],
                ];
                xy == old || xy == [old[1], old[0]]
            }
        })
        .unwrap();
    path.points_mm[0][0] = c - 1.;
    path.points_mm[1][0] = c + 1.;
    let changed = [
        [path.points_mm[0][0], path.points_mm[0][1]],
        [path.points_mm[1][0], path.points_mm[1][1]],
    ];
    for bond in island.layers[first]
        .region_coverage
        .process_interfaces
        .iter_mut()
        .flat_map(|i| i.endpoint_bonds.iter_mut().flatten())
    {
        if let ProcessEndpointBond::Skin {
            skin_segment_mm, ..
        } = bond
            && (*skin_segment_mm == old || *skin_segment_mm == [old[1], old[0]])
        {
            *skin_segment_mm = changed;
        }
    }
    assert_reemitted_interface_refuses(island, &recipe);
}

fn assert_reemitted_interface_refuses(mut plan: NormalizedPrintPlan, intent: &ManufacturingIntent) {
    for layer in &mut plan.layers {
        // Re-emission mutations retain each actual path's explicit dimensions.
        layer.region_coverage.nominal_deposition_volume_mm3 = layer
            .paths
            .iter()
            .filter(|p| {
                !matches!(
                    p.kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                )
            })
            .map(|p| {
                let h = p.height_mm();
                let flow = intent.recipe.flow_multiplier;
                let factor = if p.kind == DepositionKind::Bridge {
                    intent.recipe.process.bridge_flow_factor
                } else {
                    1.
                } * if layer.index == 0 {
                    intent.recipe.process.first_layer_flow_factor
                } else {
                    1.
                };
                p.checked_edges()
                    .unwrap()
                    .map(|edge| {
                        let area = ((edge.width_mm - h) * h
                            + std::f64::consts::PI * (h * 0.5).powi(2))
                            * flow;
                        (edge.points_mm[1][0] - edge.points_mm[0][0])
                            .hypot(edge.points_mm[1][1] - edge.points_mm[0][1])
                            * area
                            * factor
                    })
                    .sum::<f64>()
            })
            .sum();
    }
    plan.validate(intent).unwrap();
    realization::canonicalize(&mut plan, intent, &AtomicBool::new(false)).unwrap();
    let actual = backend::emit(&plan, intent, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        verifier::verify_actual(&actual, &plan, intent, &AtomicBool::new(false), None)
            .unwrap_err()
            .code,
        ManufacturingErrorCode::VerificationFailed
    );
}

#[test]
fn through_bore_splits_actual_separator_and_replay_refuses_a_forged_hole_crossing() {
    let mut scene = Scene::new(HOLE, &[pose(40., 40., 0.)]);
    author_end_zone(&mut scene, [[-1., -1., -1.], [10., 21., 9.]], true);
    let mut intent = decode_intent(include_bytes!(
        "../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    intent.recipe.infill_fraction = 0.20;
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    intent.recipe.top_skin_axis = RectilinearAxisPolicy::X;
    let bundle = scene.compile(&intent).unwrap();
    let motion = actual_motion(&bundle);
    let mut tested = 0;
    for pair in motion.windows(2).filter(|p| p[1].0 == 13) {
        let a = pair[0].2;
        let b = pair[1].2;
        assert_eq!(a[0], 50.);
        assert_eq!(b[0], 50.);
        assert!(
            a[1].max(b[1]) < 47. || a[1].min(b[1]) > 53.,
            "actual split separator must preserve the R3 through-bore"
        );
        tested += 1;
    }
    assert!(tested > 0);
    let mut forged = bundle.plan.clone();
    let layer = forged
        .layers
        .iter_mut()
        .find(|l| {
            l.region_coverage
                .process_interfaces
                .iter()
                .any(|i| i.centerlines_mm.len() == 2)
        })
        .unwrap();
    let interface = layer
        .region_coverage
        .process_interfaces
        .iter_mut()
        .find(|i| i.centerlines_mm.len() == 2)
        .unwrap();
    let joined = [
        interface.centerlines_mm[0][0],
        interface.centerlines_mm[1][1],
    ];
    let bonds = [
        interface.endpoint_bonds[0][0],
        interface.endpoint_bonds[1][1],
    ];
    interface.centerlines_mm = vec![joined];
    interface.endpoint_bonds = vec![bonds];
    layer
        .paths
        .retain(|p| p.kind != DepositionKind::SeparatorWall);
    layer.paths.push(DepositionPath {
        material_domain: None,
        kind: DepositionKind::SeparatorWall,
        owner: None,
        width: WidthProfile::Constant {
            width_mm: intent.recipe.bead_width_mm,
        },
        physical_z_mm: [layer.lower_z_mm, layer.z_mm],
        points_mm: joined.map(|p| [p[0], p[1], layer.z_mm]).to_vec(),
    });
    assert_reemitted_interface_refuses(forged, &intent);
}

#[test]
fn box_requirements_refuse_unauthorized_extra_face_and_positive_sliver() {
    let mut recipe = intent();
    recipe.recipe.infill_fraction = 0.20;
    recipe.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    recipe.recipe.top_skin_axis = RectilinearAxisPolicy::X;
    for (bounds, authorized) in [
        ([[-1., -1., -1.], [8., 21., 9.]], false),
        ([[-1., -1., -1.], [8., 12., 9.]], true),
        ([[-1., -1., -1.], [0.8, 21., 9.]], true),
    ] {
        let mut scene = Scene::new(HOLE, &[pose(40., 40., 0.)]);
        author_end_zone(&mut scene, bounds, authorized);
        assert_eq!(
            scene.compile(&recipe).unwrap_err().code,
            ManufacturingErrorCode::InvalidSpecification,
            "nonempty unfulfilled interface obligation must never become exported success"
        );
    }
    for axis in [
        RectilinearAxisPolicy::Y,
        RectilinearAxisPolicy::AlternatingXY,
        RectilinearAxisPolicy::AlternatingYX,
    ] {
        let mut scene = Scene::new(HOLE, &[pose(40., 40., 0.)]);
        author_end_zone(&mut scene, [[-1., -1., -1.], [8., 21., 9.]], true);
        recipe.recipe.rectilinear_axis = axis;
        recipe.recipe.top_skin_axis = axis;
        assert_eq!(
            scene.compile(&recipe).unwrap_err().code,
            ManufacturingErrorCode::InvalidSpecification
        );
    }
}

#[test]
fn native_process_support_regeneration_keeps_actual_occurrence_row_pitch() {
    let placements = [[40.0, 40.0, 2.0], [80.0, 40.0, 2.0]];
    let mut scene = Scene::new(BOX, &placements.map(|p| pose(p[0], p[1], p[2])));
    scene.set_process_fractions(&[0.20, 0.60]);
    let mut intent = intent();
    intent.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    intent.recipe.top_skin_axis = RectilinearAxisPolicy::X;
    intent.printer.capabilities.generated_supports = true;
    intent.recipe.supports = SupportPolicy::Breakaway {
        density: 0.3,
        xy_clearance_mm: 0.2,
        z_gap_mm: 0.2,
        interface_thickness_mm: 0.8,
    };
    let bundle = scene.compile(&intent).unwrap();
    let motion = actual_motion(&bundle);
    for (translation, fraction) in placements.into_iter().zip([0.20, 0.60]) {
        assert!(motion.iter().any(|(kind, _, end)| {
            matches!(kind, 9 | 10)
                && (translation[0]..=translation[0] + 20.0).contains(&end[0])
                && (translation[1]..=translation[1] + 10.0).contains(&end[1])
                && end[2] < translation[2]
        }));
        assert_actual_sparse_row_pitch(
            &motion,
            translation,
            [[4.0, 16.0], [1.2, 8.8], [2.0, 5.0]],
            intent.recipe.bead_width_mm,
            fraction,
        );
    }
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
            intent.recipe.top_skin_axis = policy;
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
    intent.recipe.top_skin_axis = RectilinearAxisPolicy::Y;
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
    let wall = &bundle.plan.layers[0].paths[0];
    let expected = wall
        .points_mm
        .iter()
        .copied()
        .map(|point| project_xyz6_point_mm(point).unwrap())
        .min_by(|a, b| b[0].total_cmp(&a[0]).then_with(|| a[1].total_cmp(&b[1])))
        .unwrap();
    assert_eq!(
        start, expected,
        "the authored build direction stays in world coordinates after source rotation"
    );
}

#[test]
fn saved_verification_cannot_authorize_an_incompatible_producer() {
    let mut bundle = Scene::new(BOX, &[RigidPoseMm::IDENTITY])
        .compile(&intent())
        .unwrap();
    assert!(bundle.verification.verified);
    for (planner, kernel) in [
        (
            format!("incompatible-{PLANNER_VERSION}"),
            KERNEL_NAME.into(),
        ),
        (PLANNER_VERSION.into(), "monstertruck".into()),
    ] {
        bundle.provenance.planner_version = planner;
        bundle.provenance.kernel.name = kernel;
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
    assert_eq!(ring.owner, Some(DepositionOwner::Wall));
    assert_eq!(ring.points_mm.first(), ring.points_mm.last());
    assert!(
        ring.points_mm
            .iter()
            .all(|point| { (0.0..=20.0).contains(&point[0]) && (0.0..=10.0).contains(&point[1]) })
    );
    // The native box remains 20x10x8. Wall allocation may change, but every
    // nominal stroke must account for its own realized width and physical height.
    assert!(
        (bundle
            .plan
            .layers
            .iter()
            .map(|layer| layer.height_mm)
            .sum::<f64>()
            - 8.0)
            .abs()
            < 1e-7
    );
    let expected = bundle
        .plan
        .layers
        .iter()
        .map(|layer| {
            let coverage = &layer.region_coverage;
            assert!((coverage.source_section_area_mm2 - 200.0).abs() < 1e-7);
            assert!((coverage.section_area_mm2 - 200.0).abs() < 1e-7);
            assert_eq!(coverage.protected_hole_area_mm2, 0.0);
            assert_eq!(coverage.intentional_void_area_mm2, 0.0);
            let volume = layer
                .paths
                .iter()
                .map(|path| {
                    let height = path.height_mm();
                    let factor = intent.recipe.flow_multiplier
                        * if layer.index == 0 {
                            intent.recipe.process.first_layer_flow_factor
                        } else {
                            1.0
                        }
                        * if path.kind == DepositionKind::Bridge {
                            intent.recipe.process.bridge_flow_factor
                        } else {
                            1.0
                        };
                    path.checked_edges()
                        .unwrap()
                        .map(|edge| {
                            let area = (edge.width_mm - height) * height
                                + std::f64::consts::PI * (height * 0.5).powi(2);
                            (edge.points_mm[1][0] - edge.points_mm[0][0])
                                .hypot(edge.points_mm[1][1] - edge.points_mm[0][1])
                                * area
                                * factor
                        })
                        .sum::<f64>()
                })
                .sum::<f64>();
            assert!((coverage.nominal_deposition_volume_mm3 - volume).abs() < 1e-7);
            volume
        })
        .sum::<f64>();
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
    data.printer.deposition_limits.min_bead_width_mm = 12.0;
    data.printer.deposition_limits.max_bead_width_mm = 12.0;
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
    let upper = Arc::new(
        spiling_geometry::from_step_bytes_with_cancel(CYLINDER, &AtomicBool::new(false)).unwrap(),
    );
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
    bundle.plan.layers[0].z_mm = 0.4;
    bundle.plan.layers[0].paths = vec![
        DepositionPath {
            material_domain: None,
            kind: DepositionKind::SolidFill,
            owner: None,
            width: WidthProfile::Constant {
                width_mm: intent.recipe.bead_width_mm,
            },
            physical_z_mm: [0.0, 0.4],
            points_mm: vec![[1.000_000_4, 1.0, 0.4], [5.0, 1.0, 0.4]],
        },
        DepositionPath {
            material_domain: None,
            kind: DepositionKind::SolidFill,
            owner: None,
            width: WidthProfile::Constant {
                width_mm: intent.recipe.bead_width_mm,
            },
            physical_z_mm: [0.0, 0.4],
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
    // Dose follows the actual XYZ6 chord, including the first endpoint's visible
    // rounding, and replay preserves the emitted eight-decimal absolute E.
    let filament_area = std::f64::consts::PI * (recipe.filament_diameter_mm * 0.5).powi(2);
    let mut expected_extrusion = 0.0;
    for (index, record) in replay.bytes[REPLAY_HEADER_BYTES..]
        .as_chunks::<REPLAY_RECORD_BYTES>()
        .0
        .iter()
        .enumerate()
    {
        if record[46] != 0 {
            let a = positions[index - 1];
            let b = positions[index];
            expected_extrusion +=
                (b[0] - a[0]).hypot(b[1] - a[1]).hypot(b[2] - a[2]) * bead_area / filament_area;
        }
        assert_eq!(
            f64::from_le_bytes(record[24..32].try_into().unwrap()),
            (expected_extrusion * 100_000_000.0).round() / 100_000_000.0
        );
    }
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
    // The ABI golden owns decoded layout and motion, not producer-dependent identity.
    use std::fmt::Write;
    let mut hex = String::with_capacity(replay.bytes.len() * 2);
    for byte in &replay.bytes {
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    let mut fixture = serde_json::json!({ "encoding":"hex", "descriptor":descriptor,
        "data":hex, "actual_positions":positions, "program":bundle.program });
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/protocol/replay.json"
    );
    if std::env::var_os("SPILING_UPDATE_REPLAY_FIXTURE").is_some() {
        std::fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    } else {
        let mut committed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for value in [&mut committed, &mut fixture] {
            value["descriptor"]["info"]
                .as_object_mut()
                .unwrap()
                .remove("bundle_hash");
        }
        assert_eq!(committed, fixture, "native replay ABI golden drift");
    }
}
