// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A single in-flight, bounded pipe client shared by the shell and CLI.

use spiling_contracts::geometry::{
    ArtifactId, GeometryCommand, GeometryError, GeometryErrorCode, GeometryLimits,
    GeometryResponse, SessionId,
};
use spiling_contracts::manufacturing::{
    MAX_MANUFACTURING_BUNDLE_BYTES, ManufacturingArtifactRecord, ManufacturingBundle,
    ManufacturingCommand, ManufacturingError, ManufacturingErrorCode, ManufacturingResponse,
    decode_bundle,
};
use spiling_contracts::project::{ProjectCommand, ProjectError, ProjectResponse};
use spiling_contracts::{
    FRAME_HEADER_BYTES, Frame, FrameHeader, FrameKind, Hello, MAX_BINARY_BYTES, MAX_CONTROL_BYTES,
    PROTOCOL_VERSION, Request, Response, WireError,
};
use std::{io, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("engine I/O: {0}")]
    Io(#[from] io::Error),
    #[error("engine framing: {0}")]
    Wire(#[from] WireError),
    #[error("engine control JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("engine protocol: {0}")]
    Protocol(String),
    #[error("upgrade required: {0}")]
    UpgradeRequired(String),
    #[error("engine rejected request ({code}): {message}")]
    Engine { code: String, message: String },
    #[error("engine request timed out after five seconds")]
    Timeout,
    #[error("engine exited: {0}")]
    UnexpectedExit(String),
    #[error("request ID space exhausted")]
    RequestIdExhausted,
    #[error("geometry: {0}")]
    Geometry(#[from] GeometryError),
    #[error("project: {0}")]
    Project(#[from] ProjectError),
    #[error("manufacturing: {0}")]
    Manufacturing(#[from] ManufacturingError),
    #[error("invalid local request: {0}")]
    InvalidRequest(String),
}

impl ClientError {
    /// Only complete typed domain failures and unwritten local rejections are recoverable.
    pub fn is_fatal(&self) -> bool {
        !matches!(
            self,
            Self::Geometry(_) | Self::Project(_) | Self::Manufacturing(_) | Self::InvalidRequest(_)
        )
    }
}

/// Kills on failed/cancelled operations; Tokio reaps children after drop.
struct KillGuard<'a> {
    child: &'a mut Child,
    armed: bool,
}

impl Drop for KillGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.child.start_kill();
        }
    }
}

async fn exchange(
    input: &mut (impl AsyncWrite + Unpin),
    output: &mut (impl AsyncRead + Unpin),
    frame: &Frame,
    expected: FrameKind,
) -> Result<Frame, ClientError> {
    let id = frame.header.request_id;
    input.write_all(&frame.header.encode()?).await?;
    input.write_all(&frame.payload).await?;
    input.flush().await?;
    let mut bytes = [0; FRAME_HEADER_BYTES];
    output.read_exact(&mut bytes).await?;
    let header = FrameHeader::decode(&bytes)?;
    if header.request_id != id {
        return Err(ClientError::Protocol(format!(
            "response ID {} does not match request {id}",
            header.request_id
        )));
    }
    if header.kind != expected && header.kind != FrameKind::Control {
        return Err(ClientError::Protocol(
            "unexpected response frame kind".into(),
        ));
    }
    let mut payload = vec![0; header.payload_len as usize];
    output.read_exact(&mut payload).await?;
    Ok(Frame { header, payload })
}

fn response(frame: &Frame) -> Result<Response, ClientError> {
    if frame.header.kind != FrameKind::Control {
        return Err(ClientError::Protocol("expected a control response".into()));
    }
    match serde_json::from_slice(&frame.payload)? {
        Response::Error { code, message } if code == "upgrade_required" => {
            Err(ClientError::UpgradeRequired(message))
        }
        Response::Error { code, message } => Err(ClientError::Engine { code, message }),
        value => Ok(value),
    }
}

fn corrupt_manufacturing(message: &str) -> ClientError {
    ManufacturingError::new(ManufacturingErrorCode::CorruptArtifact, message).into()
}

