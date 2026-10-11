// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One native session dispatcher; durable Jobs owns admission and execution scheduling.
use crate::{
    geometry::{self, Control, Reply, Runtime},
    jobs::Jobs,
};
use spiling_contracts::{
    ArtifactView,
    geometry::{ArtifactChunkMetadata, ArtifactId, GeometryResponse, SceneRevision, SessionId},
    manufacturing::ManufacturingResponse,
    project::ProjectRevision,
    rpc::Artifact,
};
use std::{collections::BTreeMap, sync::mpsc, time::Duration};
use tokio::sync::oneshot;
use tonic::Status;

type Capture = (SessionId, SceneRevision, ProjectRevision);
enum Command {
    Capture(oneshot::Sender<Result<Capture, Status>>),
    Execute(Control, oneshot::Sender<Result<Reply, Status>>),
    Start(
        String,
        Control,
        ProjectRevision,
        oneshot::Sender<Result<geometry::JobId, Status>>,
    ),
    Refresh(
        String,
        geometry::JobId,
        bool,
        oneshot::Sender<Result<bool, Status>>,
    ),
    Shutdown(oneshot::Sender<Result<(), Status>>),
}
#[derive(Clone)]
pub struct NativeHost {
    sender: mpsc::SyncSender<Command>,
}
impl NativeHost {
    pub fn new(jobs: Jobs, fault: Option<crate::ProjectFault>) -> Self {
        let (sender, receiver) = mpsc::sync_channel(8);
        std::thread::Builder::new().name("native-session".into()).spawn(move || {
            let mut runtime = Runtime::new(fault);
            let mut resources = BTreeMap::<String,Artifact>::new();
            let mut pending: Option<(String, geometry::JobId)> = None;
            loop {
                if let Some((name,id)) = &pending {
                    let cancelling = jobs.with_sync(|s| {
                        let op = s.get(name)?;
                        Ok(spiling_contracts::native_metadata(&op).map_err(crate::jobs::internal)?.state
                            == spiling_contracts::rpc::NativeOperationState::NativeCancelling as i32)
                    }).unwrap_or_else(fatal);
                    if cancelling { runtime.cancel(*id); }
                }
                if let Err(error) = runtime.poll() {
                    crate::diagnostic("error","native_session_interrupted",&error);
                    std::process::exit(70);
                }
                if let Some((name,id)) = &pending
                    && publish_terminal(&runtime,&jobs,name,*id,&mut resources).unwrap_or_else(fatal)
                {
                    pending = None;
                }
                let command = match receiver.recv_timeout(Duration::from_millis(2)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => { let _ = runtime.shutdown(); break; }
                };
                match command {
                    Command::Capture(tx) => { let (scene,project)=runtime.revisions(); let _=tx.send(Ok((runtime.session_id.clone(),scene,project))); }
                    Command::Execute(command,tx) => {
                        let mut reply=runtime.execute(command);
                        let result=decorate(&mut reply,&resources).map(|_|reply);
                        let _=tx.send(result);
                    }
                    Command::Start(name,command,captured_project,tx) => {
                        let result=match validate_capture(&runtime,&command,captured_project).map(|_|runtime.execute(command)) {
                            Ok(Reply::Accepted(id)) => Ok(id),
                            Ok(reply) => Err(reply_error(reply)),
                            Err(error) => Err(error),
                        };
                        if let Ok(id) = &result { pending = Some((name.clone(),*id)); }
                        if let Err(error)=&result {
                            let status = tonic_to_google(error);
                            let _=jobs.native_finish(&name,Err(status),vec![]);
                        }
                        let _=tx.send(result);
                    }
                    Command::Refresh(name,id,cancel,tx) => {
                        if cancel { runtime.cancel(id); }
                        let result = jobs.with_sync(|s| Ok(s.get(&name)?.done)).and_then(|done| {
                            if done { Ok(true) } else { publish_terminal(&runtime,&jobs,&name,id,&mut resources) }
                        });
                        let _=tx.send(result);
                    }
                    Command::Shutdown(tx) => {
                        (|| {
                            if let Some((name,id)) = pending.take() {
                                runtime.cancel(id);
                                loop {
                                    runtime.poll().map_err(Status::internal)?;
                                    if publish_terminal(&runtime,&jobs,&name,id,&mut resources)? { break; }
                                    std::thread::sleep(Duration::from_millis(2));
                                }
                            }
                            runtime.shutdown().map_err(Status::internal)
                        })().unwrap_or_else(fatal);
                        let _=tx.send(Ok(()));
                        break;
                    }
                }
            }
        }).expect("native dispatcher thread");
        Self { sender }
    }
    fn send(&self, command: Command) -> Result<(), Status> {
        self.sender.try_send(command).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => Status::resource_exhausted("native control queue full"),
            mpsc::TrySendError::Disconnected(_) => Status::unavailable("native session stopped"),
        })
    }
    pub async fn capture(&self) -> Result<Capture, Status> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Capture(tx))?;
        rx.await.map_err(crate::jobs::internal)?
    }
    pub async fn execute(&self, command: Control) -> Result<Reply, Status> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Execute(command, tx))?;
        rx.await.map_err(crate::jobs::internal)?
    }
    pub async fn start(
        &self,
        name: String,
        command: Control,
        captured_project: ProjectRevision,
    ) -> Result<geometry::JobId, Status> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Start(name, command, captured_project, tx))?;
        rx.await.map_err(crate::jobs::internal)?
    }
    pub async fn refresh(
        &self,
        name: String,
        id: geometry::JobId,
        cancel: bool,
    ) -> Result<bool, Status> {
        let (tx, rx) = oneshot::channel();
        self.send_retained(Command::Refresh(name, id, cancel, tx))
            .await?;
        rx.await.map_err(crate::jobs::internal)?
    }
    pub async fn shutdown(&self) -> Result<(), Status> {
        let (tx, rx) = oneshot::channel();
        self.send_retained(Command::Shutdown(tx)).await?;
        rx.await.map_err(crate::jobs::internal)?
    }
    async fn send_retained(&self, mut command: Command) -> Result<(), Status> {
        loop {
            match self.sender.try_send(command) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(retained)) => {
                    command = retained;
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(Status::unavailable("native session stopped"));
                }
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn enqueue_capture_for_test(
        &self,
    ) -> Result<oneshot::Receiver<Result<Capture, Status>>, Status> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Capture(tx))?;
        Ok(rx)
    }
}

