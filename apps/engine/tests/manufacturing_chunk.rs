// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{
    Frame, FrameKind, MAX_BINARY_BYTES, PROTOCOL_VERSION, Request, Response,
    geometry::{SessionId, SourceHash},
    manufacturing::{ManufacturingCommand, ManufacturingErrorCode, ManufacturingResponse},
};
use std::process::{Command, Stdio};

#[test]
fn raw_manufacturing_chunk_rejections_leave_actual_child_protocol_aligned() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_spiling-engine"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    Frame::control(
        1,
        &Request::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_build: "chunk-regression".into(),
        },
    )
    .unwrap()
    .write(&mut input)
    .unwrap();
    let hello = Frame::read(&mut output).unwrap().unwrap();
    let Response::Hello(hello) = serde_json::from_slice(&hello.payload).unwrap() else {
        panic!("hello")
    };
    let cases = [
        (
            SessionId::new(),
            0,
            1,
            ManufacturingErrorCode::StaleRevision,
        ),
        (
            hello.session_id.clone(),
            0,
            0,
            ManufacturingErrorCode::ResourceLimit,
        ),
        (
            hello.session_id.clone(),
            0,
            MAX_BINARY_BYTES + 1,
            ManufacturingErrorCode::ResourceLimit,
        ),
        (
            hello.session_id.clone(),
            u32::MAX,
            MAX_BINARY_BYTES,
            ManufacturingErrorCode::CorruptArtifact,
        ),
    ];
    let mut id = 2;
    for (session_id, offset, max_bytes, expected) in cases {
        Frame::control(
            id,
            &Request::Manufacturing {
                command: ManufacturingCommand::ReadArtifactChunk {
                    session_id,
                    hash: SourceHash::from_bytes(b"missing"),
                    offset,
                    max_bytes,
                },
            },
        )
        .unwrap()
        .write(&mut input)
        .unwrap();
        let frame = Frame::read(&mut output).unwrap().unwrap();
        assert_eq!(frame.header.request_id, id);
        assert_eq!(frame.header.kind, FrameKind::Control);
        let Response::Manufacturing {
            response: ManufacturingResponse::Error { error },
        } = serde_json::from_slice(&frame.payload).unwrap()
        else {
            panic!("typed chunk error")
        };
        assert_eq!(error.code, expected);
        id += 1;
        Frame::control(id, &Request::Ping {})
            .unwrap()
            .write(&mut input)
            .unwrap();
        let frame = Frame::read(&mut output).unwrap().unwrap();
        assert_eq!(frame.header.request_id, id);
        assert!(matches!(
            serde_json::from_slice::<Response>(&frame.payload).unwrap(),
            Response::Pong {}
        ));
        id += 1;
    }
    Frame::control(id, &Request::Shutdown {})
        .unwrap()
        .write(&mut input)
        .unwrap();
    assert!(matches!(
        serde_json::from_slice::<Response>(&Frame::read(&mut output).unwrap().unwrap().payload)
            .unwrap(),
        Response::Bye {}
    ));
    assert!(child.wait().unwrap().success());
}
