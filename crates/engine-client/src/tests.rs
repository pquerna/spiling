// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use tokio::io::duplex;

async fn reply_with(header: [u8; FRAME_HEADER_BYTES], body: &[u8]) -> Result<Frame, ClientError> {
    let (mut client, mut peer) = duplex(1024);
    let (mut output, mut input) = tokio::io::split(&mut client);
    let responder = async move {
        let mut request_header = [0; FRAME_HEADER_BYTES];
        peer.read_exact(&mut request_header).await.unwrap();
        let request_header = FrameHeader::decode(&request_header).unwrap();
        let mut payload = vec![0; request_header.payload_len as usize];
        peer.read_exact(&mut payload).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Request>(&payload).unwrap(),
            Request::Ping {}
        );
        peer.write_all(&header).await.unwrap();
        peer.write_all(body).await.unwrap();
    };
    let request = Frame::control(9, &Request::Ping {}).unwrap();
    let operation = exchange(&mut input, &mut output, &request, FrameKind::Control);
    let (_, response) = timeout(Duration::from_secs(1), async {
        tokio::join!(responder, operation)
    })
    .await
    .unwrap();
    response
}

#[tokio::test]
async fn rejects_wrong_response_id_before_reading_payload() {
    let header = FrameHeader::new(FrameKind::Control, 8, 100)
        .unwrap()
        .encode()
        .unwrap();
    assert!(
        matches!(reply_with(header, &[]).await, Err(ClientError::Protocol(message)) if message.contains("does not match"))
    );
}

#[tokio::test]
async fn rejects_oversized_response_before_reading_payload() {
    let mut header = FrameHeader::new(FrameKind::Control, 9, 0)
        .unwrap()
        .encode()
        .unwrap();
    header[12..16].copy_from_slice(&(MAX_CONTROL_BYTES + 1).to_le_bytes());
    assert!(matches!(
        reply_with(header, &[]).await,
        Err(ClientError::Wire(WireError::Length { .. }))
    ));
}

#[tokio::test]
async fn validates_control_shape_and_classifies_upgrade_required() {
    let frame = Frame::control(9, &Response::Pong {}).unwrap();
    let received = reply_with(frame.header.encode().unwrap(), &frame.payload)
        .await
        .unwrap();
    assert_eq!(response(&received).unwrap(), Response::Pong {});
    let error = Frame::control(
        9,
        &Response::Error {
            code: "upgrade_required".into(),
            message: "different protocol".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        response(&error),
        Err(ClientError::UpgradeRequired(_))
    ));
    let malformed =
        Frame::new(FrameKind::Control, 9, b"{\"type\":\"unexpected\"}".to_vec()).unwrap();
    assert!(matches!(response(&malformed), Err(ClientError::Json(_))));
    let binary = Frame::new(FrameKind::Triangle, 9, vec![]).unwrap();
    assert!(matches!(response(&binary), Err(ClientError::Protocol(_))));
}

