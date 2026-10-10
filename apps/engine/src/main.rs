// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod geometry;
mod session;

use geometry::Decoded;
use spiling_contracts::geometry::{
    GeometryCommand, GeometryLimits, GeometryResponse, KernelIdentity,
};
use spiling_contracts::manufacturing::ManufacturingCommand;
use spiling_contracts::{
    Frame, FrameKind, Hello, MAX_BINARY_BYTES, MAX_CONTROL_BYTES, PROTOCOL_VERSION, Request,
    Response, synthetic_triangle,
};
use std::{io, process::ExitCode, sync::mpsc, time::Duration};

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
        serde_json::json!({"level": level, "event": event, "message": message})
    );
}

fn reject(
    output: &mut impl io::Write,
    request_id: u32,
    code: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    diagnostic("error", code, message);
    Frame::control(
        request_id,
        &Response::Error {
            code: code.into(),
            message: message.into(),
        },
    )?
    .write(output)?;
    Err(message.into())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut negotiated = false;
    let mut last_id = 0;
    let fault = ProjectFault::from_env()?;
    let mut runtime = geometry::Runtime::new(fault);
    // Keep the dispatcher alive while stdin is idle. The reader has exactly one
    // bounded control slot, and the worker/watchdog never own stdout.
    let (incoming, requests) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        loop {
            let frame = Frame::read(&mut input);
            let terminal = !matches!(frame, Ok(Some(_)));
            if incoming.send(frame).is_err() || terminal {
                break;
            }
        }
    });
    diagnostic("info", "started", "engine awaiting protocol handshake");
    loop {
        runtime.poll().map_err(io::Error::other)?;
        let frame = match requests.recv_timeout(Duration::from_millis(10)) {
            Ok(frame) => match frame? {
                Some(frame) => frame,
                None => break,
            },
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let id = frame.header.request_id;
        if id <= last_id {
            return reject(
                &mut output,
                id,
                "invalid_request_id",
                "request IDs must increase strictly",
            );
        }
        last_id = id;
        if frame.header.kind != FrameKind::Control {
            return reject(
                &mut output,
                id,
                "invalid_request",
                "engine accepts only control requests",
            );
        }
        let request = match geometry::decode_request(&frame.payload) {
            Ok(Decoded::Request(request)) => request,
            Ok(Decoded::Domain(error)) if negotiated => {
                Frame::control(
                    id,
                    &Response::Geometry {
                        response: GeometryResponse::Error { error },
                    },
                )?
                .write(&mut output)?;
                continue;
            }
            Ok(Decoded::ManufacturingDomain(error)) if negotiated => {
                Frame::control(
                    id,
                    &Response::Manufacturing {
                        response: spiling_contracts::manufacturing::ManufacturingResponse::Error {
                            error,
                        },
                    },
                )?
                .write(&mut output)?;
                continue;
            }
            Ok(Decoded::Domain(_) | Decoded::ManufacturingDomain(_)) => {
                return reject(
                    &mut output,
                    id,
                    "handshake_required",
                    "hello must precede commands",
                );
            }
            Err(_) => {
                return reject(
                    &mut output,
                    id,
                    "invalid_request",
                    "malformed or unknown control request",
                );
            }
        };
        if !negotiated && !matches!(request, Request::Hello { .. }) {
            return reject(
                &mut output,
                id,
                "handshake_required",
                "hello must precede commands",
            );
        }
        match request {
            Request::Hello {
                protocol_version,
                client_build: _,
            } => {
                if negotiated {
                    return reject(
                        &mut output,
                        id,
                        "already_negotiated",
                        "hello may occur only once",
                    );
                }
                if protocol_version != PROTOCOL_VERSION {
                    return reject(
                        &mut output,
                        id,
                        "upgrade_required",
                        &format!(
                            "protocol mismatch: client {protocol_version}, engine {PROTOCOL_VERSION}; upgrade required"
                        ),
                    );
                }
                let hello = Hello {
                    protocol_version: PROTOCOL_VERSION,
                    engine_build: env!("CARGO_PKG_VERSION").into(),
                    kernel: KernelIdentity {
                        name: "monstertruck".into(),
                        version: "0.4.1".into(),
                        revision: "d87b4d9ced1f3baf31aa771ac0e7c663efb1c001".into(),
                    },
                    geometry_capabilities: [
                        "step_planar_cylindrical_v1",
                        "multi_instance_scene_v1",
                        "mesh_chunks_v1",
                        "native_plane_section_v1",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
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
                        "immutable_bundle_chunks_v1",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                    max_control_bytes: MAX_CONTROL_BYTES,
                    max_binary_bytes: MAX_BINARY_BYTES,
                    pid: std::process::id(),
                    session_id: runtime.session_id.clone(),
                    geometry_limits: GeometryLimits::FROZEN,
                };
                Frame::control(id, &Response::Hello(hello))?.write(&mut output)?;
                negotiated = true;
                diagnostic(
                    "info",
                    "negotiated",
                    "protocol v4 software-manufacturing project session accepted",
                );
            }
            Request::Ping {} => Frame::control(id, &Response::Pong {})?.write(&mut output)?,
            Request::Triangle {} => {
                Frame::write_payload(FrameKind::Triangle, id, &synthetic_triangle(), &mut output)?;
            }
            Request::Geometry {
                command:
                    GeometryCommand::ReadArtifactChunk {
                        session_id,
                        artifact_id,
                        chunk_index,
                    },
            } => {
                runtime.write_chunk(&session_id, artifact_id, chunk_index, id, &mut output)?;
            }
            Request::Geometry { command } => {
                let response = runtime.control(command);
                Frame::control(id, &Response::Geometry { response })?.write(&mut output)?;
            }
            Request::Project { command } => {
                let response = runtime.project_control(command);
                Frame::control(id, &Response::Project { response })?.write(&mut output)?;
            }
            Request::Manufacturing {
                command:
                    ManufacturingCommand::ReadArtifactChunk {
                        session_id,
                        hash,
                        offset,
                        max_bytes,
                    },
            } => {
                runtime.write_manufacturing_chunk(
                    &session_id,
                    &hash,
                    offset,
                    max_bytes,
                    id,
                    &mut output,
                )?;
            }
            Request::Manufacturing { command } => {
                let response = runtime.manufacturing_control(command);
                Frame::control(id, &Response::Manufacturing { response })?.write(&mut output)?;
            }
            Request::Shutdown {} => {
                runtime.shutdown().map_err(io::Error::other)?;
                Frame::control(id, &Response::Bye {})?.write(&mut output)?;
                diagnostic("info", "shutdown", "clean shutdown acknowledged");
                return Ok(());
            }
        }
    }
    runtime.shutdown().map_err(io::Error::other)?;
    diagnostic("info", "input_closed", "client pipe closed; exiting");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            diagnostic("error", "engine_exit", &error.to_string());
            ExitCode::FAILURE
        }
    }
}