fn validate_capture(
    runtime: &Runtime,
    command: &Control,
    captured_project: ProjectRevision,
) -> Result<(), Status> {
    if matches!(
        command,
        Control::Project(spiling_contracts::project::ProjectCommand::Save { .. })
    ) && runtime.revisions().1 != captured_project
    {
        return Err(domain_tonic(
            spiling_contracts::project::ProjectError::new(
                spiling_contracts::project::ProjectErrorCode::StaleRevision,
                "save project revision changed after durable admission",
            )
            .into(),
        ));
    }
    Ok(())
}

pub(crate) fn terminal_error(
    error: geometry::JobError,
    result: Option<geometry::JobResult>,
) -> Result<spiling_contracts::google::rpc::Status, Status> {
    use prost::Message;
    let committed_save = matches!(&error, geometry::JobError::Project { error } if error.code == spiling_contracts::project::ProjectErrorCode::Io);
    let mut status = spiling_contracts::domain_status(&public_error(error));
    if let Some(result) = result {
        let geometry::JobResult::ProjectSaved { info } = result else {
            return Err(Status::internal(
                "failed native result is not a committed save receipt",
            ));
        };
        if !committed_save || !info.save_uncertain {
            return Err(Status::internal(
                "failed save receipt lacks committed uncertainty",
            ));
        }
        let result = spiling_contracts::rpc::NativeOperationResult::try_from(
            spiling_contracts::geometry::JobResult::ProjectSaved { info },
        )
        .map_err(crate::jobs::internal)?;
        status.details.push(prost_types::Any {
            type_url: spiling_contracts::NATIVE_RESULT_TYPE.into(),
            value: result.encode_to_vec(),
        });
    }
    Ok(status)
}