fn decode_manufacturing_artifact(
    bytes: &[u8],
    record: &ManufacturingArtifactRecord,
) -> Result<ManufacturingBundle, ClientError> {
    if bytes.len() != record.byte_count as usize || !record.hash.matches_bytes(bytes) {
        return Err(corrupt_manufacturing(
            "manufacturing bundle size or hash mismatch",
        ));
    }
    let bundle = decode_bundle(bytes)?;
    bundle.validate_record(record)?;
    Ok(bundle)
}

/// The control envelope and raw body are one exchange, including rejected metadata.
async fn exchange_manufacturing_chunk(
    input: &mut (impl AsyncWrite + Unpin),
    output: &mut (impl AsyncRead + Unpin),
    request: &Frame,
    record: &ManufacturingArtifactRecord,
    offset: u32,
    max_bytes: u32,
    bytes: &mut Vec<u8>,
) -> Result<(), ClientError> {
    let metadata = exchange(input, output, request, FrameKind::Control).await?;
    let (hash, received_offset, total_bytes, byte_count) = match response(&metadata)? {
        Response::Manufacturing {
            response:
                ManufacturingResponse::ArtifactChunk {
                    hash,
                    offset,
                    total_bytes,
                    byte_count,
                },
        } => (hash, offset, total_bytes, byte_count),
        Response::Manufacturing {
            response: ManufacturingResponse::Error { error },
        } => return Err(error.into()),
        _ => {
            return Err(ClientError::Protocol(
                "expected manufacturing chunk metadata".into(),
            ));
        }
    };
    let mut encoded = [0; FRAME_HEADER_BYTES];
    output.read_exact(&mut encoded).await?;
    // Decode checks the 4 MiB raw-frame cap before any body allocation.
    let header = FrameHeader::decode(&encoded)?;
    if header.request_id != request.header.request_id
        || header.kind != FrameKind::ManufacturingChunk
    {
        return Err(ClientError::Protocol(
            "manufacturing raw frame kind or request ID does not match metadata".into(),
        ));
    }
    let allowed_bytes = max_bytes.min(record.byte_count - offset);
    let valid = hash == record.hash
        && received_offset == offset
        && total_bytes == record.byte_count
        && total_bytes <= MAX_MANUFACTURING_BUNDLE_BYTES
        && byte_count != 0
        && byte_count <= allowed_bytes
        && header.payload_len == byte_count;
    if !valid {
        // A complete but inconsistent domain response is recoverable. Drain its bounded
        // raw body without growing the bundle or leaving the next request misaligned.
        let mut scratch = [0; 8192];
        let mut remaining = header.payload_len as usize;
        while remaining != 0 {
            let count = remaining.min(scratch.len());
            output.read_exact(&mut scratch[..count]).await?;
            remaining -= count;
        }
        return Err(corrupt_manufacturing(
            "manufacturing chunk metadata or byte count does not match artifact",
        ));
    }
    let start = bytes.len();
    bytes.resize(start + byte_count as usize, 0);
    output.read_exact(&mut bytes[start..]).await?;
    Ok(())
}

pub struct EngineClient {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    hello: Hello,
    next_id: u32,
}

