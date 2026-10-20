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
fn geometry_only_native_scene_succeeds_without_fabricated_display_outputs() {
    let mut op = operation();
    let mut meta = native_metadata(&op).unwrap();
    meta.state = rpc::NativeOperationState::NativeSucceeded as i32;
    meta.completed_units = meta.total_units;
    meta.end_time = meta.update_time;
    let summary = SceneSummary {
        session_id: SessionId::parse(meta.session_id.clone()).unwrap(),
        revision: SceneRevision(1),
        definition_count: 1,
        occurrence_count: 1,
        bounds_mm: Some(AabbMm::new([0.0; 3], [1.0; 3]).unwrap()),
        unique_mesh_bytes: 0,
    };
    let result = rpc::NativeOperationResult::try_from(JobResult::Scene {
        summary: summary.clone(),
    })
    .unwrap();
    op.done = true;
    op.result = Some(google::longrunning::operation::Result::Response(
        prost_types::Any {
            type_url: NATIVE_RESULT_TYPE.into(),
            value: result.encode_to_vec(),
        },
    ));
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    let view = native_operation_view(&op).unwrap();
    assert!(view.outputs.is_empty());
    assert!(matches!(view.result, Some(JobResult::Scene { summary: actual }) if actual == summary));
    meta.outputs.push(rpc::Artifact {
        name: format!("artifacts/{}", "a".repeat(64)),
        sha256: "a".repeat(64),
        size_bytes: 152,
        media_type: "application/x-spiling-mesh".into(),
    });
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
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
fn program_coordinate_conditioning_keeps_native_request_and_operation_error_identity() {
    use spiling_contracts::manufacturing::{ManufacturingError, ManufacturingErrorCode};
    let expected = JobError::Manufacturing {
        error: ManufacturingError::new(
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
            "Cartesian XYZ6 gap compensation is not finite",
        ),
    };
    let request = native::manufacturing_request_status(
        "ProgramCoordinateConditioningFailure: Cartesian XYZ6 gap compensation is not finite",
    );
    assert_eq!(request.code, 9);
    assert_eq!(domain_error(&request).unwrap(), expected);
    let operation = domain_status(&expected);
    assert_eq!(operation.code, 9);
    assert_eq!(domain_error(&operation).unwrap(), expected);
    let detail = rpc::DomainErrorDetail::decode(operation.details[0].value.as_slice()).unwrap();
    let Some(rpc::domain_error_detail::Error::Manufacturing(detail)) = detail.error else {
        panic!("conditioning error must retain its manufacturing domain");
    };
    assert_eq!(detail.code, "program_coordinate_conditioning_failure");
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
        surface_overlays: spiling_contracts::surface::SurfaceOverlays::empty(),
        process_document: spiling_contracts::process::ProcessDocument::empty(),
        resolved_process: vec![],
        appearance_study: spiling_contracts::appearance::AppearanceStudy::default().info(),
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
        surface_overlays: spiling_contracts::surface::SurfaceOverlays::empty(),
        process_document: spiling_contracts::process::ProcessDocument::empty(),
        resolved_process: vec![],
        appearance_study: spiling_contracts::appearance::AppearanceStudy::default().info(),
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

#[test]
fn surface_commands_have_checked_typed_roundtrips_and_no_missing_kind_defaults() {
    use spiling_contracts::surface::*;
    let session_id = SessionId::new();
    let face_ref = FaceRef {
        session_id: session_id.clone(),
        scene_revision: SceneRevision(12),
        occurrence_id: OccurrenceId::new(1).unwrap(),
        definition_id: DefinitionId::from_source_sha256(&[1; 32]),
        face_id: FaceId::from_step_entity(9007199254740993).unwrap(),
    };
    let commands = [
        ProjectCommand::AddSurfaceFinish {
            session_id: session_id.clone(),
            base_revision: ProjectRevision(8),
            face_ref: face_ref.clone(),
            scope: SurfaceScopeKind::Occurrence,
            finish: SurfaceFinish::VerticalRibs {
                depth_mm: 0.1,
                pitch_mm: 2.0,
                edge_margin_mm: 0.8,
                phase_mm: 0.2,
            },
        },
        ProjectCommand::AddSurfaceFinish {
            session_id: session_id.clone(),
            base_revision: ProjectRevision(8),
            face_ref,
            scope: SurfaceScopeKind::Definition,
            finish: SurfaceFinish::CylindricalRibs {
                depth_mm: 0.1,
                rib_count: 16,
                phase_fraction: 0.25,
            },
        },
        ProjectCommand::RemoveSurfaceFinish {
            session_id: session_id.clone(),
            base_revision: ProjectRevision(9),
            layer_id: SurfaceLayerId::new(7).unwrap(),
        },
        ProjectCommand::SetSurfaceFinishEnabled {
            session_id,
            base_revision: ProjectRevision(10),
            layer_id: SurfaceLayerId::new(7).unwrap(),
            enabled: false,
        },
    ];
    for command in commands {
        let native::ProjectCall::Execute(wire) = native::project_call(command.clone(), "").unwrap()
        else {
            panic!("surface command must execute");
        };
        assert_eq!(ProjectCommand::try_from(wire.clone()).unwrap(), command);
        if let Some(rpc::project_request::Command::AddSurfaceFinish(add)) = wire.command {
            for scope in [0, 99] {
                let mut malformed = add.clone();
                malformed.scope = scope;
                assert!(
                    ProjectCommand::try_from(rpc::ProjectRequest {
                        parent: wire.parent.clone(),
                        command: Some(rpc::project_request::Command::AddSurfaceFinish(malformed))
                    })
                    .is_err()
                );
            }
            let mut malformed = add;
            malformed.finish = None;
            assert!(
                ProjectCommand::try_from(rpc::ProjectRequest {
                    parent: wire.parent,
                    command: Some(rpc::project_request::Command::AddSurfaceFinish(malformed))
                })
                .is_err()
            );
        }
    }
    let info = ProjectInfo {
        project_id: ProjectId::new(),
        revision: ProjectRevision(0),
        saved_revision: None,
        path_label: None,
        dirty: false,
        save_uncertain: false,
        read_only: false,
        recovered_previous: false,
        can_undo: false,
        can_redo: false,
        surface_overlays: SurfaceOverlays::empty(),
        process_document: spiling_contracts::process::ProcessDocument::empty(),
        resolved_process: vec![],
        appearance_study: spiling_contracts::appearance::AppearanceStudy::default().info(),
    };
    let mut wire: rpc::ProjectInfo = info.try_into().unwrap();
    wire.surface_overlays = None;
    assert!(ProjectInfo::try_from(wire).is_err());
}

fn canonical_image_record() -> spiling_contracts::surface::SurfaceImageRecord {
    let bytes = [b"P5\n2 2\n255\n".as_slice(), &[0, 64, 128, 255]].concat();
    spiling_contracts::surface::SurfaceImageRecord {
        image_hash: SourceHash::from_bytes(&bytes),
        width: 2,
        height: 2,
        byte_count: bytes.len() as u64,
        source_hash: SourceHash::from_bytes(b"original png"),
        source_name: "height.png".into(),
    }
}
fn canonical_image_parameters() -> spiling_contracts::surface::ImageReliefParameters {
    spiling_contracts::surface::ImageReliefParameters {
        depth_mm: 1.0,
        u_repeats: 3,
        u_phase_fraction: 0.125,
        z_period_mm: 2.0,
        z_phase_mm: 0.25,
        edge_margin_mm: 0.8,
        vertical_error_mm: 0.025,
    }
}
#[test]
fn image_operations_are_typed_revision_bound_and_reject_malformed_wrapping() {
    use spiling_contracts::surface::*;
    let session_id = SessionId::new();
    let command = ProjectCommand::ImportSurfaceImageFinish {
        session_id: session_id.clone(),
        base_revision: ProjectRevision(9),
        reference: FaceRef {
            session_id: session_id.clone(),
            scene_revision: SceneRevision(12),
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: DefinitionId::from_source_sha256(&[1; 32]),
            face_id: FaceId::from_step_entity(9007199254740993).unwrap(),
        },
        scope: SurfaceScopeKind::Occurrence,
        source: NativePath::from_os_str(std::ffi::OsStr::new("height.png")).unwrap(),
        parameters: canonical_image_parameters(),
    };
    let id = SessionId::new();
    let native::ProjectCall::ImportSurfaceImageFinish(request) =
        native::project_call(command.clone(), id.as_str()).unwrap()
    else {
        panic!("typed image import call required")
    };
    assert_eq!(request.request_id, id.as_str());
    assert_eq!(request.base_revision, 9);
    assert_eq!(ProjectCommand::try_from(request.clone()).unwrap(), command);
    for field in 0..6 {
        let mut malformed = request.clone();
        match field {
            0 => malformed.scope = 0,
            1 => malformed.scope = 99,
            2 => malformed.parameters = None,
            3 => {
                malformed.reference.as_mut().unwrap().session_id = SessionId::new().as_str().into()
            }
            4 => malformed.source = None,
            _ => malformed.request_id = "not-a-uuid".into(),
        }
        assert!(ProjectCommand::try_from(malformed).is_err());
    }
    let get = ProjectCommand::GetSurfaceImage {
        session_id,
        base_revision: ProjectRevision(9),
        image_hash: canonical_image_record().image_hash,
    };
    let native::ProjectCall::GetSurfaceImage(request) =
        native::project_call(get.clone(), id.as_str()).unwrap()
    else {
        panic!("typed canonical image retrieval required")
    };
    assert_eq!(ProjectCommand::try_from(request.clone()).unwrap(), get);
    let mut malformed = request;
    malformed.image_hash = "A".repeat(64);
    assert!(ProjectCommand::try_from(malformed).is_err());
}
#[test]
fn prepared_image_requires_exact_descriptor_and_ack_published_captured_revision() {
    use spiling_contracts::surface::*;
    let record = canonical_image_record();
    let resource = ArtifactView {
        name: format!("artifacts/{}", record.image_hash.as_str()),
        size_bytes: record.byte_count.to_string(),
        sha256: record.image_hash.as_str().into(),
        media_type: SURFACE_IMAGE_MEDIA_TYPE.into(),
    };
    let result = JobResult::SurfaceImagePrepared {
        project_id: ProjectId::new(),
        captured_revision: ProjectRevision(9),
        record: record.clone(),
        resource: resource.clone(),
    };
    let wire: rpc::NativeOperationResult = result.clone().try_into().unwrap();
    assert_eq!(JobResult::try_from(wire.clone()).unwrap(), result);
    for field in 0..5 {
        let mut malformed = wire.clone();
        let rpc::native_operation_result::Result::SurfaceImagePrepared(prepared) =
            malformed.result.as_mut().unwrap()
        else {
            unreachable!()
        };
        match field {
            0 => prepared.resource.as_mut().unwrap().size_bytes += 1,
            1 => prepared.resource.as_mut().unwrap().media_type = "image/png".into(),
            2 => {
                prepared.resource.as_mut().unwrap().sha256 =
                    SourceHash::from_bytes(b"other pixels").as_str().into()
            }
            3 => prepared.record.as_mut().unwrap().width = 3,
            _ => prepared.project_id = "not-a-uuid".into(),
        }
        assert!(JobResult::try_from(malformed).is_err());
    }
    let mut op = operation();
    let mut meta = native_metadata(&op).unwrap();
    meta.state = rpc::NativeOperationState::NativeSucceeded as i32;
    meta.completed_units = meta.total_units;
    meta.captured_project_revision = 9;
    meta.end_time = meta.update_time;
    op.done = true;
    op.result = Some(google::longrunning::operation::Result::Response(
        prost_types::Any {
            type_url: NATIVE_RESULT_TYPE.into(),
            value: wire.encode_to_vec(),
        },
    ));
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    assert!(native_operation_view(&op).is_err()); // A staged result is not a published resource.
    meta.outputs =
        vec![native::artifact_from_view(resource, MAX_SURFACE_IMAGE_BYTES as u64).unwrap()];
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    assert_eq!(native_operation_view(&op).unwrap().result, Some(result));
    meta.captured_project_revision = 10;
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    assert!(native_operation_view(&op).is_err());
}
#[test]
fn imported_image_receipt_matches_authoritative_metadata_and_new_layer() {
    use spiling_contracts::surface::*;
    let image = canonical_image_record();
    let layer_id = SurfaceLayerId::new(1).unwrap();
    let info = ProjectInfo {
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
        surface_overlays: SurfaceOverlays {
            schema_version: SURFACE_SCHEMA_VERSION,
            images: vec![image.clone()],
            layers: vec![SurfaceLayer {
                layer_id,
                enabled: true,
                scope: SurfaceScope::Definition {
                    definition_id: DefinitionId::from_source_sha256(&[1; 32]),
                },
                face_id: FaceId::from_step_entity(7).unwrap(),
                finish: SurfaceFinish::ImageRelief {
                    image_hash: image.image_hash.clone(),
                    parameters: canonical_image_parameters(),
                },
            }],
        },
        process_document: spiling_contracts::process::ProcessDocument::empty(),
        resolved_process: vec![],
        appearance_study: spiling_contracts::appearance::AppearanceStudy::default().info(),
    };
    let result = JobResult::SurfaceImageImported {
        info,
        image,
        layer_id,
    };
    let wire: rpc::NativeOperationResult = result.clone().try_into().unwrap();
    assert_eq!(JobResult::try_from(wire.clone()).unwrap(), result);
    for field in 0..5 {
        let mut malformed = wire.clone();
        let rpc::native_operation_result::Result::SurfaceImageImported(imported) =
            malformed.result.as_mut().unwrap()
        else {
            unreachable!()
        };
        match field {
            0 => imported.layer_id = 2,
            1 => imported.image.as_mut().unwrap().source_name = "rewritten.png".into(),
            2 => imported
                .info
                .as_mut()
                .unwrap()
                .surface_overlays
                .as_mut()
                .unwrap()
                .images
                .clear(),
            3 => imported.info.as_mut().unwrap().read_only = true,
            _ => {
                imported
                    .info
                    .as_mut()
                    .unwrap()
                    .surface_overlays
                    .as_mut()
                    .unwrap()
                    .layers[0]
                    .enabled = false
            }
        }
        assert!(JobResult::try_from(malformed).is_err());
    }
    let mut op = operation();
    let mut meta = native_metadata(&op).unwrap();
    meta.state = rpc::NativeOperationState::NativeSucceeded as i32;
    meta.completed_units = meta.total_units;
    meta.end_time = meta.update_time;
    op.done = true;
    op.result = Some(google::longrunning::operation::Result::Response(
        prost_types::Any {
            type_url: NATIVE_RESULT_TYPE.into(),
            value: wire.encode_to_vec(),
        },
    ));
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    assert_eq!(native_operation_view(&op).unwrap().result, Some(result));
    meta.captured_project_revision = 1;
    op.metadata.as_mut().unwrap().value = meta.encode_to_vec();
    assert!(native_operation_view(&op).is_err());
}
