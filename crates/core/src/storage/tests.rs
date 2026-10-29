// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::{
    Edit, Project,
    tests::{asset, image_asset, image_edit, imported},
};
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
        self.0.join("project")
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
        MAX_RETAINED_SOURCE_BYTES,
        MAX_MANUFACTURING_RETAINED_BYTES as usize,
        MAX_SURFACE_IMAGE_RETAINED_BYTES,
        MAX_APPEARANCE_RETAINED_BYTES,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&reused.storage, &writer.storage().unwrap()));
    drop(reused);
    drop(writer);
    assert!(open(&sandbox.target(), false, false, &flag()).is_ok());
}
#[test]
fn readonly_open_during_writer_commit_captures_one_complete_manifest() {
    let sandbox = Sandbox::new();
    let mut writer = imported();
    save_project(&mut writer, Some(sandbox.target()));
    let before = writer.capture().manifest;
    pose(&mut writer, 10.0);
    let after = writer.capture().manifest;
    let mut observed = Vec::new();
    let receipt = save(writer.capture(), writer.storage(), None, &flag(), |_| {
        observed.push(
            open(&sandbox.target(), true, false, &flag())
                .unwrap()
                .manifest,
        );
    })
    .unwrap();
    writer.mark_saved(receipt).unwrap();
    assert_eq!(observed, vec![before.clone(), before, after]);
}
#[test]
fn cancellation_wins_before_replacement_and_loses_after_publication() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let old = project.capture().manifest;
    pose(&mut project, 7.0);
    for stage in [SaveStage::AssetsSynced, SaveStage::BeforeManifestReplace] {
        let cancel = flag();
        let failure = save(
            project.capture(),
            project.storage(),
            None,
            &cancel,
            |seen| {
                if seen == stage {
                    cancel.store(true, Ordering::Release);
                }
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, ProjectErrorCode::Cancelled);
        assert_eq!(
            open(&sandbox.target(), true, false, &flag())
                .unwrap()
                .manifest,
            old
        );
    }
    let cancel = flag();
    let receipt = save(
        project.capture(),
        project.storage(),
        None,
        &cancel,
        |stage| {
            if stage == SaveStage::AfterManifestReplace {
                cancel.store(true, Ordering::Release);
            }
        },
    )
    .unwrap();
    assert!(receipt.durability_error.is_none());
    project.mark_saved(receipt).unwrap();
    assert!(!project.info().dirty);
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap()
            .manifest,
        project.capture().manifest
    );
}
#[test]
fn first_save_cancel_is_retryable_and_cleans_only_owned_stage() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    for wanted in [SaveStage::AssetsSynced, SaveStage::BeforeManifestReplace] {
        let cancel = flag();
        assert_eq!(
            save(
                project.capture(),
                None,
                Some(sandbox.target()),
                &cancel,
                |stage| {
                    if stage == wanted {
                        cancel.store(true, Ordering::Release);
                    }
                }
            )
            .unwrap_err()
            .code,
            ProjectErrorCode::Cancelled
        );
        assert!(!sandbox.target().exists());
        assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 0);
    }
    save_project(&mut project, Some(sandbox.target()));
    assert!(!project.info().dirty);
}
#[test]
fn racing_foreign_destination_is_never_overwritten_or_cleaned() {
    let sandbox = Sandbox::new();
    let project = imported();
    let failure = save(
        project.capture(),
        None,
        Some(sandbox.target()),
        &flag(),
        |stage| {
            if stage == SaveStage::BeforeManifestReplace {
                fs::create_dir(sandbox.target()).unwrap();
                fs::write(sandbox.target().join("foreign"), b"do not delete").unwrap();
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, ProjectErrorCode::Io);
    assert_eq!(
        fs::read(sandbox.target().join("foreign")).unwrap(),
        b"do not delete"
    );
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 1);
    assert_eq!(
        save(
            project.capture(),
            None,
            Some(sandbox.target()),
            &flag(),
            |_| {}
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::InvalidProject
    );
}
#[cfg(unix)]
#[test]
fn substituted_stage_path_is_not_recursively_deleted() {
    let sandbox = Sandbox::new();
    let project = imported();
    let cancel = flag();
    let failure = save(
        project.capture(),
        None,
        Some(sandbox.target()),
        &cancel,
        |stage| {
            if stage == SaveStage::AssetsSynced {
                let path = fs::read_dir(&sandbox.0)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::rename(&path, sandbox.0.join("original-owned-stage")).unwrap();
                fs::create_dir(&path).unwrap();
                fs::write(path.join("foreign"), b"foreign marker").unwrap();
                cancel.store(true, Ordering::Release);
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, ProjectErrorCode::Cancelled);
    let foreign = fs::read_dir(&sandbox.0)
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".spiling-stage-")
        })
        .unwrap()
        .path();
    assert_eq!(
        fs::read(foreign.join("foreign")).unwrap(),
        b"foreign marker"
    );
    assert!(!sandbox.target().exists());
}
#[test]
fn recovery_is_explicit_independent_and_never_reuses_published_ids() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let prior = project.capture().manifest;
    let definition_id = prior.definitions[0].definition_id.clone();
    let prepared = project
        .prepare(Edit::Add {
            definition_id: definition_id.clone(),
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    project.commit(prepared).unwrap();
    save_project(&mut project, None);
    let newer = project.capture().manifest;
    drop(project);
    fs::write(sandbox.target().join(CURRENT), b"broken current JSON").unwrap();
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    let loaded = open(&sandbox.target(), false, true, &flag()).unwrap();
    assert_eq!(loaded.manifest.definitions, prior.definitions);
    assert_eq!(loaded.manifest.occurrences, prior.occurrences);
    assert!(loaded.manifest.revision > newer.revision);
    assert_eq!(loaded.manifest.next_occurrence, newer.next_occurrence);
    let mut recovered = Project::from_loaded(loaded).unwrap();
    assert!(recovered.info().dirty && recovered.info().recovered_previous);
    assert_eq!(recovered.info().saved_revision, Some(prior.revision));
    let prepared = recovered
        .prepare(Edit::Add {
            definition_id,
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    recovered.commit(prepared).unwrap();
    assert!(
        recovered
            .snapshot()
            .occurrences
            .contains_key(&OccurrenceId::new(newer.next_occurrence).unwrap())
    );
    save_project(&mut recovered, None);
    assert!(!recovered.info().dirty && !recovered.info().recovered_previous);
    assert!(open(&sandbox.target(), true, false, &flag()).is_ok());
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
        MAX_RETAINED_SOURCE_BYTES,
        MAX_MANUFACTURING_RETAINED_BYTES as usize,
        MAX_SURFACE_IMAGE_RETAINED_BYTES,
        MAX_APPEARANCE_RETAINED_BYTES,
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
fn corrupt_missing_schema_reference_pose_and_identity_fail_typed() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let manifest = project.capture().manifest;
    drop(project);
    let current = sandbox.target().join(CURRENT);
    let original = fs::read(&current).unwrap();
    for (field, value, code) in [
        (
            "format_version",
            serde_json::json!(1),
            ProjectErrorCode::UnsupportedFormat,
        ),
        (
            "next_occurrence",
            serde_json::json!(1),
            ProjectErrorCode::InvalidProject,
        ),
        (
            "unexpected",
            serde_json::json!(true),
            ProjectErrorCode::InvalidProject,
        ),
    ] {
        let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        json[field] = value;
        fs::write(&current, serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(
            open(&sandbox.target(), false, false, &flag())
                .unwrap_err()
                .code,
            code
        );
    }
    for field in ["definition_id", "pose"] {
        let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        json["occurrences"][0][field] = if field == "definition_id" {
            serde_json::json!(asset(vec![99], "other").record.definition_id.as_str())
        } else {
            serde_json::json!({"translation_mm":[0,0,0],"rotation_xyzw":[0,0,0,2]})
        };
        fs::write(&current, serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(
            open(&sandbox.target(), false, false, &flag())
                .unwrap_err()
                .code,
            ProjectErrorCode::InvalidProject
        );
    }
    fs::write(&current, &original).unwrap();
    let source = sandbox.target().join(SOURCES).join(format!(
        "{}.step",
        manifest.definitions[0].provenance.source_hash.as_str()
    ));
    let bytes = fs::read(&source).unwrap();
    fs::write(&source, b"corrupt bytes").unwrap();
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    fs::remove_file(&source).unwrap();
    assert_eq!(
        open(&sandbox.target(), false, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::MissingAsset
    );
    fs::write(source, bytes).unwrap();
    assert!(open(&sandbox.target(), false, false, &flag()).is_ok());
}
#[cfg(unix)]
#[test]
fn symlink_nonregular_and_substituted_lock_are_rejected() {
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let current = sandbox.target().join(CURRENT);
    let original = fs::read(&current).unwrap();
    fs::remove_file(&current).unwrap();
    fs::write(sandbox.0.join("foreign.json"), &original).unwrap();
    symlink(sandbox.0.join("foreign.json"), &current).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::Io
    );
    fs::remove_file(&current).unwrap();
    fs::create_dir(&current).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::Io
    );
    fs::remove_dir(&current).unwrap();
    fs::write(&current, original).unwrap();
    let lock = sandbox.target().join(LOCK);
    fs::rename(&lock, sandbox.target().join("replaced-lock")).unwrap();
    fs::write(&lock, b"foreign lock").unwrap();
    assert!(save(project.capture(), project.storage(), None, &flag(), |_| {}).is_err());
    assert_eq!(fs::read(&lock).unwrap(), b"foreign lock");
}
#[test]
fn failure_before_replace_preserves_complete_current_and_owned_temporary_cleanup() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let old = project.capture().manifest;
    pose(&mut project, 3.0);
    let before = project.info();
    let failure = save(
        project.capture(),
        project.storage(),
        None,
        &flag(),
        |stage| {
            if stage == SaveStage::AssetsSynced {
                fs::create_dir(sandbox.target().join(PREVIOUS)).unwrap();
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, ProjectErrorCode::Io);
    assert_eq!(project.info(), before);
    fs::remove_dir(sandbox.target().join(PREVIOUS)).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap()
            .manifest,
        old
    );
    assert!(
        !fs::read_dir(sandbox.target())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| temporary_name(&entry.file_name().to_string_lossy()))
    );
}
#[test]
fn postcommit_durability_failure_attaches_uncertain_dirty_state_and_keeps_confirmed_save() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let receipt = save(
        project.capture(),
        None,
        Some(sandbox.target()),
        &flag(),
        |stage| {
            if stage == SaveStage::AfterManifestReplace {
                os::fail_next_directory_sync();
            }
        },
    )
    .unwrap();
    assert_eq!(
        receipt.durability_error.as_ref().unwrap().code,
        ProjectErrorCode::Io
    );
    project.mark_saved(receipt).unwrap();
    assert!(project.info().save_uncertain && project.info().dirty);
    assert!(project.info().path_label.is_some());
    assert!(project.info().saved_revision.is_none());
    assert!(project.info().can_undo);
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap()
            .manifest,
        project.capture().manifest
    );
    save_project(&mut project, None);
    let confirmed_revision = project.info().saved_revision;
    pose(&mut project, 11.0);
    let receipt = save(
        project.capture(),
        project.storage(),
        None,
        &flag(),
        |stage| {
            if stage == SaveStage::AfterManifestReplace {
                os::fail_next_directory_sync();
            }
        },
    )
    .unwrap();
    assert!(receipt.durability_error.is_some());
    project.mark_saved(receipt).unwrap();
    assert_eq!(project.info().saved_revision, confirmed_revision);
    assert!(project.info().save_uncertain && project.info().dirty);
    project.undo().unwrap();
    assert!(
        project.info().dirty,
        "uncertainty cannot become clean by undo"
    );
    save_project(&mut project, None);
    assert!(!project.info().save_uncertain && !project.info().dirty);
    assert!(project.info().can_redo);
}
#[test]
fn immutable_asset_budget_exhaustion_is_explicit_not_garbage_collection() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let source_directory = sandbox.target().join(SOURCES);
    for i in 0..128 {
        let bytes = format!("unreferenced complete source {i}").into_bytes();
        let hash = SourceHash::from_bytes(&bytes);
        fs::write(
            source_directory.join(format!("{}.step", hash.as_str())),
            bytes,
        )
        .unwrap();
    }
    assert_eq!(
        save(project.capture(), project.storage(), None, &flag(), |_| {})
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(fs::read_dir(source_directory).unwrap().count(), 129);
}

#[cfg(unix)]
#[test]
fn native_nonunicode_directory_basename_roundtrips_without_path_loss() {
    use std::os::unix::ffi::OsStringExt;
    let sandbox = Sandbox::new();
    let path = sandbox.0.join(std::ffi::OsString::from_vec(
        b"native-project-\xff".to_vec(),
    ));
    let mut project = imported();
    save_project(&mut project, Some(path.clone()));
    assert_eq!(project.storage().unwrap().path(), path);
    drop(project);
    assert!(open(&path, false, false, &flag()).is_ok());
}

#[cfg(unix)]
#[test]
fn source_symlink_and_nonregular_files_are_rejected_before_publication() {
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let hash = project.capture().manifest.definitions[0]
        .provenance
        .source_hash
        .clone();
    drop(project);
    let path = sandbox
        .target()
        .join(SOURCES)
        .join(format!("{}.step", hash.as_str()));
    fs::remove_file(&path).unwrap();
    fs::write(sandbox.0.join("outside.step"), b"immutable STEP source").unwrap();
    symlink(sandbox.0.join("outside.step"), &path).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::Io
    );
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::Io
    );
}