pub(crate) fn fatal<T>(error: Status) -> T {
    crate::diagnostic("error", "native_publication_failure", error.message());
    std::process::exit(70)
}
fn publish_terminal(
    runtime: &Runtime,
    jobs: &Jobs,
    name: &str,
    id: geometry::JobId,
    resources: &mut BTreeMap<String, Artifact>,
) -> Result<bool, Status> {
    let job = runtime
        .job(id)
        .ok_or_else(|| Status::internal("native correlation missing"))?;
    if !job.status.is_terminal() {
        jobs.native_progress(name, &job.stage, job.status)?;
        return Ok(false);
    }
    if let Some(error) = job.error {
        let status = terminal_error(error, job.result)?;
        jobs.native_finish(name, Err(status), vec![])?;
        return Ok(true);
    }
    let private = job
        .result
        .ok_or_else(|| Status::internal("native terminal result missing"))?;
    // No short edit can interleave between ACK/Core commit and immutable output capture.
    let mut outputs = Vec::new();
    for artifact in runtime.scene_artifacts() {
        publish_geometry(runtime, jobs, artifact, resources, &mut outputs)?;
    }
    if let geometry::JobResult::Section { artifact_id } = &private {
        publish_geometry(runtime, jobs, *artifact_id, resources, &mut outputs)?;
    }
    let asset = runtime.current_manufacturing_asset();
    if let Some(asset) = &asset {
        let descriptor = jobs.describe_native_artifact(
            asset.bytes.as_slice(),
            "application/x-spiling-manufacturing-bundle",
            16 * 1024 * 1024,
        )?;
        if descriptor.sha256 != asset.record.hash.as_str()
            || descriptor.size_bytes != u64::from(asset.record.byte_count)
        {
            return Err(Status::data_loss(
                "manufacturing immutable descriptor mismatch",
            ));
        }
        resources.insert(descriptor.sha256.clone(), descriptor.clone());
        outputs.push((descriptor, asset.bytes.as_slice()));
    }
    let result = public_result(private, resources)?;
    jobs.native_finish(name, Ok(result), outputs)?;
    Ok(true)
}