#[tokio::test]
async fn truncated_payload_and_stalled_pipe_fail() {
    let header = FrameHeader::new(FrameKind::Control, 9, 30)
        .unwrap()
        .encode()
        .unwrap();
    assert!(
        matches!(reply_with(header, b"{}").await, Err(ClientError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
    );
    let (mut client, _peer) = duplex(1024);
    let (mut output, mut input) = tokio::io::split(&mut client);
    let request = Frame::control(9, &Request::Ping {}).unwrap();
    assert!(
        timeout(
            Duration::from_millis(10),
            exchange(&mut input, &mut output, &request, FrameKind::Control)
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn rejects_geometry_chunk_over_one_mebibyte_before_body_allocation() {
    let mut header = FrameHeader::new(FrameKind::GeometryChunk, 9, 0)
        .unwrap()
        .encode()
        .unwrap();
    header[12..16].copy_from_slice(
        &(spiling_contracts::geometry::MAX_GEOMETRY_CHUNK_BYTES + 1).to_le_bytes(),
    );
    assert!(matches!(
        reply_with(header, &[]).await,
        Err(ClientError::Wire(WireError::Length { .. }))
    ));
}

#[tokio::test]
async fn rejects_wrong_binary_kind_before_body_read() {
    let header = FrameHeader::new(FrameKind::GeometryChunk, 9, 100)
        .unwrap()
        .encode()
        .unwrap();
    assert!(matches!(
        reply_with(header, &[]).await,
        Err(ClientError::Protocol(_))
    ));
}

#[test]
fn typed_geometry_errors_are_recoverable_but_malformed_control_is_not() {
    let error = GeometryError::new(GeometryErrorCode::InvalidGeometry, "invalid STEP input");
    let frame = Frame::control(
        1,
        &Response::Geometry {
            response: GeometryResponse::Error {
                error: error.clone(),
            },
        },
    )
    .unwrap();
    assert_eq!(
        response(&frame).unwrap(),
        Response::Geometry {
            response: GeometryResponse::Error {
                error: error.clone()
            },
        }
    );
    assert!(!ClientError::Geometry(error).is_fatal());
    assert!(!ClientError::InvalidRequest("use binary method".into()).is_fatal());
    assert!(
        !ClientError::Project(ProjectError::new(
            spiling_contracts::project::ProjectErrorCode::ReadOnly,
            "read-only project"
        ))
        .is_fatal()
    );
    assert!(ClientError::Protocol("unexpected shape".into()).is_fatal());
    assert!(ClientError::Timeout.is_fatal());
}

fn manufacturing_record(bytes: &[u8]) -> ManufacturingArtifactRecord {
    ManufacturingArtifactRecord {
        hash: spiling_contracts::geometry::SourceHash::from_bytes(bytes),
        input_hash: spiling_contracts::geometry::SourceHash::from_bytes(b"input"),
        byte_count: bytes.len() as u32,
        summary: spiling_contracts::manufacturing::ManufacturingSummary {
            layers: 1,
            paths: 1,
            deposition_segments: 1,
            deposited_volume_mm3: 1.0,
            filament_length_mm: 1.0,
            software_only: true,
        },
    }
}

fn chunk_metadata(record: &ManufacturingArtifactRecord, offset: u32, count: u32) -> Frame {
    Frame::control(
        9,
        &Response::Manufacturing {
            response: ManufacturingResponse::ArtifactChunk {
                hash: record.hash.clone(),
                offset,
                total_bytes: record.byte_count,
                byte_count: count,
            },
        },
    )
    .unwrap()
}

async fn manufacturing_reply(
    record: &ManufacturingArtifactRecord,
    offset: u32,
    max_bytes: u32,
    metadata: Frame,
    raw_header: [u8; FRAME_HEADER_BYTES],
    raw_body: &[u8],
) -> (Result<(), ClientError>, Vec<u8>) {
    let (mut client, mut peer) = duplex(1024);
    let (mut output, mut input) = tokio::io::split(&mut client);
    let responder = async move {
        let mut header = [0; FRAME_HEADER_BYTES];
        peer.read_exact(&mut header).await.unwrap();
        let header = FrameHeader::decode(&header).unwrap();
        let mut request = vec![0; header.payload_len as usize];
        peer.read_exact(&mut request).await.unwrap();
        assert!(matches!(
            serde_json::from_slice::<Request>(&request).unwrap(),
            Request::Manufacturing {
                command: ManufacturingCommand::ReadArtifactChunk { .. }
            }
        ));
        peer.write_all(&metadata.header.encode().unwrap())
            .await
            .unwrap();
        peer.write_all(&metadata.payload).await.unwrap();
        peer.write_all(&raw_header).await.unwrap();
        peer.write_all(raw_body).await.unwrap();
        // An unread body would corrupt this next response.
        let pong = Frame::control(10, &Response::Pong {}).unwrap();
        peer.write_all(&pong.header.encode().unwrap())
            .await
            .unwrap();
        peer.write_all(&pong.payload).await.unwrap();
    };
    let request = Frame::control(
        9,
        &Request::Manufacturing {
            command: ManufacturingCommand::ReadArtifactChunk {
                session_id: SessionId::new(),
                hash: record.hash.clone(),
                offset,
                max_bytes,
            },
        },
    )
    .unwrap();
    let operation = async {
        let mut bytes = Vec::new();
        let result = exchange_manufacturing_chunk(
            &mut input,
            &mut output,
            &request,
            record,
            offset,
            max_bytes,
            &mut bytes,
        )
        .await;
        if !result.as_ref().is_err_and(|error| error.is_fatal()) {
            let mut header = [0; FRAME_HEADER_BYTES];
            output.read_exact(&mut header).await.unwrap();
            let header = FrameHeader::decode(&header).unwrap();
            assert_eq!(header.request_id, 10);
            let mut payload = vec![0; header.payload_len as usize];
            output.read_exact(&mut payload).await.unwrap();
            assert_eq!(
                response(&Frame { header, payload }).unwrap(),
                Response::Pong {}
            );
        }
        (result, bytes)
    };
    let (_, result) = timeout(Duration::from_secs(1), async {
        tokio::join!(responder, operation)
    })
    .await
    .unwrap();
    result
}

#[tokio::test]
async fn manufacturing_reads_control_then_exact_raw_bytes_and_next_frame() {
    let record = manufacturing_record(b"abcdef");
    let header = FrameHeader::new(FrameKind::ManufacturingChunk, 9, 3)
        .unwrap()
        .encode()
        .unwrap();
    let (result, bytes) =
        manufacturing_reply(&record, 3, 3, chunk_metadata(&record, 3, 3), header, b"def").await;
    result.unwrap();
    assert_eq!(bytes, b"def");
    let header = FrameHeader::new(FrameKind::ManufacturingChunk, 9, 2)
        .unwrap()
        .encode()
        .unwrap();
    let (result, bytes) =
        manufacturing_reply(&record, 3, 3, chunk_metadata(&record, 3, 2), header, b"de").await;
    result.unwrap();
    assert_eq!(bytes, b"de");
}

#[tokio::test]
async fn inconsistent_manufacturing_metadata_is_drained_and_nonfatal() {
    let record = manufacturing_record(b"abc");
    for response in [
        ManufacturingResponse::ArtifactChunk {
            hash: spiling_contracts::geometry::SourceHash::from_bytes(b"wrong"),
            offset: 0,
            total_bytes: 3,
            byte_count: 3,
        },
        ManufacturingResponse::ArtifactChunk {
            hash: record.hash.clone(),
            offset: 1,
            total_bytes: 3,
            byte_count: 3,
        },
        ManufacturingResponse::ArtifactChunk {
            hash: record.hash.clone(),
            offset: 0,
            total_bytes: 4,
            byte_count: 3,
        },
        ManufacturingResponse::ArtifactChunk {
            hash: record.hash.clone(),
            offset: 0,
            total_bytes: MAX_MANUFACTURING_BUNDLE_BYTES + 1,
            byte_count: 3,
        },
        ManufacturingResponse::ArtifactChunk {
            hash: record.hash.clone(),
            offset: 0,
            total_bytes: 3,
            byte_count: 2,
        },
        ManufacturingResponse::ArtifactChunk {
            hash: record.hash.clone(),
            offset: 0,
            total_bytes: 3,
            byte_count: 0,
        },
        ManufacturingResponse::ArtifactChunk {
            hash: record.hash.clone(),
            offset: 0,
            total_bytes: 3,
            byte_count: 4,
        },
    ] {
        let metadata = Frame::control(9, &Response::Manufacturing { response }).unwrap();
        let header = FrameHeader::new(FrameKind::ManufacturingChunk, 9, 3)
            .unwrap()
            .encode()
            .unwrap();
        let (result, bytes) = manufacturing_reply(&record, 0, 3, metadata, header, b"abc").await;
        let error = result.unwrap_err();
        assert!(matches!(
            &error,
            ClientError::Manufacturing(ManufacturingError {
                code: ManufacturingErrorCode::CorruptArtifact,
                ..
            })
        ));
        assert!(!error.is_fatal());
        assert!(bytes.is_empty());
    }
}

#[tokio::test]
async fn manufacturing_raw_size_mismatch_is_drained_before_nonfatal_error() {
    let record = manufacturing_record(b"abc");
    let header = FrameHeader::new(FrameKind::ManufacturingChunk, 9, 4)
        .unwrap()
        .encode()
        .unwrap();
    let (result, bytes) = manufacturing_reply(
        &record,
        0,
        3,
        chunk_metadata(&record, 0, 3),
        header,
        b"abcd",
    )
    .await;
    assert!(!result.unwrap_err().is_fatal());
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn manufacturing_raw_frame_identity_and_cap_fail_before_body() {
    let record = manufacturing_record(b"abc");
    let mut oversized = FrameHeader::new(FrameKind::ManufacturingChunk, 9, 0)
        .unwrap()
        .encode()
        .unwrap();
    oversized[12..16].copy_from_slice(&(MAX_BINARY_BYTES + 1).to_le_bytes());
    for header in [
        FrameHeader::new(FrameKind::ManufacturingChunk, 8, 3)
            .unwrap()
            .encode()
            .unwrap(),
        FrameHeader::new(FrameKind::Triangle, 9, 3)
            .unwrap()
            .encode()
            .unwrap(),
        oversized,
    ] {
        let (result, bytes) =
            manufacturing_reply(&record, 0, 3, chunk_metadata(&record, 0, 3), header, b"").await;
        assert!(result.unwrap_err().is_fatal());
        assert!(bytes.is_empty());
    }
}

#[tokio::test]
async fn manufacturing_typed_error_needs_no_raw_frame() {
    let (mut client, mut peer) = duplex(1024);
    let (mut output, mut input) = tokio::io::split(&mut client);
    let error = ManufacturingError::new(ManufacturingErrorCode::CorruptArtifact, "not retained");
    let metadata = Frame::control(
        9,
        &Response::Manufacturing {
            response: ManufacturingResponse::Error {
                error: error.clone(),
            },
        },
    )
    .unwrap();
    let responder = async {
        let mut header = [0; FRAME_HEADER_BYTES];
        peer.read_exact(&mut header).await.unwrap();
        let header = FrameHeader::decode(&header).unwrap();
        let mut body = vec![0; header.payload_len as usize];
        peer.read_exact(&mut body).await.unwrap();
        peer.write_all(&metadata.header.encode().unwrap())
            .await
            .unwrap();
        peer.write_all(&metadata.payload).await.unwrap();
    };
    let request = Frame::control(9, &Request::Ping {}).unwrap();
    let record = manufacturing_record(b"abc");
    let mut bytes = Vec::new();
    let (_, result) = tokio::join!(
        responder,
        exchange_manufacturing_chunk(&mut input, &mut output, &request, &record, 0, 3, &mut bytes,)
    );
    assert!(matches!(result, Err(ClientError::Manufacturing(value)) if value == error));
    assert!(bytes.is_empty());
}

#[test]
fn every_manufacturing_error_code_is_typed_and_nonfatal() {
    use spiling_contracts::geometry::JobError;
    for code in [
        ManufacturingErrorCode::InvalidSpecification,
        ManufacturingErrorCode::UnsupportedCapability,
        ManufacturingErrorCode::UnsupportedGeometry,
        ManufacturingErrorCode::NoIntent,
        ManufacturingErrorCode::EmptyProject,
        ManufacturingErrorCode::StaleRevision,
        ManufacturingErrorCode::VerificationFailed,
        ManufacturingErrorCode::ResourceLimit,
        ManufacturingErrorCode::Cancelled,
        ManufacturingErrorCode::Io,
        ManufacturingErrorCode::CorruptArtifact,
        ManufacturingErrorCode::ReadOnly,
        ManufacturingErrorCode::Busy,
    ] {
        let error = ManufacturingError::new(code, "typed failure");
        assert!(!ClientError::Manufacturing(error.clone()).is_fatal());
        let job_error = JobError::Manufacturing { error };
        assert_eq!(
            serde_json::from_slice::<JobError>(&serde_json::to_vec(&job_error).unwrap()).unwrap(),
            job_error,
        );
    }
}

#[tokio::test]
async fn manufacturing_exchange_waits_for_the_second_frame_and_rejects_truncation() {
    for stalled in [false, true] {
        let (mut client, mut peer) = duplex(1024);
        let (mut output, mut input) = tokio::io::split(&mut client);
        let record = manufacturing_record(b"abc");
        let metadata = chunk_metadata(&record, 0, 3);
        let responder = async {
            let mut header = [0; FRAME_HEADER_BYTES];
            peer.read_exact(&mut header).await.unwrap();
            let header = FrameHeader::decode(&header).unwrap();
            let mut body = vec![0; header.payload_len as usize];
            peer.read_exact(&mut body).await.unwrap();
            peer.write_all(&metadata.header.encode().unwrap())
                .await
                .unwrap();
            peer.write_all(&metadata.payload).await.unwrap();
            if stalled {
                std::future::pending::<()>().await;
            } else {
                let header = FrameHeader::new(FrameKind::ManufacturingChunk, 9, 3)
                    .unwrap()
                    .encode()
                    .unwrap();
                peer.write_all(&header).await.unwrap();
                peer.write_all(b"a").await.unwrap();
                peer.shutdown().await.unwrap();
            }
        };
        let request = Frame::control(9, &Request::Ping {}).unwrap();
        let mut bytes = Vec::new();
        let operation = exchange_manufacturing_chunk(
            &mut input,
            &mut output,
            &request,
            &record,
            0,
            3,
            &mut bytes,
        );
        let result = timeout(Duration::from_millis(50), async {
            tokio::join!(responder, operation)
        })
        .await;
        if stalled {
            assert!(result.is_err());
        } else {
            let (_, result) = result.unwrap();
            assert!(matches!(result, Err(ClientError::Io(error))
                if error.kind() == io::ErrorKind::UnexpectedEof));
        }
    }
}

#[test]
fn fully_received_corrupt_bundle_hash_utf8_and_schema_are_nonfatal() {
    for bytes in [
        b"not JSON".as_slice(),
        b"\xff".as_slice(),
        br#"{"schema_version":1,"unknown":true}"#.as_slice(),
    ] {
        let record = manufacturing_record(bytes);
        let error = decode_manufacturing_artifact(bytes, &record).unwrap_err();
        assert!(matches!(&error, ClientError::Manufacturing(_)));
        assert!(!error.is_fatal());
    }
    let record = manufacturing_record(b"abc");
    for bytes in [b"def".as_slice(), b"ab".as_slice()] {
        let error = decode_manufacturing_artifact(bytes, &record).unwrap_err();
        assert!(matches!(
            &error,
            ClientError::Manufacturing(ManufacturingError {
                code: ManufacturingErrorCode::CorruptArtifact,
                ..
            })
        ));
        assert!(!error.is_fatal());
    }
}

#[test]
fn manufacturing_job_results_use_the_shared_job_contract() {
    use spiling_contracts::{
        geometry::JobResult,
        manufacturing::VerificationReport,
        project::{ProjectId, ProjectInfo, ProjectRevision},
    };
    let record = manufacturing_record(b"abc");
    let results = [
        JobResult::ManufacturingCompiled {
            info: ProjectInfo {
                project_id: ProjectId::new(),
                revision: ProjectRevision(1),
                saved_revision: None,
                path_label: None,
                dirty: true,
                save_uncertain: false,
                read_only: false,
                recovered_previous: false,
                can_undo: true,
                can_redo: false,
            },
            record: record.clone(),
        },
        JobResult::ManufacturingVerified {
            record,
            report: VerificationReport {
                verified: true,
                coverage: vec!["software replay".into()],
                limitations: vec!["not machine ready".into()],
                deposition_segments: 1,
                travel_segments: 0,
                deposited_volume_mm3: 1.0,
                filament_length_mm: 1.0,
                max_position_error_mm: 0.0,
                max_extrusion_error_mm: 0.0,
            },
        },
    ];
    for result in results {
        assert_eq!(
            serde_json::from_slice::<JobResult>(&serde_json::to_vec(&result).unwrap()).unwrap(),
            result,
        );
    }
}

#[cfg(test)]
mod real;