#[test]
fn source_and_persisted_byte_limits_are_checked_before_read_allocations() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let source_directory = sandbox.target().join(SOURCES);
    for index in 0..17 {
        let hash = SourceHash::from_bytes(format!("sparse budget entry {index}").as_bytes());
        let file = File::create(source_directory.join(format!("{}.step", hash.as_str()))).unwrap();
        file.set_len(MAX_SOURCE_BYTES as u64).unwrap();
    }
    assert_eq!(
        save(project.capture(), project.storage(), None, &flag(), |_| {})
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
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
            MAX_RETAINED_SOURCE_BYTES,
            MAX_MANUFACTURING_RETAINED_BYTES as usize,
            MAX_SURFACE_IMAGE_RETAINED_BYTES,
            MAX_APPEARANCE_RETAINED_BYTES,
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
fn same_session_reopen_cannot_clear_known_durability_uncertainty() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let receipt = save(
        project.capture(),
        None,
        Some(sandbox.target()),
        &flag(),
        |stage| {
            if stage == SaveStage::AfterManifestReplace {
                os::fail_next_directory_sync();
            }
        },
    )
    .unwrap();
    project.mark_saved(receipt).unwrap();
    let loaded = open_reusing(
        &sandbox.target(),
        false,
        false,
        &flag(),
        project.storage(),
        MAX_RETAINED_SOURCE_BYTES,
        MAX_MANUFACTURING_RETAINED_BYTES as usize,
        MAX_SURFACE_IMAGE_RETAINED_BYTES,
        MAX_APPEARANCE_RETAINED_BYTES,
    )
    .unwrap();
    let mut reopened = Project::from_loaded(loaded).unwrap();
    reopened.retain_high_water(&project).unwrap();
    assert!(reopened.info().save_uncertain && reopened.info().dirty);
    assert!(reopened.info().saved_revision.is_none());
    save_project(&mut reopened, None);
    assert!(!reopened.info().save_uncertain && !reopened.info().dirty);
}

