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
            process_resolution: Vec::new(),
            native_geometry: NativeGeometryCoverageSummary {
                requested_shape_error_mm: 0.1,
                achieved_shape_error_mm: 0.0,
                height_analysis_count: 1,
                slab_count: 1,
                certified_layer_count: 1,
                event_count: 2,
                geometry_work: 1,
                evidence: PreflightEvidence::Certified,
                skin_coverage: SkinCoverageScope::ScheduledMaterialDistance,
                support_clearance: SupportClearanceScope::ContinuousTargetOuterOccupancy,
                source_feature_coverage: NativeFeatureCoverageSummary {
                    disjoint_source_layer_count: 1,
                    invariant_source_union_layer_count: 0,
                    native_interface_complex_layer_count: 0,
                    owned_critical_slab_count: 0,
                },
            },
            paths: 1,
            deposition_segments: 1,
            deposited_volume_mm3: 1.0,
            filament_length_mm: 1.0,
            support_deposition_segments: 0,
            support_deposited_volume_mm3: 0.0,
            support_filament_length_mm: 0.0,
            surface_resolution: spiling_contracts::manufacturing::SurfaceResolutionTotals::empty(),
            software_only: true,
            routing: None,
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
        engine_build: spiling_contracts::ENGINE_BUILD_ID.into(),
        kernel: spiling_contracts::geometry::KERNEL_NAME.into(),
        geometry_capabilities: vec!["source_backed_geometry".into()],
        project_capabilities: vec![spiling_contracts::PROJECT_FORMAT_CAPABILITY.into()],
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

#[test]
fn surface_commands_use_typed_project_execute_and_preserve_all_authored_fields() {
    use prost::Message;
    use spiling_contracts::{
        native::{ProjectCall, project_call},
        project::{ProjectCommand, ProjectRevision},
        surface::{SurfaceFinish, SurfaceLayerId, SurfaceScopeKind},
    };
    let session = SessionId::new();
    let layer_id = SurfaceLayerId::new(17).unwrap();
    let commands = [
        ProjectCommand::AddSurfaceFinish {
            session_id: session.clone(),
            base_revision: ProjectRevision(43),
            face_ref: FaceRef {
                session_id: session.clone(),
                scene_revision: SceneRevision(9),
                occurrence_id: OccurrenceId::new(2).unwrap(),
                definition_id: DefinitionId::parse("a".repeat(64)).unwrap(),
                face_id: FaceId::parse("step:24").unwrap(),
            },
            scope: SurfaceScopeKind::Occurrence,
            finish: SurfaceFinish::VerticalRibs {
                depth_mm: 0.1,
                pitch_mm: 2.0,
                edge_margin_mm: 0.8,
                phase_mm: 0.25,
            },
        },
        ProjectCommand::AddSurfaceFinish {
            session_id: session.clone(),
            base_revision: ProjectRevision(43),
            face_ref: FaceRef {
                session_id: session.clone(),
                scene_revision: SceneRevision(9),
                occurrence_id: OccurrenceId::new(2).unwrap(),
                definition_id: DefinitionId::parse("a".repeat(64)).unwrap(),
                face_id: FaceId::parse("step:24").unwrap(),
            },
            scope: SurfaceScopeKind::Definition,
            finish: SurfaceFinish::CylindricalRibs {
                depth_mm: 0.1,
                rib_count: 12,
                phase_fraction: 0.25,
            },
        },
        ProjectCommand::RemoveSurfaceFinish {
            session_id: session.clone(),
            base_revision: ProjectRevision(44),
            layer_id,
        },
        ProjectCommand::SetSurfaceFinishEnabled {
            session_id: session,
            base_revision: ProjectRevision(45),
            layer_id,
            enabled: false,
        },
    ];
    for command in commands {
        let expected = serde_json::to_value(&command).unwrap();
        let ProjectCall::Execute(request) =
            project_call(command, &Uuid::new_v4().to_string()).unwrap()
        else {
            panic!("surface commands must use Projects.Execute");
        };
        let received = rpc::ProjectRequest::decode(request.encode_to_vec().as_slice()).unwrap();
        let decoded = ProjectCommand::try_from(received).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
    }
}

