use super::*;
use spiling_core::storage::{StorageCopyReceipt, StorageStats};

pub(super) struct MaintenanceProduct {
    pub action: StorageMaintenanceAction,
    pub stats: Option<ProjectStorageStats>,
    pub copy: Option<StorageCopyReceipt>,
}

pub(super) fn maintain(
    storage: Arc<Storage>,
    action: StorageMaintenanceAction,
    target: Option<NativePath>,
    commit: crate::session::CommitToken,
    cancel: &AtomicBool,
) -> Result<MaintenanceProduct, JobError> {
    let (stats, copy) = match action {
        StorageMaintenanceAction::Stats => (Some(storage.stats(cancel)?), None),
        StorageMaintenanceAction::CollectUnreachable => (
            Some(storage.collect_unreachable(cancel, || commit.begin_commit())?),
            None,
        ),
        StorageMaintenanceAction::CompactToNew => {
            let target = target.ok_or_else(|| {
                project_error(
                    ProjectErrorCode::InvalidProject,
                    "compact requires a new target file",
                )
            })?;
            let target = worker::project_path(target)?;
            let receipt = storage.compact_to_new(&target, cancel, |stage| {
                if stage == SaveStage::BeforeCommit && !commit.begin_commit() {
                    cancel.store(true, Ordering::Release);
                }
            })?;
            (receipt.stats.clone(), Some(receipt))
        }
    };
    Ok(MaintenanceProduct {
        action,
        stats: stats.map(stats_view),
        copy,
    })
}

fn stats_view(stats: StorageStats) -> ProjectStorageStats {
    let limits = stats.limits;
    ProjectStorageStats {
        current_raw_bytes: stats.current_raw_bytes,
        previous_only_raw_bytes: stats.previous_only_raw_bytes,
        reachable_payload_bytes: stats.reachable_payload_bytes,
        unreachable_payload_bytes: stats.unreachable_payload_bytes,
        database_bytes: stats.database_bytes,
        free_page_bytes: stats.free_page_bytes,
        journal_bytes: stats.journal_bytes,
        snapshots: stats.snapshots,
        objects: stats.objects,
        chunks: stats.chunks,
        previous_only_payload_bytes: stats.previous_only_payload_bytes,
        scratch_bytes: stats.scratch_bytes,
        scratch_entries: stats.scratch_entries,
        membership_rows: stats.membership_rows,
        mapping_rows: stats.mapping_rows,
        retained_roots: stats.retained_roots,
        limits: ProjectStorageLimits {
            chunk_bytes: limits.chunk_bytes,
            object_bytes: limits.object_bytes,
            root_bytes: limits.root_bytes,
            guard_bytes: limits.guard_bytes,
            snapshot_bytes: limits.snapshot_bytes,
            database_bytes: limits.database_bytes,
            journal_bytes: limits.journal_bytes,
            scratch_bytes: limits.scratch_bytes,
            scratch_entries: limits.scratch_entries,
            snapshots: limits.snapshots,
            membership_rows: limits.membership_rows,
            objects: limits.objects,
            mapping_rows: limits.mapping_rows,
            chunks: limits.chunks,
            members_per_snapshot: limits.members_per_snapshot,
        },
    }
}