impl EngineClient {
    pub async fn spawn(path: impl AsRef<Path>, protocol_version: u16) -> Result<Self, ClientError> {
        let mut child = Command::new(path.as_ref())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| ClientError::Protocol("missing child stdin".into()))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| ClientError::Protocol("missing child stdout".into()))?;
        let mut guard = KillGuard {
            child: &mut child,
            armed: true,
        };
        let request = Request::Hello {
            protocol_version,
            client_build: env!("CARGO_PKG_VERSION").into(),
        };
        let result = timeout(
            REQUEST_TIMEOUT,
            exchange(
                &mut input,
                &mut output,
                &Frame::control(1, &request)?,
                FrameKind::Control,
            ),
        )
        .await;
        let hello = match result {
            Ok(Ok(frame)) => match response(&frame) {
                Ok(Response::Hello(hello))
                    if hello.protocol_version == protocol_version
                        && hello.protocol_version == PROTOCOL_VERSION
                        && hello.max_control_bytes == MAX_CONTROL_BYTES
                        && hello.max_binary_bytes == MAX_BINARY_BYTES
                        && hello.geometry_limits == GeometryLimits::FROZEN
                        && Some(hello.pid) == guard.child.id() =>
                {
                    Ok(hello)
                }
                Ok(_) => Err(ClientError::Protocol(
                    "invalid hello response or negotiated limits".into(),
                )),
                Err(error) => Err(error),
            },
            Ok(Err(error)) => Err(error),
            Err(_) => Err(ClientError::Timeout),
        };
        match hello {
            Ok(hello) => {
                guard.armed = false;
                drop(guard);
                Ok(Self {
                    child,
                    input,
                    output,
                    hello,
                    next_id: 2,
                })
            }
            Err(error) => {
                let _ = guard.child.start_kill();
                let _ = timeout(REQUEST_TIMEOUT, guard.child.wait()).await;
                Err(error)
            }
        }
    }

    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Observes OS process state, not a cached assumption from the handshake.
    pub async fn status(&mut self) -> Result<bool, ClientError> {
        Ok(self.child.try_wait()?.is_none())
    }

    async fn request(&mut self, request: Request) -> Result<Frame, ClientError> {
        let expected = match &request {
            Request::Triangle {} => FrameKind::Triangle,
            Request::Geometry {
                command: GeometryCommand::ReadArtifactChunk { .. },
            } => FrameKind::GeometryChunk,
            _ => FrameKind::Control,
        };
        let frame = Frame::control(self.next_id, &request)?;
        self.request_frame(frame, expected).await
    }

    async fn request_frame(
        &mut self,
        frame: Frame,
        expected: FrameKind,
    ) -> Result<Frame, ClientError> {
        if let Some(status) = self.child.try_wait()? {
            return Err(ClientError::UnexpectedExit(status.to_string()));
        }
        let id = self.next_id;
        let Some(next_id) = id.checked_add(1) else {
            let _ = self.terminate().await;
            return Err(ClientError::RequestIdExhausted);
        };
        self.next_id = next_id;
        let mut guard = KillGuard {
            child: &mut self.child,
            armed: true,
        };
        let result = match timeout(
            REQUEST_TIMEOUT,
            exchange(&mut self.input, &mut self.output, &frame, expected),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ClientError::Timeout),
        };
        match result {
            Ok(frame) => {
                guard.armed = false;
                Ok(frame)
            }
            Err(error) => {
                let observed = guard.child.try_wait().ok().flatten();
                let _ = guard.child.start_kill();
                let _ = timeout(REQUEST_TIMEOUT, guard.child.wait()).await;
                match observed {
                    Some(status) => Err(ClientError::UnexpectedExit(status.to_string())),
                    None => Err(error),
                }
            }
        }
    }

    async fn finish<T>(&mut self, result: Result<T, ClientError>) -> Result<T, ClientError> {
        if result.as_ref().is_err_and(|error| error.is_fatal()) {
            let _ = self.terminate().await;
        }
        result
    }

    pub async fn ping(&mut self) -> Result<(), ClientError> {
        let frame = self.request(Request::Ping {}).await?;
        let result = match response(&frame) {
            Ok(Response::Pong {}) => Ok(()),
            Ok(_) => Err(ClientError::Protocol("expected pong".into())),
            Err(error) => Err(error),
        };
        self.finish(result).await
    }

    pub async fn triangle(&mut self) -> Result<Vec<u8>, ClientError> {
        let frame = self.request(Request::Triangle {}).await?;
        if frame.header.kind == FrameKind::Triangle {
            return Ok(frame.payload);
        }
        let result = match response(&frame) {
            Err(error) => Err(error),
            Ok(_) => Err(ClientError::Protocol("expected binary triangle".into())),
        };
        self.finish(result).await
    }

    /// One short control exchange; heavy geometry work is polled through jobs.
    pub async fn geometry(
        &mut self,
        command: GeometryCommand,
    ) -> Result<GeometryResponse, ClientError> {
        if matches!(command, GeometryCommand::ReadArtifactChunk { .. }) {
            return Err(ClientError::InvalidRequest(
                "use read_geometry_chunk for binary reads".into(),
            ));
        }
        let validation = match &command {
            GeometryCommand::ImportPart {
                source,
                initial_pose,
                ..
            } => source.validate().and_then(|()| initial_pose.validate()),
            GeometryCommand::AddInstance { pose, .. }
            | GeometryCommand::SetInstancePose { pose, .. } => pose.validate(),
            GeometryCommand::StartSection { plane, .. } => plane.validate(),
            _ => Ok(()),
        };
        if let Err(error) = validation {
            return Ok(GeometryResponse::Error { error });
        }
        let request = Request::Geometry { command };
        let frame = match Frame::control(self.next_id, &request) {
            Ok(frame) => frame,
            Err(WireError::Length { .. }) => {
                return Ok(GeometryResponse::Error {
                    error: GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "geometry control exceeds limit",
                    ),
                });
            }
            Err(error) => return Err(ClientError::InvalidRequest(error.to_string())),
        };
        let frame = self.request_frame(frame, FrameKind::Control).await?;
        let result = match response(&frame) {
            Ok(Response::Geometry {
                response: GeometryResponse::ProjectError { error },
            }) => Err(ClientError::Project(error)),
            Ok(Response::Geometry { response }) => Ok(response),
            Ok(_) => Err(ClientError::Protocol("expected geometry response".into())),
            Err(error) => Err(error),
        };
        self.finish(result).await
    }

    /// Bounded project control; open/save completion uses the shared native job service.
    pub async fn project(
        &mut self,
        command: ProjectCommand,
    ) -> Result<ProjectResponse, ClientError> {
        let validation = match &command {
            ProjectCommand::Open { path, .. } => path.validate(),
            ProjectCommand::Save {
                target: Some(path), ..
            } => path.validate(),
            _ => Ok(()),
        };
        validation.map_err(|error| ClientError::InvalidRequest(error.to_string()))?;
        let frame = Frame::control(self.next_id, &Request::Project { command })
            .map_err(|error| ClientError::InvalidRequest(error.to_string()))?;
        let frame = self.request_frame(frame, FrameKind::Control).await?;
        let result = match response(&frame) {
            Ok(Response::Project {
                response: ProjectResponse::Error { error },
            }) => Err(ClientError::Project(error)),
            Ok(Response::Project { response }) => Ok(response),
            Ok(_) => Err(ClientError::Protocol("expected project response".into())),
            Err(error) => Err(error),
        };
        self.finish(result).await
    }

    /// Bounded manufacturing control; compilation and replay use the shared job service.
    pub async fn manufacturing(
        &mut self,
        command: ManufacturingCommand,
    ) -> Result<ManufacturingResponse, ClientError> {
        if matches!(command, ManufacturingCommand::ReadArtifactChunk { .. }) {
            return Err(ClientError::InvalidRequest(
                "use fetch_manufacturing_bundle for binary reads".into(),
            ));
        }
        if let ManufacturingCommand::SetIntent { intent, .. } = &command {
            intent.validate()?;
        }
        let frame = Frame::control(self.next_id, &Request::Manufacturing { command })
            .map_err(|error| ClientError::InvalidRequest(error.to_string()))?;
        let frame = self.request_frame(frame, FrameKind::Control).await?;
        let result = match response(&frame) {
            Ok(Response::Manufacturing {
                response: ManufacturingResponse::Error { error },
            }) => Err(ClientError::Manufacturing(error)),
            Ok(Response::Manufacturing { response }) => Ok(response),
            Ok(_) => Err(ClientError::Protocol(
                "expected manufacturing response".into(),
            )),
            Err(error) => Err(error),
        };
        self.finish(result).await
    }

    /// Pull exact bounded JSON bytes, validate their content identity and strict schema.
    /// This is not independent program replay; that remains an engine manufacturing job.
    pub async fn fetch_manufacturing_bundle(
        &mut self,
        record: &ManufacturingArtifactRecord,
    ) -> Result<ManufacturingBundle, ClientError> {
        if record.byte_count == 0 {
            return Err(corrupt_manufacturing("manufacturing artifact is empty"));
        }
        if record.byte_count > MAX_MANUFACTURING_BUNDLE_BYTES {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "manufacturing artifact exceeds bundle byte limit",
            )
            .into());
        }
        record.validate()?;
        let mut bytes = Vec::with_capacity(record.byte_count as usize);
        while bytes.len() < record.byte_count as usize {
            let offset = bytes.len() as u32;
            let max_bytes = MAX_BINARY_BYTES.min(record.byte_count - offset);
            self.read_manufacturing_chunk(record, offset, max_bytes, &mut bytes)
                .await?;
        }
        decode_manufacturing_artifact(&bytes, record)
    }

    async fn read_manufacturing_chunk(
        &mut self,
        record: &ManufacturingArtifactRecord,
        offset: u32,
        max_bytes: u32,
        bytes: &mut Vec<u8>,
    ) -> Result<(), ClientError> {
        let frame = Frame::control(
            self.next_id,
            &Request::Manufacturing {
                command: ManufacturingCommand::ReadArtifactChunk {
                    session_id: self.hello.session_id.clone(),
                    hash: record.hash.clone(),
                    offset,
                    max_bytes,
                },
            },
        )
        .map_err(|error| ClientError::InvalidRequest(error.to_string()))?;
        if let Some(status) = self.child.try_wait()? {
            return Err(ClientError::UnexpectedExit(status.to_string()));
        }
        let Some(next_id) = self.next_id.checked_add(1) else {
            let _ = self.terminate().await;
            return Err(ClientError::RequestIdExhausted);
        };
        self.next_id = next_id;
        let mut guard = KillGuard {
            child: &mut self.child,
            armed: true,
        };
        let result = match timeout(
            REQUEST_TIMEOUT,
            exchange_manufacturing_chunk(
                &mut self.input,
                &mut self.output,
                &frame,
                record,
                offset,
                max_bytes,
                bytes,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ClientError::Timeout),
        };
        if !result.as_ref().is_err_and(|error| error.is_fatal()) {
            guard.armed = false;
            return result;
        }
        let observed = guard.child.try_wait().ok().flatten();
        let _ = guard.child.start_kill();
        let _ = timeout(REQUEST_TIMEOUT, guard.child.wait()).await;
        match observed {
            Some(status) => Err(ClientError::UnexpectedExit(status.to_string())),
            None => result,
        }
    }

    pub async fn read_geometry_chunk(
        &mut self,
        session: &SessionId,
        artifact: ArtifactId,
        chunk: u32,
    ) -> Result<Vec<u8>, ClientError> {
        let frame = self
            .request(Request::Geometry {
                command: GeometryCommand::ReadArtifactChunk {
                    session_id: session.clone(),
                    artifact_id: artifact,
                    chunk_index: chunk,
                },
            })
            .await?;
        if frame.header.kind == FrameKind::GeometryChunk {
            return Ok(frame.payload);
        }
        let result = match response(&frame) {
            Ok(Response::Geometry {
                response: GeometryResponse::Error { error },
            }) => Err(ClientError::Geometry(error)),
            Ok(Response::Geometry {
                response: GeometryResponse::ProjectError { error },
            }) => Err(ClientError::Project(error)),
            Ok(_) => Err(ClientError::Protocol("expected geometry chunk".into())),
            Err(error) => Err(error),
        };
        self.finish(result).await
    }

    pub async fn shutdown(&mut self) -> Result<(), ClientError> {
        let frame = self.request(Request::Shutdown {}).await?;
        let result = match response(&frame) {
            Ok(Response::Bye {}) => {
                let mut guard = KillGuard {
                    child: &mut self.child,
                    armed: true,
                };
                let exit = match timeout(REQUEST_TIMEOUT, guard.child.wait()).await {
                    Ok(Ok(status)) if status.success() => Ok(()),
                    Ok(Ok(status)) => Err(ClientError::UnexpectedExit(status.to_string())),
                    Ok(Err(error)) => Err(error.into()),
                    Err(_) => Err(ClientError::Timeout),
                };
                if exit.is_ok() {
                    guard.armed = false;
                }
                exit
            }
            Ok(_) => Err(ClientError::Protocol(
                "expected shutdown acknowledgement".into(),
            )),
            Err(error) => Err(error),
        };
        self.finish(result).await
    }

    /// Intentional interruption or error cleanup; waits for observed process exit.
    pub async fn terminate(&mut self) -> Result<(), ClientError> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        self.child.start_kill()?;
        timeout(REQUEST_TIMEOUT, self.child.wait())
            .await
            .map_err(|_| ClientError::Timeout)??;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
