// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::{
    Edit, Project,
    tests::{asset, image_asset, image_edit, imported},
};
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::{OccurrenceId, RigidPoseMm};
use std::{fs, sync::atomic::Ordering};
struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("spiling-core-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn target(&self) -> PathBuf {
        self.0.join("project.spil")
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn flag() -> AtomicBool {
    AtomicBool::new(false)
}
fn save_project(project: &mut Project, target: Option<PathBuf>) {
    let receipt = save(
        project.capture(),
        project.storage(),
        target,
        &flag(),
        |_| {},
    )
    .unwrap();
    assert!(receipt.durability_error.is_none());
    project.mark_saved(receipt).unwrap();
}
fn pose(project: &mut Project, x: f64) {
    let edit = project
        .prepare(Edit::SetPose {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            pose: RigidPoseMm {
                translation_mm: [x, 0.0, 0.0],
                ..RigidPoseMm::IDENTITY
            },
        })
        .unwrap();
    project.commit(edit).unwrap();
}
#[test]
fn save_open_preserves_identity_bytes_dirty_and_session_history() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let id = project.info().project_id.clone();
    save_project(&mut project, Some(sandbox.target()));
    let saved = project.info().revision;
    assert!(!project.info().dirty);
    assert!(project.info().can_undo);
    pose(&mut project, 4.0);
    assert!(project.info().dirty);
    project.undo().unwrap();
    assert!(!project.info().dirty);
    assert!(project.info().revision > saved);
    assert_eq!(project.info().saved_revision, Some(saved));
    save_project(&mut project, None);
    assert!(project.info().can_redo);
    let original = project.capture();
    drop(project);
    let loaded = open(&sandbox.target(), false, false, &flag()).unwrap();
    let reopened = Project::from_loaded(loaded).unwrap();
    assert_eq!(reopened.info().project_id, id);
    assert_eq!(reopened.capture().manifest, original.manifest);
    assert_eq!(
        reopened
            .snapshot()
            .sources
            .values()
            .next()
            .unwrap()
            .bytes
            .as_slice(),
        b"immutable STEP source"
    );
    assert!(!reopened.info().can_undo);
}
#[test]
fn os_writer_lock_readonly_snapshot_and_reused_writer_are_independent() {
    let sandbox = Sandbox::new();
    let mut writer = imported();
    save_project(&mut writer, Some(sandbox.target()));
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ProjectLocked
    );
    let readonly = open(&sandbox.target(), true, false, &flag()).unwrap();
    let mut reader = Project::from_loaded(readonly).unwrap();
    let reader_info = reader.info();
    assert!(reader_info.read_only);
    assert_eq!(
        reader
            .prepare(Edit::Remove {
                occurrence_id: OccurrenceId::new(1).unwrap()
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(reader.undo().unwrap_err().code, ProjectErrorCode::ReadOnly);
    assert_eq!(reader.redo().unwrap_err().code, ProjectErrorCode::ReadOnly);
    assert_eq!(
        save(reader.capture(), reader.storage(), None, &flag(), |_| {})
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    pose(&mut writer, 5.0);
    save_project(&mut writer, None);
    assert_eq!(reader.info(), reader_info);
    assert_eq!(
        reader
            .snapshot()
            .occurrences
            .values()
            .next()
            .unwrap()
            .pose
            .translation_mm[0],
        0.0
    );
    let reused = open_reusing(
        &sandbox.target(),
        false,
        false,
        &flag(),
        writer.storage(),
        LoadBudgets::MAXIMUM,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&reused.storage, &writer.storage().unwrap()));
    drop(reused);
    drop(writer);
    assert!(open(&sandbox.target(), false, false, &flag()).is_ok());
}
#[test]
fn same_writer_reopen_keeps_live_identity_high_water_without_dirty_revision_comparison() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let id = project.capture().manifest.definitions[0]
        .definition_id
        .clone();
    let prepared = project
        .prepare(Edit::Add {
            definition_id: id.clone(),
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let abandoned = project.capture().manifest;
    let loaded = open_reusing(
        &sandbox.target(),
        false,
        false,
        &flag(),
        project.storage(),
        LoadBudgets::MAXIMUM,
    )
    .unwrap();
    let mut reopened = Project::from_loaded(loaded).unwrap();
    reopened.retain_high_water(&project).unwrap();
    assert!(!reopened.info().dirty);
    assert!(reopened.info().revision >= abandoned.revision);
    let prepared = reopened
        .prepare(Edit::Add {
            definition_id: id,
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    reopened.commit(prepared).unwrap();
    assert!(
        reopened
            .snapshot()
            .occurrences
            .contains_key(&OccurrenceId::new(abandoned.next_occurrence).unwrap())
    );
}
#[test]
fn cancelled_open_and_unattached_save_have_typed_nonmutating_failures() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    assert_eq!(
        save(project.capture(), None, None, &flag(), |_| {})
            .unwrap_err()
            .code,
        ProjectErrorCode::NoSavedPath
    );
    save_project(&mut project, Some(sandbox.target()));
    let before = project.info();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        open_reusing(
            &sandbox.target(),
            false,
            false,
            &cancel,
            project.storage(),
            LoadBudgets::MAXIMUM,
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::Cancelled
    );
    assert_eq!(project.info(), before);
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ProjectLocked
    );
}

#[test]
fn staged_open_enforces_remaining_source_budget_without_releasing_live_writer() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let before = project.info();
    let bytes = project
        .snapshot()
        .sources
        .values()
        .map(|source| source.bytes.len())
        .sum::<usize>();
    let error = open_reusing(
        &sandbox.target(),
        false,
        false,
        &flag(),
        project.storage(),
        LoadBudgets {
            source_bytes: bytes - 1,
            ..LoadBudgets::MAXIMUM
        },
    )
    .unwrap_err();
    assert_eq!(error.code, ProjectErrorCode::ResourceLimit);
    assert_eq!(project.info(), before);
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ProjectLocked
    );
    let loaded = open_reusing(
        &sandbox.target(),
        false,
        false,
        &flag(),
        project.storage(),
        LoadBudgets {
            source_bytes: bytes,
            ..LoadBudgets::MAXIMUM
        },
    )
    .unwrap();
    assert_eq!(loaded.manifest, project.capture().manifest);
}

#[test]
fn consumed_recipe_policies_save_reopen_with_matching_durable_bundle_provenance() {
    use spiling_contracts::manufacturing::{RectilinearAxisPolicy, SeamPolicy};
    for rectilinear_axis in [
        RectilinearAxisPolicy::X,
        RectilinearAxisPolicy::Y,
        RectilinearAxisPolicy::AlternatingXY,
        RectilinearAxisPolicy::AlternatingYX,
    ] {
        let sandbox = Sandbox::new();
        let mut project = crate::tests::manufactured();
        save_project(&mut project, Some(sandbox.target()));
        let original = project.snapshot().manufacturing_artifact.clone().unwrap();
        let mut intent = crate::tests::manufacturing_intent();
        intent.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [0.6, -0.8] };
        intent.recipe.rectilinear_axis = rectilinear_axis;
        let prepared = project
            .prepare(Edit::SetManufacturingIntent {
                intent: intent.clone().into(),
            })
            .unwrap();
        project.commit(prepared).unwrap();
        assert!(project.info().dirty);
        assert!(project.snapshot().manufacturing_artifact.is_none());
        project.undo().unwrap();
        assert!(!project.info().dirty);
        assert!(Arc::ptr_eq(
            &original,
            project.snapshot().manufacturing_artifact.as_ref().unwrap()
        ));
        project.redo().unwrap();
        let bundle = crate::tests::manufacturing_bundle(&project);
        assert_eq!(bundle.provenance.intent, intent);
        assert_ne!(bundle.provenance.input_hash, original.record.input_hash);
        let asset = crate::tests::bundle_asset(&bundle);
        let prepared = project
            .prepare(Edit::PublishManufacturing {
                asset: asset.clone(),
            })
            .unwrap();
        project.commit(prepared).unwrap();
        save_project(&mut project, None);
        let manifest = project.capture().manifest;
        drop(project);
        let reopened =
            Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
        assert_eq!(reopened.capture().manifest, manifest);
        assert_eq!(
            reopened.snapshot().manufacturing_intent.as_deref(),
            Some(&intent)
        );
        let snapshot = reopened.snapshot();
        let restored = snapshot.manufacturing_artifact.as_ref().unwrap();
        assert_eq!(restored.record, asset.record);
        assert_eq!(restored.validate().unwrap().provenance.intent, intent);
        assert!(!reopened.info().dirty);
    }
}