#[test]
fn existing_corrupt_checkpoint_is_not_silently_repaired_by_save() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    save_project(&mut project, None);
    let before = fs::read(sandbox.target().join(CURRENT)).unwrap();
    fs::write(
        sandbox.target().join(PREVIOUS),
        b"foreign or corrupt checkpoint",
    )
    .unwrap();
    pose(&mut project, 2.0);
    assert_eq!(
        save(project.capture(), project.storage(), None, &flag(), |_| {})
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    assert_eq!(fs::read(sandbox.target().join(CURRENT)).unwrap(), before);
    assert_eq!(
        fs::read(sandbox.target().join(PREVIOUS)).unwrap(),
        b"foreign or corrupt checkpoint"
    );
    assert!(open(&sandbox.target(), true, false, &flag()).is_ok());
    assert_eq!(
        open(&sandbox.target(), true, true, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
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
        bytes - 1,
        MAX_MANUFACTURING_RETAINED_BYTES as usize,
        MAX_SURFACE_IMAGE_RETAINED_BYTES,
        MAX_APPEARANCE_RETAINED_BYTES,
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
        bytes,
        MAX_MANUFACTURING_RETAINED_BYTES as usize,
        MAX_SURFACE_IMAGE_RETAINED_BYTES,
        MAX_APPEARANCE_RETAINED_BYTES,
    )
    .unwrap();
    assert_eq!(loaded.manifest, project.capture().manifest);
}

#[test]
fn manufacturing_assets_precede_first_normal_and_saveas_checkpoints() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    let asset = project.snapshot().manufacturing_artifact.clone().unwrap();
    let filename = format!("{}.json", asset.record.hash.as_str());
    let first = sandbox.target();
    let receipt = save(
        project.capture(),
        None,
        Some(first.clone()),
        &flag(),
        |stage| {
            if stage == SaveStage::AssetsSynced || stage == SaveStage::BeforeManifestReplace {
                assert!(!first.exists());
            }
            if stage == SaveStage::AfterManifestReplace {
                assert_eq!(
                    fs::read(first.join(MANUFACTURING).join(&filename)).unwrap(),
                    *asset.bytes
                );
            }
        },
    )
    .unwrap();
    project.mark_saved(receipt).unwrap();
    assert!(!project.info().dirty && project.info().can_undo);
    let old = project.capture().manifest;
    let prepared = project
        .prepare(Edit::SetManufacturingIntent {
            intent: (crate::tests::manufacturing_intent()).into(),
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let next_asset = crate::tests::bundle_asset(&crate::tests::manufacturing_bundle(&project));
    let prepared = project
        .prepare(Edit::PublishManufacturing {
            asset: next_asset.clone(),
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let next_name = format!("{}.json", next_asset.record.hash.as_str());
    let receipt = save(
        project.capture(),
        project.storage(),
        None,
        &flag(),
        |stage| {
            if stage == SaveStage::AssetsSynced {
                assert_eq!(
                    fs::read(first.join(MANUFACTURING).join(&next_name)).unwrap(),
                    *next_asset.bytes
                );
                assert_eq!(open(&first, true, false, &flag()).unwrap().manifest, old);
            }
        },
    )
    .unwrap();
    project.mark_saved(receipt).unwrap();
    let recovered = Project::from_loaded(open(&first, true, true, &flag()).unwrap()).unwrap();
    assert_eq!(
        recovered
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .record,
        asset.record
    );
    assert!(recovered.info().recovered_previous && recovered.info().dirty);
    let second = sandbox.0.join("save-as");
    save_project(&mut project, Some(second.clone()));
    assert_eq!(
        fs::read(second.join(MANUFACTURING).join(next_name)).unwrap(),
        *next_asset.bytes
    );
    drop(project);
    let reopened = Project::from_loaded(open(&second, true, false, &flag()).unwrap()).unwrap();
    assert_eq!(
        reopened
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .record,
        next_asset.record
    );
    assert!(!reopened.info().dirty);
}
#[test]
fn strict_recipe_policy_checkpoint_admission_has_no_prior_format_or_policy_defaults() {
    let manifest = crate::tests::manufactured().capture().manifest;
    let original = serde_json::to_value(&manifest).unwrap();
    assert_eq!(
        parse_manifest(&serde_json::to_vec(&original).unwrap()).unwrap(),
        manifest
    );
    for field in ["seam_policy", "rectilinear_axis"] {
        let mut missing = original.clone();
        missing["manufacturing_intent"]["recipe"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_eq!(
            parse_manifest(&serde_json::to_vec(&missing).unwrap())
                .unwrap_err()
                .code,
            ProjectErrorCode::InvalidProject
        );
    }
    for (field, policy) in [
        (
            "seam_policy",
            serde_json::json!({"kind":"preferred_build_direction","xy":[2.0,0.0]}),
        ),
        ("seam_policy", serde_json::json!({"kind":"unknown"})),
        ("rectilinear_axis", serde_json::json!("unknown")),
    ] {
        let mut invalid = original.clone();
        invalid["manufacturing_intent"]["recipe"][field] = policy;
        assert_eq!(
            parse_manifest(&serde_json::to_vec(&invalid).unwrap())
                .unwrap_err()
                .code,
            ProjectErrorCode::InvalidProject
        );
    }
    let mut legacy = original;
    legacy["format_version"] = serde_json::json!(5);
    legacy["manufacturing_intent"]["printer"]["schema_version"] = serde_json::json!(6);
    legacy["manufacturing_intent"]["recipe"]
        .as_object_mut()
        .unwrap()
        .remove("seam_policy");
    legacy["manufacturing_intent"]["recipe"]
        .as_object_mut()
        .unwrap()
        .remove("rectilinear_axis");
    assert_eq!(
        parse_manifest(&serde_json::to_vec(&legacy).unwrap())
            .unwrap_err()
            .code,
        ProjectErrorCode::UnsupportedFormat
    );
    let recovery = serde_json::json!({"manifest":legacy});
    assert_eq!(
        parse_recovery(&serde_json::to_vec(&recovery).unwrap())
            .unwrap_err()
            .code,
        ProjectErrorCode::UnsupportedFormat
    );
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
    let count = project
        .snapshot()
        .manufacturing_artifact
        .as_ref()
        .unwrap()
        .bytes
        .len();
    assert_eq!(
        open_reusing(
            &sandbox.target(),
            false,
            false,
            &flag(),
            project.storage(),
            MAX_RETAINED_SOURCE_BYTES,
            count - 1,
            MAX_SURFACE_IMAGE_RETAINED_BYTES,
            MAX_APPEARANCE_RETAINED_BYTES,
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
        MAX_RETAINED_SOURCE_BYTES,
        count,
        MAX_SURFACE_IMAGE_RETAINED_BYTES,
        MAX_APPEARANCE_RETAINED_BYTES,
    )
    .unwrap();
    assert_eq!(
        loaded.manufacturing_artifact.as_ref().unwrap().bytes.len(),
        count
    );
}
#[test]
fn manufacturing_missing_corrupt_metadata_unknown_schema_and_stale_provenance_fail() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    save_project(&mut project, Some(sandbox.target()));
    let captured = project.capture();
    let asset = captured.snapshot.manufacturing_artifact.as_ref().unwrap();
    let path = sandbox
        .target()
        .join(MANUFACTURING)
        .join(format!("{}.json", asset.record.hash.as_str()));
    fs::remove_file(&path).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::MissingAsset
    );
    fs::write(&path, &asset.bytes[..asset.bytes.len() - 1]).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    let mut corrupt = asset.bytes.as_ref().clone();
    corrupt[0] = b'[';
    fs::write(&path, corrupt).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    fs::write(&path, asset.bytes.as_slice()).unwrap();
    let mut manifest = captured.manifest.clone();
    manifest.format_version = 1;
    fs::write(
        sandbox.target().join(CURRENT),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::UnsupportedFormat
    );
    manifest = captured.manifest.clone();
    manifest.occurrences[0].pose.translation_mm[0] = 2.0;
    fs::write(
        sandbox.target().join(CURRENT),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(open(&sandbox.target(), true, false, &flag()).is_err());
    fs::write(
        sandbox.target().join(CURRENT),
        serde_json::to_vec(&captured.manifest).unwrap(),
    )
    .unwrap();
    let mut object = serde_json::to_value(crate::tests::manufacturing_bundle(&project)).unwrap();
    object["unexpected"] = serde_json::json!("rejected");
    let bytes = serde_json::to_vec(&object).unwrap();
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let hash = SourceHash::from_digest(&digest);
    fs::write(
        sandbox
            .target()
            .join(MANUFACTURING)
            .join(format!("{}.json", hash.as_str())),
        &bytes,
    )
    .unwrap();
    let mut manifest = captured.manifest.clone();
    let record = manifest.manufacturing_artifact.as_mut().unwrap();
    record.hash = hash;
    record.byte_count = bytes.len() as u32;
    fs::write(
        sandbox.target().join(CURRENT),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
}
#[test]
fn manufacturing_invalidation_dirty_undo_save_and_uncertain_receipt_preserve_semantics() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    save_project(&mut project, Some(sandbox.target()));
    let confirmed = project.info().saved_revision;
    let record = project
        .snapshot()
        .manufacturing_artifact
        .as_ref()
        .unwrap()
        .record
        .clone();
    pose(&mut project, 5.0);
    assert!(project.info().dirty && project.snapshot().manufacturing_artifact.is_none());
    project.undo().unwrap();
    assert!(!project.info().dirty);
    assert_eq!(
        project
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .record,
        record
    );
    let receipt = save(
        project.capture(),
        project.storage(),
        None,
        &flag(),
        |stage| {
            if stage == SaveStage::AfterManifestReplace {
                os::fail_next_directory_sync();
            }
        },
    )
    .unwrap();
    assert!(receipt.durability_error.is_some());
    project.mark_saved(receipt).unwrap();
    assert!(project.info().dirty && project.info().save_uncertain && project.info().can_redo);
    assert_eq!(project.info().saved_revision, confirmed);
    project.redo().unwrap();
    project.undo().unwrap();
    assert!(project.info().save_uncertain);
    save_project(&mut project, None);
    assert!(!project.info().dirty && !project.info().save_uncertain && project.info().can_redo);
}
#[test]
fn manufacturing_cancelled_normal_save_keeps_previous_complete_bundle() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    save_project(&mut project, Some(sandbox.target()));
    let old = project.capture().manifest;
    pose(&mut project, 1.0);
    let cancel = flag();
    assert_eq!(
        save(
            project.capture(),
            project.storage(),
            None,
            &cancel,
            |stage| {
                if stage == SaveStage::BeforeManifestReplace {
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
        old
    );
    assert!(
        Project::from_loaded(open(&sandbox.target(), true, true, &flag()).unwrap())
            .unwrap()
            .snapshot()
            .manufacturing_artifact
            .is_some()
    );
}

#[test]
fn manufacturing_file_budget_and_unexpected_entries_fail_before_capture() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    save_project(&mut project, Some(sandbox.target()));
    let asset = project.snapshot().manufacturing_artifact.clone().unwrap();
    let path = sandbox
        .target()
        .join(MANUFACTURING)
        .join(format!("{}.json", asset.record.hash.as_str()));
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_MANUFACTURING_BUNDLE_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    fs::write(&path, asset.bytes.as_slice()).unwrap();
    fs::write(
        sandbox.target().join(MANUFACTURING).join("foreign.json"),
        b"foreign",
    )
    .unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
}
#[cfg(unix)]
#[test]
fn manufacturing_symlink_asset_and_substituted_attached_directory_are_rejected() {
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    save_project(&mut project, Some(sandbox.target()));
    let asset = project.snapshot().manufacturing_artifact.clone().unwrap();
    let directory = sandbox.target().join(MANUFACTURING);
    let path = directory.join(format!("{}.json", asset.record.hash.as_str()));
    let foreign = sandbox.0.join("foreign-bundle");
    fs::write(&foreign, asset.bytes.as_slice()).unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&foreign, &path).unwrap();
    assert!(open(&sandbox.target(), true, false, &flag()).is_err());
    fs::remove_file(&path).unwrap();
    fs::write(&path, asset.bytes.as_slice()).unwrap();
    fs::rename(&directory, sandbox.0.join("original-manufacturing")).unwrap();
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("foreign"), b"retained").unwrap();
    assert!(save(project.capture(), project.storage(), None, &flag(), |_| {}).is_err());
    assert_eq!(fs::read(directory.join("foreign")).unwrap(), b"retained");
}

#[test]
fn surface_save_open_dirty_history_read_only_and_recovery_preserve_document_and_ids() {
    use spiling_contracts::{geometry::FaceId, surface::*};
    let sandbox = Sandbox::new();
    let mut project = imported();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    let add = |project: &mut Project| {
        let prepared = project
            .prepare(Edit::AddSurfaceFinish {
                scope: SurfaceScope::Definition {
                    definition_id: definition_id.clone(),
                },
                face_id: FaceId::from_step_entity(7).unwrap(),
                finish: SurfaceFinish::CylindricalRibs {
                    depth_mm: 0.1,
                    rib_count: 16,
                    phase_fraction: 0.25,
                },
            })
            .unwrap();
        project.commit(prepared).unwrap();
    };
    add(&mut project);
    save_project(&mut project, Some(sandbox.target()));
    let first = project.capture().manifest;
    let id = first.surface_overlays.layers[0].layer_id;
    let prepared = project
        .prepare(Edit::SetSurfaceFinishEnabled {
            layer_id: id,
            enabled: false,
        })
        .unwrap();
    project.commit(prepared).unwrap();
    assert!(project.info().dirty);
    project.undo().unwrap();
    assert!(!project.info().dirty);
    project.redo().unwrap();
    add(&mut project);
    save_project(&mut project, None);
    let current = project.capture().manifest;
    let read_only =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    assert_eq!(read_only.info().surface_overlays, current.surface_overlays);
    assert_eq!(
        read_only
            .prepare(Edit::RemoveSurfaceFinish { layer_id: id })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(read_only.capture().manifest, current);
    drop(read_only);
    drop(project);
    let reopened =
        Project::from_loaded(open(&sandbox.target(), false, false, &flag()).unwrap()).unwrap();
    assert!(!reopened.info().dirty);
    assert_eq!(reopened.capture().manifest, current);
    drop(reopened);
    fs::write(sandbox.target().join(CURRENT), b"broken").unwrap();
    let mut recovered =
        Project::from_loaded(open(&sandbox.target(), false, true, &flag()).unwrap()).unwrap();
    assert_eq!(recovered.info().surface_overlays, first.surface_overlays);
    assert_eq!(
        recovered.capture().manifest.next_surface_layer,
        current.next_surface_layer
    );
    add(&mut recovered);
    assert_eq!(
        recovered
            .info()
            .surface_overlays
            .layers
            .last()
            .unwrap()
            .layer_id
            .get(),
        current.next_surface_layer
    );
    save_project(&mut recovered, None);
    assert!(!recovered.info().dirty);
}

#[test]
fn surface_manifest_rejects_structurally_corrupt_targets_and_old_format() {
    use spiling_contracts::{geometry::FaceId, surface::*};
    let sandbox = Sandbox::new();
    let mut project = imported();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    let prepared = project
        .prepare(Edit::AddSurfaceFinish {
            scope: SurfaceScope::Occurrence {
                occurrence_id: OccurrenceId::new(1).unwrap(),
                expected_definition_id: definition_id,
            },
            face_id: FaceId::from_step_entity(7).unwrap(),
            finish: SurfaceFinish::Smooth,
        })
        .unwrap();
    project.commit(prepared).unwrap();
    save_project(&mut project, Some(sandbox.target()));
    let value = serde_json::to_value(project.capture().manifest).unwrap();
    drop(project);
    for (pointer, replacement) in [
        (
            "/surface_overlays/layers/0/scope/occurrence_id",
            serde_json::json!(99),
        ),
        (
            "/surface_overlays/layers/0/scope/expected_definition_id",
            serde_json::json!(
                spiling_contracts::geometry::DefinitionId::from_source_sha256(&[1; 32])
            ),
        ),
        ("/next_surface_layer", serde_json::json!(1)),
        ("/format_version", serde_json::json!(2)),
        ("/surface_overlays/schema_version", serde_json::json!(1)),
    ] {
        let mut corrupt = value.clone();
        *corrupt.pointer_mut(pointer).unwrap() = replacement;
        fs::write(
            sandbox.target().join(CURRENT),
            serde_json::to_vec(&corrupt).unwrap(),
        )
        .unwrap();
        assert!(open(&sandbox.target(), true, false, &flag()).is_err());
    }
}

fn add_image(project: &mut Project, image: Arc<SurfaceImageAsset>) {
    let prepared = project.prepare(image_edit(project, image)).unwrap();
    project.commit(prepared).unwrap();
}

#[test]
fn image_save_open_and_readonly_capture_do_not_need_original_files() {
    let sandbox = Sandbox::new();
    let source_path = sandbox.0.join("owned.step");
    let image_path = sandbox.0.join("owned.pgm");
    fs::write(&source_path, b"independently captured STEP").unwrap();
    fs::write(&image_path, b"P5\n2 2\n255\n\x00\x40\x80\xff").unwrap();
    let mut project = Project::new();
    let prepared = project
        .prepare(Edit::Import {
            source: asset(fs::read(&source_path).unwrap(), "owned.step"),
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let bytes = fs::read(&image_path).unwrap();
    let record = SurfaceImageRecord {
        image_hash: SourceHash::from_bytes(&bytes),
        width: 2,
        height: 2,
        byte_count: bytes.len() as u64,
        source_hash: SourceHash::from_bytes(&bytes),
        source_name: "owned.pgm".into(),
    };
    let image = Arc::new(SurfaceImageAsset::new(record.clone(), bytes).unwrap());
    add_image(&mut project, image.clone());
    save_project(&mut project, Some(sandbox.target()));
    let manifest = project.capture().manifest;
    fs::remove_file(source_path).unwrap();
    fs::remove_file(image_path).unwrap();
    let stored = sandbox
        .target()
        .join(IMAGES)
        .join(format!("{}.pgm", record.image_hash.as_str()));
    assert_eq!(fs::read(stored).unwrap(), *image.bytes().as_ref());
    let mut reader =
        Project::from_loaded(open(&sandbox.target(), true, false, &flag()).unwrap()).unwrap();
    assert_eq!(reader.capture().manifest, manifest);
    assert_eq!(
        reader.surface_images()[&record.image_hash].record(),
        &record
    );
    assert_eq!(
        reader.surface_images()[&record.image_hash]
            .bytes()
            .as_slice(),
        image.bytes().as_slice()
    );
    let before = reader.info();
    assert_eq!(
        reader.prepare(image_edit(&reader, image)).unwrap_err().code,
        ProjectErrorCode::ReadOnly
    );
    assert_eq!(reader.undo().unwrap_err().code, ProjectErrorCode::ReadOnly);
    assert_eq!(reader.info(), before);
    drop(reader);
    drop(project);
    let reopened =
        Project::from_loaded(open(&sandbox.target(), false, false, &flag()).unwrap()).unwrap();
    assert_eq!(reopened.capture().manifest, manifest);
    assert_eq!(
        reopened.surface_images()[&record.image_hash].record(),
        &record
    );
}

#[test]
fn images_are_durable_before_manifest_and_cancellation_preserves_complete_snapshots() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let old = project.capture().manifest;
    let image = image_asset([0, 40, 80, 255], "boundary.pgm");
    add_image(&mut project, image.clone());
    let current = project.capture().manifest;
    let stored = sandbox
        .target()
        .join(IMAGES)
        .join(format!("{}.pgm", image.record().image_hash.as_str()));
    for stop in [SaveStage::AssetsSynced, SaveStage::BeforeManifestReplace] {
        let cancel = flag();
        let failure = save(
            project.capture(),
            project.storage(),
            None,
            &cancel,
            |stage| {
                assert_eq!(fs::read(&stored).unwrap(), *image.bytes().as_ref());
                if stage == stop {
                    cancel.store(true, Ordering::Release);
                }
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, ProjectErrorCode::Cancelled);
        let loaded = open(&sandbox.target(), true, false, &flag()).unwrap();
        assert_eq!(loaded.manifest, old);
        assert!(loaded.surface_images.is_empty());
    }
    let cancel = flag();
    let receipt = save(
        project.capture(),
        project.storage(),
        None,
        &cancel,
        |stage| {
            if stage == SaveStage::AfterManifestReplace {
                cancel.store(true, Ordering::Release);
            }
        },
    )
    .unwrap();
    project.mark_saved(receipt).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap()
            .manifest,
        current
    );
    let save_as = sandbox.0.join("copy");
    let receipt = save(
        project.capture(),
        project.storage(),
        Some(save_as.clone()),
        &flag(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(
            save_as
                .join(IMAGES)
                .join(format!("{}.pgm", image.record().image_hash.as_str()))
        )
        .unwrap(),
        *image.bytes().as_ref()
    );
    assert_eq!(receipt.manifest, current);
}

#[test]
fn disabled_image_corruption_and_metadata_disagreement_never_open_smooth() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let image = image_asset([1, 33, 127, 255], "disabled.pgm");
    add_image(&mut project, image.clone());
    let layer_id = project.info().surface_overlays.layers[0].layer_id;
    project
        .commit(
            project
                .prepare(Edit::SetSurfaceFinishEnabled {
                    layer_id,
                    enabled: false,
                })
                .unwrap(),
        )
        .unwrap();
    save_project(&mut project, Some(sandbox.target()));
    let original_manifest = fs::read(sandbox.target().join(CURRENT)).unwrap();
    let path = sandbox
        .target()
        .join(IMAGES)
        .join(format!("{}.pgm", image.record().image_hash.as_str()));
    for bytes in [
        b"P5\n2 2\n255\n\x01\x21\x7f\xfe".to_vec(),
        b"P5\n3 2\n255\n\x01\x21\x7f\xff".to_vec(),
        b"P5\n2 2\n255\n\x01\x21\x7f".to_vec(),
    ] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            open(&sandbox.target(), true, false, &flag())
                .unwrap_err()
                .code,
            ProjectErrorCode::CorruptAsset
        );
    }
    fs::remove_file(&path).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::MissingAsset
    );
    fs::write(&path, image.bytes().as_slice()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&original_manifest).unwrap();
    for (pointer, replacement) in [
        ("/surface_overlays/images/0/width", serde_json::json!(3)),
        (
            "/surface_overlays/images/0/image_hash",
            serde_json::json!(SourceHash::from_bytes(b"foreign image")),
        ),
        ("/surface_overlays/images", serde_json::json!([])),
        (
            "/surface_overlays/layers/0/finish",
            serde_json::json!({"kind":"smooth"}),
        ),
    ] {
        let mut corrupt = value.clone();
        *corrupt.pointer_mut(pointer).unwrap() = replacement;
        fs::write(
            sandbox.target().join(CURRENT),
            serde_json::to_vec(&corrupt).unwrap(),
        )
        .unwrap();
        assert!(open(&sandbox.target(), true, false, &flag()).is_err());
    }
    fs::write(sandbox.target().join(CURRENT), original_manifest).unwrap();
    assert!(open(&sandbox.target(), true, false, &flag()).is_ok());
}

#[test]
fn previous_checkpoint_keeps_image_bytes_and_artifact_coherently_after_pruning() {
    let sandbox = Sandbox::new();
    let mut project = crate::tests::manufactured();
    let image = image_asset([0, 127, 255, 44], "recovery.pgm");
    add_image(&mut project, image.clone());
    let artifact = crate::tests::bundle_asset(&crate::tests::manufacturing_bundle(&project));
    project
        .commit(
            project
                .prepare(Edit::PublishManufacturing {
                    asset: artifact.clone(),
                })
                .unwrap(),
        )
        .unwrap();
    save_project(&mut project, Some(sandbox.target()));
    let previous = project.capture().manifest;
    let layer_id = project.info().surface_overlays.layers[0].layer_id;
    project
        .commit(
            project
                .prepare(Edit::RemoveSurfaceFinish { layer_id })
                .unwrap(),
        )
        .unwrap();
    save_project(&mut project, None);
    let current = project.capture().manifest;
    assert!(current.surface_overlays.images.is_empty());
    let path = sandbox
        .target()
        .join(IMAGES)
        .join(format!("{}.pgm", image.record().image_hash.as_str()));
    assert_eq!(fs::read(&path).unwrap(), *image.bytes().as_ref());
    drop(project);
    fs::write(sandbox.target().join(CURRENT), b"corrupt current").unwrap();
    assert!(open(&sandbox.target(), false, false, &flag()).is_err());
    let recovered =
        Project::from_loaded(open(&sandbox.target(), false, true, &flag()).unwrap()).unwrap();
    assert_eq!(recovered.info().surface_overlays, previous.surface_overlays);
    assert_eq!(
        recovered.surface_images()[&image.record().image_hash]
            .bytes()
            .as_slice(),
        image.bytes().as_slice()
    );
    assert_eq!(
        recovered
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .record,
        artifact.record
    );
    assert!(recovered.info().revision > current.revision);
    assert_eq!(
        recovered.capture().manifest.next_surface_layer,
        current.next_surface_layer
    );
    drop(recovered);
    fs::write(path, b"corrupt image").unwrap();
    assert!(open(&sandbox.target(), false, true, &flag()).is_err());
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
            MAX_RETAINED_SOURCE_BYTES,
            MAX_MANUFACTURING_RETAINED_BYTES as usize,
            size - 1,
            MAX_APPEARANCE_RETAINED_BYTES,
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
        MAX_RETAINED_SOURCE_BYTES,
        MAX_MANUFACTURING_RETAINED_BYTES as usize,
        size,
        MAX_APPEARANCE_RETAINED_BYTES,
    )
    .unwrap();
    assert_eq!(
        loaded.surface_images.values().next().unwrap().bytes().len(),
        size
    );
}

#[test]
fn image_file_directory_and_scratch_budgets_are_not_bypassed_by_unreferenced_files() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let directory = sandbox.target().join(IMAGES);
    let oversized = directory.join(format!(
        "{}.pgm",
        SourceHash::from_bytes(b"oversized").as_str()
    ));
    File::create(&oversized)
        .unwrap()
        .set_len(MAX_SURFACE_IMAGE_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    fs::remove_file(oversized).unwrap();
    let first = directory.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let second = directory.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    File::create(&first)
        .unwrap()
        .set_len(MAX_SURFACE_IMAGE_BYTES as u64)
        .unwrap();
    fs::write(&second, b"x").unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    fs::remove_file(first).unwrap();
    fs::remove_file(second).unwrap();
    for tag in 0..MAX_PROJECT_STORED_ASSETS {
        fs::write(
            directory.join(format!(
                "{}.pgm",
                SourceHash::from_bytes(format!("stored-{tag}").as_bytes()).as_str()
            )),
            b"x",
        )
        .unwrap();
    }
    let image = image_asset([1, 2, 3, 4], "new.pgm");
    add_image(&mut project, image);
    let before = project.info();
    assert_eq!(
        save(project.capture(), project.storage(), None, &flag(), |_| {})
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    assert_eq!(
        fs::read_dir(&directory).unwrap().count(),
        MAX_PROJECT_STORED_ASSETS
    );
}

#[cfg(unix)]
#[test]
fn image_symlinks_nonregular_files_and_attached_directory_substitution_fail_closed() {
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    let mut project = imported();
    let image = image_asset([0, 32, 128, 255], "owned.pgm");
    add_image(&mut project, image.clone());
    save_project(&mut project, Some(sandbox.target()));
    let directory = sandbox.target().join(IMAGES);
    let path = directory.join(format!("{}.pgm", image.record().image_hash.as_str()));
    let foreign = sandbox.0.join("foreign.pgm");
    fs::write(&foreign, image.bytes().as_slice()).unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&foreign, &path).unwrap();
    assert!(open(&sandbox.target(), true, false, &flag()).is_err());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(open(&sandbox.target(), true, false, &flag()).is_err());
    fs::remove_dir(&path).unwrap();
    fs::write(&path, image.bytes().as_slice()).unwrap();
    fs::rename(&directory, sandbox.0.join("original-images")).unwrap();
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("foreign"), b"retained").unwrap();
    assert!(save(project.capture(), project.storage(), None, &flag(), |_| {}).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), *image.bytes().as_ref());
    assert_eq!(fs::read(directory.join("foreign")).unwrap(), b"retained");
}