fn view(artifact: &Artifact) -> Result<ArtifactView, Status> {
    spiling_contracts::native::artifact_to_view(artifact.clone(), 64 * 1024 * 1024)
        .map_err(crate::jobs::internal)
}
fn resource(hash: &str, resources: &BTreeMap<String, Artifact>) -> Result<ArtifactView, Status> {
    view(
        resources
            .get(hash)
            .ok_or_else(|| Status::internal("native artifact not durably published"))?,
    )
}
fn publish_geometry<'a>(
    runtime: &'a Runtime,
    jobs: &Jobs,
    id: ArtifactId,
    resources: &mut BTreeMap<String, Artifact>,
    outputs: &mut Vec<(Artifact, &'a [u8])>,
) -> Result<(), Status> {
    for (metadata, bytes) in runtime
        .artifact_chunks(id)
        .map_err(|e| domain_tonic(e.into()))?
    {
        let (hash, size, media) = match metadata {
            ArtifactChunkMetadata::Mesh { metadata } => (
                metadata.sha256.as_str(),
                metadata.byte_count,
                "application/x-spiling-mesh",
            ),
            ArtifactChunkMetadata::Section { metadata } => (
                metadata.sha256.as_str(),
                metadata.byte_count,
                "application/x-spiling-section",
            ),
        };
        let descriptor = jobs.describe_native_artifact(bytes, media, 1024 * 1024)?;
        if descriptor.sha256 != hash || descriptor.size_bytes != u64::from(size) {
            return Err(Status::data_loss(
                "native immutable chunk descriptor mismatch",
            ));
        }
        resources.insert(descriptor.sha256.clone(), descriptor.clone());
        if !outputs.iter().any(|(a, _)| a.name == descriptor.name) {
            outputs.push((descriptor, bytes));
        }
    }
    Ok(())
}
fn decorate(reply: &mut Reply, resources: &BTreeMap<String, Artifact>) -> Result<(), Status> {
    match reply {
        Reply::Geometry(GeometryResponse::ArtifactPage {
            chunks,
            resources: out,
            ..
        }) => {
            *out = chunks
                .iter()
                .map(|chunk| match chunk {
                    ArtifactChunkMetadata::Mesh { metadata } => {
                        resource(metadata.sha256.as_str(), resources)
                    }
                    ArtifactChunkMetadata::Section { metadata } => {
                        resource(metadata.sha256.as_str(), resources)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
        }
        Reply::Manufacturing(ManufacturingResponse::Status {
            artifact,
            resource: out,
            ..
        }) => {
            *out = artifact
                .as_ref()
                .map(|a| resource(a.hash.as_str(), resources))
                .transpose()?;
        }
        _ => {}
    }
    Ok(())
}
pub(crate) fn public_error(error: geometry::JobError) -> spiling_contracts::geometry::JobError {
    match error {
        geometry::JobError::Geometry { error } => error.into(),
        geometry::JobError::Project { error } => error.into(),
        geometry::JobError::Manufacturing { error } => error.into(),
    }
}
pub(crate) fn domain_tonic(error: spiling_contracts::geometry::JobError) -> Status {
    use prost::Message;
    let status = spiling_contracts::domain_status(&error);
    Status::with_details(
        tonic::Code::from_i32(status.code),
        status.message.clone(),
        status.encode_to_vec().into(),
    )
}
fn tonic_to_google(error: &Status) -> spiling_contracts::google::rpc::Status {
    use prost::Message;
    spiling_contracts::google::rpc::Status::decode(error.details()).unwrap_or_else(|_| {
        spiling_contracts::google::rpc::Status {
            code: error.code() as i32,
            message: error.message().into(),
            details: vec![],
        }
    })
}
pub(crate) fn reply_error(reply: Reply) -> Status {
    match reply {
        Reply::Geometry(GeometryResponse::Error { error }) => domain_tonic(error.into()),
        Reply::Geometry(GeometryResponse::ProjectError { error }) => domain_tonic(error.into()),
        Reply::Project(spiling_contracts::project::ProjectResponse::Error { error }) => {
            domain_tonic(error.into())
        }
        Reply::Manufacturing(ManufacturingResponse::Error { error }) => domain_tonic(error.into()),
        _ => Status::internal("native service returned unexpected response"),
    }
}
fn public_result(
    result: geometry::JobResult,
    resources: &BTreeMap<String, Artifact>,
) -> Result<spiling_contracts::geometry::JobResult, Status> {
    use spiling_contracts::geometry::JobResult as Public;
    Ok(match result {
        geometry::JobResult::Scene { summary } => Public::Scene { summary },
        geometry::JobResult::Section { artifact_id } => Public::Section { artifact_id },
        geometry::JobResult::ProjectSaved { info } => Public::ProjectSaved { info },
        geometry::JobResult::ManufacturingCompiled { info, record } => {
            let descriptor = resource(record.hash.as_str(), resources)?;
            Public::ManufacturingCompiled {
                info,
                record,
                resource: descriptor,
            }
        }
        geometry::JobResult::ManufacturingVerified { record, report } => {
            let descriptor = resource(record.hash.as_str(), resources)?;
            Public::ManufacturingVerified {
                record,
                report,
                resource: descriptor,
            }
        }
    })
}