#[test]
fn manufacturing_fresh_reopen_ignores_deleted_original_and_readonly_rejects_edits() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    let external = sandbox.0.join("part.step");
    let source = project.snapshot().sources.values().next().unwrap().clone();
    fs::write(&external, source.bytes.as_slice()).unwrap();
    let capture = project.capture();
    save_project(&mut project, Some(sandbox.target()));
    fs::remove_file(external).unwrap();
    drop(project);
    let mut reopened =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    assert_eq!(reopened.capture().manifest, capture.manifest);
    assert_eq!(
        reopened
            .snapshot()
            .sources
            .values()
            .next()
            .unwrap()
            .bytes
            .as_slice(),
        source.bytes.as_slice()
    );
    assert_eq!(
        reopened
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .bytes
            .as_slice(),
        capture
            .snapshot
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .bytes
            .as_slice()
    );
    assert_eq!(
        reopened
            .prepare(Edit::SetManufacturingIntent {
                intent: crate::tests::manufacturing_intent().into()
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        reopened
            .prepare(Edit::PublishManufacturing {
                asset: capture.snapshot.manufacturing_artifact.clone().unwrap()
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        reopened.undo().unwrap_err().code,
        ProjectErrorCode::ReadOnly
    );
}
#[test]
fn manufacturing_remaining_capture_budget_preserves_live_writer_and_state() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    save_project(&mut project, Some(sandbox.target()));
    let before = project.info();
    let asset = project.snapshot().manufacturing_artifact.clone().unwrap();
    // Opening reads the bundle, then the raw program, and retains its frames.
    let program_raw = asset.program.len() as usize;
    let count = asset.bytes.len() + program_raw.max(asset.program.retained_bytes());
    assert_eq!(
        open_reusing(
            &sandbox.target(),
            false,
            false,
            &flag(),
            project.storage(),
            LoadBudgets {
                manufacturing_bytes: count - 1,
                ..LoadBudgets::MAXIMUM
            },
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ProjectLocked
    );
    let loaded = open_reusing(
        &sandbox.target(),
        false,
        false,
        &flag(),
        project.storage(),
        LoadBudgets {
            manufacturing_bytes: count,
            ..LoadBudgets::MAXIMUM
        },
    )
    .unwrap();
    assert_eq!(
        loaded.layouts[0]
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .bytes
            .len(),
        asset.bytes.len()
    );
}
#[test]
fn staged_image_open_checks_remaining_allocation_before_releasing_live_state() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let image = image_asset([0, 1, 2, 3], "budget.pgm");
    add_image(&mut project, image.clone());
    save_project(&mut project, Some(sandbox.target()));
    let before = project.info();
    let size = image.bytes().len();
    assert_eq!(
        open_reusing(
            &sandbox.target(),
            false,
            false,
            &flag(),
            project.storage(),
            LoadBudgets {
                image_bytes: size - 1,
                ..LoadBudgets::MAXIMUM
            },
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    assert!(Arc::ptr_eq(
        project.surface_images().values().next().unwrap(),
        &image
    ));
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ProjectLocked
    );
    let loaded = open_reusing(
        &sandbox.target(),
        false,
        false,
        &flag(),
        project.storage(),
        LoadBudgets {
            image_bytes: size,
            ..LoadBudgets::MAXIMUM
        },
    )
    .unwrap();
    assert_eq!(
        loaded.surface_images.values().next().unwrap().bytes().len(),
        size
    );
}

#[test]
fn process_durable_source_deleted_readonly_reopen_preserves_inheritance_reset_and_high_water() {
    use spiling_contracts::{process::*, surface::SurfaceScope};
    let sandbox = Sandbox::new();
    let original_path = sandbox.0.join("original.step");
    fs::write(&original_path, b"captured process source").unwrap();
    let mut project = Project::new();
    let prepared = project
        .prepare(Edit::Import {
            source: asset(fs::read(&original_path).unwrap(), "original.step"),
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let prepared = project
        .prepare(Edit::SetManufacturingIntent {
            intent: Arc::new(crate::tests::manufacturing_intent()),
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    let definition = SurfaceScope::Definition {
        definition_id: definition_id.clone(),
    };
    let occurrence = SurfaceScope::Occurrence {
        occurrence_id: OccurrenceId::new(1).unwrap(),
        expected_definition_id: definition_id,
    };
    let set = |project: &mut Project, scope: SurfaceScope, fraction| {
        let prepared = project
            .prepare(Edit::Process {
                edit: ProcessEdit::AddLayer {
                    scope,
                    selector: ProcessSelector::WholeOwnedVolume,
                    interface: None,
                    action: ProcessAction::Replace {
                        value: ProcessValue::SparseFraction { fraction },
                    },
                },
            })
            .unwrap();
        project.commit(prepared).unwrap();
    };
    set(&mut project, definition, 0.3);
    set(&mut project, occurrence.clone(), 0.4);
    set(&mut project, occurrence.clone(), 0.6);
    save_project(&mut project, Some(sandbox.target()));
    assert!(!project.info().dirty);
    let prepared = project
        .prepare(Edit::Process {
            edit: ProcessEdit::Reset {
                scope: occurrence.clone(),
            },
        })
        .unwrap();
    project.commit(prepared).unwrap();
    assert!(project.info().dirty);
    assert_eq!(project.info().resolved_process[0].value.fraction, 0.3);
    project.undo().unwrap();
    assert!(!project.info().dirty);
    assert_eq!(project.info().resolved_process[0].value.fraction, 0.6);
    project.redo().unwrap();
    save_project(&mut project, None);
    let saved = project.capture().manifest;
    let authoritative = project.info().resolved_process;
    fs::remove_file(original_path).unwrap();
    drop(project);
    let mut reopened =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    assert_eq!(reopened.capture().manifest, saved);
    assert_eq!(reopened.info().resolved_process, authoritative);
    assert_eq!(reopened.info().resolved_process[0].inherited.fraction, 0.3);
    assert_eq!(
        reopened
            .snapshot()
            .sources
            .values()
            .next()
            .unwrap()
            .bytes
            .as_slice(),
        b"captured process source"
    );
    assert!(!reopened.info().dirty);
    assert_eq!(
        reopened
            .prepare(Edit::Process {
                edit: ProcessEdit::Reset {
                    scope: occurrence.clone()
                }
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        reopened.undo().unwrap_err().code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        save(
            reopened.capture(),
            reopened.storage(),
            None,
            &flag(),
            |_| {}
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::ReadOnly
    );
    drop(reopened);
    let mut writer =
        Project::from_loaded(open(&sandbox.target(), false, false, &flag()).unwrap()).unwrap();
    set(&mut writer, occurrence, 0.7);
    let layer = writer
        .info()
        .process_document
        .layers
        .last()
        .unwrap()
        .clone();
    assert_eq!(layer.layer_id.get(), saved.next_process_layer);
    assert_eq!(
        layer.operations[0].operation_id.get(),
        saved.next_process_operation
    );
}

#[test]
fn box_process_intent_persists_explicit_permission_bounds_and_allocator_high_water() {
    use spiling_contracts::{process::*, surface::SurfaceScope};
    let sandbox = Sandbox::new();
    let mut project = imported();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    let scope = SurfaceScope::Definition {
        definition_id: definition_id.clone(),
    };
    let set = |project: &mut Project, interface, min, max| {
        let prepared = project
            .prepare(Edit::Process {
                edit: ProcessEdit::SetBoxSparseFraction {
                    scope: scope.clone(),
                    min,
                    max,
                    fraction: 0.6,
                    interface,
                },
            })
            .unwrap();
        project.commit(prepared).unwrap();
    };
    set(&mut project, None, [-1.0; 3], [2.0; 3]);
    save_project(&mut project, Some(sandbox.target()));
    let unauthorized = project.capture().manifest;
    drop(project);
    let mut reopened =
        Project::from_loaded(open(&sandbox.target(), false, false, &flag()).unwrap()).unwrap();
    assert_eq!(reopened.capture().manifest, unauthorized);
    set(
        &mut reopened,
        Some(ProcessInterfacePermission::SeparatorWall),
        [1.0; 3],
        [4.0; 3],
    );
    let authorized = reopened.capture().manifest;
    assert_eq!(
        authorized.next_process_layer,
        unauthorized.next_process_layer
    );
    assert_eq!(
        authorized.next_process_operation,
        unauthorized.next_process_operation
    );
    save_project(&mut reopened, None);
    drop(reopened);
    let mut reopened =
        Project::from_loaded(open(&sandbox.target(), false, false, &flag()).unwrap()).unwrap();
    assert_eq!(reopened.capture().manifest, authorized);
    assert_eq!(
        reopened
            .info()
            .process_document
            .resolve_definition(&definition_id, 0.2)
            .fraction,
        0.2
    );
    assert_eq!(
        reopened
            .info()
            .process_document
            .resolve_at(&definition_id, OccurrenceId::new(1).unwrap(), 0.2, [2.0; 3])
            .fraction,
        0.6
    );
    let prepared = reopened
        .prepare(Edit::Process {
            edit: ProcessEdit::Reset { scope },
        })
        .unwrap();
    reopened.commit(prepared).unwrap();
    assert!(reopened.info().process_document.layers.is_empty());
    reopened.undo().unwrap();
    assert_eq!(
        reopened.capture().manifest.process_document,
        authorized.process_document
    );
    assert_eq!(
        reopened.capture().manifest.next_process_operation,
        authorized.next_process_operation
    );
}

fn appearance_project() -> (Project, Arc<PaletteAsset>) {
    let mut project = crate::tests::manufactured();
    let asset = crate::tests::palette_asset("palette.json");
    crate::tests::commit_appearance(
        &mut project,
        Edit::ImportAppearancePalette {
            asset: asset.clone(),
        },
    );
    let target = crate::tests::appearance_target();
    crate::tests::commit_appearance(
        &mut project,
        Edit::SetAppearanceTarget {
            target: target.clone(),
            limits: spiling_contracts::appearance::SearchLimits::default(),
        },
    );
    let report = crate::tests::appearance_report(&asset, target);
    crate::tests::commit_appearance(
        &mut project,
        Edit::AcceptAppearanceReport {
            report,
            selection: Some("red/catalog".into()),
        },
    );
    (project, asset)
}

#[test]
fn appearance_original_source_deletion_reopen_and_readonly_preserve_artifact() {
    let sandbox = Sandbox::new();
    let (mut project, asset) = appearance_project();
    let source = sandbox.0.join("palette.json");
    fs::write(&source, asset.original().as_slice()).unwrap();
    let imported_bytes = fs::read(&source).unwrap();
    assert_eq!(imported_bytes.as_slice(), asset.original().as_slice());
    fs::remove_file(&source).unwrap();
    let artifact = project.snapshot().manufacturing_artifact.clone().unwrap();
    let captured = project.capture().manifest;
    save_project(&mut project, Some(sandbox.target()));
    assert!(project.info().can_undo);
    let mut readonly =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    assert_eq!(readonly.capture().manifest, captured);
    let reopened_asset = readonly
        .snapshot()
        .appearance_palettes
        .values()
        .next()
        .unwrap()
        .clone();
    assert_eq!(
        reopened_asset.original().as_slice(),
        asset.original().as_slice()
    );
    assert_eq!(
        reopened_asset.canonical().as_slice(),
        asset.canonical().as_slice()
    );
    assert_eq!(reopened_asset.document(), asset.document());
    assert_eq!(
        readonly
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .bytes
            .as_slice(),
        artifact.bytes.as_slice()
    );
    assert_eq!(
        readonly
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .record
            .input_hash,
        artifact.record.input_hash
    );
    assert_eq!(
        readonly
            .prepare(Edit::SetAppearanceTarget {
                target: crate::tests::appearance_target(),
                limits: spiling_contracts::appearance::SearchLimits::default(),
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        readonly.undo().unwrap_err().code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        save(
            readonly.capture(),
            readonly.storage(),
            None,
            &flag(),
            |_| {}
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::ReadOnly
    );
}

#[test]
fn appearance_staged_open_enforces_remaining_budget_without_releasing_writer() {
    let sandbox = Sandbox::new();
    let (mut project, _) = appearance_project();
    save_project(&mut project, Some(sandbox.target()));
    let before = project.info();
    assert_eq!(
        open_reusing(
            &sandbox.target(),
            false,
            false,
            &flag(),
            project.storage(),
            LoadBudgets {
                appearance_bytes: 0,
                ..LoadBudgets::MAXIMUM
            },
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ProjectLocked
    );
}

#[test]
fn optical_reports_reconstruct_source_independently_readonly_with_exact_bytes_and_shared_request() {
    let sandbox = Sandbox::new();
    let (mut project, asset, report) = crate::optical_tests::optical_project();
    let palette_path = sandbox.0.join("optical-palette.json");
    let request_path = sandbox.0.join("optical-request.json");
    fs::write(&palette_path, asset.original().as_slice()).unwrap();
    fs::write(&request_path, report.request.encode().unwrap()).unwrap();
    let report_bytes = report.encode().unwrap();
    let record = report.canonical_record().unwrap();
    let manufactured = project
        .snapshot()
        .manufacturing_artifact
        .as_ref()
        .unwrap()
        .clone();
    save_project(&mut project, Some(sandbox.target()));
    fs::remove_file(palette_path).unwrap();
    fs::remove_file(request_path).unwrap();
    let mut readonly =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    let optical = readonly.appearance_study().optical.as_ref().unwrap();
    let reconstructed = optical.report.as_ref().unwrap();
    assert_eq!(reconstructed.encode().unwrap(), report_bytes);
    assert_eq!(optical.report_record.as_ref(), Some(&record));
    assert_eq!(
        record.validate_bytes(&report_bytes).unwrap(),
        **reconstructed
    );
    assert!(Arc::ptr_eq(&optical.request, &reconstructed.request));
    assert_eq!(
        optical.request.encode().unwrap(),
        report.request.encode().unwrap()
    );
    assert_eq!(
        readonly
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .bytes
            .as_slice(),
        manufactured.bytes.as_slice()
    );
    assert_eq!(
        readonly
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .record
            .input_hash,
        manufactured.record.input_hash
    );
    assert_eq!(
        readonly
            .prepare(Edit::SetOpticalStudy {
                request: crate::optical_tests::optical_request()
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        readonly
            .prepare(Edit::AcceptOpticalReport {
                report: reconstructed.clone(),
                selection: Some(spiling_contracts::optical::OpticalSelection::Requested)
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        readonly.undo().unwrap_err().code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(
        save(
            readonly.capture(),
            readonly.storage(),
            None,
            &flag(),
            |_| {}
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::ReadOnly
    );
}

fn add_image(project: &mut Project, image: Arc<SurfaceImageAsset>) {
    let prepared = project.prepare(image_edit(project, image)).unwrap();
    project.commit(prepared).unwrap();
}

fn connection(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .unwrap()
}

fn replace_guard(path: &Path, guard: &ProjectRecoveryGuard) {
    let bytes = serde_json::to_vec(guard).unwrap();
    connection(path)
        .execute(
            "UPDATE container SET guard=?1,guard_hash=?2",
            rusqlite::params![bytes, &Sha256::digest(&bytes)[..]],
        )
        .unwrap();
}

fn corrupt_root(path: &Path, previous: bool) {
    let selected = if previous {
        "previous_snapshot"
    } else {
        "current_snapshot"
    };
    connection(path).execute(&format!(
        "UPDATE chunks SET codec=0,data=zeroblob(raw_length) WHERE chunk_id IN (SELECT chunk_id FROM object_chunks WHERE object_id=(SELECT root_object FROM snapshots WHERE snapshot_id=(SELECT {selected} FROM container)))"
    ), []).unwrap();
}

fn rewrite_current(path: &Path, root: &[u8], members: &[Member]) {
    let mut connection = connection(path);
    let transaction = connection.transaction().unwrap();
    let root_id = ObjectId::of(root);
    transaction
        .execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,?3)",
            rusqlite::params![
                root_id.as_bytes(),
                i64::try_from(root.len()).unwrap(),
                i64::try_from(root.len().div_ceil(spiling_core_ptac::CHUNK_BYTES)).unwrap()
            ],
        )
        .unwrap();
    for (ordinal, chunk) in root.chunks(spiling_core_ptac::CHUNK_BYTES).enumerate() {
        let chunk_id = ObjectId::of(chunk);
        transaction
            .execute(
                "INSERT OR IGNORE INTO chunks VALUES(?1,?2,0,?3)",
                rusqlite::params![
                    chunk_id.as_bytes(),
                    i64::try_from(chunk.len()).unwrap(),
                    chunk
                ],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT OR IGNORE INTO object_chunks VALUES(?1,?2,?3)",
                rusqlite::params![
                    root_id.as_bytes(),
                    i64::try_from(ordinal).unwrap(),
                    chunk_id.as_bytes()
                ],
            )
            .unwrap();
    }
    let mut members = members.to_vec();
    members.sort_unstable();
    let mut digest = Sha256::new();
    digest.update(b"spiling:ptac:snapshot:v1\0");
    digest.update(root_id.as_bytes());
    digest.update(u32::try_from(members.len()).unwrap().to_le_bytes());
    for member in &members {
        digest.update(member.role.get().to_le_bytes());
        digest.update(member.object.as_bytes());
    }
    let snapshot: [u8; 32] = digest.finalize().into();
    transaction
        .execute(
            "INSERT OR IGNORE INTO snapshots VALUES(?1,?2)",
            rusqlite::params![&snapshot, root_id.as_bytes()],
        )
        .unwrap();
    for member in members {
        transaction
            .execute(
                "INSERT OR IGNORE INTO snapshot_objects VALUES(?1,?2,?3)",
                rusqlite::params![&snapshot, member.role.get(), member.object.as_bytes()],
            )
            .unwrap();
    }
    transaction
        .execute(
            "UPDATE container SET current_snapshot=?1",
            rusqlite::params![&snapshot],
        )
        .unwrap();
    transaction.commit().unwrap();
}

fn captured_root(path: &Path) -> (Vec<u8>, Vec<Member>) {
    let container = Container::open_existing(path, true).unwrap();
    let cancel = flag();
    let capture = container.capture(Selection::Current, &cancel).unwrap();
    (capture.root.clone(), capture.members.clone())
}

#[test]
fn container_role_closure_refuses_missing_extra_and_unknown_members() {
    for change in 0..3 {
        let sandbox = Sandbox::new();
        let mut project = imported();
        save_project(&mut project, Some(sandbox.target()));
        let (root, mut members) = captured_root(&sandbox.target());
        match change {
            0 => {
                members.clear();
            }
            1 => {
                members.push(Member {
                    role: ObjectRole::new(IMAGE).unwrap(),
                    object: members[0].object,
                });
            }
            _ => {
                members[0].role = ObjectRole::new(99).unwrap();
            }
        }
        drop(project);
        rewrite_current(&sandbox.target(), &root, &members);
        assert_eq!(
            open(&sandbox.target(), true, false, &flag())
                .unwrap_err()
                .code,
            ProjectErrorCode::InvalidProject
        );
    }
}

#[test]
fn ordinary_and_readonly_load_consume_every_guard_floor_without_false_dirty() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let saved_revision = project.info().revision;
    let mut guard = ProjectRecoveryGuard::from_manifest(&project.capture().manifest);
    guard.revision_high_water = ProjectRevision(100);
    guard.next_occurrence_high_water = 200;
    guard.next_surface_layer_high_water = 300;
    guard.next_process_layer_high_water = 400;
    guard.next_process_operation_high_water = 500;
    drop(project);
    replace_guard(&sandbox.target(), &guard);
    for read_only in [true, false] {
        let mut project =
            Project::from_loaded(open(&sandbox.target(), read_only, false, &flag()).unwrap())
                .unwrap();
        assert!(!project.info().dirty);
        assert_eq!(project.info().saved_revision, Some(saved_revision));
        let manifest = project.capture().manifest;
        assert_eq!(manifest.revision, ProjectRevision(100));
        assert_eq!(manifest.next_occurrence, 200);
        assert_eq!(manifest.next_surface_layer, 300);
        assert_eq!(manifest.next_process_layer, 400);
        assert_eq!(manifest.next_process_operation, 500);
        if !read_only {
            let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
            let edit = project
                .prepare(Edit::Add {
                    definition_id,
                    pose: RigidPoseMm::IDENTITY,
                })
                .unwrap();
            project.commit(edit).unwrap();
            assert!(
                project
                    .snapshot()
                    .occurrences
                    .contains_key(&OccurrenceId::new(200).unwrap())
            );
            assert_eq!(project.info().revision, ProjectRevision(101));
        }
    }
}

#[test]
fn corrupt_current_recovery_is_dirty_fresh_and_retains_valid_previous_on_save() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    add_image(&mut project, image_asset([0, 80, 160, 255], "recovery.png"));
    let previous = project.capture().manifest;
    save_project(&mut project, Some(sandbox.target()));
    pose(&mut project, 9.0);
    save_project(&mut project, None);
    let mut guard = ProjectRecoveryGuard::from_manifest(&project.capture().manifest);
    guard.revision_high_water = ProjectRevision(100);
    guard.next_occurrence_high_water = 200;
    guard.next_surface_layer_high_water = 300;
    guard.next_process_layer_high_water = 400;
    guard.next_process_operation_high_water = 500;
    drop(project);
    replace_guard(&sandbox.target(), &guard);
    corrupt_root(&sandbox.target(), false);
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    let mut recovered =
        Project::from_loaded(open(&sandbox.target(), false, true, &flag()).unwrap()).unwrap();
    assert!(recovered.info().dirty && recovered.info().recovered_previous);
    assert_eq!(recovered.info().saved_revision, Some(previous.revision));
    assert_eq!(recovered.info().revision, ProjectRevision(101));
    assert_eq!(recovered.capture().manifest.next_surface_layer, 300);
    assert_eq!(recovered.capture().manifest.next_process_layer, 400);
    assert_eq!(recovered.capture().manifest.next_process_operation, 500);
    let source = recovered
        .snapshot()
        .sources
        .values()
        .next()
        .unwrap()
        .bytes
        .clone();
    let image = recovered
        .snapshot()
        .surface_images
        .values()
        .next()
        .unwrap()
        .bytes()
        .clone();
    save_project(&mut recovered, None);
    assert!(!recovered.info().dirty && !recovered.info().recovered_previous);
    assert!(Arc::ptr_eq(
        &source,
        &recovered.snapshot().sources.values().next().unwrap().bytes
    ));
    assert!(Arc::ptr_eq(
        &image,
        recovered
            .snapshot()
            .surface_images
            .values()
            .next()
            .unwrap()
            .bytes()
    ));
    let previous = open(&sandbox.target(), true, true, &flag()).unwrap();
    assert_eq!(previous.saved_revision, ProjectRevision(2));
    assert_eq!(
        previous.manifest.layouts[0].occurrences[0]
            .pose
            .translation_mm[0],
        0.0
    );
}

#[test]
fn damaged_previous_does_not_block_current_but_bad_guard_blocks_both() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    pose(&mut project, 3.0);
    save_project(&mut project, None);
    let manifest = project.capture().manifest;
    drop(project);
    corrupt_root(&sandbox.target(), true);
    assert!(open(&sandbox.target(), true, false, &flag()).is_ok());
    assert!(open(&sandbox.target(), true, true, &flag()).is_err());
    let mut guard = ProjectRecoveryGuard::from_manifest(&manifest);
    guard.revision_high_water = ProjectRevision::ZERO;
    replace_guard(&sandbox.target(), &guard);
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
}

#[test]
fn malformed_mismatched_and_each_insufficient_guard_refuse_every_load() {
    for field in 0..8 {
        let sandbox = Sandbox::new();
        let mut project = imported();
        save_project(&mut project, Some(sandbox.target()));
        pose(&mut project, 3.0);
        save_project(&mut project, None);
        let mut guard = ProjectRecoveryGuard::from_manifest(&project.capture().manifest);
        guard.next_occurrence_high_water = 10;
        guard.next_surface_layer_high_water = 10;
        guard.next_process_layer_high_water = 10;
        guard.next_process_operation_high_water = 10;
        let (root, members) = captured_root(&sandbox.target());
        let mut manifest = parse_manifest(&root).unwrap();
        manifest.next_surface_layer = 10;
        manifest.next_occurrence = 10;
        manifest.next_process_layer = 10;
        manifest.next_process_operation = 10;
        drop(project);
        rewrite_current(
            &sandbox.target(),
            &serde_json::to_vec(&manifest).unwrap(),
            &members,
        );
        match field {
            0 => {
                guard.project_id = ProjectId::new();
            }
            1 => {
                guard.revision_high_water = ProjectRevision::ZERO;
            }
            2 => {
                guard.next_occurrence_high_water = 1;
            }
            3 => {
                guard.next_surface_layer_high_water = 1;
            }
            4 => {
                guard.next_process_layer_high_water = 1;
            }
            5 => {
                guard.next_process_operation_high_water = 1;
            }
            _ => {}
        }
        replace_guard(&sandbox.target(), &guard);
        if field == 6 {
            let bytes = b"{}";
            connection(&sandbox.target())
                .execute(
                    "UPDATE container SET guard=?1,guard_hash=?2",
                    rusqlite::params![bytes.as_slice(), &Sha256::digest(bytes)[..]],
                )
                .unwrap();
        } else if field == 7 {
            connection(&sandbox.target())
                .execute("UPDATE container SET guard_hash=zeroblob(32)", [])
                .unwrap();
        }
        assert_eq!(
            open(&sandbox.target(), true, false, &flag())
                .unwrap_err()
                .code,
            ProjectErrorCode::InvalidProject
        );
        assert_eq!(
            open(&sandbox.target(), true, true, &flag())
                .unwrap_err()
                .code,
            ProjectErrorCode::InvalidProject
        );
    }
}

#[test]
fn recovery_binding_refuses_intervening_publication_and_preserves_live_highwater() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    pose(&mut project, 2.0);
    save_project(&mut project, None);
    pose(&mut project, 3.0);
    let loaded = open_reusing(
        &sandbox.target(),
        false,
        true,
        &flag(),
        project.storage(),
        LoadBudgets::MAXIMUM,
    )
    .unwrap();
    let mut recovered = Project::from_loaded(loaded).unwrap();
    recovered.retain_high_water(&project).unwrap();
    assert!(recovered.info().revision > project.info().revision);
    save_project(&mut project, None);
    assert_eq!(
        save(
            recovered.capture(),
            recovered.storage(),
            None,
            &flag(),
            |_| {}
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::StaleRevision
    );
}

#[test]
fn recovery_revision_exhaustion_refuses_and_normal_open_stays_clean() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    pose(&mut project, 2.0);
    save_project(&mut project, None);
    let mut guard = ProjectRecoveryGuard::from_manifest(&project.capture().manifest);
    guard.revision_high_water = ProjectRevision(u32::MAX);
    drop(project);
    replace_guard(&sandbox.target(), &guard);
    let project =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    assert!(!project.info().dirty);
    assert_eq!(
        open(&sandbox.target(), true, true, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
}

#[test]
fn cancellation_is_late_for_existing_commit_and_new_target_publication() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    for stop in [
        SaveStage::Staged,
        SaveStage::RowsInserted,
        SaveStage::Committed,
        SaveStage::BeforePublish,
        SaveStage::BeforeCommit,
    ] {
        let cancel = flag();
        let failure = save(
            project.capture(),
            None,
            Some(sandbox.target()),
            &cancel,
            |stage| {
                if stage == stop {
                    cancel.store(true, Ordering::Release);
                }
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, ProjectErrorCode::Cancelled);
        assert!(!sandbox.target().exists());
    }
    save_project(&mut project, Some(sandbox.target()));
    let original = project.capture().manifest;
    pose(&mut project, 4.0);
    for stop in [
        SaveStage::Staged,
        SaveStage::RowsInserted,
        SaveStage::BeforeCommit,
    ] {
        let cancel = flag();
        assert_eq!(
            save(
                project.capture(),
                project.storage(),
                None,
                &cancel,
                |stage| {
                    if stage == stop {
                        cancel.store(true, Ordering::Release);
                    }
                }
            )
            .unwrap_err()
            .code,
            ProjectErrorCode::Cancelled
        );
        assert_eq!(
            open(&sandbox.target(), true, false, &flag())
                .unwrap()
                .manifest,
            original
        );
    }
    let cancel = flag();
    let receipt = save(
        project.capture(),
        project.storage(),
        None,
        &cancel,
        |stage| {
            if stage == SaveStage::Committed {
                cancel.store(true, Ordering::Release);
            }
        },
    )
    .unwrap();
    assert_eq!(receipt.outcome, PublicationOutcome::PublishedConfirmed);
    project.mark_saved(receipt).unwrap();
    assert!(!project.info().dirty);
}

#[test]
fn save_callbacks_expose_one_actual_publication_decision() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let cancel = flag();
    let mut stages = Vec::new();
    let receipt = save(
        project.capture(),
        None,
        Some(sandbox.target()),
        &cancel,
        |stage| {
            stages.push(stage);
            if stage == SaveStage::Published {
                cancel.store(true, Ordering::Release);
            }
        },
    )
    .unwrap();
    assert_eq!(receipt.outcome, PublicationOutcome::PublishedConfirmed);
    assert_eq!(
        stages,
        [
            SaveStage::Staged,
            SaveStage::RowsInserted,
            SaveStage::Committed,
            SaveStage::BeforePublish,
            SaveStage::BeforeCommit,
            SaveStage::Published,
        ]
    );
    project.mark_saved(receipt).unwrap();
    pose(&mut project, 4.0);
    stages.clear();
    let receipt = save(
        project.capture(),
        project.storage(),
        None,
        &flag(),
        |stage| stages.push(stage),
    )
    .unwrap();
    assert_eq!(receipt.outcome, PublicationOutcome::PublishedConfirmed);
    assert_eq!(
        stages,
        [
            SaveStage::Staged,
            SaveStage::RowsInserted,
            SaveStage::BeforeCommit,
            SaveStage::Committed,
        ]
    );
}

#[test]
fn saveas_is_exclusive_selfcontained_and_retains_last_independent_save() {
    let sandbox = Sandbox::new();
    let (mut project, _) = appearance_project();
    add_image(&mut project, image_asset([0, 80, 160, 255], "copy.png"));
    save_project(&mut project, Some(sandbox.target()));
    let previous_revision = project.info().saved_revision.unwrap();
    pose(&mut project, 6.0);
    let copy = sandbox.0.join("copy.spil");
    save_project(&mut project, Some(copy.clone()));
    assert_eq!(project.storage().unwrap().path(), copy);
    let recovery = open(&copy, true, true, &flag()).unwrap();
    assert_eq!(recovery.saved_revision, previous_revision);
    assert!(recovery.layouts[0].manufacturing_artifact.is_none());
    assert!(!recovery.surface_images.is_empty());
    assert!(!recovery.appearance_palettes.is_empty());
    let foreign = sandbox.0.join("foreign.spil");
    fs::write(&foreign, b"foreign").unwrap();
    assert!(
        save(
            project.capture(),
            project.storage(),
            Some(foreign.clone()),
            &flag(),
            |_| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(foreign).unwrap(), b"foreign");
    drop(recovery);
    drop(project);
    let moved = sandbox.0.join("moved.spil");
    fs::rename(copy, &moved).unwrap();
    assert!(open(&moved, false, false, &flag()).is_ok());
}

#[test]
fn old_directories_and_foreign_files_are_refused_without_conversion() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.target()).unwrap();
    fs::write(sandbox.target().join("manifest.json"), b"old directory").unwrap();
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::UnsupportedFormat
    );
    assert_eq!(
        fs::read(sandbox.target().join("manifest.json")).unwrap(),
        b"old directory"
    );
}

#[test]
fn uncertainty_receipts_keep_confirmed_baseline_and_block_unknown_writes_until_reopen() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let baseline = project.info().saved_revision;
    pose(&mut project, 4.0);
    let mut receipt = save(project.capture(), project.storage(), None, &flag(), |_| {}).unwrap();
    receipt.outcome = PublicationOutcome::OutcomeUnknown;
    receipt.binding = None;
    receipt.durability_error = Some(error(
        ProjectErrorCode::Io,
        "injected unknown receipt classification",
    ));
    project.mark_saved(receipt).unwrap();
    assert!(project.info().save_uncertain && project.info().dirty);
    assert_eq!(project.info().saved_revision, baseline);
    project.undo().unwrap();
    assert!(project.info().save_uncertain && project.info().dirty);
    assert_eq!(
        save(project.capture(), project.storage(), None, &flag(), |_| {})
            .unwrap_err()
            .code,
        ProjectErrorCode::Busy
    );
    let loaded = open_reusing(
        &sandbox.target(),
        false,
        false,
        &flag(),
        project.storage(),
        LoadBudgets::MAXIMUM,
    )
    .unwrap();
    let mut reopened = Project::from_loaded(loaded).unwrap();
    reopened.retain_high_water(&project).unwrap();
    assert!(reopened.info().save_uncertain && reopened.info().dirty);
    assert_eq!(reopened.info().saved_revision, baseline);
    save_project(&mut reopened, None);
    assert!(!reopened.info().save_uncertain && !reopened.info().dirty);
}

#[test]
fn maintenance_preserves_project_and_reader_arcs_and_compact_keeps_two_roots() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    for value in [2.0, 3.0, 4.0] {
        pose(&mut project, value);
        save_project(&mut project, None);
    }
    let reader =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    let source = reader
        .snapshot()
        .sources
        .values()
        .next()
        .unwrap()
        .bytes
        .clone();
    let info = project.info();
    let storage = project.storage().unwrap();
    let before = storage.stats(&flag()).unwrap();
    assert!(storage.collect_unreachable(&flag(), || false).is_err());
    assert_eq!(storage.stats(&flag()).unwrap(), before);
    let after = storage.collect_unreachable(&flag(), || true).unwrap();
    assert!(after.snapshots < before.snapshots);
    assert_eq!(project.info(), info);
    assert_eq!(source.as_slice(), b"immutable STEP source");
    let copy = sandbox.0.join("compact.spil");
    let receipt = storage.compact_to_new(&copy, &flag(), |_| {}).unwrap();
    assert_eq!(receipt.outcome, PublicationOutcome::PublishedConfirmed);
    assert!(receipt.stats.is_some());
    assert_eq!(project.info(), info);
    let original_heads = storage
        .container
        .capture(Selection::Current, &flag())
        .unwrap()
        .heads;
    let copy_heads = receipt
        .storage
        .container
        .capture(Selection::Current, &flag())
        .unwrap()
        .heads;
    assert_eq!(original_heads.current(), copy_heads.current());
    assert_eq!(original_heads.previous(), copy_heads.previous());
    assert_eq!(
        open(&copy, true, false, &flag()).unwrap().manifest,
        project.capture().manifest
    );
    assert_eq!(
        reader
            .snapshot()
            .sources
            .values()
            .next()
            .unwrap()
            .bytes
            .as_slice(),
        source.as_slice()
    );
    assert!(
        storage
            .compact_to_new(storage.path(), &flag(), |_| {})
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn samewriter_symlink_aliases_are_never_reused_for_open_or_save() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let before = project.info();
    let alias = sandbox.0.join("alias.spil");
    std::os::unix::fs::symlink(sandbox.target(), &alias).unwrap();
    let directory_alias = sandbox.0.join("directory-alias");
    std::os::unix::fs::symlink(&sandbox.0, &directory_alias).unwrap();
    for path in [alias.clone(), directory_alias.join("project.spil")] {
        assert!(
            open_reusing(
                &path,
                false,
                false,
                &flag(),
                project.storage(),
                LoadBudgets::MAXIMUM
            )
            .is_err()
        );
        assert!(
            save(
                project.capture(),
                project.storage(),
                Some(path),
                &flag(),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(project.info(), before);
    }
    assert!(
        fs::symlink_metadata(alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap()
            .manifest,
        project.capture().manifest
    );
}

#[test]
fn unconfirmed_saveas_attaches_target_without_claiming_a_confirmed_saved_revision() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    pose(&mut project, 3.0);
    let target = sandbox.0.join("uncertain-copy.spil");
    let mut receipt = save(
        project.capture(),
        project.storage(),
        Some(target.clone()),
        &flag(),
        |_| {},
    )
    .unwrap();
    receipt.outcome = PublicationOutcome::PublishedUnconfirmed;
    receipt.durability_error = Some(error(
        ProjectErrorCode::Io,
        "injected unconfirmed receipt classification",
    ));
    project.mark_saved(receipt).unwrap();
    assert_eq!(project.storage().unwrap().path(), target);
    assert!(project.info().save_uncertain && project.info().dirty);
    assert_eq!(project.info().saved_revision, None);
    save_project(&mut project, None);
    assert!(!project.info().save_uncertain && !project.info().dirty);
    assert_eq!(project.info().saved_revision, Some(project.info().revision));
}

#[test]
fn saveas_capture_budget_includes_uncommitted_prepared_source_allocations() {
    let project = imported();
    let before = remaining_budgets(&project).unwrap();
    let incoming = asset(vec![7; MAX_SOURCE_BYTES], "prepared.step");
    let prepared = project
        .prepare(Edit::Import {
            source: incoming.clone(),
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    let during = remaining_budgets(&project).unwrap();
    assert_eq!(
        before.source_bytes - during.source_bytes,
        incoming.bytes.capacity()
    );
    drop(prepared);
    assert_eq!(
        remaining_budgets(&project).unwrap().source_bytes,
        before.source_bytes
    );
}

#[test]
fn canonical_image_metadata_is_checked_beyond_container_hash_and_role_integrity() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let bytes = b"P5\n2 3\n255\n\x00\x30\x60\x90\xc0\xff".to_vec();
    let mut record = image_asset([0, 80, 160, 255], "metadata.png")
        .record()
        .clone();
    record.image_hash = SourceHash::from_bytes(&bytes);
    record.height = 3;
    record.byte_count = u64::try_from(bytes.len()).unwrap();
    add_image(
        &mut project,
        Arc::new(SurfaceImageAsset::new(record, bytes).unwrap()),
    );
    save_project(&mut project, Some(sandbox.target()));
    let (root, members) = captured_root(&sandbox.target());
    let mut manifest = parse_manifest(&root).unwrap();
    manifest.surface_overlays.images[0].width = 3;
    manifest.surface_overlays.images[0].height = 2;
    manifest.validate().unwrap();
    drop(project);
    rewrite_current(
        &sandbox.target(),
        &serde_json::to_vec(&manifest).unwrap(),
        &members,
    );
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
}

#[test]
fn manufacturing_metadata_is_checked_beyond_container_hash_and_role_integrity() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    save_project(&mut project, Some(sandbox.target()));
    let (root, members) = captured_root(&sandbox.target());
    let mut manifest = parse_manifest(&root).unwrap();
    manifest.layouts[0]
        .manufacturing_artifact
        .as_mut()
        .unwrap()
        .byte_count += 1;
    manifest.validate().unwrap();
    drop(project);
    rewrite_current(
        &sandbox.target(),
        &serde_json::to_vec(&manifest).unwrap(),
        &members,
    );
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
}

#[test]
fn root_parsing_is_strict_without_legacy_or_missing_field_defaults() {
    let manifest = imported().capture().manifest;
    let original = serde_json::to_value(&manifest).unwrap();
    for missing in [
        "appearance_study",
        "analysis",
        "next_surface_layer",
        "next_process_layer",
        "next_process_operation",
        "layouts",
    ] {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(missing);
        assert_eq!(
            parse_manifest(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code,
            ProjectErrorCode::InvalidProject
        );
    }
    for missing in [
        "manufacturing_intent",
        "manufacturing_options",
        "manufacturing_artifact",
        "hardware",
        "occurrences",
        "layout_id",
        "name",
    ] {
        let mut value = original.clone();
        value["layouts"][0].as_object_mut().unwrap().remove(missing);
        assert!(
            parse_manifest(&serde_json::to_vec(&value).unwrap()).is_err(),
            "missing {missing}"
        );
    }
    let mut unknown = original.clone();
    unknown["unknown"] = serde_json::json!(true);
    assert_eq!(
        parse_manifest(&serde_json::to_vec(&unknown).unwrap())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    let duplicate = serde_json::to_string(&manifest)
        .unwrap()
        .replace("{", "{\"revision\":1,");
    assert_eq!(
        parse_manifest(duplicate.as_bytes()).unwrap_err().code,
        ProjectErrorCode::InvalidProject
    );
    let mut old = original;
    old["format_version"] = serde_json::json!(5);
    assert_eq!(
        parse_manifest(&serde_json::to_vec(&old).unwrap())
            .unwrap_err()
            .code,
        ProjectErrorCode::UnsupportedFormat
    );
}

#[test]
fn manufacturing_options_are_self_contained_revision_bound_and_read_only_inspectable() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    save_project(&mut project, Some(sandbox.target()));
    let original = project.snapshot();
    let external = sandbox.0.join("part.step");
    fs::write(
        &external,
        original.sources.values().next().unwrap().bytes.as_slice(),
    )
    .unwrap();
    let option = ManufacturingOption::new(
        "Retained fine".into(),
        crate::context_hash(&original).unwrap(),
        original.manufacturing_intent.as_ref().unwrap().clone(),
    )
    .unwrap();
    let prepared = project
        .prepare(Edit::RetainManufacturingOption {
            option: option.clone(),
        })
        .unwrap();
    project.commit(prepared).unwrap();
    assert!(project.info().dirty);
    project.undo().unwrap();
    assert!(!project.info().dirty);
    project.redo().unwrap();
    assert!(project.info().dirty);
    save_project(&mut project, None);
    assert!(!project.info().dirty);
    let manifest = project.capture().manifest;
    let expected_guard = ProjectRecoveryGuard::from_manifest(&manifest);
    fs::remove_file(external).unwrap();
    drop(project);
    let mut reader =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    assert_eq!(reader.capture().manifest, manifest);
    assert_eq!(
        ProjectRecoveryGuard::from_manifest(&reader.capture().manifest),
        expected_guard
    );
    assert!(!reader.info().dirty);
    assert_eq!(
        reader
            .manufacturing_option(reader.info().revision, &option.id)
            .unwrap(),
        option
    );
    assert_eq!(
        reader
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .record,
        original.manufacturing_artifact.as_ref().unwrap().record
    );
    assert_eq!(
        crate::context_hash(&reader.snapshot()).unwrap(),
        option.context_hash
    );
    let trial = crate::trial_snapshot(&reader.snapshot(), option.intent.clone()).unwrap();
    assert!(trial.manufacturing_artifact.is_none());
    assert!(reader.snapshot().manufacturing_artifact.is_some());
    for edit in [
        Edit::RetainManufacturingOption {
            option: option.clone(),
        },
        Edit::RemoveManufacturingOption {
            id: option.id.clone(),
        },
        Edit::ApplyManufacturingOption {
            id: option.id.clone(),
        },
    ] {
        assert_eq!(
            reader.prepare(edit).unwrap_err().code,
            ProjectErrorCode::ReadOnly
        );
    }
    assert_eq!(reader.undo().unwrap_err().code, ProjectErrorCode::ReadOnly);
    assert_eq!(reader.redo().unwrap_err().code, ProjectErrorCode::ReadOnly);
    assert_eq!(reader.capture().manifest, manifest);
}

#[test]
fn stale_context_options_survive_ptac_without_restoring_old_source_state() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let option = ManufacturingOption::new(
        "Earlier pose".into(),
        crate::context_hash(&project.snapshot()).unwrap(),
        Arc::new(crate::tests::manufacturing_intent()),
    )
    .unwrap();
    let prepared = project
        .prepare(Edit::RetainManufacturingOption {
            option: option.clone(),
        })
        .unwrap();
    project.commit(prepared).unwrap();
    pose(&mut project, 4.0);
    save_project(&mut project, Some(sandbox.target()));
    drop(project);
    let project =
        Project::from_loaded(open(&sandbox.target(), false, false, &flag()).unwrap()).unwrap();
    assert_eq!(
        project
            .manufacturing_option(project.info().revision, &option.id)
            .unwrap(),
        option
    );
    assert_ne!(
        crate::context_hash(&project.snapshot()).unwrap(),
        option.context_hash
    );
    assert_eq!(
        project
            .prepare(Edit::ApplyManufacturingOption { id: option.id })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    assert_eq!(
        project
            .snapshot()
            .occurrences
            .values()
            .next()
            .unwrap()
            .pose
            .translation_mm[0],
        4.0
    );
    assert!(!project.info().dirty);
}

#[test]
fn option_domain_corruption_refuses_even_with_rehashed_closed_ptac_root() {
    for change in 0..6 {
        let sandbox = Sandbox::new();
        let mut project = crate::tests::manufactured();
        let option = ManufacturingOption::new(
            "Saved option".into(),
            crate::context_hash(&project.snapshot()).unwrap(),
            Arc::new(crate::tests::manufacturing_intent()),
        )
        .unwrap();
        let prepared = project
            .prepare(Edit::RetainManufacturingOption { option })
            .unwrap();
        project.commit(prepared).unwrap();
        save_project(&mut project, Some(sandbox.target()));
        let (root, members) = captured_root(&sandbox.target());
        let mut value: serde_json::Value = serde_json::from_slice(&root).unwrap();
        match change {
            0 => {
                value["layouts"][0]["manufacturing_options"][0]["id"] =
                    serde_json::json!(SourceHash::from_bytes(b"forged identity"))
            }
            1 => {
                value["layouts"][0]["manufacturing_options"][0]["name"] =
                    serde_json::json!("Rebound name")
            }
            2 => {
                value["layouts"][0]["manufacturing_options"][0]["intent"]["recipe"]["print_speed_mm_s"] =
                    serde_json::json!(9.0)
            }
            3 => {
                value["layouts"][0]["manufacturing_options"][0]["context_hash"] =
                    serde_json::json!(SourceHash::from_bytes(b"foreign context"))
            }
            4 => {
                let duplicate = value["layouts"][0]["manufacturing_options"][0].clone();
                value["layouts"][0]["manufacturing_options"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            _ => {
                value["layouts"][0]["manufacturing_options"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("intent");
            }
        }
        drop(project);
        rewrite_current(
            &sandbox.target(),
            &serde_json::to_vec(&value).unwrap(),
            &members,
        );
        assert_eq!(
            open(&sandbox.target(), true, false, &flag())
                .unwrap_err()
                .code,
            ProjectErrorCode::InvalidProject
        );
    }
}

#[test]
fn empty_project_has_a_complete_first_snapshot_and_zero_saved_revision() {
    let sandbox = Sandbox::new();
    let mut project = Project::new();
    save_project(&mut project, Some(sandbox.target()));
    assert_eq!(project.info().saved_revision, Some(ProjectRevision::ZERO));
    drop(project);
    let reopened =
        Project::from_loaded(open(&sandbox.target(), false, false, &flag()).unwrap()).unwrap();
    assert!(!reopened.info().dirty);
    assert!(reopened.snapshot().sources.is_empty());
    assert_eq!(reopened.info().saved_revision, Some(ProjectRevision::ZERO));
}
