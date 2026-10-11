// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::geometry::{Control, JobStatus};
use crate::native::NativeHost;
use spiling_contracts::{
    NATIVE_METADATA_TYPE, NATIVE_RESULT_TYPE, geometry::SessionId, native_metadata,
};

const NATIVE_STORAGE_BUDGET: u64 = 256 * 1024 * 1024;

pub struct NativeAdmission {
    pub parent: String,
    pub request_id: String,
    pub digest: String,
    pub reservation: u64,
    pub command: Control,
}

pub(super) fn pack_native(meta: &NativeOperationMetadata) -> prost_types::Any {
    prost_types::Any {
        type_url: NATIVE_METADATA_TYPE.into(),
        value: meta.encode_to_vec(),
    }
}
pub(super) fn state_from_diagnostic(state: DiagnosticState) -> NativeOperationState {
    match state {
        DiagnosticState::Queued => NativeOperationState::NativeQueued,
        DiagnosticState::Running => NativeOperationState::NativeRunning,
        DiagnosticState::Cancelling => NativeOperationState::NativeCancelling,
        DiagnosticState::Succeeded => NativeOperationState::NativeSucceeded,
        DiagnosticState::Cancelled => NativeOperationState::NativeCancelled,
        DiagnosticState::Interrupted => NativeOperationState::NativeInterrupted,
        _ => NativeOperationState::NativeFailed,
    }
}
pub(super) fn terminal_native(
    op: &mut Operation,
    state: NativeOperationState,
    error: Option<(Code, &str)>,
) -> Result<(), Status> {
    let mut meta = native_metadata(op).map_err(internal)?;
    meta.state = state as i32;
    meta.phase = state.as_str_name().to_lowercase();
    meta.state_version += 1;
    meta.update_time = Some(now());
    meta.end_time = meta.update_time;
    op.done = true;
    if let Some((code, message)) = error {
        use spiling_contracts::geometry::GeometryErrorCode as E;
        let domain_code = if matches!(
            state,
            NativeOperationState::NativeCancelled | NativeOperationState::NativeInterrupted
        ) {
            E::Cancelled
        } else {
            match code {
                Code::Cancelled => E::Cancelled,
                Code::Aborted => E::StaleRevision,
                Code::ResourceExhausted => E::ResourceLimit,
                Code::InvalidArgument => E::InvalidGeometry,
                Code::FailedPrecondition => E::Busy,
                Code::NotFound => E::UnknownHandle,
                _ => E::KernelFailure,
            }
        };
        let mut status = spiling_contracts::domain_status(
            &spiling_contracts::geometry::GeometryError::new(domain_code, message).into(),
        );
        if state == NativeOperationState::NativeInterrupted {
            status.code = Code::Aborted as i32;
        }
        op.result = Some(Outcome::Error(status));
    }
    op.metadata = Some(pack_native(&meta));
    Ok(())
}
pub(super) fn cancel_native(s: &mut Inner, mut op: Operation) -> Result<(), Status> {
    let mut meta = native_metadata(&op).map_err(internal)?;
    if !op.done && meta.state != NativeOperationState::NativeCancelling as i32 {
        meta.state = NativeOperationState::NativeCancelling as i32;
        meta.phase = "cancelling".into();
        meta.state_version += 1;
        meta.update_time = Some(now());
        op.metadata = Some(pack_native(&meta));
        s.save(&op)?;
    }
    Ok(())
}

