// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Deliberate deep checks against a built native engine, not a simulated pipe peer.
use super::*;
use spiling_contracts::{
    display::{validate_mesh_chunk, validate_mesh_manifest},
    geometry::*,
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
async fn import(client: &mut EngineClient, path: &Path) -> EngineJob {
    let summary = scene(client).await;
    let command = GeometryCommand::ImportPart {
        session_id: summary.session_id.clone(),
        base_revision: summary.revision,
        source: NativePath::from_os_str(path.as_os_str()).unwrap(),
        initial_pose: RigidPoseMm::IDENTITY,
    };
    let job_id = match client.geometry(command).await.unwrap() {
        GeometryResponse::JobAccepted { job_id } => job_id,
        response => panic!("unexpected {response:?}"),
    };
    timeout(Duration::from_secs(75), async {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match client
                .geometry(GeometryCommand::GetJob {
                    session_id: summary.session_id.clone(),
                    job_id,
                })
                .await
                .unwrap()
            {
                GeometryResponse::Job { job } if job.status.is_terminal() => return job,
                GeometryResponse::Job { .. } => {}
                response => panic!("unexpected {response:?}"),
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
    let mut client = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
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
    let error = client
        .read_geometry_chunk(&before.session_id, ArtifactId::new(u32::MAX).unwrap(), 0)
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::Geometry(_)));
    assert!(!error.is_fatal());
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
    assert!(!client.status().await.unwrap());
}

#[tokio::test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
async fn bounded_mesh_transfer_stable_ids_eviction_and_fresh_restart_session() {
    let mut client = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
    let session = client.hello().session_id.clone();
    assert_eq!(
        import(&mut client, &fixture("box-mm.step")).await.status,
        JobStatus::Completed
    );
    let (definition, occurrence) = records(&mut client).await;
    let faces = face_rows(&mut client, &definition).await;
    assert_eq!(faces.len(), 6);
    let rows = match client
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
            next_offset: None,
            ..
        } => chunks
            .into_iter()
            .map(|row| match row {
                ArtifactChunkMetadata::Mesh { metadata } => metadata,
                _ => panic!("wrong artifact type"),
            })
            .collect::<Vec<_>>(),
        response => panic!("unexpected {response:?}"),
    };
    validate_mesh_manifest(&rows).unwrap();
    for row in &rows {
        let payload = client
            .read_geometry_chunk(&session, definition.mesh_artifact_id, row.chunk_index)
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
    let mut restarted = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
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
    let mut client = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
    let next_id = client.next_id;
    let summary = scene(&mut client).await;
    let expected_next = next_id + 1;
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
    assert_eq!(client.next_id, expected_next);
    let result = client
        .geometry(GeometryCommand::ReadArtifactChunk {
            session_id: client.hello().session_id.clone(),
            artifact_id: ArtifactId::new(1).unwrap(),
            chunk_index: 0,
        })
        .await;
    assert!(matches!(result, Err(ClientError::InvalidRequest(_))));
    assert_eq!(client.next_id, expected_next);
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
    assert_eq!(client.next_id, expected_next);
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
    let mut client = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
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

#[tokio::test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
async fn rejects_previous_and_next_protocol_versions() {
    for version in [PROTOCOL_VERSION - 1, PROTOCOL_VERSION + 1] {
        assert!(matches!(
            EngineClient::spawn(engine(), version).await,
            Err(ClientError::UpgradeRequired(_))
        ));
    }
}

async fn project_job(
    client: &mut EngineClient,
    response: ProjectResponse,
) -> Result<JobResult, ClientError> {
    let ProjectResponse::JobAccepted { job_id } = response else {
        panic!("expected project job");
    };
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let GeometryResponse::Job { job } = client
            .geometry(GeometryCommand::GetJob {
                session_id: client.hello().session_id.clone(),
                job_id,
            })
            .await?
        else {
            panic!("expected job status");
        };
        if job.status == JobStatus::Completed {
            return Ok(job.result.unwrap());
        }
        if job.status.is_terminal() {
            return Err(match job.error.unwrap() {
                JobError::Geometry { error } => ClientError::Geometry(error),
                JobError::Project { error } => ClientError::Project(error),
                JobError::Manufacturing { error } => ClientError::Manufacturing(error),
            });
        }
    }
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
    let mut writer = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
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
    let mut second = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
    let error = open_project(&mut second, &path, false).await.unwrap_err();
    assert!(
        matches!(&error, ClientError::Project(error) if error.code == ProjectErrorCode::ProjectLocked)
    );
    assert!(!error.is_fatal());
    second.ping().await.unwrap();
    let mut reader = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
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
        let error = reader.geometry(command).await.unwrap_err();
        assert!(
            matches!(&error, ClientError::Project(error) if error.code == ProjectErrorCode::ReadOnly)
        );
        assert!(!error.is_fatal());
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
        let error = reader.project(command).await.unwrap_err();
        assert!(
            matches!(&error, ClientError::Project(error) if error.code == ProjectErrorCode::ReadOnly)
        );
        assert!(!error.is_fatal());
        reader.ping().await.unwrap();
    }
    assert_eq!(scene(&mut reader).await, readonly);
    let next_id = reader.next_id;
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
    assert_eq!(reader.next_id, next_id);
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
    let mut fresh = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
    open_project(&mut fresh, &path, false).await.unwrap();
    fresh.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "actual manufacturing client errors; requires SPILING_TEST_ENGINE"]
async fn manufacturing_rejections_preserve_pid_session_and_unwritten_preflight() {
    let mut client = EngineClient::spawn(engine(), PROTOCOL_VERSION)
        .await
        .unwrap();
    let pid = client.pid();
    let session = client.hello().session_id.clone();
    assert_eq!(client.hello().protocol_version, 4);
    for capability in [
        "planar_software_compile_v1",
        "independent_program_replay_v1",
        "immutable_bundle_chunks_v1",
    ] {
        assert!(
            client
                .hello()
                .manufacturing_capabilities
                .iter()
                .any(|value| value == capability)
        );
    }
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
        },
    ] {
        let error = client.manufacturing(command).await.unwrap_err();
        assert!(matches!(&error, ClientError::Manufacturing(_)));
        assert!(!error.is_fatal());
        client.ping().await.unwrap();
    }
    let mut record = manufacturing_record(b"not retained");
    let next_id = client.next_id;
    let error = client
        .manufacturing(ManufacturingCommand::ReadArtifactChunk {
            session_id: session.clone(),
            hash: record.hash.clone(),
            offset: 0,
            max_bytes: 1,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::InvalidRequest(_)));
    assert_eq!(client.next_id, next_id);
    record.byte_count = MAX_MANUFACTURING_BUNDLE_BYTES + 1;
    let error = client
        .fetch_manufacturing_bundle(&record)
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
    assert_eq!(client.next_id, next_id);
    record.byte_count = b"not retained".len() as u32;
    let error = client
        .fetch_manufacturing_bundle(&record)
        .await
        .unwrap_err();
    assert!(matches!(&error, ClientError::Manufacturing(_)));
    assert!(!error.is_fatal());
    client.ping().await.unwrap();
    assert_eq!(client.pid(), pid);
    assert_eq!(client.hello().session_id, session);
    client.shutdown().await.unwrap();
}
