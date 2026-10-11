// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::native::decode_manufacturing_artifact;
use super::{
    ClientError, Duration, EngineClient, Hello, MAX_ARTIFACT_BYTES, MAX_CONTROL_BYTES, Operation,
    Path, Uuid, timeout,
};
use spiling_contracts::{geometry::*, manufacturing::*, rpc};

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
fn structured_status_preserves_domain_errors_and_rejects_inconsistent_details() {
    use prost::Message;
    let error = JobError::Project {
        error: spiling_contracts::project::ProjectError::new(
            spiling_contracts::project::ProjectErrorCode::ReadOnly,
            "read-only project",
        ),
    };
    let details = spiling_contracts::native::domain_status(&error);
    let status = tonic::Status::with_details(
        tonic::Code::from_i32(details.code),
        details.message.clone(),
        details.encode_to_vec().into(),
    );
    assert!(
        matches!(super::native::rpc_error(status), ClientError::Project(error) if error.code == spiling_contracts::project::ProjectErrorCode::ReadOnly)
    );
    let status = tonic::Status::with_details(
        tonic::Code::Internal,
        "wrong",
        details.encode_to_vec().into(),
    );
    assert!(matches!(
        super::native::rpc_error(status),
        ClientError::Protocol(_)
    ));
}

#[test]
fn call_deadlines_and_typed_failures_do_not_cancel_work_or_disconnect_owner() {
    assert!(!ClientError::Timeout.is_fatal());
    assert!(!ClientError::Rpc(tonic::Status::deadline_exceeded("observation expired")).is_fatal());
    assert!(!ClientError::InvalidRequest("unwritten request".into()).is_fatal());
    assert!(
        !ClientError::Geometry(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "invalid input"
        ))
        .is_fatal()
    );
    assert!(ClientError::Protocol("malformed metadata".into()).is_fatal());
}

#[test]
fn native_operation_views_reject_wrong_any_session_and_terminal_invariants() {
    use prost::Message;
    let session = SessionId::new();
    let metadata = rpc::NativeOperationMetadata {
        state: rpc::NativeOperationState::NativeQueued as i32,
        state_version: 1,
        session_id: session.as_str().into(),
        captured_scene_revision: 3,
        captured_project_revision: 5,
        phase: "queued".into(),
        completed_units: 0,
        total_units: 1,
        create_time: Some(prost_types::Timestamp {
            seconds: 10,
            nanos: 0,
        }),
        update_time: Some(prost_types::Timestamp {
            seconds: 10,
            nanos: 0,
        }),
        ..Default::default()
    };
    let operation = Operation {
        name: format!(
            "sessions/{}/operations/{}",
            session.as_str(),
            Uuid::new_v4()
        ),
        metadata: Some(prost_types::Any {
            type_url: spiling_contracts::NATIVE_METADATA_TYPE.into(),
            value: metadata.encode_to_vec(),
        }),
        done: false,
        result: None,
    };
    let view = spiling_contracts::native_operation_view(&operation).unwrap();
    assert_eq!(view.captured_scene_revision, SceneRevision(3));
    assert_eq!(
        view.captured_project_revision,
        spiling_contracts::project::ProjectRevision(5)
    );
    let mut invalid = operation.clone();
    invalid.metadata.as_mut().unwrap().type_url = spiling_contracts::METADATA_TYPE.into();
    assert!(spiling_contracts::native_operation_view(&invalid).is_err());
    let mut invalid = operation.clone();
    invalid.done = true;
    assert!(spiling_contracts::native_operation_view(&invalid).is_err());
    let mut invalid = operation.clone();
    invalid.name = format!(
        "sessions/{}/operations/{}",
        SessionId::new().as_str(),
        Uuid::new_v4()
    );
    assert!(spiling_contracts::native_operation_view(&invalid).is_err());
}

#[test]
fn checked_readiness_rejects_missing_native_identity_in_otherwise_current_info() {
    let info = rpc::EngineInfo {
        instance_id: Uuid::new_v4().to_string(),
        engine_build: env!("CARGO_PKG_VERSION").into(),
        kernel: "monstertruck".into(),
        geometry_capabilities: vec!["source_backed_geometry".into()],
        project_capabilities: vec!["recoverable_projects_v2".into()],
        manufacturing_capabilities: vec!["planar_software_compile_v1".into()],
        max_message_bytes: MAX_CONTROL_BYTES,
        max_artifact_bytes: MAX_ARTIFACT_BYTES,
        pid: 123,
        session_id: SessionId::new().as_str().into(),
        ..Default::default()
    };
    assert!(Hello::try_from(info).is_err());
}

mod real;
