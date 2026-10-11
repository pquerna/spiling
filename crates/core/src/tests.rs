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
    edit(
        &mut project,
        Edit::Import {
            source: asset(vec![99], "new.step"),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert_eq!(project.retained_sources().len(), 1);
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
            schema_version: 1,
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
            bead_width_mm: 0.4,
            perimeter_count: 1,
            infill_fraction: 1.0,
            print_speed_mm_s: 10.0,
            travel_speed_mm_s: 20.0,
            flow_multiplier: 1.0,
        },
    }
}
pub(crate) fn manufacturing_bundle(project: &Project) -> ManufacturingBundle {
    let manifest = project.capture().manifest;
    let intent = manifest.manufacturing_intent.unwrap().as_ref().clone();
    let area = (intent.recipe.bead_width_mm - intent.recipe.layer_height_mm)
        * intent.recipe.layer_height_mm
        + std::f64::consts::PI * (intent.recipe.layer_height_mm / 2.0).powi(2);
    let filament =
        area / (std::f64::consts::PI * (intent.recipe.filament_diameter_mm / 2.0).powi(2));
    ManufacturingBundle {
        schema_version: 1,
        provenance: ManufacturingProvenance {
            input_hash: input_fingerprint(
                &manifest.project_id,
                &manifest.definitions,
                &manifest.occurrences,
                &intent,
            )
            .unwrap(),
            project_id: manifest.project_id,
            input_revision: manifest.revision,
            definitions: manifest.definitions,
            occurrences: manifest.occurrences,
            intent,
            kernel: KernelIdentity {
                name: "storage-metadata-fixture".into(),
                version: "1".into(),
                revision: "1".into(),
            },
            planner_version: "1".into(),
            emitter_version: "1".into(),
            verifier_version: "1".into(),
        },
        plan: NormalizedPrintPlan {
            schema_version: 1,
            sampling_tolerance_mm: 0.001,
            offset_tolerance_mm: 0.001,
            layer_quantization_mm: 0.000001,
            layers: vec![PrintLayer {
                index: 0,
                z_mm: 0.2,
                section_z_mm: 0.1,
                paths: vec![DepositionPath {
                    kind: DepositionKind::SolidFill,
                    points_mm: vec![[1.0, 1.0, 0.2], [2.0, 1.0, 0.2]],
                }],
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
            deposited_volume_mm3: area,
            filament_length_mm: filament,
            max_position_error_mm: 0.0,
            max_extrusion_error_mm: 0.0,
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
    intent.recipe.print_speed_mm_s = 12.0;
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
