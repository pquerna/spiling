// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod geometry;
mod jobs;
mod native;
mod session;
use geometry::{Control, Reply};
use jobs::Jobs;
use native::NativeHost;
use prost::Message;
use sha2::{Digest, Sha256};
use spiling_contracts::{
    MAX_ARTIFACT_BYTES, MAX_CONTROL_BYTES, StartupInfo, TRANSFER_FRAGMENT_BYTES,
    google::{bytestream::*, longrunning::*},
    rpc::*,
};
use std::io::{BufRead, Read};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{Request, Response, Status};
use uuid::Uuid;

#[derive(Clone)]
struct Service {
    jobs: Jobs,
    native: NativeHost,
    info: EngineInfo,
    stop: watch::Sender<bool>,
    observers: Arc<Semaphore>,
    downloads: Arc<Semaphore>,
}
#[tonic::async_trait]
impl engine_server::Engine for Service {
    async fn get_engine_info(&self, _: Request<()>) -> Result<Response<EngineInfo>, Status> {
        Ok(Response::new(self.info.clone()))
    }
    async fn shutdown(&self, _: Request<()>) -> Result<Response<()>, Status> {
        self.stop.send_replace(true);
        Ok(Response::new(()))
    }
    async fn run_diagnostic(
        &self,
        req: Request<RunDiagnosticRequest>,
    ) -> Result<Response<Operation>, Status> {
        Ok(Response::new(self.jobs.start(req.into_inner()).await?))
    }
    type WatchOperationStream = ReceiverStream<Result<Operation, Status>>;
    async fn watch_operation(
        &self,
        req: Request<WatchOperationRequest>,
    ) -> Result<Response<Self::WatchOperationStream>, Status> {
        let permit = self
            .observers
            .clone()
            .try_acquire_owned()
            .map_err(|_| Status::resource_exhausted("observer limit"))?;
        let mut watch = self.jobs.subscribe(req.into_inner().name).await?;
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let _permit = permit;
            loop {
                let op = watch.borrow_and_update().clone();
                let done = op.done;
                if tx.send(Ok(op)).await.is_err() || done {
                    break;
                }
                tokio::select! { _ = tx.closed() => break, changed = watch.changed() => if changed.is_err() { break; } }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
#[tonic::async_trait]
impl operations_server::Operations for Service {
    async fn get_operation(
        &self,
        req: Request<GetOperationRequest>,
    ) -> Result<Response<Operation>, Status> {
        Ok(Response::new(self.jobs.get(req.into_inner().name).await?))
    }
    async fn list_operations(
        &self,
        req: Request<ListOperationsRequest>,
    ) -> Result<Response<ListOperationsResponse>, Status> {
        let req = req.into_inner();
        if !req.filter.is_empty() || req.return_partial_success {
            return Err(Status::unimplemented(
                "filters and wildcard parents are not supported",
            ));
        }
        Ok(Response::new(
            self.jobs
                .list(req.name, req.page_size, req.page_token)
                .await?,
        ))
    }
    async fn delete_operation(
        &self,
        _: Request<DeleteOperationRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented(
            "retained operations cannot yet be deleted",
        ))
    }
    async fn cancel_operation(
        &self,
        req: Request<CancelOperationRequest>,
    ) -> Result<Response<()>, Status> {
        self.jobs.cancel(req.into_inner().name).await?;
        Ok(Response::new(()))
    }
    async fn wait_operation(
        &self,
        req: Request<WaitOperationRequest>,
    ) -> Result<Response<Operation>, Status> {
        let _permit = self
            .observers
            .clone()
            .try_acquire_owned()
            .map_err(|_| Status::resource_exhausted("observer limit"))?;
        let req = req.into_inner();
        let duration = match req.timeout {
            Some(d) if d.seconds >= 0 && (0..1_000_000_000).contains(&d.nanos) => {
                Duration::new(d.seconds as u64, d.nanos as u32).min(Duration::from_secs(30))
            }
            Some(_) => return Err(Status::invalid_argument("invalid wait timeout")),
            None => Duration::from_secs(30),
        };
        let mut watch = self.jobs.subscribe(req.name).await?;
        let _ = tokio::time::timeout(duration, async {
            loop {
                if watch.borrow().done {
                    break;
                }
                if watch.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        Ok(Response::new(watch.borrow().clone()))
    }
}
#[tonic::async_trait]
impl artifacts_server::Artifacts for Service {
    async fn get_artifact(
        &self,
        req: Request<GetArtifactRequest>,
    ) -> Result<Response<Artifact>, Status> {
        Ok(Response::new(
            self.jobs.artifact(req.into_inner().name).await?,
        ))
    }
}
#[tonic::async_trait]
impl byte_stream_server::ByteStream for Service {
    type ReadStream = ReceiverStream<Result<ReadResponse, Status>>;
    async fn write(
        &self,
        _: Request<tonic::Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        Err(Status::unimplemented(
            "artifacts are published by engine operations",
        ))
    }
    async fn query_write_status(
        &self,
        _: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        Err(Status::unimplemented(
            "external artifact writes are not supported",
        ))
    }
    async fn read(&self, req: Request<ReadRequest>) -> Result<Response<Self::ReadStream>, Status> {
        let permit = self
            .downloads
            .clone()
            .try_acquire_owned()
            .map_err(|_| Status::resource_exhausted("download limit"))?;
        let req = req.into_inner();
        if req.read_offset < 0 {
            return Err(Status::out_of_range("negative read offset"));
        }
        if req.read_limit < 0 {
            return Err(Status::invalid_argument("negative read limit"));
        }
        let info = self.jobs.artifact(req.resource_name.clone()).await?;
        if req.read_offset as u64 > info.size_bytes {
            return Err(Status::out_of_range("read_offset exceeds artifact size"));
        }
        let end = if req.read_limit == 0 {
            info.size_bytes
        } else {
            (req.read_offset as u64)
                .saturating_add(req.read_limit as u64)
                .min(info.size_bytes)
        };
        let jobs = self.jobs.clone();
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            let _permit = permit;
            let mut offset = req.read_offset as u64;
            while offset < end {
                if tx.is_closed() {
                    break;
                }
                let count = ((end - offset) as usize).min(TRANSFER_FRAGMENT_BYTES);
                let bytes = jobs
                    .fragment(req.resource_name.clone(), offset, count)
                    .await;
                match bytes {
                    Ok(data) => {
                        let length = data.len() as u64;
                        if length == 0 {
                            let _ = tx.send(Err(Status::data_loss("artifact truncated"))).await;
                            break;
                        }
                        if tx.send(Ok(ReadResponse { data })).await.is_err() {
                            break;
                        }
                        offset += length;
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error)).await;
                        break;
                    }
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

fn native_digest<T: Message>(method: &str, request: &T) -> String {
    let mut hash = Sha256::new();
    hash.update(method.as_bytes());
    hash.update([0]);
    hash.update(request.encode_to_vec());
    format!("{:x}", hash.finalize())
}
fn request_error(status: spiling_contracts::google::rpc::Status) -> Status {
    Status::with_details(
        tonic::Code::from_i32(status.code),
        status.message.clone(),
        status.encode_to_vec().into(),
    )
}

#[tonic::async_trait]
impl geometry_server::Geometry for Service {
    async fn execute(
        &self,
        req: Request<GeometryRequest>,
    ) -> Result<Response<GeometryReply>, Status> {
        let command = req.into_inner().try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::geometry_request_status(&e))
        })?;
        match self.native.execute(Control::Geometry(command)).await? {
            Reply::Geometry(
                response @ (spiling_contracts::geometry::GeometryResponse::Error { .. }
                | spiling_contracts::geometry::GeometryResponse::ProjectError { .. }),
            ) => Err(native::reply_error(Reply::Geometry(response))),
            Reply::Geometry(response) => GeometryReply::try_from(response)
                .map(Response::new)
                .map_err(jobs::internal),
            reply => Err(native::reply_error(reply)),
        }
    }
    async fn import_part(
        &self,
        req: Request<ImportPartRequest>,
    ) -> Result<Response<Operation>, Status> {
        let mut req = req.into_inner();
        let id = std::mem::take(&mut req.request_id);
        let parent = req.parent.clone();
        let digest = native_digest("Geometry.ImportPart", &req);
        let command = req.try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::geometry_request_status(&e))
        })?;
        let input = jobs::NativeAdmission {
            parent,
            request_id: id,
            digest,
            command: Control::Geometry(command),
            reservation: 64 * 1024 * 1024,
        };
        Ok(Response::new(
            self.jobs.start_native(self.native.clone(), input).await?,
        ))
    }
    async fn start_section(
        &self,
        req: Request<StartSectionRequest>,
    ) -> Result<Response<Operation>, Status> {
        let mut req = req.into_inner();
        let id = std::mem::take(&mut req.request_id);
        let parent = req.parent.clone();
        let digest = native_digest("Geometry.StartSection", &req);
        let command = req.try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::geometry_request_status(&e))
        })?;
        let input = jobs::NativeAdmission {
            parent,
            request_id: id,
            digest,
            command: Control::Geometry(command),
            reservation: 16 * 1024 * 1024,
        };
        Ok(Response::new(
            self.jobs.start_native(self.native.clone(), input).await?,
        ))
    }
}
#[tonic::async_trait]
impl projects_server::Projects for Service {
    async fn execute(
        &self,
        req: Request<ProjectRequest>,
    ) -> Result<Response<ProjectReply>, Status> {
        let command = req.into_inner().try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::project_request_status(&e))
        })?;
        match self.native.execute(Control::Project(command)).await? {
            Reply::Project(
                response @ spiling_contracts::project::ProjectResponse::Error { .. },
            ) => Err(native::reply_error(Reply::Project(response))),
            Reply::Project(response) => ProjectReply::try_from(response)
                .map(Response::new)
                .map_err(jobs::internal),
            reply => Err(native::reply_error(reply)),
        }
    }
    async fn open(&self, req: Request<OpenProjectRequest>) -> Result<Response<Operation>, Status> {
        let mut req = req.into_inner();
        let id = std::mem::take(&mut req.request_id);
        let parent = req.parent.clone();
        let digest = native_digest("Projects.Open", &req);
        let command = req.try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::project_request_status(&e))
        })?;
        let input = jobs::NativeAdmission {
            parent,
            request_id: id,
            digest,
            command: Control::Project(command),
            reservation: 80 * 1024 * 1024,
        };
        Ok(Response::new(
            self.jobs.start_native(self.native.clone(), input).await?,
        ))
    }
    async fn save(&self, req: Request<SaveProjectRequest>) -> Result<Response<Operation>, Status> {
        let mut req = req.into_inner();
        let id = std::mem::take(&mut req.request_id);
        let parent = req.parent.clone();
        let digest = native_digest("Projects.Save", &req);
        let command = req.try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::project_request_status(&e))
        })?;
        let input = jobs::NativeAdmission {
            parent,
            request_id: id,
            digest,
            command: Control::Project(command),
            reservation: 0,
        };
        Ok(Response::new(
            self.jobs.start_native(self.native.clone(), input).await?,
        ))
    }
}
#[tonic::async_trait]
impl manufacturing_server::Manufacturing for Service {
    async fn execute(
        &self,
        req: Request<ManufacturingRequest>,
    ) -> Result<Response<ManufacturingReply>, Status> {
        let command = req.into_inner().try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::manufacturing_request_status(&e))
        })?;
        match self.native.execute(Control::Manufacturing(command)).await? {
            Reply::Manufacturing(
                response @ spiling_contracts::manufacturing::ManufacturingResponse::Error { .. },
            ) => Err(native::reply_error(Reply::Manufacturing(response))),
            Reply::Manufacturing(response) => ManufacturingReply::try_from(response)
                .map(Response::new)
                .map_err(jobs::internal),
            reply => Err(native::reply_error(reply)),
        }
    }
    async fn compile(
        &self,
        req: Request<CompileManufacturingRequest>,
    ) -> Result<Response<Operation>, Status> {
        let mut req = req.into_inner();
        let id = std::mem::take(&mut req.request_id);
        let parent = req.parent.clone();
        let digest = native_digest("Manufacturing.Compile", &req);
        let command = req.try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::manufacturing_request_status(&e))
        })?;
        let input = jobs::NativeAdmission {
            parent,
            request_id: id,
            digest,
            command: Control::Manufacturing(command),
            reservation: 16 * 1024 * 1024,
        };
        Ok(Response::new(
            self.jobs.start_native(self.native.clone(), input).await?,
        ))
    }
    async fn verify(
        &self,
        req: Request<VerifyManufacturingRequest>,
    ) -> Result<Response<Operation>, Status> {
        let mut req = req.into_inner();
        let id = std::mem::take(&mut req.request_id);
        let parent = req.parent.clone();
        let digest = native_digest("Manufacturing.Verify", &req);
        let command = req.try_into().map_err(|e: String| {
            request_error(spiling_contracts::native::manufacturing_request_status(&e))
        })?;
        let input = jobs::NativeAdmission {
            parent,
            request_id: id,
            digest,
            command: Control::Manufacturing(command),
            reservation: 16 * 1024 * 1024,
        };
        Ok(Response::new(
            self.jobs.start_native(self.native.clone(), input).await?,
        ))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ProjectFault {
    AfterAssets,
    BeforeManifestReplace,
    AfterManifestReplace,
}
impl ProjectFault {
    fn from_env() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        match std::env::var("SPILING_PROJECT_FAULT") {
            Err(std::env::VarError::NotPresent) => Ok(None),
            Ok(value) => match value.as_str() {
                "after_assets" => Ok(Some(Self::AfterAssets)),
                "before_manifest_replace" => Ok(Some(Self::BeforeManifestReplace)),
                "after_manifest_replace" => Ok(Some(Self::AfterManifestReplace)),
                _ => Err("unknown SPILING_PROJECT_FAULT stage".into()),
            },
            Err(error) => Err(error.into()),
        }
    }
    pub(crate) fn matches(self, stage: spiling_core::storage::SaveStage) -> bool {
        matches!(
            (self, stage),
            (
                Self::AfterAssets,
                spiling_core::storage::SaveStage::AssetsSynced
            ) | (
                Self::BeforeManifestReplace,
                spiling_core::storage::SaveStage::BeforeManifestReplace
            ) | (
                Self::AfterManifestReplace,
                spiling_core::storage::SaveStage::AfterManifestReplace
            )
        )
    }
}
pub(crate) fn diagnostic(level: &str, event: &str, message: &str) {
    eprintln!(
        "{}",
        serde_json::json!({"level":level,"event":event,"message":message})
    );
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--store")) {
        return Err("engine requires --store PATH".into());
    }
    let store = PathBuf::from(args.next().ok_or("store path missing")?);
    if args.next().is_some() {
        return Err("unexpected engine argument".into());
    }
    let (stop, mut stopping) = watch::channel(false);
    let (startup_tx, startup_rx) = tokio::sync::oneshot::channel();
    let eof = stop.clone();
    // An ordinary detached reader thread does not prevent async-runtime shutdown.
    // Tokio stdin uses a blocking pool read that cannot be cancelled while owner stdin is open.
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut input = std::io::BufReader::new(stdin.lock());
        let mut capability = Vec::new();
        let result = (&mut input).take(129).read_until(b'\n', &mut capability);
        let _ = startup_tx.send(result.map(|_| capability));
        let mut byte = [0];
        let _ = input.read(&mut byte);
        eof.send_replace(true);
    });
    let capability = tokio::time::timeout(Duration::from_secs(5), startup_rx).await???;
    if capability.len() != 65
        || capability.last() != Some(&b'\n')
        || !capability[..64].iter().all(u8::is_ascii_hexdigit)
    {
        return Err("invalid startup capability".into());
    }
    let capability = String::from_utf8(capability[..64].to_vec())?;
    let jobs = Jobs::open(&store)?;
    let native = NativeHost::new(jobs.clone(), ProjectFault::from_env()?);
    let (session_id, _, _) = native.capture().await?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let instance_id = Uuid::new_v4().to_string();
    let info = EngineInfo {
        instance_id: instance_id.clone(),
        engine_build: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        kernel: "monstertruck".into(),
        geometry_capabilities: [
            "step_planar_cylindrical_v1",
            "multi_instance_scene_v1",
            "mesh_chunks_v1",
            "native_plane_section_v1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        session_id: session_id.as_str().to_owned(),
        kernel_identity: Some(
            spiling_contracts::geometry::KernelIdentity {
                name: "monstertruck".into(),
                version: "0.4.1".into(),
                revision: "d87b4d9ced1f3baf31aa771ac0e7c663efb1c001".into(),
            }
            .try_into()
            .map_err(jobs::internal)?,
        ),
        geometry_limits: Some(
            spiling_contracts::geometry::GeometryLimits::FROZEN
                .try_into()
                .map_err(jobs::internal)?,
        ),
        project_capabilities: [
            "recoverable_projects_v2",
            "project_transactions_v1",
            "project_read_only_v1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        manufacturing_capabilities: [
            "planar_software_compile_v1",
            "independent_program_replay_v1",
            "immutable_bundle_bytestream_v1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        max_message_bytes: MAX_CONTROL_BYTES,
        max_artifact_bytes: MAX_ARTIFACT_BYTES,
    };
    let startup = StartupInfo {
        endpoint: format!("http://{}", listener.local_addr()?),
        instance_id,
        pid: info.pid,
    };
    println!("{}", serde_json::to_string(&startup)?);
    let service = Service {
        jobs,
        native,
        info,
        stop,
        observers: Arc::new(Semaphore::new(16)),
        downloads: Arc::new(Semaphore::new(8)),
    };
    let authenticate = move |request: Request<()>| -> Result<Request<()>, Status> {
        if request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            != Some(capability.as_str())
        {
            return Err(Status::unauthenticated("invalid launch capability"));
        }
        Ok(request)
    };
    let shutdown_jobs = service.jobs.clone();
    let mut drain_signal = stopping.clone();
    let shutdown_native = service.native.clone();
    let server = tonic::transport::Server::builder()
        .max_concurrent_streams(32)
        .add_service(tonic::service::interceptor::InterceptedService::new(
            engine_server::EngineServer::new(service.clone())
                .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
                .max_encoding_message_size(MAX_CONTROL_BYTES as usize),
            authenticate.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            operations_server::OperationsServer::new(service.clone())
                .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
                .max_encoding_message_size(MAX_CONTROL_BYTES as usize),
            authenticate.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            byte_stream_server::ByteStreamServer::new(service.clone())
                .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
                .max_encoding_message_size(MAX_CONTROL_BYTES as usize),
            authenticate.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            geometry_server::GeometryServer::new(service.clone())
                .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
                .max_encoding_message_size(MAX_CONTROL_BYTES as usize),
            authenticate.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            projects_server::ProjectsServer::new(service.clone())
                .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
                .max_encoding_message_size(MAX_CONTROL_BYTES as usize),
            authenticate.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            manufacturing_server::ManufacturingServer::new(service.clone())
                .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
                .max_encoding_message_size(MAX_CONTROL_BYTES as usize),
            authenticate.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            artifacts_server::ArtifactsServer::new(service)
                .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
                .max_encoding_message_size(MAX_CONTROL_BYTES as usize),
            authenticate,
        ))
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
            if !*stopping.borrow() {
                let _ = stopping.changed().await;
            }
        });
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result?,
        _ = drain_signal.changed() => {
            // Bound drain time even when a consumer holds an unread stream.
            if let Ok(result) = tokio::time::timeout(Duration::from_secs(2), &mut server).await { result?; }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(12), shutdown_native.shutdown()).await;
    shutdown_jobs.interrupt_all().await?;
    Ok(())
}

#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"level":"error", "event":"engine_exit", "message":error.to_string()})
            );
            std::process::ExitCode::FAILURE
        }
    }
}
