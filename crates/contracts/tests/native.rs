// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use prost::Message;
use spiling_contracts::{geometry::*, project::*, *};

fn operation() -> google::longrunning::Operation {
    let meta = rpc::NativeOperationMetadata {
        state: rpc::NativeOperationState::NativeRunning as i32,
        state_version: u64::MAX,
        session_id: "550e8400-e29b-41d4-a716-446655440000".into(),
        phase: "importing".into(),
        total_units: 1,
        create_time: Some(prost_types::Timestamp {
            seconds: 1,
            nanos: 0,
        }),
        update_time: Some(prost_types::Timestamp {
            seconds: 1,
            nanos: 0,
        }),
        ..Default::default()
    };
    google::longrunning::Operation{name:"sessions/550e8400-e29b-41d4-a716-446655440000/operations/550e8400-e29b-41d4-a716-446655440001".into(),metadata:Some(prost_types::Any{type_url:NATIVE_METADATA_TYPE.into(),value:meta.encode_to_vec()}),done:false,result:None}
}
#[test]
fn native_snapshots_validate_identity_state_timestamps_and_terminal_details() {
    let mut op = operation();
    assert_eq!(
        native_operation_view(&op).unwrap().state_version,
        u64::MAX.to_string()
    );
    let mut meta = native_metadata(&op).unwrap();
    meta.state = rpc::NativeOperationState::NativeInterrupted as i32;
    meta.end_time = meta.update_time;
    op.done = true;
    let mut status = domain_status(&JobError::Geometry {
        error: GeometryError::new(
            GeometryErrorCode::Cancelled,
            "engine restarted; native handles are not resumed",
        ),
    });
    status.code = 10;
    op.result = Some(google::longrunning::operation::Result::Error(status));
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    assert_eq!(
        native_operation_view(&op).unwrap().status,
        JobStatus::Interrupted
    );
    op.metadata.as_mut().unwrap().type_url = METADATA_TYPE.into();
    assert!(native_operation_view(&op).is_err());
    op.metadata.as_mut().unwrap().type_url = NATIVE_METADATA_TYPE.into();
    if let Some(google::longrunning::operation::Result::Error(s)) = &mut op.result {
        s.details.clear();
    }
    assert!(native_operation_view(&op).is_err());
}
#[test]
fn domain_status_keeps_codes_and_rejects_cross_domain_or_unknown_details() {
    let e = JobError::Geometry {
        error: GeometryError::new(GeometryErrorCode::InvalidPose, "invalid quaternion"),
    };
    let mut s = domain_status(&e);
    assert_eq!(domain_error(&s).unwrap(), e);
    s.details[0].type_url = RESULT_TYPE.into();
    assert!(domain_error(&s).is_err());
    let s = native::geometry_request_status("InvalidPose: invalid quaternion");
    assert!(matches!(
        domain_error(&s).unwrap(),
        JobError::Geometry {
            error: GeometryError {
                code: GeometryErrorCode::InvalidPose,
                ..
            }
        }
    ));
}
#[test]
fn artifact_limits_are_class_specific_and_sizes_are_canonical() {
    let hash = "a".repeat(64);
    let mut a = ArtifactView {
        name: format!("artifacts/{hash}"),
        sha256: hash,
        media_type: "application/x-spiling-mesh".into(),
        size_bytes: "1048576".into(),
    };
    assert_eq!(
        native::validate_artifact_view(&a, u64::from(MAX_GEOMETRY_CHUNK_BYTES)).unwrap(),
        1048576
    );
    assert!(native::validate_artifact_view(&a, u64::from(MAX_ARTIFACT_BYTES)).is_err());
    a.size_bytes = "01048576".into();
    assert!(native::validate_artifact_view(&a, u64::from(MAX_GEOMETRY_CHUNK_BYTES)).is_err());
}
#[test]
fn clean_new_and_semantically_clean_undo_project_info_are_valid() {
    let info = ProjectInfo {
        project_id: ProjectId::new(),
        revision: ProjectRevision(2),
        saved_revision: Some(ProjectRevision(1)),
        path_label: Some("project".into()),
        dirty: false,
        save_uncertain: false,
        read_only: false,
        recovered_previous: false,
        can_undo: true,
        can_redo: true,
    };
    let wire: rpc::ProjectInfo = info.clone().try_into().unwrap();
    assert_eq!(ProjectInfo::try_from(wire).unwrap(), info);
    let info = ProjectInfo {
        saved_revision: None,
        path_label: None,
        revision: ProjectRevision(0),
        can_undo: false,
        can_redo: false,
        ..info
    };
    assert!(rpc::ProjectInfo::try_from(info).is_ok());
}

