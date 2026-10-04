// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{
    Frame, FrameKind, Hello, MAX_BINARY_BYTES, MAX_CONTROL_BYTES, PROTOCOL_VERSION, Request,
    Response, synthetic_triangle,
};
use std::{io, process::ExitCode};

fn diagnostic(level: &str, event: &str, message: &str) {
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
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    let mut negotiated = false;
    let mut last_id = 0;
    diagnostic("info", "started", "engine awaiting protocol handshake");
    while let Some(frame) = Frame::read(&mut input)? {
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
        let request: Request = match serde_json::from_slice(&frame.payload) {
            Ok(request) => request,
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
                    kernel: "monstertruck".into(),
                    geometry_capabilities: Vec::new(),
                    max_control_bytes: MAX_CONTROL_BYTES,
                    max_binary_bytes: MAX_BINARY_BYTES,
                    pid: std::process::id(),
                };
                Frame::control(id, &Response::Hello(hello))?.write(&mut output)?;
                negotiated = true;
                diagnostic(
                    "info",
                    "negotiated",
                    "protocol handshake accepted; geometry capabilities are empty",
                );
            }
            Request::Ping {} => Frame::control(id, &Response::Pong {})?.write(&mut output)?,
            Request::Triangle {} => {
                Frame::write_payload(FrameKind::Triangle, id, &synthetic_triangle(), &mut output)?;
            }
            Request::Shutdown {} => {
                Frame::control(id, &Response::Bye {})?.write(&mut output)?;
                diagnostic("info", "shutdown", "clean shutdown acknowledged");
                return Ok(());
            }
        }
    }
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
