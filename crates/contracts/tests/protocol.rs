// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::geometry::*;
use spiling_contracts::*;
use std::io::{self, Cursor};

#[test]
fn flat_serde_shape_and_strict_requests() {
    let hello = Hello {
        protocol_version: PROTOCOL_VERSION,
        engine_build: "test".into(),
        kernel: KernelIdentity {
            name: "monstertruck".into(),
            version: "0.4.1".into(),
            revision: "test".into(),
        },
        geometry_capabilities: vec![],
        project_capabilities: vec![],
        manufacturing_capabilities: vec![],
        max_control_bytes: MAX_CONTROL_BYTES,
        max_binary_bytes: MAX_BINARY_BYTES,
        pid: 7,
        session_id: SessionId::new(),
        geometry_limits: GeometryLimits::FROZEN,
    };
    let value = serde_json::to_value(Response::Hello(hello.clone())).unwrap();
    assert_eq!(value["type"], "hello");
    assert_eq!(value["pid"], 7);
    assert!(value.get("content").is_none());
    assert_eq!(
        serde_json::from_value::<Response>(value).unwrap(),
        Response::Hello(hello)
    );
    for bytes in [
        r#"{"type":"unknown"}"#,
        r#"{"type":"ping","extra":1}"#,
        r#"{"type":"hello","protocol_version":1}"#,
    ] {
        assert!(serde_json::from_str::<Request>(bytes).is_err());
    }
}

#[test]
fn length_limits_are_inclusive_and_checked_before_payload() {
    for (kind, limit) in [
        (FrameKind::Control, MAX_CONTROL_BYTES),
        (FrameKind::Triangle, MAX_BINARY_BYTES),
        (FrameKind::GeometryChunk, MAX_GEOMETRY_CHUNK_BYTES),
        (FrameKind::ManufacturingChunk, MAX_BINARY_BYTES),
    ] {
        let header = FrameHeader::new(kind, 1, limit as usize).unwrap();
        assert_eq!(
            FrameHeader::decode(&header.encode().unwrap()).unwrap(),
            header
        );
        assert!(matches!(
            FrameHeader::new(kind, 1, limit as usize + 1),
            Err(WireError::Length { .. })
        ));
        let mut bytes = header.encode().unwrap();
        bytes[12..16].copy_from_slice(&(limit + 1).to_le_bytes());
        // No payload exists: Length must precede any attempt to read or allocate it.
        assert!(matches!(
            Frame::read(&mut Cursor::new(bytes)),
            Err(WireError::Length { .. })
        ));
    }
    assert!(matches!(
        FrameHeader::new(FrameKind::Control, 1, usize::MAX),
        Err(WireError::Length { .. })
    ));
}

#[test]
fn invalid_header_fields_are_rejected() {
    let valid = FrameHeader::new(FrameKind::Control, 4, 0)
        .unwrap()
        .encode()
        .unwrap();
    let mut bytes = valid;
    bytes[0] = 0;
    assert!(matches!(FrameHeader::decode(&bytes), Err(WireError::Magic)));
    bytes = valid;
    bytes[4..6].copy_from_slice(&(PROTOCOL_VERSION + 1).to_le_bytes());
    assert!(matches!(
        FrameHeader::decode(&bytes),
        Err(WireError::Version(version)) if version == PROTOCOL_VERSION + 1
    ));
    bytes = valid;
    bytes[4..6].copy_from_slice(&3u16.to_le_bytes());
    assert!(matches!(
        FrameHeader::decode(&bytes),
        Err(WireError::Version(3))
    ));
    bytes = valid;
    bytes[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(matches!(
        FrameHeader::decode(&bytes),
        Err(WireError::Kind(u16::MAX))
    ));
    bytes = valid;
    bytes[8..12].fill(0);
    assert!(matches!(
        FrameHeader::decode(&bytes),
        Err(WireError::RequestId)
    ));
}

#[test]
fn framing_rejects_every_truncation_and_trailing_bytes() {
    let frame = Frame::control(42, &Request::Ping {}).unwrap();
    let mut bytes = Vec::new();
    frame.write(&mut bytes).unwrap();
    assert!(Frame::read(&mut Cursor::new([])).unwrap().is_none());
    for length in 1..bytes.len() {
        assert!(
            matches!(Frame::read(&mut Cursor::new(&bytes[..length])), Err(WireError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
        );
        assert!(Frame::decode(&bytes[..length]).is_err());
    }
    let decoded = Frame::decode(&bytes).unwrap();
    assert_eq!(decoded.header.request_id, 42);
    assert_eq!(decoded.payload, frame.payload);
    bytes.push(0);
    assert!(matches!(Frame::decode(&bytes), Err(WireError::Size)));
}

#[test]
fn concatenated_frames_and_mutated_lengths() {
    let first = Frame::control(1, &Request::Ping {}).unwrap();
    let second = Frame::control(2, &Request::Shutdown {}).unwrap();
    let mut bytes = Vec::new();
    first.write(&mut bytes).unwrap();
    second.write(&mut bytes).unwrap();
    let mut input = Cursor::new(bytes);
    assert_eq!(
        Frame::read(&mut input).unwrap().unwrap().header.request_id,
        1
    );
    assert_eq!(
        Frame::read(&mut input).unwrap().unwrap().header.request_id,
        2
    );
    assert!(Frame::read(&mut input).unwrap().is_none());
    let mut mutated = first;
    mutated.header.payload_len += 1;
    assert!(matches!(
        mutated.write(&mut Vec::new()),
        Err(WireError::Size)
    ));
}

#[test]
fn triangle_matches_cross_language_golden() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/protocol/triangle.json")).unwrap();
    assert_eq!(fixture["encoding"], "hex");
    let hex = fixture["data"].as_str().unwrap();
    let decoded: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
        .collect();
    assert_eq!(decoded, synthetic_triangle());
    assert_eq!(decoded.len(), 64);
}

#[test]
fn manufacturing_namespace_and_common_jobs_are_interoperable_and_strict() {
    use spiling_contracts::manufacturing::*;
    let session = SessionId::new();
    let request = Request::Manufacturing {
        command: ManufacturingCommand::Compile {
            session_id: session.clone(),
            base_revision: spiling_contracts::project::ProjectRevision(9),
        },
    };
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["type"], "manufacturing");
    assert_eq!(value["command"]["op"], "compile");
    assert_eq!(
        serde_json::from_value::<Request>(value.clone()).unwrap(),
        request
    );
    let mut invalid = value;
    invalid["command"]["unknown"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Request>(invalid).is_err());
    let error: JobError = ManufacturingError::new(
        ManufacturingErrorCode::VerificationFailed,
        "tampered program",
    )
    .into();
    let value = serde_json::to_value(&error).unwrap();
    assert_eq!(value["domain"], "manufacturing");
    assert_eq!(serde_json::from_value::<JobError>(value).unwrap(), error);
    let frame = Frame::new(
        FrameKind::ManufacturingChunk,
        8,
        b"{\"schema_version\":1}".to_vec(),
    )
    .unwrap();
    let mut bytes = Vec::new();
    frame.write(&mut bytes).unwrap();
    assert_eq!(u16::from_le_bytes(bytes[6..8].try_into().unwrap()), 4);
    let decoded = Frame::decode(&bytes).unwrap();
    assert_eq!(decoded.header.kind, FrameKind::ManufacturingChunk);
    assert_eq!(decoded.payload, frame.payload);
}