#[test]
fn old_project_without_image_directory_is_explicitly_refused() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    save_project(&mut project, Some(sandbox.target()));
    let mut old = serde_json::to_value(project.capture().manifest).unwrap();
    drop(project);
    old["format_version"] = serde_json::json!(3);
    old["surface_overlays"]["schema_version"] = serde_json::json!(2);
    old["surface_overlays"]
        .as_object_mut()
        .unwrap()
        .remove("images");
    fs::remove_dir(sandbox.target().join(IMAGES)).unwrap();
    fs::write(
        sandbox.target().join(CURRENT),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::UnsupportedFormat
    );
}

#[cfg(unix)]
#[test]
fn first_save_checks_image_directory_identity_and_never_cleans_a_substitute() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    add_image(&mut project, image_asset([1, 2, 3, 4], "stage.pgm"));
    let mut substituted = None;
    let failure = save(
        project.capture(),
        None,
        Some(sandbox.target()),
        &flag(),
        |stage| {
            if stage == SaveStage::AssetsSynced {
                let owned_stage = fs::read_dir(&sandbox.0)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let images = owned_stage.join(IMAGES);
                fs::rename(&images, sandbox.0.join("original-images")).unwrap();
                fs::create_dir(&images).unwrap();
                fs::write(images.join("foreign"), b"foreign marker").unwrap();
                substituted = Some(images);
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, ProjectErrorCode::InvalidProject);
    assert_eq!(
        fs::read(substituted.unwrap().join("foreign")).unwrap(),
        b"foreign marker"
    );
    assert!(!sandbox.target().exists());
}

#[test]
fn first_image_save_cancellation_leaves_no_published_project_and_can_retry() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let image = image_asset([10, 20, 30, 40], "cancelled.pgm");
    add_image(&mut project, image.clone());
    let before = project.info();
    let cancel = flag();
    assert_eq!(
        save(
            project.capture(),
            None,
            Some(sandbox.target()),
            &cancel,
            |stage| {
                if stage == SaveStage::AssetsSynced {
                    cancel.store(true, Ordering::Release);
                }
            }
        )
        .unwrap_err()
        .code,
        ProjectErrorCode::Cancelled
    );
    assert!(!sandbox.target().exists());
    assert_eq!(fs::read_dir(&sandbox.0).unwrap().count(), 0);
    assert_eq!(project.info(), before);
    save_project(&mut project, Some(sandbox.target()));
    let loaded = open(&sandbox.target(), true, false, &flag()).unwrap();
    assert_eq!(
        loaded.surface_images.values().next().unwrap().record(),
        image.record()
    );
}