impl Runtime {
    pub(super) fn maintain_storage(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        action: StorageMaintenanceAction,
        target: Option<NativePath>,
    ) -> Result<Reply, ProjectError> {
        if session != &self.session_id || base != self.project.info().revision {
            return Err(project_error(
                ProjectErrorCode::StaleRevision,
                "session or project revision is stale",
            ));
        }
        if self.active.is_some() {
            return Err(project_error(
                ProjectErrorCode::Busy,
                "an engine job is active",
            ));
        }
        if (action == StorageMaintenanceAction::CompactToNew) != target.is_some() {
            return Err(project_error(
                ProjectErrorCode::InvalidProject,
                "only compact-to-new requires a new target file",
            ));
        }
        if action != StorageMaintenanceAction::Stats && self.project.info().read_only {
            return Err(project_error(
                ProjectErrorCode::ReadOnly,
                "project is read-only",
            ));
        }
        if action == StorageMaintenanceAction::CompactToNew && self.uncertain_copy.is_some() {
            return Err(project_error(
                ProjectErrorCode::Busy,
                "an uncertain compact target retains its lease; inspect explicitly and restart before compacting again",
            ));
        }
        let storage = self.project.storage().ok_or_else(|| {
            project_error(
                ProjectErrorCode::NoSavedPath,
                "storage maintenance requires a saved .spil project file",
            )
        })?;
        if let Some(target) = &target {
            target.validate().map_err(project_geometry_error)?;
        }
        self.start(Purpose::StorageMaintenance, 0, move |job, _, watchdog| {
            Work::StorageMaintenance {
                storage,
                action,
                target,
                commit: watchdog.commit_token(job),
            }
        })
        .map_err(project_geometry_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acknowledge(runtime: &mut Runtime, kind: PublicationKind, purpose: Purpose) -> EngineJob {
        let job = JobId::new(1).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        runtime.watchdog.start(job, cancel.clone());
        runtime.jobs.insert(
            job,
            EngineJob {
                job_id: job,
                status: JobStatus::Running,
                stage: "committing".into(),
                result: None,
                error: None,
            },
        );
        runtime.active = Some(Active {
            id: job,
            base: runtime.revision,
            project_base: runtime.project.info().revision,
            artifact: ArtifactId::new(1).unwrap(),
            purpose,
            cancel,
            deciding: true,
            promote: true,
            publication: Some(Publication {
                kind,
                artifact: None,
            }),
            failure: None,
        });
        runtime
            .event(Event {
                job,
                transition: Transition::Acknowledged { promoted: true },
                staged: None,
            })
            .unwrap();
        runtime.job(job).unwrap()
    }

    #[test]
    fn uncertain_save_receipts_attach_before_failed_ack_without_success_or_baseline() {
        for outcome in [
            PublicationOutcome::OutcomeUnknown,
            PublicationOutcome::PublishedUnconfirmed,
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut runtime = Runtime::new(None);
            let before = runtime.project.info();
            let mut receipt = spiling_core::storage::save(
                runtime.project.capture(),
                None,
                Some(root.path().join("attempted.spil")),
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
            receipt.outcome = outcome;
            receipt.durability_error = Some(project_error(
                ProjectErrorCode::Io,
                "injected uncertain receipt",
            ));
            let job = acknowledge(&mut runtime, PublicationKind::Save(receipt), Purpose::Save);
            assert_eq!(job.status, JobStatus::Failed);
            let Some(JobResult::ProjectSaveUncertain { info }) = job.result else {
                panic!("failed-only uncertain receipt")
            };
            assert!(info.save_uncertain && info.dirty);
            assert_eq!(info.saved_revision, before.saved_revision);
            assert_eq!(runtime.project.info(), info);
            assert_eq!(
                runtime.project.storage().unwrap().path(),
                root.path().join("attempted.spil")
            );
            assert_eq!(runtime.revision, SceneRevision::ZERO);
            runtime.shutdown().unwrap();
        }
    }

    #[test]
    fn uncertain_compact_ack_retains_bounded_target_lease_without_attaching_or_stats() {
        let root = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::new(None);
        let before = runtime.project.info();
        let target = spiling_core::storage::save(
            runtime.project.capture(),
            None,
            Some(root.path().join("copy.spil")),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap()
        .storage;
        let retained = Arc::downgrade(&target);
        let product = MaintenanceProduct {
            action: StorageMaintenanceAction::CompactToNew,
            stats: None,
            copy: Some(StorageCopyReceipt {
                storage: target,
                outcome: PublicationOutcome::OutcomeUnknown,
                durability_error: None,
                stats: None,
            }),
        };
        let job = acknowledge(
            &mut runtime,
            PublicationKind::StorageMaintenance(product),
            Purpose::StorageMaintenance,
        );
        assert_eq!(job.status, JobStatus::Failed);
        assert!(job.result.is_none());
        assert_eq!(runtime.project.info(), before);
        assert!(runtime.project.storage().is_none());
        assert!(retained.upgrade().is_some());
        assert_eq!(runtime.revision, SceneRevision::ZERO);
        let session = runtime.session_id.clone();
        let refused = runtime.maintain_storage(
            &session,
            before.revision,
            StorageMaintenanceAction::CompactToNew,
            Some(NativePath::from_os_str(root.path().join("second.spil").as_os_str()).unwrap()),
        );
        assert!(matches!(
            refused,
            Err(ProjectError {
                code: ProjectErrorCode::Busy,
                ..
            })
        ));
        runtime.shutdown().unwrap();
        drop(runtime);
        assert!(retained.upgrade().is_none());
    }
}