impl Jobs {
    pub async fn start_native(
        &self,
        host: NativeHost,
        admission: NativeAdmission,
    ) -> Result<Operation, Status> {
        // Both admission and the execution handoff survive a dropped RPC future.
        let jobs = self.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = tx.send(jobs.accept_native(host, admission).await);
        });
        rx.await.map_err(internal)?
    }
    async fn accept_native(
        &self,
        host: NativeHost,
        admission: NativeAdmission,
    ) -> Result<Operation, Status> {
        validate_native_parent(&admission.parent)?;
        let id = if admission.request_id.is_empty() {
            Uuid::new_v4()
        } else {
            Uuid::parse_str(&admission.request_id)
                .map_err(|_| Status::invalid_argument("request_id must be a UUID"))?
        };
        if id.is_nil() {
            return Err(Status::invalid_argument("request_id must not be zero UUID"));
        }
        let request_id = id.to_string();
        let parent = admission.parent.clone();
        let digest = admission.digest.clone();
        // Historical deduplicated results do not require a live native session.
        let existing = self
            .with(move |s| {
                let old: Option<(String, Vec<u8>)> =
                    s.db.query_row(
                        "SELECT digest,operation FROM operations WHERE parent=? AND request_id=?",
                        params![parent, request_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(internal)?;
                old.map(|(previous, bytes)| {
                    if previous != digest {
                        return Err(Status::already_exists(
                            "request_id already used with different parameters",
                        ));
                    }
                    Operation::decode(bytes.as_slice()).map_err(internal)
                })
                .transpose()
            })
            .await?;
        if let Some(op) = existing {
            return Ok(op);
        }
        let (session, scene, project) = host.capture().await?;
        if admission.parent != format!("sessions/{}", session.as_str()) {
            return Err(crate::native::domain_tonic(
                spiling_contracts::geometry::GeometryError::new(
                    spiling_contracts::geometry::GeometryErrorCode::StaleRevision,
                    "native session is stale",
                )
                .into(),
            ));
        }
        let request_id = id.to_string();
        let parent = admission.parent;
        let digest = admission.digest;
        let reservation = admission.reservation;
        let (op, created) = self.with(move |s| {
            let old: Option<(String, Vec<u8>)> = s.db.query_row(
                "SELECT digest,operation FROM operations WHERE parent=? AND request_id=?", params![parent, request_id],
                |r| Ok((r.get(0)?,r.get(1)?))
            ).optional().map_err(internal)?;
            if let Some((previous, bytes)) = old {
                if previous != digest { return Err(Status::already_exists("request_id already used with different parameters")); }
                return Ok((Operation::decode(bytes.as_slice()).map_err(internal)?,false));
            }
            let rows = {
                let mut stmt = s.db.prepare("SELECT operation FROM operations").map_err(internal)?;
                stmt.query_map([], |r| r.get::<_,Vec<u8>>(0)).map_err(internal)?.collect::<Result<Vec<_>,_>>().map_err(internal)?
            };
            if rows.len() >= MAX_OPERATIONS || rows.iter().filter(|b| Operation::decode(b.as_slice()).is_ok_and(|o| !o.done)).count() >= MAX_PENDING {
                return Err(Status::resource_exhausted("operation admission capacity reached"));
            }
            let reserved = u64::try_from(s.db.query_row("SELECT COALESCE(SUM(reserved_bytes),0) FROM operations WHERE name IN (SELECT name FROM native_operations)",[],|r| r.get::<_,i64>(0)).map_err(internal)?).map_err(internal)?;
            if reservation > NATIVE_STORAGE_BUDGET.saturating_sub(reserved) { return Err(Status::resource_exhausted("native durable artifact reservation capacity reached")); }
            let meta = NativeOperationMetadata {
                state: NativeOperationState::NativeQueued as i32, state_version: 1,
                session_id: session.as_str().to_owned(), captured_scene_revision: scene.get(), captured_project_revision: project.get(),
                phase: "queued".into(), completed_units: 0, total_units: 1, outputs: vec![],
                create_time: Some(now()), update_time: Some(now()), end_time: None,
            };
            let op = Operation { name: format!("{parent}/operations/{}", Uuid::new_v4()), metadata: Some(pack_native(&meta)), done: false, result: None };
            let tx = s.db.transaction().map_err(internal)?;
            tx.execute("INSERT INTO operations VALUES(?,?,?,?,?,?)",params![op.name,parent,request_id,digest,i64::try_from(reservation).map_err(internal)?,op.encode_to_vec()]).map_err(internal)?;
            tx.execute("INSERT INTO native_operations VALUES(?)",[&op.name]).map_err(internal)?;
            tx.commit().map_err(internal)?;
            Ok((op,true))
        }).await?;
        if created {
            let jobs = self.clone();
            let name = op.name.clone();
            tokio::spawn(async move {
                if let Err(error) = jobs
                    .run_native(host, name.clone(), admission.command, project)
                    .await
                {
                    let _ = jobs
                        .with(move |s| {
                            let mut op = s.get(&name)?;
                            if !op.done {
                                terminal_native(
                                    &mut op,
                                    NativeOperationState::NativeFailed,
                                    Some((error.code(), error.message())),
                                )?;
                                s.save(&op)?;
                            }
                            Ok(())
                        })
                        .await;
                }
            });
        }
        Ok(op)
    }
    async fn run_native(
        &self,
        host: NativeHost,
        name: String,
        command: Control,
        captured_project: spiling_contracts::project::ProjectRevision,
    ) -> Result<(), Status> {
        let mut updates = self.subscribe(name.clone()).await?;
        let permit = loop {
            if native_cancelled(&updates.borrow()) {
                return self.finish_native_cancel(name, &command).await;
            }
            tokio::select! {
                permit = self.worker.clone().acquire_owned() => break permit.map_err(internal)?,
                changed = updates.changed() => { changed.map_err(internal)?; }
            }
        };
        if native_cancelled(&updates.borrow()) {
            return self.finish_native_cancel(name, &command).await;
        }
        let id = host.start(name.clone(), command, captured_project).await?;
        loop {
            let op = self
                .get(name.clone())
                .await
                .unwrap_or_else(crate::native::fatal);
            if op.done {
                break;
            }
            let cancelling = native_metadata(&op)
                .map_err(internal)
                .unwrap_or_else(crate::native::fatal)
                .state
                == NativeOperationState::NativeCancelling as i32;
            match host.refresh(name.clone(), id, cancelling).await {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => crate::native::fatal::<()>(error),
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(permit);
        Ok(())
    }
    async fn finish_native_cancel(&self, name: String, command: &Control) -> Result<(), Status> {
        let error: spiling_contracts::geometry::JobError = match command {
            Control::Geometry(_) => spiling_contracts::geometry::GeometryError::new(
                spiling_contracts::geometry::GeometryErrorCode::Cancelled,
                "operation cancelled",
            )
            .into(),
            Control::Project(_) => spiling_contracts::project::ProjectError::new(
                spiling_contracts::project::ProjectErrorCode::Cancelled,
                "operation cancelled",
            )
            .into(),
            Control::Manufacturing(_) => spiling_contracts::manufacturing::ManufacturingError::new(
                spiling_contracts::manufacturing::ManufacturingErrorCode::Cancelled,
                "operation cancelled",
            )
            .into(),
        };
        let status = spiling_contracts::domain_status(&error);
        self.with(move |s| {
            let mut op = s.get(&name)?;
            if !op.done {
                terminal_native(&mut op, NativeOperationState::NativeCancelled, None)?;
                op.result = Some(Outcome::Error(status));
                s.save(&op)?;
            }
            Ok(())
        })
        .await
    }
    pub(crate) fn native_progress(
        &self,
        name: &str,
        stage: &str,
        status: JobStatus,
    ) -> Result<(), Status> {
        self.with_sync(|s| {
            let mut op = s.get(name)?;
            if op.done {
                return Ok(());
            }
            let mut meta = native_metadata(&op).map_err(internal)?;
            let state = if meta.state == NativeOperationState::NativeCancelling as i32
                || status == JobStatus::Cancelling
            {
                NativeOperationState::NativeCancelling
            } else {
                NativeOperationState::NativeRunning
            };
            let phase = if state == NativeOperationState::NativeCancelling && stage != "committing"
            {
                "cancelling"
            } else {
                stage
            };
            if meta.phase != phase || meta.state != state as i32 {
                meta.phase = phase.into();
                meta.state = state as i32;
                meta.state_version += 1;
                meta.update_time = Some(now());
                op.metadata = Some(pack_native(&meta));
                s.save(&op)?;
            }
            Ok(())
        })
    }
    pub(crate) fn native_finish(
        &self,
        name: &str,
        result: Result<spiling_contracts::geometry::JobResult, RpcStatus>,
        buffers: Vec<(Artifact, &[u8])>,
    ) -> Result<(), Status> {
        self.with_sync(|s| {
            let mut op = s.get(name)?;
            if op.done { return Ok(()); }
            let mut meta = native_metadata(&op).map_err(internal)?;
            meta.outputs = buffers.iter().map(|(artifact,_)|artifact.clone()).collect();
            meta.completed_units = 1;
            op.metadata = Some(pack_native(&meta));
            match result {
                Ok(result) => {
                    let result = NativeOperationResult::try_from(result).map_err(internal)?;
                    op.result = Some(Outcome::Response(prost_types::Any {type_url:NATIVE_RESULT_TYPE.into(),value:result.encode_to_vec()}));
                    terminal_native(&mut op, NativeOperationState::NativeSucceeded, None)?;
                }
                Err(error) => {
                    let state = if error.code == Code::Cancelled as i32 { NativeOperationState::NativeCancelled } else { NativeOperationState::NativeFailed };
                    terminal_native(&mut op,state,None)?;
                    op.result = Some(Outcome::Error(error));
                }
            }
            let mut size: u64 = 0;
            let tx = s.db.transaction().map_err(internal)?;
            let mut used = u64::try_from(tx.query_row("SELECT COALESCE(SUM(length(bytes)),0) FROM artifacts JOIN artifact_types USING(name)", [], |r|r.get::<_,i64>(0)).map_err(internal)?).map_err(internal)?;
            for (artifact,bytes) in buffers {
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM artifacts WHERE name=?)", [&artifact.name], |r|r.get(0)).map_err(internal)?;
                if !exists {
                    if artifact.size_bytes > NATIVE_STORAGE_BUDGET.saturating_sub(used) {
                        return Err(Status::resource_exhausted("native immutable storage capacity reached"));
                    }
                    used += artifact.size_bytes;
                    size += artifact.size_bytes;
                    tx.execute("INSERT INTO artifacts VALUES(?,?)",params![artifact.name,bytes]).map_err(internal)?;
                    tx.execute("INSERT INTO artifact_types VALUES(?,?)",params![artifact.name,artifact.media_type]).map_err(internal)?;
                }
            }
            tx.execute("UPDATE operations SET operation=?,reserved_bytes=? WHERE name=?",params![op.encode_to_vec(),i64::try_from(size).map_err(internal)?,op.name]).map_err(internal)?;
            tx.commit().map_err(internal)?; s.notify(&op); Ok(())
        })
    }
    pub(crate) fn describe_native_artifact(
        &self,
        bytes: &[u8],
        media_type: &str,
        limit: u64,
    ) -> Result<Artifact, Status> {
        if bytes.len() as u64 > limit {
            return Err(Status::resource_exhausted(
                "native artifact exceeds declared cap",
            ));
        }
        let hash = format!("{:x}", Sha256::digest(bytes));
        let artifact = Artifact {
            name: format!("artifacts/{hash}"),
            size_bytes: bytes.len() as u64,
            sha256: hash,
            media_type: media_type.into(),
        };
        Ok(artifact)
    }
}
fn native_cancelled(op: &Operation) -> bool {
    op.done
        || native_metadata(op)
            .is_ok_and(|m| m.state == NativeOperationState::NativeCancelling as i32)
}

pub(super) fn validate_native_parent(parent: &str) -> Result<(), Status> {
    let id = parent
        .strip_prefix("sessions/")
        .ok_or_else(|| Status::invalid_argument("parent must be sessions/{uuid}"))?;
    SessionId::parse(id).map_err(|_| Status::invalid_argument("invalid native session parent"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acknowledged_commit_wins_persisted_cancel_and_late_cancel_is_immutable() {
        let directory = tempfile::tempdir().unwrap();
        let jobs = Jobs::open(directory.path()).unwrap();
        let session = SessionId::new();
        let parent = format!("sessions/{}", session.as_str());
        let name = format!("{parent}/operations/{}", Uuid::new_v4());
        let info = spiling_core::Project::new().info();
        let meta = NativeOperationMetadata {
            state: NativeOperationState::NativeCancelling as i32,
            state_version: 2,
            session_id: session.as_str().to_owned(),
            captured_scene_revision: 0,
            captured_project_revision: info.revision.get(),
            phase: "cancelling".into(),
            completed_units: 0,
            total_units: 1,
            outputs: vec![],
            create_time: Some(now()),
            update_time: Some(now()),
            end_time: None,
        };
        let op = Operation {
            name: name.clone(),
            metadata: Some(pack_native(&meta)),
            done: false,
            result: None,
        };
        jobs.with_sync(|s| {
            let tx = s.db.transaction().map_err(internal)?;
            tx.execute(
                "INSERT INTO operations VALUES(?,?,?,?,?,?)",
                params![
                    name,
                    parent,
                    Uuid::new_v4().to_string(),
                    "commit-race",
                    0,
                    op.encode_to_vec()
                ],
            )
            .map_err(internal)?;
            tx.execute("INSERT INTO native_operations VALUES(?)", [&name])
                .map_err(internal)?;
            tx.commit().map_err(internal)?;
            Ok(())
        })
        .unwrap();

        // Runtime's ACK-winning terminal result is authoritative, not ledger cancel intent.
        jobs.native_finish(
            &name,
            Ok(spiling_contracts::geometry::JobResult::ProjectSaved { info: info.clone() }),
            vec![],
        )
        .unwrap();
        let completed = jobs.get(name.clone()).await.unwrap();
        let view = spiling_contracts::native_operation_view(&completed).unwrap();
        assert_eq!(
            view.status,
            spiling_contracts::geometry::JobStatus::Completed
        );
        assert!(view.error.is_none());
        assert!(
            matches!(view.result, Some(spiling_contracts::geometry::JobResult::ProjectSaved {info:receipt}) if receipt == info)
        );
        assert_eq!(native_metadata(&completed).unwrap().state_version, 3);
        jobs.cancel(name.clone()).await.unwrap();
        assert_eq!(
            jobs.get(name).await.unwrap().encode_to_vec(),
            completed.encode_to_vec()
        );
    }

    fn seed_native(
        jobs: &Jobs,
        session: &SessionId,
        scene: spiling_contracts::geometry::SceneRevision,
        project: spiling_contracts::project::ProjectRevision,
    ) -> Operation {
        let parent = format!("sessions/{}", session.as_str());
        let op = Operation {
            name: format!("{parent}/operations/{}", Uuid::new_v4()),
            metadata: Some(pack_native(&NativeOperationMetadata {
                state: NativeOperationState::NativeQueued as i32,
                state_version: 1,
                session_id: session.as_str().to_owned(),
                captured_scene_revision: scene.get(),
                captured_project_revision: project.get(),
                phase: "queued".into(),
                completed_units: 0,
                total_units: 1,
                outputs: vec![],
                create_time: Some(now()),
                update_time: Some(now()),
                end_time: None,
            })),
            done: false,
            result: None,
        };
        jobs.with_sync(|s| {
            let tx = s.db.transaction().map_err(internal)?;
            tx.execute(
                "INSERT INTO operations VALUES(?,?,?,?,?,?)",
                params![
                    op.name,
                    parent,
                    Uuid::new_v4().to_string(),
                    "native-regression",
                    64 * 1024 * 1024,
                    op.encode_to_vec()
                ],
            )
            .map_err(internal)?;
            tx.execute("INSERT INTO native_operations VALUES(?)", [&op.name])
                .map_err(internal)?;
            tx.commit().map_err(internal)?;
            Ok(())
        })
        .unwrap();
        op
    }

    #[tokio::test]
    async fn cancellation_between_dispatch_read_and_progress_reaches_commit_lock() {
        use spiling_contracts::{geometry::SceneRevision, project::ProjectRevision};
        use std::sync::atomic::{AtomicBool, Ordering};
        let directory = tempfile::tempdir().unwrap();
        let jobs = Jobs::open(directory.path()).unwrap();
        let op = seed_native(
            &jobs,
            &SessionId::new(),
            SceneRevision::ZERO,
            ProjectRevision::ZERO,
        );
        jobs.native_progress(&op.name, "importing", JobStatus::Running)
            .unwrap();
        let read = jobs.get(op.name.clone()).await.unwrap();
        assert_eq!(
            native_metadata(&read).unwrap().state,
            NativeOperationState::NativeRunning as i32
        );
        jobs.cancel(op.name.clone()).await.unwrap();
        // Progress uses the dispatcher's stale read, after cancellation has committed.
        jobs.native_progress(&op.name, "importing", JobStatus::Running)
            .unwrap();
        let cancel_intent = jobs
            .with_sync(|s| {
                let meta = native_metadata(&s.get(&op.name)?).map_err(internal)?;
                assert_eq!(meta.phase, "cancelling");
                Ok(meta.state == NativeOperationState::NativeCancelling as i32)
            })
            .unwrap();
        assert!(cancel_intent);
        let watchdog = crate::session::Watchdog::new();
        let id = crate::geometry::JobId::new(1).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        watchdog.start(id, flag.clone());
        if cancel_intent {
            watchdog.cancel(id);
        }
        assert!(flag.load(Ordering::Acquire));
        assert!(
            !watchdog.begin_commit(id),
            "persisted cancellation must reach the native promotion lock"
        );
        watchdog.finish(id);
    }

    #[tokio::test]
    async fn saturated_control_queue_retains_execution_until_native_ack_and_outputs() {
        use spiling_contracts::geometry::{GeometryCommand, NativePath, RigidPoseMm};
        let directory = tempfile::tempdir().unwrap();
        let jobs = Jobs::open(directory.path()).unwrap();
        let host = NativeHost::new(jobs.clone(), None);
        let (session, scene, project) = host.capture().await.unwrap();
        let op = seed_native(&jobs, &session, scene, project);
        let held_permit = jobs.worker.clone().acquire_owned().await.unwrap();
        let (ready, entered) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let store = jobs.clone();
        let blocker = tokio::task::spawn_blocking(move || {
            store
                .with_sync(|_| {
                    ready.send(()).unwrap();
                    resume.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        });
        entered.await.unwrap();
        // The real dispatcher starts real native work, then blocks at the next
        // ledger read. This deterministically holds all eight control slots full.
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/box-mm.step");
        let id = host
            .start(
                op.name.clone(),
                Control::Geometry(GeometryCommand::ImportPart {
                    session_id: session,
                    base_revision: scene,
                    source: NativePath::from_os_str(source.as_os_str()).unwrap(),
                    initial_pose: RigidPoseMm::IDENTITY,
                }),
                project,
            )
            .await
            .unwrap();
        let queued = (0..8)
            .map(|_| host.enqueue_capture_for_test().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            host.enqueue_capture_for_test().unwrap_err().code(),
            Code::ResourceExhausted
        );
        let active = host.clone();
        let name = op.name.clone();
        let execution = tokio::spawn(async move {
            let _permit = held_permit;
            loop {
                if active.refresh(name.clone(), id, false).await.unwrap() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        tokio::task::yield_now().await;
        assert!(!execution.is_finished());
        assert_eq!(
            jobs.worker.available_permits(),
            0,
            "backpressure must retain execution ownership"
        );
        release.send(()).unwrap();
        blocker.await.unwrap();
        for capture in queued {
            capture.await.unwrap().unwrap();
        }
        tokio::time::timeout(Duration::from_secs(60), execution)
            .await
            .unwrap()
            .unwrap();
        let completed = jobs.get(op.name).await.unwrap();
        let view = spiling_contracts::native_operation_view(&completed).unwrap();
        assert_eq!(
            view.status,
            spiling_contracts::geometry::JobStatus::Completed
        );
        assert!(matches!(
            view.result,
            Some(spiling_contracts::geometry::JobResult::Scene { .. })
        ));
        assert!(!view.outputs.is_empty());
        for output in native_metadata(&completed).unwrap().outputs {
            let bytes = jobs
                .fragment(output.name, 0, output.size_bytes as usize)
                .await
                .unwrap();
            assert_eq!(format!("{:x}", Sha256::digest(&bytes)), output.sha256);
        }
        assert_eq!(jobs.worker.available_permits(), 1);
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn failed_committed_save_retains_its_own_uncertain_receipt_after_reconnect() {
        use spiling_contracts::{
            geometry::{JobError as PublicError, JobResult as PublicResult, SceneRevision},
            project::{ProjectError, ProjectErrorCode, ProjectRevision},
        };
        let directory = tempfile::tempdir().unwrap();
        let jobs = Jobs::open(directory.path()).unwrap();
        let op = seed_native(
            &jobs,
            &SessionId::new(),
            SceneRevision::ZERO,
            ProjectRevision::ZERO,
        );
        let mut receipt = spiling_core::Project::new().info();
        receipt.save_uncertain = true;
        receipt.dirty = true;
        receipt.path_label = Some("committed-save-as-target".into());
        let status = crate::native::terminal_error(
            crate::geometry::JobError::Project {
                error: ProjectError::new(
                    ProjectErrorCode::Io,
                    "manifest replaced; durability uncertain",
                ),
            },
            Some(crate::geometry::JobResult::ProjectSaved {
                info: receipt.clone(),
            }),
        )
        .unwrap();
        jobs.native_finish(&op.name, Err(status), vec![]).unwrap();
        drop(jobs);
        let reopened = Jobs::open(directory.path()).unwrap();
        let view = spiling_contracts::native_operation_view(&reopened.get(op.name).await.unwrap())
            .unwrap();
        assert_eq!(view.status, spiling_contracts::geometry::JobStatus::Failed);
        assert!(
            matches!(view.error,Some(PublicError::Project {error}) if error.code==ProjectErrorCode::Io)
        );
        assert!(matches!(view.result,Some(PublicResult::ProjectSaved {info}) if info==receipt));
        // A later session's Get cannot substitute for this captured save receipt.
        assert_ne!(receipt, spiling_core::Project::new().info());
    }
}