#[test]
fn removing_current_image_cannot_publish_over_a_corrupt_recovery_dependency() {
    let sandbox = Sandbox::new();
    let mut project = imported();
    let image = image_asset([0, 3, 9, 27], "checkpoint.pgm");
    add_image(&mut project, image.clone());
    save_project(&mut project, Some(sandbox.target()));
    let committed = fs::read(sandbox.target().join(CURRENT)).unwrap();
    let layer_id = project.info().surface_overlays.layers[0].layer_id;
    project
        .commit(
            project
                .prepare(Edit::RemoveSurfaceFinish { layer_id })
                .unwrap(),
        )
        .unwrap();
    assert!(project.surface_images().is_empty());
    let before = project.info();
    let path = sandbox
        .target()
        .join(IMAGES)
        .join(format!("{}.pgm", image.record().image_hash.as_str()));
    fs::remove_file(&path).unwrap();
    assert_eq!(
        save(project.capture(), project.storage(), None, &flag(), |_| {})
            .unwrap_err()
            .code,
        ProjectErrorCode::MissingAsset
    );
    assert_eq!(project.info(), before);
    assert_eq!(fs::read(sandbox.target().join(CURRENT)).unwrap(), committed);
    fs::write(&path, image.bytes().as_slice()).unwrap();
    save_project(&mut project, None);
    let recovered = open(&sandbox.target(), true, true, &flag()).unwrap();
    assert_eq!(
        recovered.surface_images.values().next().unwrap().record(),
        image.record()
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
fn process_disabled_targets_and_required_counters_are_strict_on_fresh_reopen() {
    use spiling_contracts::{process::*, surface::SurfaceScope};
    let sandbox = Sandbox::new();
    let mut project = imported();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    let prepared = project
        .prepare(Edit::Process {
            edit: ProcessEdit::AddLayer {
                scope: SurfaceScope::Occurrence {
                    occurrence_id: OccurrenceId::new(1).unwrap(),
                    expected_definition_id: definition_id,
                },
                selector: ProcessSelector::WholeOwnedVolume,
                interface: None,
                action: ProcessAction::Replace {
                    value: ProcessValue::SparseFraction { fraction: 0.5 },
                },
            },
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let layer_id = project.info().process_document.layers[0].layer_id;
    let prepared = project
        .prepare(Edit::Process {
            edit: ProcessEdit::SetLayerEnabled {
                layer_id,
                enabled: false,
            },
        })
        .unwrap();
    project.commit(prepared).unwrap();
    save_project(&mut project, Some(sandbox.target()));
    let valid = serde_json::to_value(project.capture().manifest).unwrap();
    drop(project);
    for (pointer, replacement) in [
        (
            "/process_document/layers/0/scope/occurrence_id",
            serde_json::json!(99),
        ),
        (
            "/process_document/layers/0/scope/expected_definition_id",
            serde_json::json!(
                spiling_contracts::geometry::DefinitionId::from_source_sha256(&[1; 32])
            ),
        ),
        (
            "/process_document/layers/0/operations/0/action/value/fraction",
            serde_json::json!(0),
        ),
        ("/next_process_layer", serde_json::json!(1)),
        ("/next_process_operation", serde_json::json!(1)),
        (
            "/format_version",
            serde_json::json!(spiling_contracts::project::PROJECT_FORMAT_VERSION - 1),
        ),
        ("/process_document/schema_version", serde_json::json!(0)),
    ] {
        let mut corrupt = valid.clone();
        *corrupt.pointer_mut(pointer).unwrap() = replacement;
        fs::write(
            sandbox.target().join(CURRENT),
            serde_json::to_vec(&corrupt).unwrap(),
        )
        .unwrap();
        assert!(open(&sandbox.target(), true, false, &flag()).is_err());
    }
    for field in [
        "process_document",
        "next_process_layer",
        "next_process_operation",
    ] {
        let mut corrupt = valid.clone();
        corrupt.as_object_mut().unwrap().remove(field);
        fs::write(
            sandbox.target().join(CURRENT),
            serde_json::to_vec(&corrupt).unwrap(),
        )
        .unwrap();
        assert!(open(&sandbox.target(), true, false, &flag()).is_err());
    }
}

#[test]
fn process_recovery_preserves_newer_layer_and_operation_allocator_high_water() {
    use spiling_contracts::{process::*, surface::SurfaceScope};
    let sandbox = Sandbox::new();
    let mut project = imported();
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    let scope = SurfaceScope::Definition { definition_id };
    let add = |project: &mut Project, fraction| {
        let prepared = project
            .prepare(Edit::Process {
                edit: ProcessEdit::AddLayer {
                    scope: scope.clone(),
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
    add(&mut project, 0.3);
    save_project(&mut project, Some(sandbox.target()));
    let previous = project.capture().manifest;
    add(&mut project, 0.4);
    add(&mut project, 0.6);
    save_project(&mut project, None);
    let current = project.capture().manifest;
    drop(project);
    fs::write(sandbox.target().join(CURRENT), b"interrupted manifest").unwrap();
    let mut recovered =
        Project::from_loaded(open(&sandbox.target(), false, true, &flag()).unwrap()).unwrap();
    assert_eq!(recovered.info().process_document, previous.process_document);
    assert_eq!(
        recovered.capture().manifest.next_process_layer,
        current.next_process_layer
    );
    assert_eq!(
        recovered.capture().manifest.next_process_operation,
        current.next_process_operation
    );
    add(&mut recovered, 0.7);
    let latest = recovered
        .info()
        .process_document
        .layers
        .last()
        .unwrap()
        .clone();
    assert_eq!(latest.layer_id.get(), current.next_process_layer);
    assert_eq!(
        latest.operations[0].operation_id.get(),
        current.next_process_operation
    );
    save_project(&mut recovered, None);
    assert!(!recovered.info().dirty);
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
fn appearance_assets_precede_manifest_and_save_preserves_semantic_history() {
    let sandbox = Sandbox::new();
    let (mut project, asset) = appearance_project();
    save_project(&mut project, Some(sandbox.target()));
    let saved = project.capture().manifest;
    let mut target = crate::tests::appearance_target();
    target.delta_e00_tolerance = 4.0;
    crate::tests::commit_appearance(
        &mut project,
        Edit::SetAppearanceTarget {
            target,
            limits: spiling_contracts::appearance::SearchLimits::default(),
        },
    );
    let next = project.capture().manifest;
    let receipt = save(
        project.capture(),
        project.storage(),
        None,
        &flag(),
        |stage| {
            if stage == SaveStage::AssetsSynced {
                assert_eq!(
                    fs::read(
                        sandbox
                            .target()
                            .join(PALETTE_ORIGINALS)
                            .join(format!("{}.json", asset.record().original_hash.as_str()))
                    )
                    .unwrap(),
                    *asset.original().as_ref()
                );
                assert_eq!(
                    fs::read(
                        sandbox
                            .target()
                            .join(PALETTE_CANONICAL)
                            .join(format!("{}.json", asset.record().canonical_hash.as_str()))
                    )
                    .unwrap(),
                    *asset.canonical().as_ref()
                );
                assert_eq!(
                    open(&sandbox.target(), true, false, &flag())
                        .unwrap()
                        .manifest,
                    saved
                );
            }
        },
    )
    .unwrap();
    project.mark_saved(receipt).unwrap();
    assert!(!project.info().dirty && project.info().can_undo);
    project.undo().unwrap();
    assert!(project.info().dirty);
    assert_eq!(
        project.appearance_study().selection.as_deref(),
        Some("red/catalog")
    );
    project.redo().unwrap();
    assert!(!project.info().dirty);
    assert_eq!(
        project.capture().manifest.appearance_study,
        next.appearance_study
    );
    let recovered =
        Project::from_loaded(open(&sandbox.target(), true, true, &flag()).unwrap()).unwrap();
    assert_eq!(recovered.appearance_study(), &saved.appearance_study);
    assert!(recovered.info().dirty && recovered.info().recovered_previous);
    assert_eq!(
        recovered
            .snapshot()
            .manufacturing_artifact
            .as_ref()
            .unwrap()
            .record,
        saved.manufacturing_artifact.unwrap()
    );
}

#[test]
fn appearance_storage_rejects_corruption_missing_assets_and_producer_mismatch() {
    for original in [true, false] {
        let sandbox = Sandbox::new();
        let (mut project, asset) = appearance_project();
        save_project(&mut project, Some(sandbox.target()));
        let (directory, hash) = if original {
            (PALETTE_ORIGINALS, &asset.record().original_hash)
        } else {
            (PALETTE_CANONICAL, &asset.record().canonical_hash)
        };
        let path = sandbox
            .target()
            .join(directory)
            .join(format!("{}.json", hash.as_str()));
        let bytes = fs::read(&path).unwrap();
        let mut bad = bytes.clone();
        bad[0] ^= 1;
        fs::write(&path, bad).unwrap();
        assert_eq!(
            open(&sandbox.target(), true, false, &flag())
                .unwrap_err()
                .code,
            ProjectErrorCode::CorruptAsset
        );
        assert_eq!(
            save(project.capture(), project.storage(), None, &flag(), |_| {})
                .unwrap_err()
                .code,
            ProjectErrorCode::CorruptAsset
        );
        fs::write(&path, bytes).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(
            open(&sandbox.target(), true, false, &flag())
                .unwrap_err()
                .code,
            ProjectErrorCode::MissingAsset
        );
    }
    let sandbox = Sandbox::new();
    let (mut project, _) = appearance_project();
    save_project(&mut project, Some(sandbox.target()));
    let path = sandbox.target().join(CURRENT);
    let bytes = fs::read(&path).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    manifest["appearance_study"]["report"]["producer"] = "foreign".into();
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
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
            MAX_RETAINED_SOURCE_BYTES,
            MAX_MANUFACTURING_RETAINED_BYTES as usize,
            MAX_SURFACE_IMAGE_RETAINED_BYTES,
            0,
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
fn appearance_directory_attachment_and_exclusive_assets_are_preserved() {
    for directory in [PALETTE_ORIGINALS, PALETTE_CANONICAL] {
        let sandbox = Sandbox::new();
        let (mut project, _) = appearance_project();
        save_project(&mut project, Some(sandbox.target()));
        let detached = sandbox.0.join("detached");
        fs::rename(sandbox.target().join(directory), &detached).unwrap();
        fs::create_dir(sandbox.target().join(directory)).unwrap();
        let before = fs::read(sandbox.target().join(CURRENT)).unwrap();
        assert!(save(project.capture(), project.storage(), None, &flag(), |_| {}).is_err());
        assert_eq!(fs::read(sandbox.target().join(CURRENT)).unwrap(), before);
        assert!(detached.exists());
    }
}

#[test]
fn appearance_storage_rejects_hash_valid_original_canonical_semantic_disagreement() {
    let sandbox = Sandbox::new();
    let (mut project, asset) = appearance_project();
    save_project(&mut project, Some(sandbox.target()));
    let mut document = asset.document().clone();
    document.rows[0].name = "Different original evidence record".into();
    let original = serde_json::to_vec(&document).unwrap();
    let hash = SourceHash::from_bytes(&original);
    fs::write(
        sandbox
            .target()
            .join(PALETTE_ORIGINALS)
            .join(format!("{}.json", hash.as_str())),
        &original,
    )
    .unwrap();
    let path = sandbox.target().join(CURRENT);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["appearance_study"]["palette"]["original_hash"] = hash.as_str().into();
    manifest["appearance_study"]["palette"]["original_bytes"] = original.len().into();
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
}

#[test]
fn appearance_save_never_collects_palette_assets_needed_by_previous_or_history() {
    let sandbox = Sandbox::new();
    let (mut project, first) = appearance_project();
    save_project(&mut project, Some(sandbox.target()));
    let mut document = first.document().clone();
    document.name = "Second palette".into();
    let canonical = document.canonical_bytes().unwrap();
    let original = serde_json::to_vec_pretty(&document).unwrap();
    let record = spiling_contracts::appearance::PaletteRecord::new(
        &document,
        &original,
        &canonical,
        "second.json".into(),
    )
    .unwrap();
    let second =
        Arc::new(PaletteAsset::from_document(record, original, canonical, document).unwrap());
    crate::tests::commit_appearance(
        &mut project,
        Edit::ImportAppearancePalette {
            asset: second.clone(),
        },
    );
    save_project(&mut project, None);
    for asset in [&first, &second] {
        assert_eq!(
            fs::read(
                sandbox
                    .target()
                    .join(PALETTE_ORIGINALS)
                    .join(format!("{}.json", asset.record().original_hash.as_str()))
            )
            .unwrap(),
            *asset.original().as_ref(),
        );
        assert_eq!(
            fs::read(
                sandbox
                    .target()
                    .join(PALETTE_CANONICAL)
                    .join(format!("{}.json", asset.record().canonical_hash.as_str()))
            )
            .unwrap(),
            *asset.canonical().as_ref(),
        );
    }
    let previous =
        Project::from_loaded(open(&sandbox.target(), true, true, &flag()).unwrap()).unwrap();
    assert_eq!(
        previous.appearance_study().palette.as_ref().unwrap(),
        first.record()
    );
    project.undo().unwrap();
    assert_eq!(
        project.appearance_study().palette.as_ref().unwrap(),
        first.record()
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

#[test]
fn optical_recovery_preserves_accepted_report_and_rejects_forged_palette_evidence_closure() {
    let sandbox = Sandbox::new();
    let (mut project, _, report) = crate::optical_tests::optical_project();
    save_project(&mut project, Some(sandbox.target()));
    let first = project.capture().manifest;
    let mut request = crate::optical_tests::optical_request();
    request.patch.width_mm = 3.0;
    crate::tests::commit_appearance(&mut project, Edit::SetOpticalStudy { request });
    save_project(&mut project, None);
    let recovered =
        Project::from_loaded(open(&sandbox.target(), true, true, &flag()).unwrap()).unwrap();
    assert_eq!(recovered.appearance_study(), &first.appearance_study);
    assert_eq!(
        recovered
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .report
            .as_ref()
            .unwrap()
            .encode()
            .unwrap(),
        report.encode().unwrap()
    );
    assert!(recovered.info().recovered_previous && recovered.info().dirty);

    // A fresh descriptor hash is not authority to change the retained evidence.
    let path = sandbox.target().join(CURRENT);
    let mut forged = first;
    let optical = forged.appearance_study.optical.as_mut().unwrap();
    let changed = Arc::make_mut(optical.report.as_mut().unwrap());
    changed.material_evidence_hashes[0] = SourceHash::from_bytes(b"foreign material evidence");
    optical.report_record = Some(changed.canonical_record().unwrap());
    forged.validate().unwrap();
    fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );
    let changed = Arc::make_mut(
        forged
            .appearance_study
            .optical
            .as_mut()
            .unwrap()
            .report
            .as_mut()
            .unwrap(),
    );
    changed.producer = "unrecognized-optical-model".into();
    fs::write(path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert_eq!(
        open(&sandbox.target(), true, false, &flag())
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
}
