// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
pub(crate) fn asset(bytes: Vec<u8>, name: &str) -> Arc<SourceAsset> {
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    Arc::new(
        SourceAsset::new(
            StoredDefinition {
                definition_id: DefinitionId::from_source_sha256(&digest),
                provenance: SourceProvenance {
                    source_hash: SourceHash::from_digest(&digest),
                    source_name: name.to_owned(),
                    source_unit: SourceUnit::Millimetre,
                    uncertainty_mm: Some(0.001),
                },
            },
            bytes,
        )
        .unwrap(),
    )
}
pub(crate) fn image_asset(pixels: [u8; 4], name: &str) -> Arc<SurfaceImageAsset> {
    image_asset_with_capacity(pixels, name, 15)
}
fn image_asset_with_capacity(
    pixels: [u8; 4],
    name: &str,
    capacity: usize,
) -> Arc<SurfaceImageAsset> {
    let mut bytes = Vec::with_capacity(capacity);
    bytes.extend_from_slice(b"P5\n2 2\n255\n");
    bytes.extend_from_slice(&pixels);
    let record = SurfaceImageRecord {
        image_hash: SourceHash::from_bytes(&bytes),
        width: 2,
        height: 2,
        byte_count: bytes.len() as u64,
        source_hash: SourceHash::from_bytes(name.as_bytes()),
        source_name: name.to_owned(),
    };
    Arc::new(SurfaceImageAsset::new(record, bytes).unwrap())
}
pub(crate) fn image_parameters() -> ImageReliefParameters {
    ImageReliefParameters {
        depth_mm: 0.4,
        u_repeats: 2,
        u_phase_fraction: 0.125,
        z_period_mm: 3.0,
        z_phase_mm: 0.5,
        edge_margin_mm: 0.0,
        vertical_error_mm: 0.05,
    }
}
pub(crate) fn image_edit(project: &Project, asset: Arc<SurfaceImageAsset>) -> Edit {
    Edit::AddImageSurfaceFinish {
        asset,
        scope: definition_scope(project),
        face_id: FaceId::from_step_entity(7).unwrap(),
        parameters: image_parameters(),
    }
}
pub(crate) fn imported() -> Project {
    let mut project = Project::new();
    let edit = project
        .prepare(Edit::Import {
            source: asset(b"immutable STEP source".to_vec(), "part.step"),
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    project.commit(edit).unwrap();
    project
}
fn edit(project: &mut Project, value: Edit) {
    let prepared = project.prepare(value).unwrap();
    project.commit(prepared).unwrap();
}
#[test]
fn captured_vec_and_history_share_source_allocation() {
    let bytes = b"shared original bytes".to_vec();
    let pointer = bytes.as_ptr();
    let source = asset(bytes, "part.step");
    assert_eq!(pointer, source.bytes.as_ptr());
    let mut project = Project::new();
    let prepared = project
        .prepare(Edit::Import {
            source: source.clone(),
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    assert!(project.snapshot().sources.is_empty());
    assert_eq!(
        pointer,
        prepared
            .snapshot()
            .sources
            .values()
            .next()
            .unwrap()
            .bytes
            .as_ptr()
    );
    project.commit(prepared).unwrap();
    let capture = project.capture();
    edit(
        &mut project,
        Edit::Remove {
            occurrence_id: OccurrenceId::new(1).unwrap(),
        },
    );
    assert!(project.snapshot().sources.is_empty());
    assert_eq!(project.retained_sources().len(), 1);
    assert_eq!(pointer, project.retained_sources()[0].bytes.as_ptr());
    assert_eq!(capture.snapshot.occurrences.len(), 1);
    project.undo().unwrap();
    assert!(Arc::ptr_eq(
        project.snapshot().sources.values().next().unwrap(),
        &source
    ));
}
#[test]
fn preparation_is_atomic_stale_and_foreign_commits_fail() {
    let mut project = imported();
    let original = project.info();
    let prepared = project
        .prepare(Edit::Remove {
            occurrence_id: OccurrenceId::new(1).unwrap(),
        })
        .unwrap();
    assert_eq!(project.info(), original);
    let mut other = Project::new();
    assert_eq!(
        other.commit(prepared).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
    let prepared = project
        .prepare(Edit::Remove {
            occurrence_id: OccurrenceId::new(1).unwrap(),
        })
        .unwrap();
    edit(
        &mut project,
        Edit::SetPose {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert_eq!(
        project.commit(prepared).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
    let before = project.info();
    let bad = RigidPoseMm {
        translation_mm: [f64::NAN, 0.0, 0.0],
        ..RigidPoseMm::IDENTITY
    };
    assert_eq!(
        project
            .prepare(Edit::SetPose {
                occurrence_id: OccurrenceId::new(1).unwrap(),
                pose: bad
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    assert_eq!(project.info(), before);
}
#[test]
fn undo_redo_bound_revision_and_allocator_never_rewind() {
    let mut project = imported();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    edit(
        &mut project,
        Edit::Add {
            definition_id: definition_id.clone(),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    project.undo().unwrap();
    assert_eq!(project.info().revision.get(), 3);
    assert_eq!(project.capture().manifest.next_occurrence, 3);
    edit(
        &mut project,
        Edit::Add {
            definition_id,
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert!(
        project
            .snapshot()
            .occurrences
            .contains_key(&OccurrenceId::new(3).unwrap())
    );
    assert_eq!(project.redo().unwrap_err().code, ProjectErrorCode::NoRedo);
    for i in 0..80 {
        edit(
            &mut project,
            Edit::SetPose {
                occurrence_id: OccurrenceId::new(1).unwrap(),
                pose: RigidPoseMm {
                    translation_mm: [i as f64, 0.0, 0.0],
                    ..RigidPoseMm::IDENTITY
                },
            },
        );
    }
    let revision = project.info().revision;
    for _ in 0..64 {
        project.undo().unwrap();
    }
    assert_eq!(project.undo().unwrap_err().code, ProjectErrorCode::NoUndo);
    assert_eq!(project.info().revision.get(), revision.get() + 64);
    for _ in 0..64 {
        project.redo().unwrap();
    }
    assert_eq!(project.redo().unwrap_err().code, ProjectErrorCode::NoRedo);
    assert_eq!(project.info().revision.get(), revision.get() + 128);
}
#[test]
fn duplicate_import_preserves_original_provenance_and_shared_identity() {
    let mut project = imported();
    let original = project.snapshot().sources.values().next().unwrap().clone();
    let duplicate = asset(original.bytes.as_ref().clone(), "other-basename.step");
    edit(
        &mut project,
        Edit::Import {
            source: duplicate,
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert_eq!(project.snapshot().sources.len(), 1);
    assert!(Arc::ptr_eq(
        project.snapshot().sources.values().next().unwrap(),
        &original
    ));
    let mut conflicting = (*original).clone();
    conflicting.record.provenance.source_unit = SourceUnit::Inch;
    assert_eq!(
        project
            .prepare(Edit::Import {
                source: Arc::new(conflicting),
                pose: RigidPoseMm::IDENTITY
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
}
#[test]
fn source_history_budget_rejects_without_mutation_and_releases_pruned_pins() {
    let mut project = Project::new();
    for i in 0..32 {
        edit(
            &mut project,
            Edit::Import {
                source: asset(vec![i + 1], "source.step"),
                pose: RigidPoseMm::IDENTITY,
            },
        );
        edit(
            &mut project,
            Edit::Remove {
                occurrence_id: OccurrenceId::new(i as u32 + 1).unwrap(),
            },
        );
    }
    assert_eq!(project.retained_sources().len(), 32);
    let before = project.info();
    assert_eq!(
        project
            .prepare(Edit::Import {
                source: asset(vec![99], "new.step"),
                pose: RigidPoseMm::IDENTITY
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    for _ in 0..64 {
        project.undo().unwrap();
    }
    assert!(project.snapshot().occurrences.is_empty());
    // Redo still owns 32 distinct sources: admission cannot drop them first.
    let before = project.info();
    assert_eq!(
        project
            .prepare(Edit::Import {
                source: asset(vec![99], "new.step"),
                pose: RigidPoseMm::IDENTITY,
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    // Reuse an already-owned allocation for a fitting transaction that clears redo.
    let retained = project.retained_sources().pop().unwrap();
    edit(
        &mut project,
        Edit::Import {
            source: retained.clone(),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert_eq!(project.retained_sources().len(), 1);
    assert!(Arc::ptr_eq(
        project.snapshot().sources.values().next().unwrap(),
        &retained
    ));
    assert!(!project.info().can_redo);
    edit(
        &mut project,
        Edit::Import {
            source: asset(vec![99], "new.step"),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert_eq!(project.retained_sources().len(), 2);
    assert!(!project.info().can_redo);
}
#[test]
fn persistent_counter_and_revision_exhaustion_are_atomic() {
    let mut project = imported();
    project.next_occurrence = u32::MAX;
    let id = project.snapshot().sources.keys().next().unwrap().clone();
    assert_eq!(
        project
            .prepare(Edit::Add {
                definition_id: id,
                pose: RigidPoseMm::IDENTITY
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    project.revision = ProjectRevision(u32::MAX);
    assert_eq!(
        project.undo().unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.snapshot().occurrences.len(), 1);
}
#[test]
fn source_digest_and_definition_identity_are_checked() {
    let source = asset(vec![1, 2, 3], "part.step");
    assert_eq!(
        SourceAsset::new(source.record.clone(), vec![1, 2, 4])
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    let mut record = source.record.clone();
    record.definition_id = asset(vec![4], "other.step").record.definition_id.clone();
    assert_eq!(
        SourceAsset::new(record, vec![1, 2, 3]).unwrap_err().code,
        ProjectErrorCode::CorruptAsset
    );
}

#[test]
fn retained_raw_byte_limit_counts_history_not_only_active_sources() {
    let mut project = Project::new();
    for tag in 1..=4u8 {
        let source = asset(vec![tag; MAX_SOURCE_BYTES], "large.step");
        edit(
            &mut project,
            Edit::Import {
                source,
                pose: RigidPoseMm::IDENTITY,
            },
        );
        edit(
            &mut project,
            Edit::Remove {
                occurrence_id: OccurrenceId::new(tag as u32).unwrap(),
            },
        );
    }
    assert!(project.snapshot().sources.is_empty());
    assert_eq!(
        project
            .retained_sources()
            .iter()
            .map(|source| source.bytes.len())
            .sum::<usize>(),
        MAX_RETAINED_SOURCE_BYTES
    );
    let before = project.info();
    assert_eq!(
        project
            .prepare(Edit::Import {
                source: asset(vec![5], "over-budget.step"),
                pose: RigidPoseMm::IDENTITY
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
}

pub(crate) fn manufacturing_intent() -> ManufacturingIntent {
    ManufacturingIntent {
        printer: PrinterSpecification {
            schema_version: MANUFACTURING_SCHEMA_VERSION,
            id: "storage-cartesian".into(),
            revision: "1".into(),
            name: "Software storage fixture".into(),
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
            nozzle_diameter_mm: 0.4,
            max_feed_mm_s: 100.0,
            max_volumetric_flow_mm3_s: 10.0,
        },
        recipe: PlanarPrintRecipe {
            id: "solid".into(),
            revision: "1".into(),
            material_id: "fixture".into(),
            material_revision: "1".into(),
            filament_diameter_mm: 1.75,
            layer_height_mm: 0.2,
            native_shape_error_mm: 0.1,
            bead_width_mm: 0.4,
            perimeter_count: 1,
            infill_fraction: 1.0,
            top_skin_thickness_mm: 0.2,
            bottom_skin_thickness_mm: 0.2,
            print_speed_mm_s: 10.0,
            travel_speed_mm_s: 20.0,
            flow_multiplier: 1.0,
            process: spiling_contracts::manufacturing::ProcessPolicy {
                first_layer_speed_factor: 1.0,
                first_layer_flow_factor: 1.0,
                fan_first_layers: 0,
                fan_after_first_layers: 0,
                bridge_speed_factor: 1.0,
                bridge_flow_factor: 1.0,
                bridge_fan_pwm: 0,
                max_bridge_span_mm: 10.0,
                retraction: spiling_contracts::manufacturing::RetractionPolicy::Disabled,
            },
            supports: spiling_contracts::manufacturing::SupportPolicy::Disabled,
        },
    }
}
pub(crate) fn manufacturing_bundle(project: &Project) -> ManufacturingBundle {
    let manifest = project.capture().manifest;
    let intent = manifest.manufacturing_intent.unwrap().as_ref().clone();
    let recipe = &intent.recipe;
    let path_length_mm = 1.0;
    let section_area_mm2 = path_length_mm * recipe.bead_width_mm;
    let volume = path_length_mm
        * ((recipe.bead_width_mm - recipe.layer_height_mm) * recipe.layer_height_mm
            + std::f64::consts::PI * (recipe.layer_height_mm / 2.0).powi(2))
        * recipe.flow_multiplier;
    let filament = volume / (std::f64::consts::PI * (recipe.filament_diameter_mm / 2.0).powi(2));
    let region_coverage = PlanarRegionCoverage {
        section_area_mm2,
        source_section_area_mm2: section_area_mm2,
        surface_added_area_mm2: 0.0,
        wall_area_mm2: 0.0,
        bottom_skin_area_mm2: section_area_mm2,
        top_skin_area_mm2: 0.0,
        solid_interior_area_mm2: 0.0,
        sparse_interior_area_mm2: 0.0,
        intentional_void_area_mm2: 0.0,
        protected_hole_area_mm2: 0.0,
        nominal_deposition_volume_mm3: volume,
        requested_infill_fraction: recipe.infill_fraction,
        support_area_mm2: 0.0,
        support_nominal_deposition_volume_mm3: 0.0,
    };
    ManufacturingBundle {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
        provenance: ManufacturingProvenance {
            input_hash: input_fingerprint(
                &manifest.project_id,
                &manifest.definitions,
                &manifest.occurrences,
                &intent,
                &manifest.surface_overlays,
            )
            .unwrap(),
            project_id: manifest.project_id,
            input_revision: manifest.revision,
            definitions: manifest.definitions,
            occurrences: manifest.occurrences,
            intent,
            surface_overlays: manifest.surface_overlays,
            surface_images: project
                .surface_images()
                .values()
                .map(|asset| SurfaceImageContent {
                    image_hash: asset.record().image_hash.clone(),
                    bytes: asset.bytes().clone(),
                })
                .collect(),
            kernel: KernelIdentity {
                name: "storage-metadata-fixture".into(),
                version: "1".into(),
                revision: "1".into(),
            },
            planner_version: "7".into(),
            emitter_version: "4".into(),
            verifier_version: "4".into(),
        },
        plan: NormalizedPrintPlan {
            schema_version: MANUFACTURING_SCHEMA_VERSION,
            native_geometry: NativeGeometryCoverageSummary {
                requested_shape_error_mm: 0.1,
                achieved_shape_error_mm: 0.0,
                height_analysis_count: 1,
                slab_count: 1,
                certified_layer_count: 1,
                event_count: 2,
                geometry_work: 1,
                evidence: PreflightEvidence::Certified,
                skin_coverage: SkinCoverageScope::ScheduledMaterialDistance,
                support_clearance: SupportClearanceScope::ContinuousTargetOuterOccupancy,
                source_feature_coverage: NativeFeatureCoverageSummary {
                    disjoint_source_layer_count: 1,
                    invariant_source_union_layer_count: 0,
                    native_interface_complex_layer_count: 0,
                    owned_critical_slab_count: 0,
                },
            },
            sampling_tolerance_mm: 0.001,
            surface_tolerance_mm: 0.001,
            offset_tolerance_mm: 0.001,
            layer_quantization_mm: 0.000001,
            surface_resolution: vec![],
            layers: vec![PrintLayer {
                index: 0,
                z_mm: 0.2,
                height_mm: 0.2,
                section_z_mm: 0.1,
                paths: vec![DepositionPath {
                    kind: DepositionKind::BottomSkin,
                    points_mm: vec![[1.0, 1.0, 0.2], [2.0, 1.0, 0.2]],
                }],
                region_coverage,
            }],
        },
        program: format!(
            "; SOFTWARE VALIDATION ONLY, NOT MACHINE READY\nG21\nG90\nM82\nG92 E0\nG0 X1 Y1 Z0.2 F1200\nG1 X2 Y1 Z0.2 E{filament:.9} F600\n"
        ),
        // Storage tests exercise schema/integrity, not an independent replay service.
        verification: VerificationReport {
            verified: false,
            coverage: vec!["analytic storage fixture".into()],
            limitations: vec!["not independently replayed".into()],
            deposition_segments: 1,
            travel_segments: 1,
            deposited_volume_mm3: volume,
            filament_length_mm: filament,
            max_position_error_mm: 0.0,
            max_extrusion_error_mm: 0.0,
            lifecycle_motion_count: 0,
            priming_filament_length_mm: 0.0,
            support_deposition_segments: 0,
            support_deposited_volume_mm3: 0.0,
            support_filament_length_mm: 0.0,
            retraction_count: 0,
            retracted_filament_mm: 0.0,
        },
    }
}
pub(crate) fn bundle_asset(bundle: &ManufacturingBundle) -> Arc<ManufacturingAsset> {
    let mut bytes = serde_json::to_vec(bundle).unwrap();
    // Model exact-length file capture, not serde's geometric spare capacity.
    bytes.shrink_to_fit();
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    Arc::new(
        ManufacturingAsset::new(
            ManufacturingArtifactRecord {
                hash: SourceHash::from_digest(&digest),
                input_hash: bundle.provenance.input_hash.clone(),
                byte_count: bytes.len() as u32,
                summary: bundle.summary(),
            },
            bytes,
        )
        .unwrap(),
    )
}

#[test]
fn exact_floating_totals_admit_durable_bundles_but_reject_changed_records() {
    let project = manufactured();
    let mut bundle = manufacturing_bundle(&project);
    // Core checks durable metadata, not physical/replay claims in this unverified fixture.
    let volume = 230.33379795276542;
    bundle.verification.deposited_volume_mm3 = volume;
    let asset = bundle_asset(&bundle);
    assert_eq!(
        asset
            .validate()
            .unwrap()
            .summary()
            .deposited_volume_mm3
            .to_bits(),
        volume.to_bits()
    );
    let mut changed = asset.record.clone();
    changed.summary.deposited_volume_mm3 = f64::from_bits(volume.to_bits() + 1);
    assert_eq!(
        ManufacturingAsset::new(changed, asset.bytes.as_ref().clone())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset,
    );
}
pub(crate) fn manufactured() -> Project {
    let mut project = imported();
    edit(
        &mut project,
        Edit::SetManufacturingIntent {
            intent: (manufacturing_intent()).into(),
        },
    );
    let asset = bundle_asset(&manufacturing_bundle(&project));
    edit(&mut project, Edit::PublishManufacturing { asset });
    project
}
#[test]
fn manufacturing_geometry_and_intent_history_restore_coherent_shared_assets() {
    let mut project = manufactured();
    let original = project.snapshot().manufacturing_artifact.clone().unwrap();
    let revision = project.info().revision;
    edit(
        &mut project,
        Edit::SetPose {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            pose: RigidPoseMm {
                translation_mm: [3.0, 0.0, 0.0],
                ..RigidPoseMm::IDENTITY
            },
        },
    );
    assert!(project.snapshot().manufacturing_artifact.is_none());
    assert_eq!(project.retained_manufacturing_assets().len(), 1);
    project.undo().unwrap();
    assert!(Arc::ptr_eq(
        &original,
        project.snapshot().manufacturing_artifact.as_ref().unwrap()
    ));
    assert!(project.info().revision > revision);
    project.redo().unwrap();
    assert!(project.snapshot().manufacturing_artifact.is_none());
    project.undo().unwrap();
    let mut intent = manufacturing_intent();
    intent.recipe.top_skin_thickness_mm = 0.4;
    edit(
        &mut project,
        Edit::SetManufacturingIntent {
            intent: intent.into(),
        },
    );
    assert!(project.snapshot().manufacturing_artifact.is_none());
    project.undo().unwrap();
    assert!(Arc::ptr_eq(
        &original,
        project.snapshot().manufacturing_artifact.as_ref().unwrap()
    ));
}
#[test]
fn manufacturing_publication_rejects_stale_foreign_and_semantic_mismatch_atomically() {
    let mut project = manufactured();
    let before = project.info();
    let original = project.snapshot().manufacturing_artifact.clone().unwrap();
    assert_eq!(
        project
            .prepare(Edit::PublishManufacturing { asset: original })
            .unwrap_err()
            .code,
        ProjectErrorCode::StaleRevision
    );
    let mut bundle = manufacturing_bundle(&project);
    bundle.provenance.project_id = ProjectId::new();
    bundle.provenance.input_hash = input_fingerprint(
        &bundle.provenance.project_id,
        &bundle.provenance.definitions,
        &bundle.provenance.occurrences,
        &bundle.provenance.intent,
        &bundle.provenance.surface_overlays,
    )
    .unwrap();
    assert_eq!(
        project
            .prepare(Edit::PublishManufacturing {
                asset: bundle_asset(&bundle)
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    let mut bundle = manufacturing_bundle(&project);
    bundle.provenance.intent.recipe.print_speed_mm_s = 11.0;
    bundle.provenance.input_hash = input_fingerprint(
        &bundle.provenance.project_id,
        &bundle.provenance.definitions,
        &bundle.provenance.occurrences,
        &bundle.provenance.intent,
        &bundle.provenance.surface_overlays,
    )
    .unwrap();
    assert_eq!(
        project
            .prepare(Edit::PublishManufacturing {
                asset: bundle_asset(&bundle)
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    assert_eq!(project.info(), before);
    let prepared = project
        .prepare(Edit::SetManufacturingIntent {
            intent: (manufacturing_intent()).into(),
        })
        .unwrap();
    project.undo().unwrap();
    assert_eq!(
        project.commit(prepared).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
}
#[test]
fn manufacturing_vec_hash_count_summary_and_strict_schema_are_validated() {
    let project = manufactured();
    let asset = project.snapshot().manufacturing_artifact.clone().unwrap();
    let mut wrong = asset.record.clone();
    wrong.summary.layers += 1;
    assert_eq!(
        ManufacturingAsset::new(wrong, asset.bytes.as_ref().clone())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    let mut wrong = asset.record.clone();
    wrong.byte_count -= 1;
    assert_eq!(
        ManufacturingAsset::new(wrong, asset.bytes.as_ref().clone())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    let mut bytes = asset.bytes.as_ref().clone();
    bytes[0] = b'[';
    assert_eq!(
        ManufacturingAsset::new(asset.record.clone(), bytes)
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    let mut object = serde_json::to_value(manufacturing_bundle(&project)).unwrap();
    object["unknown"] = serde_json::json!(true);
    let bytes = serde_json::to_vec(&object).unwrap();
    let mut record = asset.record.clone();
    record.hash = SourceHash::from_digest(&Sha256::digest(&bytes).into());
    record.byte_count = bytes.len() as u32;
    assert_eq!(
        ManufacturingAsset::new(record, bytes).unwrap_err().code,
        ProjectErrorCode::CorruptAsset
    );
}
#[test]
fn intent_only_project_is_semantically_dirty() {
    let mut project = Project::new();
    assert!(!project.info().dirty);
    edit(
        &mut project,
        Edit::SetManufacturingIntent {
            intent: (manufacturing_intent()).into(),
        },
    );
    assert!(project.info().dirty);
    project.undo().unwrap();
    assert!(!project.info().dirty);
}

#[test]
fn manufacturing_retained_history_budget_is_separate_and_failed_publish_is_atomic() {
    let mut project = imported();
    edit(
        &mut project,
        Edit::SetManufacturingIntent {
            intent: (manufacturing_intent()).into(),
        },
    );
    for _ in 0..4 {
        let mut bundle = manufacturing_bundle(&project);
        bundle.program.push(';');
        bundle
            .program
            .extend(std::iter::repeat_n('x', 15 * 1024 * 1024));
        let asset = bundle_asset(&bundle);
        edit(&mut project, Edit::PublishManufacturing { asset });
        edit(
            &mut project,
            Edit::SetManufacturingIntent {
                intent: (manufacturing_intent()).into(),
            },
        );
    }
    assert!(project.snapshot().manufacturing_artifact.is_none());
    assert_eq!(project.retained_manufacturing_assets().len(), 4);
    assert_eq!(project.retained_sources().len(), 1);
    let before = project.info();
    let mut bundle = manufacturing_bundle(&project);
    bundle.program.push(';');
    bundle
        .program
        .extend(std::iter::repeat_n('x', 15 * 1024 * 1024));
    assert_eq!(
        project
            .prepare(Edit::PublishManufacturing {
                asset: bundle_asset(&bundle)
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    project.undo().unwrap();
    assert!(project.snapshot().manufacturing_artifact.is_some());
}

fn surface_edit(project: &Project, scope: SurfaceScope, finish: SurfaceFinish) -> Edit {
    assert!(!project.snapshot().sources.is_empty());
    Edit::AddSurfaceFinish {
        scope,
        face_id: FaceId::from_step_entity(7).unwrap(),
        finish,
    }
}
fn definition_scope(project: &Project) -> SurfaceScope {
    SurfaceScope::Definition {
        definition_id: project.snapshot().sources.keys().next().unwrap().clone(),
    }
}
#[test]
fn surface_transactions_share_immutable_history_and_never_reuse_ids() {
    let mut project = imported();
    let original = project.snapshot().surface_overlays.clone();
    let scope = definition_scope(&project);
    let add = surface_edit(&project, scope.clone(), SurfaceFinish::Smooth);
    edit(&mut project, add);
    let authored = project.snapshot().surface_overlays.clone();
    assert!(original.layers.is_empty());
    assert!(!Arc::ptr_eq(&authored, &original));
    assert_eq!(authored.layers[0].layer_id.get(), 1);
    let prepared = project
        .prepare(Edit::SetSurfaceFinishEnabled {
            layer_id: authored.layers[0].layer_id,
            enabled: false,
        })
        .unwrap();
    edit(
        &mut project,
        Edit::SetPose {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert!(Arc::ptr_eq(&authored, &project.snapshot().surface_overlays));
    assert_eq!(
        project.commit(prepared).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
    project.undo().unwrap();
    project.undo().unwrap();
    assert!(Arc::ptr_eq(&original, &project.snapshot().surface_overlays));
    let add = surface_edit(&project, scope, SurfaceFinish::Smooth);
    edit(&mut project, add);
    assert_eq!(project.info().surface_overlays.layers[0].layer_id.get(), 2);
    assert_eq!(project.capture().manifest.next_surface_layer, 3);
    let before = project.info();
    assert_eq!(
        project
            .prepare(Edit::RemoveSurfaceFinish {
                layer_id: SurfaceLayerId::new(50).unwrap()
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    assert_eq!(project.info(), before);
}
#[test]
fn surface_cascade_preserves_shared_definition_until_last_occurrence_and_undo_restores() {
    let mut project = imported();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    edit(
        &mut project,
        Edit::Add {
            definition_id: definition_id.clone(),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    let add = surface_edit(
        &project,
        SurfaceScope::Definition {
            definition_id: definition_id.clone(),
        },
        SurfaceFinish::Smooth,
    );
    edit(&mut project, add);
    let add = surface_edit(
        &project,
        SurfaceScope::Occurrence {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            expected_definition_id: definition_id,
        },
        SurfaceFinish::Smooth,
    );
    edit(&mut project, add);
    edit(
        &mut project,
        Edit::Remove {
            occurrence_id: OccurrenceId::new(1).unwrap(),
        },
    );
    assert_eq!(project.info().surface_overlays.layers.len(), 1);
    let definition_doc = project.snapshot().surface_overlays.clone();
    edit(
        &mut project,
        Edit::Remove {
            occurrence_id: OccurrenceId::new(2).unwrap(),
        },
    );
    assert!(project.info().surface_overlays.layers.is_empty());
    project.undo().unwrap();
    assert!(Arc::ptr_eq(
        &definition_doc,
        &project.snapshot().surface_overlays
    ));
    project.undo().unwrap();
    assert_eq!(project.info().surface_overlays.layers.len(), 2);
}
#[test]
fn surface_limits_unknown_targets_and_allocator_exhaustion_are_atomic() {
    let mut project = imported();
    let unknown = SurfaceScope::Definition {
        definition_id: DefinitionId::from_source_sha256(&[1; 32]),
    };
    assert_eq!(
        project
            .prepare(surface_edit(&project, unknown, SurfaceFinish::Smooth))
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    let scope = definition_scope(&project);
    for _ in 0..MAX_SURFACE_LAYERS {
        let add = surface_edit(&project, scope.clone(), SurfaceFinish::Smooth);
        edit(&mut project, add);
    }
    let before = project.info();
    assert_eq!(
        project
            .prepare(surface_edit(&project, scope, SurfaceFinish::Smooth))
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    let mut project = imported();
    project.next_surface_layer = u32::MAX;
    assert_eq!(
        project
            .prepare(surface_edit(
                &project,
                definition_scope(&project),
                SurfaceFinish::Smooth
            ))
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
}
#[test]
fn surface_edits_invalidate_exact_artifact_and_undo_restores_captured_provenance() {
    let mut project = imported();
    edit(
        &mut project,
        Edit::SetManufacturingIntent {
            intent: manufacturing_intent().into(),
        },
    );
    let add = surface_edit(&project, definition_scope(&project), SurfaceFinish::Smooth);
    edit(&mut project, add);
    let bundle = manufacturing_bundle(&project);
    let asset = bundle_asset(&bundle);
    edit(
        &mut project,
        Edit::PublishManufacturing {
            asset: asset.clone(),
        },
    );
    let id = project.info().surface_overlays.layers[0].layer_id;
    edit(
        &mut project,
        Edit::SetSurfaceFinishEnabled {
            layer_id: id,
            enabled: false,
        },
    );
    assert!(project.snapshot().manufacturing_artifact.is_none());
    assert!(!project.info().surface_overlays.layers[0].enabled);
    project.undo().unwrap();
    assert!(Arc::ptr_eq(
        project.snapshot().manufacturing_artifact.as_ref().unwrap(),
        &asset
    ));
    assert_eq!(
        project.info().surface_overlays,
        bundle.provenance.surface_overlays
    );
    let mut foreign = manufacturing_bundle(&project);
    foreign.provenance.surface_overlays.layers[0].enabled = false;
    foreign.provenance.input_hash = input_fingerprint(
        &foreign.provenance.project_id,
        &foreign.provenance.definitions,
        &foreign.provenance.occurrences,
        &foreign.provenance.intent,
        &foreign.provenance.surface_overlays,
    )
    .unwrap();
    assert_eq!(
        project
            .prepare(Edit::PublishManufacturing {
                asset: bundle_asset(&foreign)
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
}

#[test]
fn image_factory_validates_exact_canonical_content_without_copying() {
    let bytes = b"P5\n2 2\n255\n\x00\x7f\xff\x01".to_vec();
    let pointer = bytes.as_ptr();
    let record = SurfaceImageRecord {
        image_hash: SourceHash::from_bytes(&bytes),
        width: 2,
        height: 2,
        byte_count: bytes.len() as u64,
        source_hash: SourceHash::from_bytes(b"original PNG"),
        source_name: "source.png".into(),
    };
    let image = SurfaceImageAsset::new(record.clone(), bytes).unwrap();
    assert_eq!(image.bytes().as_ptr(), pointer);
    let mut corrupt = image.bytes().as_ref().clone();
    corrupt[14] ^= 1;
    assert_eq!(
        SurfaceImageAsset::new(record.clone(), corrupt)
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    let mut wrong_dimensions = record;
    wrong_dimensions.width = 3;
    assert!(SurfaceImageAsset::new(wrong_dimensions, image.bytes().as_ref().clone()).is_err());
}

#[test]
fn image_reimport_preserves_first_admission_across_current_undo_and_redo() {
    let mut project = imported();
    let first = image_asset([0, 64, 128, 255], "first.png");
    let incoming = image_asset([0, 64, 128, 255], "other-encoding.png");
    assert_ne!(first.record(), incoming.record());
    let add = image_edit(&project, first.clone());
    edit(&mut project, add);
    let add = image_edit(&project, incoming.clone());
    let prepared = project.prepare(add).unwrap();
    assert!(Arc::ptr_eq(
        prepared.surface_images().values().next().unwrap(),
        &first
    ));
    assert_eq!(
        prepared.surface_overlays().images,
        vec![first.record().clone()]
    );
    project.commit(prepared).unwrap();
    for id in [1, 2] {
        edit(
            &mut project,
            Edit::RemoveSurfaceFinish {
                layer_id: SurfaceLayerId::new(id).unwrap(),
            },
        );
    }
    assert!(project.surface_images().is_empty());
    let add = image_edit(&project, incoming.clone());
    edit(&mut project, add);
    assert!(Arc::ptr_eq(
        project.surface_images().values().next().unwrap(),
        &first
    ));
    project.undo().unwrap();
    assert!(project.surface_images().is_empty());
    let add = image_edit(&project, incoming);
    edit(&mut project, add);
    assert!(Arc::ptr_eq(
        project.surface_images().values().next().unwrap(),
        &first
    ));
    assert_eq!(
        project.info().surface_overlays.images,
        vec![first.record().clone()]
    );
    assert_eq!(project.retained_surface_images().len(), 1);
    assert_eq!(project.info().surface_overlays.layers[0].layer_id.get(), 4);
    // In a fresh project after one undo, only redo owns this image.
    let mut project = imported();
    let add = image_edit(&project, first.clone());
    edit(&mut project, add);
    project.undo().unwrap();
    assert!(project.current.surface_images.is_empty());
    assert!(
        project
            .undo
            .iter()
            .all(|snapshot| snapshot.surface_images.is_empty())
    );
    let prepared = project
        .prepare(image_edit(
            &project,
            image_asset([0, 64, 128, 255], "redo-reimport.png"),
        ))
        .unwrap();
    assert!(Arc::ptr_eq(
        prepared.surface_images().values().next().unwrap(),
        &first
    ));
    assert_eq!(
        prepared.surface_overlays().images,
        vec![first.record().clone()]
    );
}

#[test]
fn image_references_are_sorted_include_disabled_and_prune_only_current() {
    let mut project = imported();
    let first = image_asset([1, 2, 3, 4], "one.pgm");
    let second = image_asset([4, 3, 2, 1], "two.pgm");
    for asset in [first.clone(), second.clone(), first.clone()] {
        let add = image_edit(&project, asset);
        edit(&mut project, add);
    }
    edit(
        &mut project,
        Edit::SetSurfaceFinishEnabled {
            layer_id: SurfaceLayerId::new(2).unwrap(),
            enabled: false,
        },
    );
    let doc = project.info().surface_overlays;
    assert_eq!(doc.images.len(), 2);
    assert!(
        doc.images
            .windows(2)
            .all(|records| records[0].image_hash < records[1].image_hash)
    );
    edit(
        &mut project,
        Edit::RemoveSurfaceFinish {
            layer_id: SurfaceLayerId::new(1).unwrap(),
        },
    );
    assert_eq!(project.surface_images().len(), 2);
    edit(
        &mut project,
        Edit::RemoveSurfaceFinish {
            layer_id: SurfaceLayerId::new(3).unwrap(),
        },
    );
    assert_eq!(project.surface_images().len(), 1);
    assert!(
        project
            .surface_images()
            .contains_key(&second.record().image_hash)
    );
    edit(
        &mut project,
        Edit::Remove {
            occurrence_id: OccurrenceId::new(1).unwrap(),
        },
    );
    assert!(project.surface_images().is_empty());
    assert!(project.info().surface_overlays.images.is_empty());
    project.undo().unwrap();
    assert!(Arc::ptr_eq(
        project.surface_images().values().next().unwrap(),
        &second
    ));
    assert!(!project.info().surface_overlays.layers[0].enabled);
    assert_eq!(project.retained_surface_images().len(), 2);
}

#[test]
fn unknown_image_finish_and_invalid_mapping_fail_atomically() {
    let project = imported();
    let before = project.info();
    let finish = SurfaceFinish::ImageRelief {
        image_hash: SourceHash::from_bytes(b"unknown"),
        parameters: image_parameters(),
    };
    assert_eq!(
        project
            .prepare(surface_edit(&project, definition_scope(&project), finish))
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    let mut parameters = image_parameters();
    parameters.z_phase_mm = parameters.z_period_mm;
    assert_eq!(
        project
            .prepare(Edit::AddImageSurfaceFinish {
                asset: image_asset([0; 4], "invalid.pgm"),
                scope: definition_scope(&project),
                face_id: FaceId::from_step_entity(7).unwrap(),
                parameters,
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    assert_eq!(project.info(), before);
}

#[test]
fn image_capacity_budget_counts_history_and_incoming_before_deduplication() {
    let mut project = imported();
    for tag in 0..4 {
        let add = image_edit(
            &project,
            image_asset_with_capacity([tag; 4], "capacity.pgm", 1024 * 1024),
        );
        edit(&mut project, add);
        let layer_id = project.info().surface_overlays.layers[0].layer_id;
        edit(&mut project, Edit::RemoveSurfaceFinish { layer_id });
    }
    assert!(project.surface_images().is_empty());
    assert_eq!(
        project.retained_surface_image_bytes().unwrap(),
        MAX_SURFACE_IMAGE_RETAINED_BYTES
    );
    let before = project.info();
    let add = image_edit(
        &project,
        image_asset([0; 4], "same-content-new-admission.pgm"),
    );
    assert_eq!(
        project.prepare(add).unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    let retained = project.retained_surface_images().pop().unwrap();
    let add = image_edit(&project, retained.clone());
    edit(&mut project, add);
    assert!(Arc::ptr_eq(
        project.surface_images().values().next().unwrap(),
        &retained
    ));
    assert_eq!(
        project.retained_surface_image_bytes().unwrap(),
        MAX_SURFACE_IMAGE_RETAINED_BYTES
    );
}

#[test]
fn image_budget_includes_concurrent_prepared_candidates_until_released() {
    let project = imported();
    let before = project.info();
    let first = project
        .prepare(image_edit(
            &project,
            image_asset_with_capacity([1; 4], "one.pgm", 2 * 1024 * 1024),
        ))
        .unwrap();
    let second = project
        .prepare(image_edit(
            &project,
            image_asset_with_capacity([2; 4], "two.pgm", 2 * 1024 * 1024),
        ))
        .unwrap();
    assert_eq!(
        project.retained_surface_image_bytes().unwrap(),
        MAX_SURFACE_IMAGE_RETAINED_BYTES
    );
    assert_eq!(
        project
            .prepare(image_edit(&project, image_asset([3; 4], "three.pgm")))
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    drop(first);
    let third = project
        .prepare(image_edit(&project, image_asset([3; 4], "three.pgm")))
        .unwrap();
    assert_eq!(
        project.retained_surface_image_bytes().unwrap(),
        2 * 1024 * 1024 + 15
    );
    drop(second);
    drop(third);
    assert_eq!(project.retained_surface_image_bytes().unwrap(), 0);
}

#[test]
fn live_image_count_and_payload_are_independent_bounded_admission_gates() {
    let mut project = imported();
    for tag in 0..MAX_SURFACE_IMAGES {
        let add = image_edit(&project, image_asset([tag as u8; 4], "live.pgm"));
        edit(&mut project, add);
        let snapshot = project.snapshot();
        assert_eq!(snapshot.surface_images.len(), tag + 1);
        assert_eq!(snapshot.surface_overlays.images.len(), tag + 1);
        assert!(snapshot.surface_overlays.images.capacity() <= MAX_SURFACE_IMAGES);
    }
    let before = project.info();
    assert_eq!(
        project
            .prepare(image_edit(
                &project,
                image_asset([255; 4], "seventeenth.pgm")
            ))
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    let mut project = imported();
    for tag in 0..16u8 {
        let mut bytes = Vec::with_capacity(MAX_SURFACE_IMAGE_BYTES);
        bytes.extend_from_slice(b"P5\n256 256\n255\n");
        bytes.resize(MAX_SURFACE_IMAGE_BYTES, tag);
        let record = SurfaceImageRecord {
            image_hash: SourceHash::from_bytes(&bytes),
            width: 256,
            height: 256,
            byte_count: bytes.len() as u64,
            source_hash: SourceHash::from_bytes(&[tag]),
            source_name: "maximum.pgm".into(),
        };
        let asset = Arc::new(SurfaceImageAsset::new(record, bytes).unwrap());
        let before = project.info();
        let result = project.prepare(image_edit(&project, asset));
        if tag == 15 {
            assert_eq!(result.unwrap_err().code, ProjectErrorCode::ResourceLimit);
            assert_eq!(project.info(), before);
        } else {
            project.commit(result.unwrap()).unwrap();
        }
    }
}

#[test]
fn image_history_restores_asset_and_coherent_bundle_content_provenance() {
    let mut project = manufactured();
    let image = image_asset([0, 128, 255, 32], "relief.png");
    let add = image_edit(&project, image.clone());
    edit(&mut project, add);
    let bundle = manufacturing_bundle(&project);
    assert!(Arc::ptr_eq(
        &bundle.provenance.surface_images[0].bytes,
        image.bytes()
    ));
    let artifact = bundle_asset(&bundle);
    edit(
        &mut project,
        Edit::PublishManufacturing {
            asset: artifact.clone(),
        },
    );
    let layer_id = project.info().surface_overlays.layers[0].layer_id;
    edit(&mut project, Edit::RemoveSurfaceFinish { layer_id });
    assert!(project.surface_images().is_empty());
    assert!(project.snapshot().manufacturing_artifact.is_none());
    project.undo().unwrap();
    assert!(Arc::ptr_eq(
        project.surface_images().values().next().unwrap(),
        &image
    ));
    assert!(Arc::ptr_eq(
        project.snapshot().manufacturing_artifact.as_ref().unwrap(),
        &artifact
    ));
    let mut wrong_input = manufacturing_bundle(&project);
    wrong_input.provenance.surface_overlays.images[0].source_name = "foreign.png".into();
    wrong_input.provenance.input_hash = input_fingerprint(
        &wrong_input.provenance.project_id,
        &wrong_input.provenance.definitions,
        &wrong_input.provenance.occurrences,
        &wrong_input.provenance.intent,
        &wrong_input.provenance.surface_overlays,
    )
    .unwrap();
    let before = project.info();
    assert_eq!(
        project
            .prepare(Edit::PublishManufacturing {
                asset: bundle_asset(&wrong_input)
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    assert_eq!(project.info(), before);
}

#[test]
fn occurrence_image_cascade_preserves_shared_definition_image_until_final_target() {
    let mut project = imported();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    edit(
        &mut project,
        Edit::Add {
            definition_id: definition_id.clone(),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    let definition_image = image_asset([1, 4, 8, 16], "definition.pgm");
    let occurrence_image = image_asset([16, 8, 4, 1], "occurrence.pgm");
    let add = image_edit(&project, definition_image.clone());
    edit(&mut project, add);
    edit(
        &mut project,
        Edit::AddImageSurfaceFinish {
            asset: occurrence_image.clone(),
            scope: SurfaceScope::Occurrence {
                occurrence_id: OccurrenceId::new(1).unwrap(),
                expected_definition_id: definition_id,
            },
            face_id: FaceId::from_step_entity(7).unwrap(),
            parameters: image_parameters(),
        },
    );
    let original = project.snapshot();
    edit(
        &mut project,
        Edit::Remove {
            occurrence_id: OccurrenceId::new(1).unwrap(),
        },
    );
    assert_eq!(project.surface_images().len(), 1);
    assert!(Arc::ptr_eq(
        project.surface_images().values().next().unwrap(),
        &definition_image
    ));
    edit(
        &mut project,
        Edit::Remove {
            occurrence_id: OccurrenceId::new(2).unwrap(),
        },
    );
    assert!(project.surface_images().is_empty());
    project.undo().unwrap();
    project.undo().unwrap();
    assert!(Arc::ptr_eq(&project.snapshot(), &original));
    assert!(Arc::ptr_eq(
        &project.surface_images()[&occurrence_image.record().image_hash],
        &occurrence_image
    ));
}

#[test]
fn stale_image_candidate_cannot_replace_authoritative_assets_or_advance_allocators() {
    let mut project = imported();
    let first = project
        .prepare(image_edit(
            &project,
            image_asset_with_capacity([1; 4], "stale.pgm", 2 * 1024 * 1024),
        ))
        .unwrap();
    let second_image = image_asset_with_capacity([2; 4], "committed.pgm", 2 * 1024 * 1024);
    let second = project
        .prepare(image_edit(&project, second_image.clone()))
        .unwrap();
    project.commit(second).unwrap();
    let before = project.capture().manifest;
    assert_eq!(
        project.commit(first).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
    assert_eq!(project.capture().manifest, before);
    assert!(Arc::ptr_eq(
        project.surface_images().values().next().unwrap(),
        &second_image
    ));
    assert_eq!(
        project.retained_surface_image_bytes().unwrap(),
        2 * 1024 * 1024
    );
}

#[test]
fn bundle_image_content_is_complete_even_for_disabled_layers_and_old_formats_fail() {
    let mut project = manufactured();
    let add = image_edit(&project, image_asset([0, 127, 255, 33], "bundle.pgm"));
    edit(&mut project, add);
    edit(
        &mut project,
        Edit::SetSurfaceFinishEnabled {
            layer_id: SurfaceLayerId::new(1).unwrap(),
            enabled: false,
        },
    );
    let bundle = manufacturing_bundle(&project);
    let check_rejected = |value: &serde_json::Value| {
        let bytes = serde_json::to_vec(value).unwrap();
        let record = ManufacturingArtifactRecord {
            hash: SourceHash::from_bytes(&bytes),
            input_hash: bundle.provenance.input_hash.clone(),
            byte_count: bytes.len() as u32,
            summary: bundle.summary(),
        };
        assert_eq!(
            ManufacturingAsset::new(record, bytes).unwrap_err().code,
            ProjectErrorCode::CorruptAsset
        );
    };
    let original = serde_json::to_value(&bundle).unwrap();
    let mut missing = original.clone();
    missing["provenance"]["surface_images"] = serde_json::json!([]);
    check_rejected(&missing);
    let mut corrupt = original.clone();
    corrupt["provenance"]["surface_images"][0]["bytes"][14] = serde_json::json!(34);
    check_rejected(&corrupt);
    let mut disagreement = original.clone();
    disagreement["provenance"]["surface_images"][0]["image_hash"] =
        serde_json::json!(SourceHash::from_bytes(b"wrong"));
    check_rejected(&disagreement);
    let mut old = original;
    old["schema_version"] = serde_json::json!(4);
    old["provenance"]
        .as_object_mut()
        .unwrap()
        .remove("surface_images");
    old["plan"]
        .as_object_mut()
        .unwrap()
        .remove("surface_resolution");
    check_rejected(&old);
}