#[test]
fn project_typed_dispatch_preserves_scene_revision_and_native_paths() {
    let session_id = SessionId::parse("550e8400-e29b-41d4-a716-446655440000").unwrap();
    let command = ProjectCommand::Undo {
        session_id: session_id.clone(),
        base_revision: SceneRevision(17),
    };
    let native::ProjectCall::Execute(wire) = native::project_call(command.clone(), "").unwrap()
    else {
        panic!("wrong call");
    };
    assert_eq!(ProjectCommand::try_from(wire).unwrap(), command);
    let wire = rpc::OpenProjectRequest {
        parent: format!("sessions/{}", session_id.as_str()),
        request_id: String::new(),
        base_revision: 17,
        path: Some(rpc::NativePath {
            value: Some(rpc::native_path::Value::UnixBytes(vec![b'p', 0, b'q'])),
        }),
        read_only: false,
        recover_previous: false,
        discard_changes: false,
    };
    assert!(ProjectCommand::try_from(wire).is_err());
    let malformed = rpc::NativePath {
        value: Some(rpc::native_path::Value::WindowsUtf16(rpc::Utf16Path {
            units: vec![65536],
        })),
    };
    assert!(NativePath::try_from(malformed).is_err());
    #[cfg(unix)]
    {
        let path = NativePath::UnixBytes {
            hex: "70ff71".into(),
        };
        let wire: rpc::NativePath = path.clone().try_into().unwrap();
        assert_eq!(NativePath::try_from(wire).unwrap(), path);
    }
}

#[test]
fn failed_committed_save_keeps_operation_specific_uncertain_receipt() {
    let mut op = operation();
    let info = ProjectInfo {
        project_id: ProjectId::new(),
        revision: ProjectRevision(7),
        saved_revision: Some(ProjectRevision(7)),
        path_label: Some("attached-save-as".into()),
        dirty: true,
        save_uncertain: true,
        read_only: false,
        recovered_previous: false,
        can_undo: true,
        can_redo: false,
    };
    let result = JobResult::ProjectSaved { info: info.clone() };
    let wire: rpc::NativeOperationResult = result.clone().try_into().unwrap();
    let receipt = prost_types::Any {
        type_url: NATIVE_RESULT_TYPE.into(),
        value: wire.encode_to_vec(),
    };
    let error = JobError::Project {
        error: ProjectError::new(
            ProjectErrorCode::Io,
            "manifest replaced but directory sync failed",
        ),
    };
    let mut status = domain_status(&error);
    status.details.push(receipt.clone());
    let mut meta = native_metadata(&op).unwrap();
    meta.state = rpc::NativeOperationState::NativeFailed as i32;
    meta.captured_project_revision = 7;
    meta.end_time = meta.update_time;
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    op.done = true;
    op.result = Some(google::longrunning::operation::Result::Error(
        status.clone(),
    ));
    let view = native_operation_view(&op).unwrap();
    assert_eq!(view.status, JobStatus::Failed);
    assert_eq!(view.result, Some(result.clone()));
    assert_eq!(view.error, Some(error.clone()));
    assert_eq!(native_result(&op).unwrap(), Some(result));
    assert_eq!(domain_error(&status).unwrap(), error);

    let reject = |details: Vec<prost_types::Any>| {
        let mut bad = status.clone();
        bad.details = details;
        let mut bad_op = op.clone();
        bad_op.result = Some(google::longrunning::operation::Result::Error(bad));
        assert!(native_operation_view(&bad_op).is_err());
    };
    reject(vec![
        status.details[0].clone(),
        receipt.clone(),
        receipt.clone(),
    ]);
    reject(vec![receipt.clone(), receipt.clone()]);
    let mut unknown = receipt.clone();
    unknown.type_url = RESULT_TYPE.into();
    reject(vec![status.details[0].clone(), unknown]);
    let certain = JobResult::ProjectSaved {
        info: ProjectInfo {
            save_uncertain: false,
            dirty: false,
            ..info.clone()
        },
    };
    let certain: rpc::NativeOperationResult = certain.try_into().unwrap();
    reject(vec![
        status.details[0].clone(),
        prost_types::Any {
            type_url: NATIVE_RESULT_TYPE.into(),
            value: certain.encode_to_vec(),
        },
    ]);
    reject(vec![
        status.details[0].clone(),
        prost_types::Any {
            type_url: NATIVE_RESULT_TYPE.into(),
            value: vec![255],
        },
    ]);
    let invalid_info = rpc::ProjectInfo {
        project_id: info.project_id.as_str().into(),
        revision: 7,
        saved_revision: Some(8),
        save_uncertain: true,
        dirty: true,
        ..Default::default()
    };
    let invalid_receipt = rpc::NativeOperationResult {
        result: Some(rpc::native_operation_result::Result::ProjectSaved(
            invalid_info,
        )),
    };
    reject(vec![
        status.details[0].clone(),
        prost_types::Any {
            type_url: NATIVE_RESULT_TYPE.into(),
            value: invalid_receipt.encode_to_vec(),
        },
    ]);
    let wrong: rpc::NativeOperationResult = JobResult::Section {
        artifact_id: ArtifactId::new(1).unwrap(),
    }
    .try_into()
    .unwrap();
    reject(vec![
        status.details[0].clone(),
        prost_types::Any {
            type_url: NATIVE_RESULT_TYPE.into(),
            value: wrong.encode_to_vec(),
        },
    ]);
    let mut wrong_domain = domain_status(&JobError::Geometry {
        error: GeometryError::new(
            GeometryErrorCode::SourceIo,
            "manifest replaced but directory sync failed",
        ),
    });
    wrong_domain.details.push(receipt);
    let mut bad_op = op.clone();
    bad_op.result = Some(google::longrunning::operation::Result::Error(wrong_domain));
    assert!(native_operation_view(&bad_op).is_err());
    meta.state = rpc::NativeOperationState::NativeInterrupted as i32;
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    assert!(
        native_operation_view(&op).is_err(),
        "receipt may accompany Failed only"
    );
}
