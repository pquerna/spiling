// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::{
    Edit, Project,
    tests::{asset, imported},
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
            count - 1
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
