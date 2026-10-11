// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Deliberate deep checks against a built native engine, not a simulated pipe peer.
use super::*;
use spiling_contracts::project::{ProjectCommand, ProjectResponse};
use spiling_contracts::{
    display::{validate_mesh_chunk, validate_mesh_manifest},
    project::ProjectErrorCode,
};
use std::path::PathBuf;
fn engine() -> PathBuf {
    std::env::var_os("SPILING_TEST_ENGINE")
        .map(PathBuf::from)
        .expect("set SPILING_TEST_ENGINE to the built native spiling-engine")
}
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name)
}
async fn scene(client: &mut EngineClient) -> SceneSummary {
    let command = GeometryCommand::GetScene {
        session_id: client.hello().session_id.clone(),
    };
    match client.geometry(command).await.unwrap() {
        GeometryResponse::Scene { summary } => summary,
        response => panic!("unexpected {response:?}"),
    }
}
async fn import(client: &mut EngineClient, path: &Path) -> spiling_contracts::NativeOperationView {
    let summary = scene(client).await;
    let command = GeometryCommand::ImportPart {
        session_id: summary.session_id.clone(),
        base_revision: summary.revision,
        source: NativePath::from_os_str(path.as_os_str()).unwrap(),
        initial_pose: RigidPoseMm::IDENTITY,
    };
    let operation = match client.geometry(command).await.unwrap() {
        GeometryResponse::OperationAccepted { operation } => operation,
        response => panic!("unexpected {response:?}"),
    };
    timeout(Duration::from_secs(75), async {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let current = client
                .native_operation(operation.name.clone())
                .await
                .unwrap();
            if current.status.is_terminal() {
                return current;
            }
        }
    })
    .await
    .unwrap()
}
async fn records(client: &mut EngineClient) -> (DefinitionRecord, OccurrenceRecord) {
    let summary = scene(client).await;
    let defs = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: summary.session_id.clone(),
            revision: summary.revision,
            kind: ScenePageKind::Definitions,
            offset: 0,
        })
        .await
        .unwrap();
    let occs = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: summary.session_id,
            revision: summary.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap();
    match (defs, occs) {
        (
            GeometryResponse::ScenePage { definitions, .. },
            GeometryResponse::ScenePage { occurrences, .. },
        ) => (definitions[0].clone(), occurrences[0].clone()),
        response => panic!("unexpected {response:?}"),
    }
}
async fn face_rows(client: &mut EngineClient, definition: &DefinitionRecord) -> Vec<FaceIndexRow> {
    match client
        .geometry(GeometryCommand::GetFaceIndexPage {
            session_id: client.hello().session_id.clone(),
            definition_id: definition.definition_id.clone(),
            offset: 0,
        })
        .await
        .unwrap()
    {
        GeometryResponse::FaceIndexPage {
            faces,
            next_offset: None,
        } => faces,
        response => panic!("unexpected {response:?}"),
    }
}

#[tokio::test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
async fn failed_import_and_stale_handles_preserve_scene_session_and_pid() {
    let mut client = EngineClient::spawn(engine()).await.unwrap();
    let pid = client.pid();
    assert_eq!(
        import(&mut client, &fixture("box-mm.step")).await.status,
        JobStatus::Completed
    );
    let before = scene(&mut client).await;
    let bad = fixture("truncated.step");
    // A missing/adversarial source must be a typed failure, never engine death.
    let failed = import(&mut client, &bad).await;
    assert_eq!(failed.status, JobStatus::Failed);
    assert!(failed.error.is_some());
    assert_eq!(scene(&mut client).await, before);
    assert_eq!(client.pid(), pid);
    client.ping().await.unwrap();
    let response = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: before.session_id.clone(),
            revision: SceneRevision::ZERO,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap();
    assert!(matches!(
        response,
        GeometryResponse::Error {
            error: GeometryError {
                code: GeometryErrorCode::StaleRevision,
                ..
            }
        }
    ));
    let (definition, occurrence) = records(&mut client).await;
    let faces = face_rows(&mut client, &definition).await;
    let reference = FaceRef {
        session_id: before.session_id.clone(),
        scene_revision: before.revision,
        occurrence_id: occurrence.occurrence_id,
        definition_id: definition.definition_id.clone(),
        face_id: faces[0].face_id.clone(),
    };
    assert!(matches!(
        client
            .geometry(GeometryCommand::InspectFace {
                reference: reference.clone()
            })
            .await
            .unwrap(),
        GeometryResponse::FaceInspection { .. }
    ));
    let mut stale = reference;
    stale.scene_revision = SceneRevision::ZERO;
    assert!(matches!(
        client
            .geometry(GeometryCommand::InspectFace { reference: stale })
            .await
            .unwrap(),
        GeometryResponse::Error {
            error: GeometryError {
                code: GeometryErrorCode::StaleRevision,
                ..
            }
        }
    ));
    let response = client
        .geometry(GeometryCommand::GetScene {
            session_id: SessionId::new(),
        })
        .await
        .unwrap();
    assert!(matches!(response, GeometryResponse::Error { .. }));
    let response = client
        .geometry(GeometryCommand::GetArtifactPage {
            session_id: before.session_id.clone(),
            artifact_id: ArtifactId::new(u32::MAX).unwrap(),
            kind: ArtifactPageKind::Chunks,
            offset: 0,
        })
        .await
        .unwrap();
    assert!(matches!(response, GeometryResponse::Error { .. }));
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
    assert!(!client.status().await.unwrap());
}

