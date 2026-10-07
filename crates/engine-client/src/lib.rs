// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared gRPC client. RPC cancellation does not own accepted operations or the process.
use sha2::{Digest, Sha256};
use spiling_contracts::{
    Hello, MAX_ARTIFACT_BYTES, MAX_CONTROL_BYTES, StartupInfo,
    google::{bytestream::*, longrunning::*},
    metadata,
    rpc::*,
};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    time::timeout,
};
use tonic::{
    Request, Status,
    transport::{Channel, Endpoint},
};
use uuid::Uuid;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("engine I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("engine startup JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("engine transport: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("engine RPC: {0}")]
    Rpc(#[from] Status),
    #[error("engine contract: {0}")]
    Protocol(String),
    #[error("engine exchange timed out")]
    Timeout,
    #[error("engine exited: {0}")]
    UnexpectedExit(String),
}

/// Cloneable RPC access without process ownership. Dropping a call never kills the child.
#[derive(Clone)]
pub struct EngineRpc {
    channel: Channel,
    capability: String,
}
impl EngineRpc {
    fn request<T>(&self, value: T) -> Request<T> {
        let mut request = Request::new(value);
        request.metadata_mut().insert(
            "authorization",
            self.capability
                .parse()
                .expect("validated hexadecimal capability"),
        );
        request.set_timeout(REQUEST_TIMEOUT);
        request
    }
    fn engine(&self) -> engine_client::EngineClient<Channel> {
        engine_client::EngineClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
            .max_encoding_message_size(MAX_CONTROL_BYTES as usize)
    }
    fn operations(&self) -> operations_client::OperationsClient<Channel> {
        operations_client::OperationsClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
            .max_encoding_message_size(MAX_CONTROL_BYTES as usize)
    }
    fn artifacts(&self) -> artifacts_client::ArtifactsClient<Channel> {
        artifacts_client::ArtifactsClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
            .max_encoding_message_size(MAX_CONTROL_BYTES as usize)
    }
    pub async fn info(&self) -> Result<EngineInfo, ClientError> {
        Ok(self
            .engine()
            .get_engine_info(self.request(()))
            .await?
            .into_inner())
    }
    pub async fn run_diagnostic(
        &self,
        request: RunDiagnosticRequest,
    ) -> Result<Operation, ClientError> {
        Ok(self
            .engine()
            .run_diagnostic(self.request(request))
            .await?
            .into_inner())
    }
    pub async fn get_operation(&self, name: impl Into<String>) -> Result<Operation, ClientError> {
        Ok(self
            .operations()
            .get_operation(self.request(GetOperationRequest { name: name.into() }))
            .await?
            .into_inner())
    }
    pub async fn cancel_operation(&self, name: impl Into<String>) -> Result<(), ClientError> {
        self.operations()
            .cancel_operation(self.request(CancelOperationRequest { name: name.into() }))
            .await?;
        Ok(())
    }
    pub async fn list_operations(
        &self,
        request: ListOperationsRequest,
    ) -> Result<ListOperationsResponse, ClientError> {
        Ok(self
            .operations()
            .list_operations(self.request(request))
            .await?
            .into_inner())
    }
    pub async fn wait_operation(
        &self,
        name: impl Into<String>,
        duration: Duration,
    ) -> Result<Operation, ClientError> {
        let mut request = self.request(WaitOperationRequest {
            name: name.into(),
            timeout: Some(prost_types_duration(duration)),
        });
        request.set_timeout(duration.min(Duration::from_secs(30)) + Duration::from_secs(1));
        Ok(self
            .operations()
            .wait_operation(request)
            .await?
            .into_inner())
    }
    pub async fn watch_operation(
        &self,
        name: impl Into<String>,
    ) -> Result<tonic::Streaming<Operation>, ClientError> {
        let mut request = self.request(WatchOperationRequest { name: name.into() });
        // A watch is explicitly long lived, unlike unary acceptance/status calls.
        request.metadata_mut().remove("grpc-timeout");
        Ok(self.engine().watch_operation(request).await?.into_inner())
    }
    pub async fn get_artifact(&self, name: impl Into<String>) -> Result<Artifact, ClientError> {
        Ok(self
            .artifacts()
            .get_artifact(self.request(GetArtifactRequest { name: name.into() }))
            .await?
            .into_inner())
    }
    pub async fn read_artifact_range(
        &self,
        request: ReadRequest,
    ) -> Result<tonic::Streaming<ReadResponse>, ClientError> {
        Ok(
            byte_stream_client::ByteStreamClient::new(self.channel.clone())
                .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
                .read(self.request(request))
                .await?
                .into_inner(),
        )
    }
    pub async fn read_artifact(&self, artifact: &Artifact) -> Result<Vec<u8>, ClientError> {
        if artifact.size_bytes > u64::from(MAX_ARTIFACT_BYTES) {
            return Err(ClientError::Protocol(
                "artifact exceeds client allocation limit".into(),
            ));
        }
        let mut stream = self
            .read_artifact_range(ReadRequest {
                resource_name: artifact.name.clone(),
                read_offset: 0,
                read_limit: 0,
            })
            .await?;
        let mut bytes = Vec::with_capacity(artifact.size_bytes as usize);
        while let Some(fragment) = stream.message().await? {
            if fragment.data.is_empty()
                || bytes.len() + fragment.data.len() > artifact.size_bytes as usize
            {
                return Err(ClientError::Protocol(
                    "artifact fragment range invalid".into(),
                ));
            }
            bytes.extend_from_slice(&fragment.data);
        }
        if bytes.len() as u64 != artifact.size_bytes
            || format!("{:x}", Sha256::digest(&bytes)) != artifact.sha256
        {
            return Err(ClientError::Protocol(
                "artifact size/checksum mismatch".into(),
            ));
        }
        Ok(bytes)
    }
    pub async fn triangle(&self) -> Result<Vec<u8>, ClientError> {
        let op = self
            .run_diagnostic(RunDiagnosticRequest {
                parent: "diagnostics/default".into(),
                request_id: Uuid::new_v4().to_string(),
                chunk_count: 1,
                delay_ms: 0,
                chunk_bytes: 64,
                input_revision: "diagnostic".into(),
            })
            .await?;
        let mut watch = self.watch_operation(op.name).await?;
        while let Some(op) = watch.message().await? {
            if op.done {
                if let Some(operation::Result::Error(error)) = &op.result {
                    return Err(ClientError::Protocol(error.message.clone()));
                }
                let meta = metadata(&op).map_err(ClientError::Protocol)?;
                return self
                    .read_artifact(
                        meta.outputs.first().ok_or_else(|| {
                            ClientError::Protocol("triangle artifact missing".into())
                        })?,
                    )
                    .await;
            }
        }
        Err(ClientError::Protocol(
            "watch ended without terminal operation".into(),
        ))
    }
}
fn prost_types_duration(value: Duration) -> prost_types::Duration {
    prost_types::Duration {
        seconds: value.as_secs().min(30) as i64,
        nanos: value.subsec_nanos() as i32,
    }
}

pub struct EngineClient {
    child: Child,
    _liveness: ChildStdin,
    rpc: EngineRpc,
    hello: Hello,
    _temporary_store: Option<tempfile::TempDir>,
}
impl EngineClient {
    pub async fn spawn(path: impl AsRef<Path>) -> Result<Self, ClientError> {
        let directory = tempfile::tempdir()?;
        let mut client = Self::spawn_in(path, directory.path()).await?;
        client._temporary_store = Some(directory);
        Ok(client)
    }
    pub async fn spawn_in(
        path: impl AsRef<Path>,
        store: impl AsRef<Path>,
    ) -> Result<Self, ClientError> {
        let mut child = Command::new(path.as_ref())
            .arg("--store")
            .arg(store.as_ref())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let result = async {
            let mut input = child
                .stdin
                .take()
                .ok_or_else(|| ClientError::Protocol("stdin missing".into()))?;
            let output = child
                .stdout
                .take()
                .ok_or_else(|| ClientError::Protocol("stdout missing".into()))?;
            let capability = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            input
                .write_all(format!("{capability}\n").as_bytes())
                .await?;
            input.flush().await?;
            let mut line = Vec::new();
            let reader = BufReader::new(output);
            timeout(
                REQUEST_TIMEOUT,
                reader.take(4097).read_until(b'\n', &mut line),
            )
            .await
            .map_err(|_| ClientError::Timeout)??;
            if line.len() > 4096 || line.last() != Some(&b'\n') {
                return Err(ClientError::Protocol(
                    "invalid bounded startup response".into(),
                ));
            }
            let startup: StartupInfo = serde_json::from_slice(&line)?;
            let address = startup
                .endpoint
                .strip_prefix("http://")
                .and_then(|s| s.parse::<std::net::SocketAddrV4>().ok())
                .ok_or_else(|| ClientError::Protocol("invalid local endpoint".into()))?;
            if *address.ip() != std::net::Ipv4Addr::LOCALHOST
                || address.port() == 0
                || Some(startup.pid) != child.id()
            {
                return Err(ClientError::Protocol(
                    "startup child identity invalid".into(),
                ));
            }
            let channel = Endpoint::from_shared(startup.endpoint)?
                .connect_timeout(REQUEST_TIMEOUT)
                .connect()
                .await?;
            let rpc = EngineRpc {
                channel,
                capability,
            };
            let info = rpc.info().await?;
            if info.pid != startup.pid
                || info.instance_id != startup.instance_id
                || info.max_message_bytes != MAX_CONTROL_BYTES
                || info.max_artifact_bytes != MAX_ARTIFACT_BYTES
            {
                return Err(ClientError::Protocol(
                    "engine readiness identity/limits invalid".into(),
                ));
            }
            Ok((input, rpc, Hello::from(info)))
        }
        .await;
        match result {
            Ok((input, rpc, hello)) => Ok(Self {
                child,
                _liveness: input,
                rpc,
                hello,
                _temporary_store: None,
            }),
            Err(error) => {
                let _ = child.start_kill();
                let _ = timeout(REQUEST_TIMEOUT, child.wait()).await;
                Err(error)
            }
        }
    }
    pub fn hello(&self) -> &Hello {
        &self.hello
    }
    pub fn rpc(&self) -> EngineRpc {
        self.rpc.clone()
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
    pub async fn status(&mut self) -> Result<bool, ClientError> {
        Ok(self.child.try_wait()?.is_none())
    }
    pub async fn ping(&self) -> Result<(), ClientError> {
        self.rpc.info().await?;
        Ok(())
    }
    pub async fn triangle(&self) -> Result<Vec<u8>, ClientError> {
        self.rpc.triangle().await
    }
    pub async fn shutdown(&mut self) -> Result<(), ClientError> {
        self.rpc.engine().shutdown(self.rpc.request(())).await?;
        let status = timeout(REQUEST_TIMEOUT, self.child.wait())
            .await
            .map_err(|_| ClientError::Timeout)??;
        if !status.success() {
            return Err(ClientError::UnexpectedExit(status.to_string()));
        }
        Ok(())
    }
    pub async fn terminate(&mut self) -> Result<(), ClientError> {
        if self.child.try_wait()?.is_none() {
            self.child.start_kill()?;
            timeout(REQUEST_TIMEOUT, self.child.wait())
                .await
                .map_err(|_| ClientError::Timeout)??;
        }
        Ok(())
    }
}
