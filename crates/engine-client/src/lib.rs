// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A single in-flight, bounded pipe client shared by the shell and CLI.

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
    id: u32,
    request: &Request,
) -> Result<Frame, ClientError> {
    let frame = Frame::control(id, request)?;
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
            exchange(&mut input, &mut output, 1, &request),
        )
        .await;
        let hello = match result {
            Ok(Ok(frame)) => match response(&frame) {
                Ok(Response::Hello(hello))
                    if hello.protocol_version == protocol_version
                        && hello.protocol_version == PROTOCOL_VERSION
                        && hello.max_control_bytes == MAX_CONTROL_BYTES
                        && hello.max_binary_bytes == MAX_BINARY_BYTES
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
            exchange(&mut self.input, &mut self.output, id, &request),
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
        if result.is_err() {
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
