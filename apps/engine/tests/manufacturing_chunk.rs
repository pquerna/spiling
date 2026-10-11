// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{
    NativeOperationView, geometry::*, google::bytestream::ReadRequest, manufacturing::*,
};
use spiling_engine_client::{ClientError, EngineClient};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tonic::Code;

const ENGINE: &str = env!("CARGO_BIN_EXE_spiling-engine");

async fn completed(client: &EngineClient, name: &str) -> NativeOperationView {
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(70),
            "native operation deadline"
        );
        let operation = client.native_operation(name).await.unwrap();
        if operation.status.is_terminal() {
            assert_eq!(operation.status, JobStatus::Completed, "{operation:?}");
            assert!(operation.error.is_none(), "{operation:?}");
            return operation;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn rpc_code(error: ClientError) -> Code {
    let ClientError::Rpc(status) = error else {
        panic!("expected canonical RPC status: {error:?}");
    };
    status.code()
}

#[tokio::test]
async fn manufacturing_bytestream_rejections_leave_actual_child_healthy() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let session_id = client.hello().session_id.clone();
    let GeometryResponse::Scene { summary } = client
        .geometry(GeometryCommand::GetScene {
            session_id: session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("expected initial scene");
    };
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry/box-mm.step");
    let GeometryResponse::OperationAccepted { operation } = client
        .geometry(GeometryCommand::ImportPart {
            session_id: session_id.clone(),
            base_revision: summary.revision,
            source: NativePath::from_os_str(source.as_os_str()).unwrap(),
            initial_pose: RigidPoseMm::IDENTITY,
        })
        .await
        .unwrap()
    else {
        panic!("expected accepted native import");
    };
    completed(&client, &operation.name).await;
    let ManufacturingResponse::Status { info, .. } = client
        .manufacturing(ManufacturingCommand::Get {
            session_id: session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("expected manufacturing status");
    };
    let intent: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../fixtures/manufacturing/solid-fill.intent.json"
    ))
    .unwrap();
    let ManufacturingResponse::Status { info, .. } = client
        .manufacturing(ManufacturingCommand::SetIntent {
            session_id: session_id.clone(),
            base_revision: info.revision,
            intent: intent.into(),
        })
        .await
        .unwrap()
    else {
        panic!("expected committed explicit intent");
    };
    let ManufacturingResponse::OperationAccepted { operation } = client
        .manufacturing(ManufacturingCommand::Compile {
            session_id,
            base_revision: info.revision,
        })
        .await
        .unwrap()
    else {
        panic!("expected accepted native compilation");
    };
    let operation = completed(&client, &operation.name).await;
    let Some(JobResult::ManufacturingCompiled {
        record, resource, ..
    }) = operation.result
    else {
        panic!("expected compiled immutable bundle");
    };
    let bundle = client
        .fetch_manufacturing_bundle(&record, &resource)
        .await
        .unwrap();
    assert!(bundle.verification.verified);
    assert!(record.summary.software_only);

    // Session validation stays on the typed domain service, not immutable resource names.
    assert!(matches!(client.manufacturing(ManufacturingCommand::Get {
        session_id: SessionId::new(),
    }).await, Err(ClientError::Manufacturing(error)) if error.code == ManufacturingErrorCode::StaleRevision));
    client.ping().await.unwrap();

    // The removed max_bytes control is replaced by descriptor and aggregate bundle bounds.
    let mut empty = record.clone();
    empty.byte_count = 0;
    assert!(
        matches!(client.fetch_manufacturing_bundle(&empty, &resource).await,
        Err(ClientError::Manufacturing(error)) if error.code == ManufacturingErrorCode::CorruptArtifact)
    );
    client.ping().await.unwrap();
    let mut oversized = record.clone();
    oversized.byte_count = MAX_MANUFACTURING_BUNDLE_BYTES + 1;
    assert!(
        matches!(client.fetch_manufacturing_bundle(&oversized, &resource).await,
        Err(ClientError::Manufacturing(error)) if error.code == ManufacturingErrorCode::ResourceLimit)
    );
    client.ping().await.unwrap();

    let rpc = client.rpc();
    let cases = [
        (
            format!("artifacts/{}", SourceHash::from_bytes(b"missing").as_str()),
            0,
            1,
            Code::NotFound,
        ),
        (resource.name.clone(), -1, 1, Code::OutOfRange),
        (resource.name.clone(), 0, -1, Code::InvalidArgument),
        (
            resource.name.clone(),
            i64::from(u32::MAX),
            i64::from(MAX_MANUFACTURING_BUNDLE_BYTES),
            Code::OutOfRange,
        ),
    ];
    for (resource_name, read_offset, read_limit, expected) in cases {
        let error = rpc
            .read_artifact_range(ReadRequest {
                resource_name,
                read_offset,
                read_limit,
            })
            .await
            .expect_err("invalid ByteStream request must fail");
        assert_eq!(rpc_code(error), expected);
        client.ping().await.unwrap();
    }

    // Standard ByteStream defines zero as all remaining bytes, and clamps positive limits
    // to immutable resource size. Both still obey the bundle's aggregate allocation cap.
    let mut expected_bytes = None;
    for read_limit in [0, i64::from(MAX_MANUFACTURING_BUNDLE_BYTES) + 1] {
        let mut stream = rpc
            .read_artifact_range(ReadRequest {
                resource_name: resource.name.clone(),
                read_offset: 0,
                read_limit,
            })
            .await
            .unwrap();
        let mut bytes = Vec::with_capacity(record.byte_count as usize);
        while let Some(fragment) = stream.message().await.unwrap() {
            assert!(bytes.len() + fragment.data.len() <= record.byte_count as usize);
            bytes.extend_from_slice(&fragment.data);
        }
        assert_eq!(bytes.len(), record.byte_count as usize);
        assert!(record.hash.matches_bytes(&bytes));
        assert_eq!(decode_bundle(&bytes).unwrap(), bundle);
        if let Some(expected) = &expected_bytes {
            assert_eq!(&bytes, expected);
        } else {
            expected_bytes = Some(bytes);
        }
        client.ping().await.unwrap();
    }
    client.shutdown().await.unwrap();
    assert!(!client.status().await.unwrap());
}