#[test]
fn supervised_readiness_rejects_mixed_surface_builds_and_missing_cutover_capabilities() {
    let mut hello = Hello {
        instance_id: Uuid::new_v4().to_string(),
        engine_build: spiling_contracts::ENGINE_BUILD_ID.into(),
        kernel: spiling_contracts::geometry::KERNEL_NAME.into(),
        geometry_capabilities: vec![
            "step_native_trimmed_v2".into(),
            "native_geometry_queries_v2".into(),
            "native_bed_contact_v1".into(),
            "native_vertical_rectangular_wall_v1".into(),
            "native_vertical_cylinder_face_v1".into(),
        ],
        project_capabilities: vec![
            spiling_contracts::PROJECT_FORMAT_CAPABILITY.into(),
            "source_face_surface_finish_v3".into(),
            "surface_image_assets_v1".into(),
            "source_bound_process_v3".into(),
            "appearance_palette_study_c1a".into(),
            "optical_construction_reference_study_v1".into(),
        ],
        manufacturing_capabilities: vec![
            spiling_contracts::MANUFACTURING_SCHEMA_CAPABILITY.into(),
            "mapped_image_relief_v1".into(),
            "varying_z_material_domains_v1".into(),
            "bounded_vertical_rib_construction_v1".into(),
            "bounded_cylindrical_rib_construction_v1".into(),
            "fixed_dual_material_routing_v1".into(),
            "whole_occurrence_layer_cycles_v1".into(),
        ],
        max_control_bytes: MAX_CONTROL_BYTES,
        max_binary_bytes: MAX_ARTIFACT_BYTES,
        pid: 123,
        session_id: SessionId::new(),
        kernel_identity: KernelIdentity {
            name: spiling_contracts::geometry::KERNEL_NAME.into(),
            version: "test-kernel-version".into(),
            revision: "test-kernel-revision".into(),
        },
        geometry_limits: GeometryLimits::FROZEN,
    };
    super::validate_engine_cutover(&hello).unwrap();
    hello.kernel = "unsupported-kernel".into();
    assert!(super::validate_engine_cutover(&hello).is_err());
    hello.kernel = spiling_contracts::geometry::KERNEL_NAME.into();
    hello.kernel_identity.name = "unsupported-kernel".into();
    assert!(super::validate_engine_cutover(&hello).is_err());
    hello.kernel_identity.name = spiling_contracts::geometry::KERNEL_NAME.into();
    hello.engine_build = env!("CARGO_PKG_VERSION").into();
    assert!(super::validate_engine_cutover(&hello).is_err());
    hello.engine_build = spiling_contracts::ENGINE_BUILD_ID.into();
    hello.project_capabilities[0] = "unsupported_project_format".into();
    assert!(super::validate_engine_cutover(&hello).is_err());
    hello.project_capabilities[0] = spiling_contracts::PROJECT_FORMAT_CAPABILITY.into();
    hello.manufacturing_capabilities[0] = "unsupported_manufacturing_schema".into();
    assert!(super::validate_engine_cutover(&hello).is_err());
    hello.manufacturing_capabilities[0] = spiling_contracts::MANUFACTURING_SCHEMA_CAPABILITY.into();
    hello.project_capabilities[1] = "source_face_surface_finish_v1".into();
    assert!(super::validate_engine_cutover(&hello).is_err());
    hello.project_capabilities[1] = "source_face_surface_finish_v3".into();
    hello.project_capabilities.pop();
    assert!(super::validate_engine_cutover(&hello).is_err());
    hello
        .project_capabilities
        .push("appearance_palette_study_c1a".into());
    hello.geometry_capabilities.pop();
    assert!(super::validate_engine_cutover(&hello).is_err());
    hello
        .geometry_capabilities
        .push("native_vertical_cylinder_face_v1".into());
    hello.manufacturing_capabilities.pop();
    assert!(super::validate_engine_cutover(&hello).is_err());
}