#[tokio::test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
async fn bounded_mesh_transfer_stable_ids_eviction_and_fresh_restart_session() {
    let mut client = EngineClient::spawn(engine()).await.unwrap();
    let session = client.hello().session_id.clone();
    assert_eq!(
        import(&mut client, &fixture("box-mm.step")).await.status,
        JobStatus::Completed
    );
    let (definition, occurrence) = records(&mut client).await;
    let faces = face_rows(&mut client, &definition).await;
    assert_eq!(faces.len(), 6);
    let (rows, resources, artifact_summary) = match client
        .geometry(GeometryCommand::GetArtifactPage {
            session_id: session.clone(),
            artifact_id: definition.mesh_artifact_id,
            kind: ArtifactPageKind::Chunks,
            offset: 0,
        })
        .await
        .unwrap()
    {
        GeometryResponse::ArtifactPage {
            chunks,
            resources,
            summary,
            next_offset: None,
            ..
        } => (
            chunks
                .into_iter()
                .map(|row| match row {
                    ArtifactChunkMetadata::Mesh { metadata } => metadata,
                    _ => panic!("wrong artifact type"),
                })
                .collect::<Vec<_>>(),
            resources,
            summary,
        ),
        response => panic!("unexpected {response:?}"),
    };
    validate_mesh_manifest(&rows).unwrap();
    assert_eq!(rows.len(), resources.len());
    for (row, resource) in rows.iter().zip(&resources) {
        let payload = client
            .read_geometry_chunk(
                resource,
                &ArtifactChunkMetadata::Mesh {
                    metadata: row.clone(),
                },
                &artifact_summary,
            )
            .await
            .unwrap();
        assert!(payload.len() <= MAX_GEOMETRY_CHUNK_BYTES as usize);
        validate_mesh_chunk(
            &payload,
            row,
            &session,
            definition.mesh_artifact_id,
            &definition.definition_id,
            row.chunk_index,
        )
        .unwrap();
    }
    let summary = scene(&mut client).await;
    let response = client
        .geometry(GeometryCommand::RemoveInstance {
            session_id: session.clone(),
            base_revision: summary.revision,
            occurrence_id: occurrence.occurrence_id,
        })
        .await
        .unwrap();
    assert!(matches!(
        response,
        GeometryResponse::SceneChanged {
            summary: SceneSummary {
                definition_count: 0,
                occurrence_count: 0,
                ..
            }
        }
    ));
    let removed = scene(&mut client).await;
    let error = client
        .geometry(GeometryCommand::AddInstance {
            session_id: session.clone(),
            base_revision: removed.revision,
            definition_id: definition.definition_id.clone(),
            pose: RigidPoseMm::IDENTITY,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Project(error) if error.code == ProjectErrorCode::InvalidProject)
    );
    assert!(!error.is_fatal());
    assert_eq!(scene(&mut client).await, removed);
    client.ping().await.unwrap();
    assert_eq!(
        import(&mut client, &fixture("box-mm.step")).await.status,
        JobStatus::Completed
    );
    let (again, _) = records(&mut client).await;
    assert_eq!(again.definition_id, definition.definition_id);
    assert_eq!(face_rows(&mut client, &again).await, faces);
    client.shutdown().await.unwrap();
    let mut restarted = EngineClient::spawn(engine()).await.unwrap();
    assert_ne!(restarted.hello().session_id, session);
    assert_eq!(scene(&mut restarted).await.occurrence_count, 0);
    assert_eq!(
        import(&mut restarted, &fixture("box-mm.step")).await.status,
        JobStatus::Completed
    );
    let (after, _) = records(&mut restarted).await;
    assert_eq!(after.definition_id, definition.definition_id);
    assert_eq!(face_rows(&mut restarted, &after).await, faces);
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
async fn preflight_rejections_leave_transport_unwritten_and_live() {
    let mut client = EngineClient::spawn(engine()).await.unwrap();
    let summary = scene(&mut client).await;
    let command = GeometryCommand::ImportPart {
        session_id: summary.session_id,
        base_revision: summary.revision,
        source: NativePath::UnixBytes {
            hex: "61".repeat(MAX_NATIVE_PATH_UNITS as usize + 1),
        },
        initial_pose: RigidPoseMm::IDENTITY,
    };
    assert!(matches!(
        client.geometry(command).await.unwrap(),
        GeometryResponse::Error { .. }
    ));
    let response = client
        .geometry(GeometryCommand::SetInstancePose {
            session_id: client.hello().session_id.clone(),
            base_revision: SceneRevision::ZERO,
            occurrence_id: OccurrenceId::new(1).unwrap(),
            pose: RigidPoseMm {
                translation_mm: [0.0; 3],
                rotation_xyzw: [0.0; 4],
            },
        })
        .await
        .unwrap();
    assert!(matches!(
        response,
        GeometryResponse::Error {
            error: GeometryError {
                code: GeometryErrorCode::InvalidPose,
                ..
            }
        }
    ));
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
async fn native_source_bytes_are_preserved_and_termination_reaps() {
    use std::os::unix::ffi::OsStringExt;
    let root = std::env::temp_dir().join(format!("spiling-client-{}", SessionId::new().as_str()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join(std::ffi::OsString::from_vec(b"box-\xff.step".to_vec()));
    std::fs::copy(fixture("box-mm.step"), &path).unwrap();
    let mut client = EngineClient::spawn(engine()).await.unwrap();
    assert_eq!(
        import(&mut client, &path).await.status,
        JobStatus::Completed
    );
    let pid = client.pid().unwrap();
    client.terminate().await.unwrap();
    assert!(!client.status().await.unwrap());
    #[cfg(target_os = "linux")]
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}

async fn project_job(
    client: &mut EngineClient,
    response: ProjectResponse,
) -> Result<JobResult, ClientError> {
    let ProjectResponse::OperationAccepted { operation } = response else {
        panic!("expected project operation");
    };
    timeout(Duration::from_secs(75), async {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let current = client.native_operation(operation.name.clone()).await?;
            if current.status == JobStatus::Completed {
                return Ok(current.result.unwrap());
            }
            if current.status.is_terminal() {
                return Err(match current.error.unwrap() {
                    JobError::Geometry { error } => ClientError::Geometry(error),
                    JobError::Project { error } => ClientError::Project(error),
                    JobError::Manufacturing { error } => ClientError::Manufacturing(error),
                });
            }
        }
    })
    .await
    .unwrap()
}

async fn open_project(
    client: &mut EngineClient,
    path: &Path,
    read_only: bool,
) -> Result<JobResult, ClientError> {
    let summary = scene(client).await;
    let response = client
        .project(ProjectCommand::Open {
            session_id: summary.session_id,
            base_revision: summary.revision,
            path: NativePath::from_os_str(path.as_os_str()).unwrap(),
            read_only,
            recover_previous: false,
            discard_changes: false,
        })
        .await?;
    project_job(client, response).await
}
#[tokio::test]
#[ignore = "actual native project locking; requires SPILING_TEST_ENGINE"]
async fn project_writer_lock_read_only_rejections_and_kill_release_are_recoverable() {
    use spiling_contracts::project::ProjectErrorCode;
    let root = std::env::temp_dir().join(format!(
        "spiling-client-project-{}",
        SessionId::new().as_str()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("project");
    let mut writer = EngineClient::spawn(engine()).await.unwrap();
    assert!(
        writer
            .hello()
            .project_capabilities
            .iter()
            .any(|value| value == "recoverable_projects_v2")
    );
    assert_eq!(
        import(&mut writer, &fixture("box-mm.step")).await.status,
        JobStatus::Completed
    );
    let summary = scene(&mut writer).await;
    let accepted = writer
        .project(ProjectCommand::Save {
            session_id: summary.session_id,
            base_revision: summary.revision,
            target: Some(NativePath::from_os_str(path.as_os_str()).unwrap()),
        })
        .await
        .unwrap();
    assert!(matches!(
        project_job(&mut writer, accepted).await.unwrap(),
        JobResult::ProjectSaved { .. }
    ));
    let mut second = EngineClient::spawn(engine()).await.unwrap();
    let error = open_project(&mut second, &path, false).await.unwrap_err();
    assert!(
        matches!(&error, ClientError::Project(error) if error.code == ProjectErrorCode::ProjectLocked)
    );
    assert!(!error.is_fatal());
    second.ping().await.unwrap();
    let mut reader = EngineClient::spawn(engine()).await.unwrap();
    open_project(&mut reader, &path, true).await.unwrap();
    let readonly = scene(&mut reader).await;
    let (definition, occurrence) = records(&mut reader).await;
    for command in [
        GeometryCommand::AddInstance {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
            definition_id: definition.definition_id,
            pose: RigidPoseMm::IDENTITY,
        },
        GeometryCommand::SetInstancePose {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
            occurrence_id: occurrence.occurrence_id,
            pose: RigidPoseMm::IDENTITY,
        },
        GeometryCommand::RemoveInstance {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
            occurrence_id: occurrence.occurrence_id,
        },
        GeometryCommand::ImportPart {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
            source: NativePath::from_os_str(fixture("cylinder.step").as_os_str()).unwrap(),
            initial_pose: RigidPoseMm::IDENTITY,
        },
    ] {
        match reader.geometry(command).await {
            Ok(GeometryResponse::OperationAccepted { operation }) => {
                let operation = terminal_native(&reader, operation.name).await;
                assert_eq!(operation.status, JobStatus::Failed, "{operation:?}");
                assert!(
                    matches!(&operation.error, Some(JobError::Project { error }) if error.code == ProjectErrorCode::ReadOnly)
                );
                assert!(operation.outputs.is_empty());
            }
            Err(error) => {
                assert!(
                    matches!(&error, ClientError::Project(error) if error.code == ProjectErrorCode::ReadOnly)
                );
                assert!(!error.is_fatal());
            }
            other => panic!("read-only geometry mutation must fail: {other:?}"),
        }
        reader.ping().await.unwrap();
    }
    for command in [
        ProjectCommand::Save {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
            target: None,
        },
        ProjectCommand::Undo {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
        },
        ProjectCommand::Redo {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
        },
    ] {
        match reader.project(command).await {
            Ok(ProjectResponse::OperationAccepted { operation }) => {
                let operation = terminal_native(&reader, operation.name).await;
                assert_eq!(operation.status, JobStatus::Failed, "{operation:?}");
                assert!(
                    matches!(&operation.error, Some(JobError::Project { error }) if error.code == ProjectErrorCode::ReadOnly)
                );
                assert!(operation.outputs.is_empty());
            }
            Err(error) => {
                assert!(
                    matches!(&error, ClientError::Project(error) if error.code == ProjectErrorCode::ReadOnly)
                );
                assert!(!error.is_fatal());
            }
            other => panic!("read-only project mutation must fail: {other:?}"),
        }
        reader.ping().await.unwrap();
    }
    assert_eq!(scene(&mut reader).await, readonly);
    let error = reader
        .project(ProjectCommand::Open {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
            path: NativePath::UnixBytes {
                hex: "61".repeat(MAX_NATIVE_PATH_UNITS as usize + 1),
            },
            read_only: true,
            recover_previous: false,
            discard_changes: true,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::InvalidRequest(_)));
    reader.ping().await.unwrap();
    let replacement = reader
        .project(ProjectCommand::New {
            session_id: readonly.session_id.clone(),
            base_revision: readonly.revision,
            discard_changes: false,
        })
        .await
        .unwrap();
    match replacement {
        ProjectResponse::SceneChanged { info, summary } => {
            assert!(!info.read_only);
            assert!(summary.revision > readonly.revision);
            assert_eq!(summary.occurrence_count, 0);
        }
        _ => panic!("read-only document must permit switching to a new project"),
    }
    writer.terminate().await.unwrap();
    assert!(!writer.status().await.unwrap());
    open_project(&mut second, &path, false).await.unwrap();
    second.shutdown().await.unwrap();
    reader.shutdown().await.unwrap();
    let mut fresh = EngineClient::spawn(engine()).await.unwrap();
    open_project(&mut fresh, &path, false).await.unwrap();
    fresh.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "actual manufacturing client errors; requires SPILING_TEST_ENGINE"]
async fn manufacturing_rejections_preserve_pid_session_and_unwritten_preflight() {
    let mut client = EngineClient::spawn(engine()).await.unwrap();
    let pid = client.pid();
    let session = client.hello().session_id.clone();
    let status = client
        .manufacturing(ManufacturingCommand::Get {
            session_id: session.clone(),
        })
        .await
        .unwrap();
    let ManufacturingResponse::Status {
        info,
        intent: None,
        artifact: None,
        resource: None,
    } = status
    else {
        panic!("fresh process must have no manufacturing intent or artifact");
    };
    for command in [
        ManufacturingCommand::Compile {
            session_id: session.clone(),
            base_revision: info.revision,
        },
        ManufacturingCommand::Inspect {
            session_id: session.clone(),
            base_revision: info.revision,
        },
    ] {
        let ManufacturingResponse::OperationAccepted { operation } =
            client.manufacturing(command).await.unwrap()
        else {
            panic!("manufacturing admission")
        };
        let operation = terminal_native(&client, operation.name).await;
        assert_eq!(operation.status, JobStatus::Failed, "{operation:?}");
        assert!(
            matches!(&operation.error, Some(JobError::Manufacturing { error }) if error.code == ManufacturingErrorCode::NoIntent)
        );
        assert!(operation.outputs.is_empty());
    }
    let mut record = manufacturing_record(b"not retained");
    let descriptor = spiling_contracts::ArtifactView {
        name: format!("artifacts/{}", record.hash.as_str()),
        size_bytes: record.byte_count.to_string(),
        sha256: record.hash.as_str().into(),
        media_type: "application/x-spiling-manufacturing-bundle".into(),
    };
    record.byte_count = MAX_MANUFACTURING_BUNDLE_BYTES + 1;
    let error = client
        .fetch_manufacturing_bundle(&record, &descriptor)
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        ClientError::Manufacturing(ManufacturingError {
            code: ManufacturingErrorCode::ResourceLimit,
            ..
        })
    ));
    assert!(!error.is_fatal());
    record.byte_count = b"not retained".len() as u32;
    let error = client
        .fetch_manufacturing_bundle(&record, &descriptor)
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        ClientError::Rpc(_) | ClientError::Manufacturing(_)
    ));
    assert!(!error.is_fatal());
    client.ping().await.unwrap();
    assert_eq!(client.pid(), pid);
    assert_eq!(client.hello().session_id, session);
    client.shutdown().await.unwrap();
}

async fn terminal_native(
    client: &EngineClient,
    name: String,
) -> spiling_contracts::NativeOperationView {
    timeout(Duration::from_secs(75), async {
        loop {
            let operation = client.native_operation(name.clone()).await.unwrap();
            if operation.status.is_terminal() {
                return operation;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap()
}

async fn completed_native(
    client: &EngineClient,
    name: String,
) -> spiling_contracts::NativeOperationView {
    let operation = terminal_native(client, name).await;
    assert_eq!(operation.status, JobStatus::Completed, "{operation:?}");
    operation
}

#[tokio::test]
#[ignore = "actual saved manufacturing bundle and independent replay; requires SPILING_TEST_ENGINE"]
async fn manufacturing_bundle_survives_save_restart_and_source_removal() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source-box.step");
    std::fs::copy(fixture("box-mm.step"), &source).unwrap();
    let project = root.path().join("project");
    let mut client = EngineClient::spawn(engine()).await.unwrap();
    assert_eq!(
        import(&mut client, &source).await.status,
        JobStatus::Completed
    );
    let session = client.hello().session_id.clone();
    let ManufacturingResponse::Status { info, .. } = client
        .manufacturing(ManufacturingCommand::Get {
            session_id: session.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("manufacturing status");
    };
    let intent = decode_intent(include_bytes!(
        "../../../../fixtures/manufacturing/solid-fill.intent.json"
    ))
    .unwrap();
    let ManufacturingResponse::Status { info, .. } = client
        .manufacturing(ManufacturingCommand::SetIntent {
            session_id: session.clone(),
            base_revision: info.revision,
            intent: intent.into(),
        })
        .await
        .unwrap()
    else {
        panic!("intent status");
    };
    let request_id = Uuid::new_v4().to_string();
    let compile = ManufacturingCommand::Compile {
        session_id: session.clone(),
        base_revision: info.revision,
    };
    let ManufacturingResponse::OperationAccepted { operation } = client
        .rpc()
        .manufacturing_with_request_id(compile.clone(), &request_id)
        .await
        .unwrap()
    else {
        panic!("compile admission");
    };
    // Retrying only the admission ID observes retained work, never a second compiler.
    let ManufacturingResponse::OperationAccepted { operation: retry } = client
        .rpc()
        .manufacturing_with_request_id(compile, &request_id)
        .await
        .unwrap()
    else {
        panic!("compile retry");
    };
    assert_eq!(retry.name, operation.name);
    let completed = completed_native(&client, operation.name.clone()).await;
    let Some(JobResult::ManufacturingCompiled {
        record,
        resource,
        info,
    }) = completed.result
    else {
        panic!("compiled result");
    };
    let bundle = client
        .fetch_manufacturing_bundle(&record, &resource)
        .await
        .unwrap();
    assert!(bundle.summary().software_only);
    assert!(!bundle.program.is_empty());
    let mut wrong_input = record.clone();
    wrong_input.input_hash = SourceHash::from_bytes(b"wrong semantic input");
    assert!(
        !client
            .fetch_manufacturing_bundle(&wrong_input, &resource)
            .await
            .unwrap_err()
            .is_fatal()
    );
    let mut wrong_summary = record.clone();
    wrong_summary.summary.deposited_volume_mm3 += 1.0;
    assert!(
        !client
            .fetch_manufacturing_bundle(&wrong_summary, &resource)
            .await
            .unwrap_err()
            .is_fatal()
    );
    client.ping().await.unwrap();
    let ManufacturingResponse::OperationAccepted { operation: verify } = client
        .manufacturing(ManufacturingCommand::Inspect {
            session_id: session.clone(),
            base_revision: info.revision,
        })
        .await
        .unwrap()
    else {
        panic!("verification admission");
    };
    let verified = completed_native(&client, verify.name).await;
    assert!(
        matches!(verified.result, Some(JobResult::ManufacturingVerified { report, .. }) if report.verified)
    );
    let scene = scene(&mut client).await;
    let save = client
        .project(ProjectCommand::Save {
            session_id: session.clone(),
            base_revision: scene.revision,
            target: Some(NativePath::from_os_str(project.as_os_str()).unwrap()),
        })
        .await
        .unwrap();
    assert!(
        matches!(project_job(&mut client, save).await.unwrap(), JobResult::ProjectSaved { info } if !info.dirty && !info.save_uncertain)
    );
    client.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();
    let mut reopened = EngineClient::spawn(engine()).await.unwrap();
    assert_ne!(reopened.hello().session_id, session);
    open_project(&mut reopened, &project, false).await.unwrap();
    let ManufacturingResponse::Status {
        info,
        artifact: Some(reloaded),
        resource: Some(resource),
        ..
    } = reopened
        .manufacturing(ManufacturingCommand::Get {
            session_id: reopened.hello().session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("saved manufacturing artifact missing");
    };
    assert_eq!(reloaded, record);
    let reopened_bundle = reopened
        .fetch_manufacturing_bundle(&reloaded, &resource)
        .await
        .unwrap();
    assert_eq!(reopened_bundle.program, bundle.program);
    assert_eq!(reopened_bundle.provenance, bundle.provenance);
    let ManufacturingResponse::OperationAccepted { operation: replay } = reopened
        .manufacturing(ManufacturingCommand::Inspect {
            session_id: reopened.hello().session_id.clone(),
            base_revision: info.revision,
        })
        .await
        .unwrap()
    else {
        panic!("fresh independent replay admission");
    };
    assert!(
        matches!(completed_native(&reopened, replay.name).await.result,
        Some(JobResult::ManufacturingVerified { report, .. }) if report.verified)
    );
    reopened.shutdown().await.unwrap();
}
