// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{
    PROTOCOL_VERSION,
    display::{validate_mesh_chunk, validate_section_chunk},
    geometry::*,
};
use spiling_engine_client::EngineClient;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const ENGINE: &str = env!("CARGO_BIN_EXE_spiling-engine");
fn fixture(name: &str) -> NativePath {
    NativePath::from_os_str(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry")
            .join(name)
            .as_os_str(),
    )
    .unwrap()
}
async fn command(client: &mut EngineClient, command: GeometryCommand) -> GeometryResponse {
    client.geometry(command).await.unwrap()
}
async fn scene(client: &mut EngineClient) -> SceneSummary {
    let session_id = client.hello().session_id.clone();
    let GeometryResponse::Scene { summary } =
        command(client, GeometryCommand::GetScene { session_id }).await
    else {
        panic!("expected scene");
    };
    summary.validate().unwrap();
    summary
}
async fn wait_job(client: &mut EngineClient, job_id: JobId) -> EngineJob {
    let start = Instant::now();
    loop {
        assert!(
            start.elapsed() < Duration::from_secs(70),
            "native job did not reach terminal status"
        );
        let session_id = client.hello().session_id.clone();
        let GeometryResponse::Job { job } =
            command(client, GeometryCommand::GetJob { session_id, job_id }).await
        else {
            panic!("expected job");
        };
        if job.status.is_terminal() {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn start_import(client: &mut EngineClient, source: NativePath, pose: RigidPoseMm) -> JobId {
    let summary = scene(client).await;
    let GeometryResponse::JobAccepted { job_id } = command(
        client,
        GeometryCommand::ImportPart {
            session_id: summary.session_id,
            base_revision: summary.revision,
            source,
            initial_pose: pose,
        },
    )
    .await
    else {
        panic!("expected accepted import");
    };
    job_id
}
async fn import(client: &mut EngineClient, name: &str, pose: RigidPoseMm) -> SceneSummary {
    let id = start_import(client, fixture(name), pose).await;
    let job = wait_job(client, id).await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    let Some(JobResult::Scene { summary }) = job.result else {
        panic!("expected import scene result");
    };
    summary
}
async fn definitions(client: &mut EngineClient, summary: &SceneSummary) -> Vec<DefinitionRecord> {
    let mut offset = 0;
    let mut records = vec![];
    loop {
        let GeometryResponse::ScenePage {
            definitions,
            occurrences,
            next_offset,
        } = command(
            client,
            GeometryCommand::GetScenePage {
                session_id: summary.session_id.clone(),
                revision: summary.revision,
                kind: ScenePageKind::Definitions,
                offset,
            },
        )
        .await
        else {
            panic!("expected definitions page");
        };
        assert!(occurrences.is_empty());
        assert!(definitions.len() <= SCENE_PAGE_SIZE as usize);
        records.extend(definitions);
        if let Some(next) = next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    records
}
async fn occurrences(client: &mut EngineClient, summary: &SceneSummary) -> Vec<OccurrenceRecord> {
    let mut offset = 0;
    let mut records = vec![];
    loop {
        let GeometryResponse::ScenePage {
            definitions,
            occurrences,
            next_offset,
        } = command(
            client,
            GeometryCommand::GetScenePage {
                session_id: summary.session_id.clone(),
                revision: summary.revision,
                kind: ScenePageKind::Occurrences,
                offset,
            },
        )
        .await
        else {
            panic!("expected occurrence page");
        };
        assert!(definitions.is_empty());
        assert!(occurrences.len() <= SCENE_PAGE_SIZE as usize);
        records.extend(occurrences);
        if let Some(next) = next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    records
}
async fn faces(client: &mut EngineClient, definition: &DefinitionRecord) -> Vec<FaceIndexRow> {
    let session_id = client.hello().session_id.clone();
    let GeometryResponse::FaceIndexPage { faces, next_offset } = command(
        client,
        GeometryCommand::GetFaceIndexPage {
            session_id,
            definition_id: definition.definition_id.clone(),
            offset: 0,
        },
    )
    .await
    else {
        panic!("expected face page");
    };
    assert!(next_offset.is_none());
    faces
}
async fn mesh(client: &mut EngineClient, definition: &DefinitionRecord) -> Vec<SourceHash> {
    let session_id = client.hello().session_id.clone();
    let artifact_id = definition.mesh_artifact_id;
    let mut offset = 0;
    let mut hashes = vec![];
    let mut total = 0;
    loop {
        let GeometryResponse::ArtifactPage {
            summary,
            chunks,
            loops,
            next_offset,
        } = command(
            client,
            GeometryCommand::GetArtifactPage {
                session_id: session_id.clone(),
                artifact_id,
                kind: ArtifactPageKind::Chunks,
                offset,
            },
        )
        .await
        else {
            panic!("expected mesh page");
        };
        assert!(loops.is_empty());
        assert!(chunks.len() <= ARTIFACT_PAGE_SIZE as usize);
        for chunk in chunks {
            let ArtifactChunkMetadata::Mesh { metadata } = chunk else {
                panic!("expected mesh metadata");
            };
            assert_eq!(metadata.chunk_index, hashes.len() as u32);
            let bytes = client
                .read_geometry_chunk(&session_id, artifact_id, metadata.chunk_index)
                .await
                .unwrap();
            assert!(bytes.len() <= MAX_GEOMETRY_CHUNK_BYTES as usize);
            validate_mesh_chunk(
                &bytes,
                &metadata,
                &session_id,
                artifact_id,
                &definition.definition_id,
                metadata.chunk_index,
            )
            .unwrap();
            total += bytes.len() as u32;
            hashes.push(metadata.sha256);
        }
        if let Some(next) = next_offset {
            offset = next;
        } else {
            let ArtifactSummary::Mesh {
                chunk_count,
                total_bytes,
                ..
            } = summary
            else {
                panic!("expected mesh summary");
            };
            assert_eq!(chunk_count, hashes.len() as u32);
            assert_eq!(total_bytes, total);
            break;
        }
    }
    hashes
}
fn expect_error(response: GeometryResponse, code: GeometryErrorCode) {
    let GeometryResponse::Error { error } = response else {
        panic!("expected {code:?}, got {response:?}");
    };
    assert_eq!(error.code, code);
    assert!(error.message.len() <= MAX_ERROR_MESSAGE_BYTES as usize);
}
fn geometry_job_error(job: EngineJob) -> GeometryErrorCode {
    let Some(JobError::Geometry { error }) = job.error.as_ref() else {
        panic!("expected tagged geometry job error: {job:?}");
    };
    error.code
}
async fn discard_project_history(client: &mut EngineClient) -> SceneSummary {
    let summary = scene(client).await;
    let spiling_contracts::project::ProjectResponse::SceneChanged { summary, .. } = client
        .project(spiling_contracts::project::ProjectCommand::New {
            session_id: summary.session_id,
            base_revision: summary.revision,
            discard_changes: true,
        })
        .await
        .unwrap()
    else {
        panic!("expected fresh project");
    };
    summary
}
async fn remove(client: &mut EngineClient, occurrence_id: OccurrenceId) -> SceneSummary {
    let summary = scene(client).await;
    let GeometryResponse::SceneChanged { summary } = command(
        client,
        GeometryCommand::RemoveInstance {
            session_id: summary.session_id,
            base_revision: summary.revision,
            occurrence_id,
        },
    )
    .await
    else {
        panic!("expected removal");
    };
    summary
}
async fn section(client: &mut EngineClient, plane: PlaneMm) -> EngineJob {
    let summary = scene(client).await;
    let GeometryResponse::JobAccepted { job_id } = command(
        client,
        GeometryCommand::StartSection {
            session_id: summary.session_id,
            base_revision: summary.revision,
            plane,
        },
    )
    .await
    else {
        panic!("expected section job");
    };
    wait_job(client, job_id).await
}
async fn section_loops(
    client: &mut EngineClient,
    artifact_id: ArtifactId,
) -> (SectionSummary, Vec<(SectionLoopMetadata, Vec<[f64; 3]>)>) {
    let session_id = client.hello().session_id.clone();
    let mut offset = 0;
    let mut descriptors = vec![];
    let summary;
    loop {
        let GeometryResponse::ArtifactPage {
            summary: artifact,
            chunks,
            loops,
            next_offset,
        } = command(
            client,
            GeometryCommand::GetArtifactPage {
                session_id: session_id.clone(),
                artifact_id,
                kind: ArtifactPageKind::Chunks,
                offset,
            },
        )
        .await
        else {
            panic!("expected section chunks");
        };
        assert!(loops.is_empty());
        descriptors.extend(chunks);
        if let Some(next) = next_offset {
            offset = next;
        } else {
            let ArtifactSummary::Section { summary: value } = artifact else {
                panic!("expected section summary");
            };
            summary = value;
            break;
        }
    }
    let mut metadata = vec![];
    offset = 0;
    loop {
        let GeometryResponse::ArtifactPage {
            loops, next_offset, ..
        } = command(
            client,
            GeometryCommand::GetArtifactPage {
                session_id: session_id.clone(),
                artifact_id,
                kind: ArtifactPageKind::Loops,
                offset,
            },
        )
        .await
        else {
            panic!("expected loop page");
        };
        metadata.extend(loops);
        if let Some(next) = next_offset {
            offset = next;
        } else {
            break;
        }
    }
    let mut loops = vec![];
    let mut byte_count = 0;
    for descriptor in descriptors {
        let ArtifactChunkMetadata::Section {
            metadata: descriptor,
        } = descriptor
        else {
            panic!("expected section chunk");
        };
        assert_eq!(descriptor.first_loop_ordinal, loops.len() as u32);
        let bytes = client
            .read_geometry_chunk(&session_id, artifact_id, descriptor.chunk_index)
            .await
            .unwrap();
        let decoded = validate_section_chunk(
            &bytes,
            &descriptor,
            &summary,
            &session_id,
            artifact_id,
            descriptor.chunk_index,
        )
        .unwrap();
        for index in 0..descriptor.loop_count as usize {
            let start = decoded.loop_offsets.get(index).unwrap() as usize;
            let end = decoded.loop_offsets.get(index + 1).unwrap() as usize;
            let points = (start..end)
                .map(|i| decoded.points_relative_mm.get(i).unwrap())
                .collect();
            let row = metadata[loops.len()].clone();
            assert_eq!(row.ordinal, loops.len() as u32);
            loops.push((row, points));
        }
        byte_count += bytes.len() as u32;
    }
    assert_eq!(loops.len() as u32, summary.total_loop_count);
    assert_eq!(byte_count, summary.total_bytes);
    (summary, loops)
}
fn area(points: &[[f64; 3]], frame: PlaneFrameMm) -> f64 {
    let dot = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
    points
        .windows(2)
        .map(|p| {
            dot(p[0], frame.x_axis) * dot(p[1], frame.y_axis)
                - dot(p[1], frame.x_axis) * dot(p[0], frame.y_axis)
        })
        .sum::<f64>()
        / 2.0
}

#[tokio::test]
async fn real_scene_reuse_pages_faces_and_final_removal() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let pid = client.pid();
    let initial = scene(&mut client).await;
    assert_eq!(initial.revision, SceneRevision::ZERO);
    assert_eq!(initial.occurrence_count, 0);
    let first = import(&mut client, "box-mm.step", RigidPoseMm::IDENTITY).await;
    let definition = definitions(&mut client, &first).await.remove(0);
    assert_eq!(definition.face_count, 6);
    assert_eq!(definition.bounds_mm.min, [0.0; 3]);
    assert_eq!(definition.bounds_mm.max, [20.0, 10.0, 8.0]);
    let face_table = faces(&mut client, &definition).await;
    assert_eq!(face_table.len(), 6);
    let hashes = mesh(&mut client, &definition).await;
    let pose = RigidPoseMm {
        translation_mm: [80.0, 0.0, 0.0],
        rotation_xyzw: [
            0.0,
            0.0,
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
        ],
    };
    let mut summary = import(&mut client, "box-mm.step", pose).await;
    assert_eq!(summary.definition_count, 1);
    assert_eq!(summary.occurrence_count, 2);
    assert_eq!(summary.unique_mesh_bytes, first.unique_mesh_bytes);
    assert_eq!(
        definitions(&mut client, &summary).await[0].mesh_artifact_id,
        definition.mesh_artifact_id
    );
    for index in 2..128 {
        let pose = RigidPoseMm {
            translation_mm: [index as f64 * 40.0, 0.0, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let GeometryResponse::SceneChanged { summary: changed } = command(
            &mut client,
            GeometryCommand::AddInstance {
                session_id: summary.session_id.clone(),
                base_revision: summary.revision,
                definition_id: definition.definition_id.clone(),
                pose,
            },
        )
        .await
        else {
            panic!("expected repeated instance");
        };
        summary = changed;
    }
    assert_eq!(summary.occurrence_count, 128);
    assert_eq!(summary.unique_mesh_bytes, first.unique_mesh_bytes);
    assert_eq!(mesh(&mut client, &definition).await, hashes);
    expect_error(
        command(
            &mut client,
            GeometryCommand::GetScenePage {
                session_id: summary.session_id.clone(),
                revision: first.revision,
                kind: ScenePageKind::Occurrences,
                offset: 0,
            },
        )
        .await,
        GeometryErrorCode::StaleRevision,
    );
    let records = occurrences(&mut client, &summary).await;
    assert_eq!(records.len(), 128);
    let reference = FaceRef {
        session_id: summary.session_id.clone(),
        scene_revision: summary.revision,
        occurrence_id: records[0].occurrence_id,
        definition_id: definition.definition_id.clone(),
        face_id: face_table[0].face_id.clone(),
    };
    let GeometryResponse::FaceInspection { inspection } = command(
        &mut client,
        GeometryCommand::InspectFace {
            reference: reference.clone(),
        },
    )
    .await
    else {
        panic!("expected native face");
    };
    assert_eq!(inspection.face.face_id, reference.face_id);
    assert_eq!(inspection.provenance, definition.provenance);
    let other_ref = FaceRef {
        occurrence_id: records[1].occurrence_id,
        ..reference.clone()
    };
    let GeometryResponse::FaceInspection { inspection: other } = command(
        &mut client,
        GeometryCommand::InspectFace {
            reference: other_ref,
        },
    )
    .await
    else {
        panic!("expected repeated face");
    };
    assert_eq!(inspection.face, other.face);
    assert_ne!(
        inspection.reference.occurrence_id,
        other.reference.occurrence_id
    );
    assert_ne!(inspection.pose, other.pose);
    let session = summary.session_id.clone();
    assert!(matches!(
        command(
            &mut client,
            GeometryCommand::ReleaseArtifact {
                session_id: session.clone(),
                artifact_id: definition.mesh_artifact_id
            }
        )
        .await,
        GeometryResponse::Released {}
    ));
    // Releasing the consumer lease cannot free the still-live definition's mesh.
    assert_eq!(mesh(&mut client, &definition).await, hashes);
    for occurrence in records {
        summary = remove(&mut client, occurrence.occurrence_id).await;
    }
    assert_eq!(summary.definition_count, 0);
    assert_eq!(summary.occurrence_count, 0);
    assert_eq!(summary.unique_mesh_bytes, 0);
    assert_eq!(summary.bounds_mm, None);
    // The final removal is still undoable; replacement explicitly releases history pins.
    summary = discard_project_history(&mut client).await;
    expect_error(
        command(
            &mut client,
            GeometryCommand::GetArtifactPage {
                session_id: session.clone(),
                artifact_id: definition.mesh_artifact_id,
                kind: ArtifactPageKind::Chunks,
                offset: 0,
            },
        )
        .await,
        GeometryErrorCode::UnknownHandle,
    );
    assert!(
        matches!(client.read_geometry_chunk(&session,definition.mesh_artifact_id,0).await,Err(spiling_engine_client::ClientError::Geometry(error)) if error.code==GeometryErrorCode::UnknownHandle)
    );
    client.ping().await.unwrap();
    assert!(matches!(
        command(
            &mut client,
            GeometryCommand::ReleaseArtifact {
                session_id: session.clone(),
                artifact_id: definition.mesh_artifact_id
            }
        )
        .await,
        GeometryResponse::Released {}
    ));
    expect_error(
        command(&mut client, GeometryCommand::InspectFace { reference }).await,
        GeometryErrorCode::StaleRevision,
    );
    expect_error(
        command(
            &mut client,
            GeometryCommand::AddInstance {
                session_id: session.clone(),
                base_revision: summary.revision,
                definition_id: definition.definition_id.clone(),
                pose: RigidPoseMm::IDENTITY,
            },
        )
        .await,
        GeometryErrorCode::UnknownHandle,
    );
    let reimported = import(&mut client, "box-mm.step", RigidPoseMm::IDENTITY).await;
    let current = definitions(&mut client, &reimported).await.remove(0);
    assert_eq!(current.definition_id, definition.definition_id);
    assert_ne!(current.mesh_artifact_id, definition.mesh_artifact_id);
    assert_eq!(faces(&mut client, &current).await, face_table);
    assert_eq!(mesh(&mut client, &current).await, hashes);
    assert_eq!(client.pid(), pid);
    client.shutdown().await.unwrap();
    let mut restarted = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    assert_ne!(restarted.hello().session_id, session);
    let fresh = import(&mut restarted, "box-mm.step", RigidPoseMm::IDENTITY).await;
    let fresh_def = definitions(&mut restarted, &fresh).await.remove(0);
    assert_eq!(fresh_def.definition_id, definition.definition_id);
    assert_eq!(faces(&mut restarted, &fresh_def).await, face_table);
    expect_error(
        command(
            &mut restarted,
            GeometryCommand::GetScene {
                session_id: session,
            },
        )
        .await,
        GeometryErrorCode::StaleRevision,
    );
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_source_keeps_scene_pid_and_revision_and_units_are_native() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let before = import(&mut client, "box-mm.step", RigidPoseMm::IDENTITY).await;
    let pid = client.pid();
    let id = start_import(
        &mut client,
        fixture("missing-units.step"),
        RigidPoseMm::IDENTITY,
    )
    .await;
    let failed = wait_job(&mut client, id).await;
    assert_eq!(failed.status, JobStatus::Failed);
    assert_eq!(
        geometry_job_error(failed),
        GeometryErrorCode::UnsupportedUnits
    );
    assert_eq!(scene(&mut client).await, before);
    let source = fixture("does-not-exist.step");
    let id = start_import(&mut client, source, RigidPoseMm::IDENTITY).await;
    assert_eq!(
        geometry_job_error(wait_job(&mut client, id).await),
        GeometryErrorCode::SourceIo
    );
    assert_eq!(scene(&mut client).await, before);
    let after = import(&mut client, "box-inch.step", RigidPoseMm::IDENTITY).await;
    assert_eq!(after.definition_count, 2);
    let defs = definitions(&mut client, &after).await;
    for definition in &defs {
        for i in 0..3 {
            assert!((definition.bounds_mm.min[i]).abs() < 1e-6);
            assert!((definition.bounds_mm.max[i] - [20.0, 10.0, 8.0][i]).abs() < 1e-6);
        }
    }
    assert!(
        defs.iter()
            .any(|d| d.provenance.source_unit == SourceUnit::Inch)
    );
    assert_eq!(client.pid(), pid);
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_sections_preserve_hole_winding_and_domain_survival() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let before = import(&mut client, "through-hole.step", RigidPoseMm::IDENTITY).await;
    let job = section(
        &mut client,
        PlaneMm {
            origin_mm: [0.0, 0.0, 4.0],
            normal: [0.0, 0.0, 2.0],
        },
    )
    .await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    let Some(JobResult::Section { artifact_id }) = job.result else {
        panic!("expected section artifact");
    };
    let (metadata, loops) = section_loops(&mut client, artifact_id).await;
    assert_eq!(metadata.revision, before.revision);
    assert_eq!(metadata.plane.normal, [0.0, 0.0, 1.0]);
    assert_eq!(loops.len(), 2);
    assert!(!loops[0].0.is_hole);
    assert!(loops[1].0.is_hole);
    assert_eq!(loops[0].0.occurrence_id, loops[1].0.occurrence_id);
    let areas: Vec<_> = loops.iter().map(|(_, p)| area(p, metadata.frame)).collect();
    assert!(areas[0] > 0.0);
    assert!(areas[1] < 0.0);
    let expected = 400.0 - 9.0 * std::f64::consts::PI;
    let tolerance = (80.0 + 6.0 * std::f64::consts::PI) * SECTION_SAMPLING_TOLERANCE_MM
        + std::f64::consts::PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2);
    assert!((areas.iter().sum::<f64>() - expected).abs() <= tolerance);
    for (_, points) in &loops {
        for point in points {
            assert!(point[2].abs() <= SECTION_PLANE_TOLERANCE_MM);
        }
    }
    let empty = section(
        &mut client,
        PlaneMm {
            origin_mm: [0.0, 0.0, 40.0],
            normal: [0.0, 0.0, 1.0],
        },
    )
    .await;
    let Some(JobResult::Section { artifact_id }) = empty.result else {
        panic!("expected empty section artifact");
    };
    let (empty, loops) = section_loops(&mut client, artifact_id).await;
    assert!(loops.is_empty());
    assert_eq!(empty.chunk_count, 0);
    assert_eq!(empty.total_bytes, 0);
    let tangent = section(
        &mut client,
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        },
    )
    .await;
    assert_eq!(tangent.status, JobStatus::Failed);
    assert_eq!(
        geometry_job_error(tangent),
        GeometryErrorCode::DegenerateSection
    );
    client.ping().await.unwrap();
    assert_eq!(scene(&mut client).await, before);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn multi_part_world_sections_and_large_origin_preserve_unique_meshes() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    import(&mut client, "box-mm.step", RigidPoseMm::IDENTITY).await;
    let second = import(
        &mut client,
        "cylinder.step",
        RigidPoseMm {
            translation_mm: [40.0, 0.0, 0.0],
            ..RigidPoseMm::IDENTITY
        },
    )
    .await;
    let defs = definitions(&mut client, &second).await;
    let box_def = defs.iter().find(|d| d.face_count == 6).unwrap();
    let GeometryResponse::SceneChanged { summary } = command(
        &mut client,
        GeometryCommand::AddInstance {
            session_id: second.session_id.clone(),
            base_revision: second.revision,
            definition_id: box_def.definition_id.clone(),
            pose: RigidPoseMm {
                translation_mm: [80.0, 0.0, 0.0],
                rotation_xyzw: [
                    0.0,
                    0.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                    std::f64::consts::FRAC_1_SQRT_2,
                ],
            },
        },
    )
    .await
    else {
        panic!("expected third occurrence");
    };
    assert_eq!(summary.definition_count, 2);
    assert_eq!(summary.occurrence_count, 3);
    let records = occurrences(&mut client, &summary).await;
    let mut original_hashes = vec![];
    for d in &defs {
        original_hashes.push(mesh(&mut client, d).await);
    }
    let job = section(
        &mut client,
        PlaneMm {
            origin_mm: [0.0, 0.0, 4.0],
            normal: [0.0, 0.0, 1.0],
        },
    )
    .await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    let Some(JobResult::Section { artifact_id }) = job.result else {
        panic!("expected combined section");
    };
    let (metadata, loops) = section_loops(&mut client, artifact_id).await;
    assert_eq!(loops.len(), 3);
    for ((row, points), record) in loops.iter().zip(&records) {
        assert_eq!(row.occurrence_id, record.occurrence_id);
        assert_eq!(row.definition_id, record.definition_id);
        let measured = area(points, metadata.frame);
        let expected = if row.definition_id == box_def.definition_id {
            200.0
        } else {
            25.0 * std::f64::consts::PI
        };
        let perimeter = if row.definition_id == box_def.definition_id {
            60.0
        } else {
            10.0 * std::f64::consts::PI
        };
        assert!(
            (measured - expected).abs()
                <= perimeter * 0.005 + std::f64::consts::PI * 0.005f64.powi(2)
        );
    }
    for record in &records {
        let current = scene(&mut client).await;
        let mut pose = record.pose;
        pose.translation_mm = pose.translation_mm.map(|v| v + 1e9);
        assert!(matches!(
            command(
                &mut client,
                GeometryCommand::SetInstancePose {
                    session_id: current.session_id,
                    base_revision: current.revision,
                    occurrence_id: record.occurrence_id,
                    pose
                }
            )
            .await,
            GeometryResponse::SceneChanged { .. }
        ));
    }
    for (d, hashes) in defs.iter().zip(original_hashes) {
        assert_eq!(mesh(&mut client, d).await, hashes);
    }
    let job = section(
        &mut client,
        PlaneMm {
            origin_mm: [1e9, 1e9, 1e9 + 4.0],
            normal: [0.0, 0.0, 1.0],
        },
    )
    .await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    let Some(JobResult::Section { artifact_id }) = job.result else {
        panic!("expected large-origin section");
    };
    let (_, large_loops) = section_loops(&mut client, artifact_id).await;
    assert_eq!(large_loops.len(), 3);
    for ((_, small), (_, large)) in loops.iter().zip(&large_loops) {
        assert_eq!(small.len(), large.len());
        for (a, b) in small.iter().zip(large) {
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 1e-4);
            }
        }
    }
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_busy_and_stale_publication_leave_committed_scene() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let before = import(&mut client, "box-mm.step", RigidPoseMm::IDENTITY).await;
    let pid = client.pid();
    let job_id = start_import(
        &mut client,
        fixture("perforated-plate.step"),
        RigidPoseMm::IDENTITY,
    )
    .await;
    let started = Instant::now();
    let response = command(
        &mut client,
        GeometryCommand::CancelJob {
            session_id: before.session_id.clone(),
            job_id,
        },
    )
    .await;
    assert!(
        started.elapsed() <= Duration::from_millis(500),
        "cancel exchange was blocked by native work"
    );
    let GeometryResponse::Job { job } = response else {
        panic!("expected cancel status");
    };
    assert_eq!(job.status, JobStatus::Cancelling);
    let cancelled = wait_job(&mut client, job_id).await;
    assert_eq!(cancelled.status, JobStatus::Cancelled, "{cancelled:?}");
    assert_eq!(scene(&mut client).await, before);
    assert_eq!(client.pid(), pid);
    let job_id = start_import(
        &mut client,
        fixture("perforated-plate.step"),
        RigidPoseMm::IDENTITY,
    )
    .await;
    expect_error(
        command(
            &mut client,
            GeometryCommand::StartSection {
                session_id: before.session_id.clone(),
                base_revision: before.revision,
                plane: PlaneMm {
                    origin_mm: [0.0, 0.0, 4.0],
                    normal: [0.0, 0.0, 1.0],
                },
            },
        )
        .await,
        GeometryErrorCode::Busy,
    );
    let occurrence = occurrences(&mut client, &before).await.remove(0);
    let started = Instant::now();
    let GeometryResponse::SceneChanged { summary: moved } = command(
        &mut client,
        GeometryCommand::SetInstancePose {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            occurrence_id: occurrence.occurrence_id,
            pose: RigidPoseMm {
                translation_mm: [10.0, 0.0, 0.0],
                ..RigidPoseMm::IDENTITY
            },
        },
    )
    .await
    else {
        panic!("pose command must remain responsive");
    };
    assert!(
        started.elapsed() <= Duration::from_millis(500),
        "pose exchange was blocked by native work"
    );
    let stale_job = wait_job(&mut client, job_id).await;
    assert_eq!(stale_job.status, JobStatus::Failed, "{stale_job:?}");
    assert_eq!(
        geometry_job_error(stale_job),
        GeometryErrorCode::StaleRevision
    );
    assert_eq!(scene(&mut client).await, moved);
    assert_eq!(client.pid(), pid);
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn twenty_remove_reimport_cycles_keep_live_scene_and_artifact_pins_bounded() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let mut stable = None;
    for _ in 0..20 {
        let summary = import(&mut client, "box-mm.step", RigidPoseMm::IDENTITY).await;
        assert_eq!(summary.definition_count, 1);
        let definition = definitions(&mut client, &summary).await.remove(0);
        if let Some(id) = &stable {
            assert_eq!(&definition.definition_id, id);
        } else {
            stable = Some(definition.definition_id.clone());
        }
        let occurrence = occurrences(&mut client, &summary).await.remove(0);
        assert!(matches!(
            command(
                &mut client,
                GeometryCommand::ReleaseArtifact {
                    session_id: summary.session_id,
                    artifact_id: definition.mesh_artifact_id
                }
            )
            .await,
            GeometryResponse::Released {}
        ));
        let empty = remove(&mut client, occurrence.occurrence_id).await;
        assert_eq!(empty.unique_mesh_bytes, 0);
        assert_eq!(empty.definition_count, 0);
        let empty = discard_project_history(&mut client).await;
        expect_error(
            command(
                &mut client,
                GeometryCommand::GetArtifactPage {
                    session_id: empty.session_id,
                    artifact_id: definition.mesh_artifact_id,
                    kind: ArtifactPageKind::Chunks,
                    offset: 0,
                },
            )
            .await,
            GeometryErrorCode::UnknownHandle,
        );
    }
    client.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn native_source_paths_preserve_non_utf8_bytes_and_reject_nonregular_handles() {
    use std::os::unix::ffi::OsStringExt;
    let directory =
        std::env::temp_dir().join(format!("spiling-engine-{}", SessionId::new().as_str()));
    std::fs::create_dir(&directory).unwrap();
    let filename = std::ffi::OsString::from_vec(b"part-\xff.step".to_vec());
    let source = directory.join(filename);
    let original =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry/box-mm.step");
    std::fs::copy(&original, &source).unwrap();
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let job = start_import(
        &mut client,
        NativePath::from_os_str(source.as_os_str()).unwrap(),
        RigidPoseMm::IDENTITY,
    )
    .await;
    assert_eq!(
        wait_job(&mut client, job).await.status,
        JobStatus::Completed
    );
    let summary = scene(&mut client).await;
    let definition = definitions(&mut client, &summary).await.remove(0);
    assert!(
        definition
            .provenance
            .source_hash
            .matches_bytes(&std::fs::read(&original).unwrap())
    );
    assert_eq!(definition.provenance.source_name, "part-�.step");
    let job = start_import(
        &mut client,
        NativePath::from_os_str(directory.as_os_str()).unwrap(),
        RigidPoseMm::IDENTITY,
    )
    .await;
    assert_eq!(
        geometry_job_error(wait_job(&mut client, job).await),
        GeometryErrorCode::SourceIo
    );
    assert_eq!(scene(&mut client).await, summary);
    client.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[tokio::test]
async fn frozen_plate_transfers_real_bounded_chunks_and_releases_the_lease() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let summary = import(&mut client, "perforated-plate.step", RigidPoseMm::IDENTITY).await;
    assert!(summary.unique_mesh_bytes > 4 * 1024 * 1024);
    assert!(summary.unique_mesh_bytes <= MAX_SCENE_MESH_BYTES);
    let definition = definitions(&mut client, &summary).await.remove(0);
    let hashes = mesh(&mut client, &definition).await;
    assert!(hashes.len() > 4);
    let occurrence = occurrences(&mut client, &summary).await.remove(0);
    // Replacement releases native/history pins; the consumer lease still owns packed bytes.
    let empty = remove(&mut client, occurrence.occurrence_id).await;
    assert_eq!(empty.definition_count, 0);
    let empty = discard_project_history(&mut client).await;
    assert_eq!(mesh(&mut client, &definition).await, hashes);
    assert!(matches!(
        command(
            &mut client,
            GeometryCommand::ReleaseArtifact {
                session_id: empty.session_id.clone(),
                artifact_id: definition.mesh_artifact_id
            }
        )
        .await,
        GeometryResponse::Released {}
    ));
    expect_error(
        command(
            &mut client,
            GeometryCommand::GetArtifactPage {
                session_id: empty.session_id,
                artifact_id: definition.mesh_artifact_id,
                kind: ArtifactPageKind::Chunks,
                offset: 0,
            },
        )
        .await,
        GeometryErrorCode::UnknownHandle,
    );
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_job_eviction_does_not_retain_artifact_leases() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let session_id = client.hello().session_id.clone();
    let mut first = None;
    for _ in 0..MAX_TERMINAL_JOBS + 1 {
        let job = section(
            &mut client,
            PlaneMm {
                origin_mm: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
            },
        )
        .await;
        assert_eq!(job.status, JobStatus::Completed);
        if first.is_none() {
            first = Some(job.job_id);
        }
        let Some(JobResult::Section { artifact_id }) = job.result else {
            panic!("expected empty scene section");
        };
        assert!(matches!(
            command(
                &mut client,
                GeometryCommand::ReleaseArtifact {
                    session_id: session_id.clone(),
                    artifact_id
                }
            )
            .await,
            GeometryResponse::Released {}
        ));
        expect_error(
            command(
                &mut client,
                GeometryCommand::GetArtifactPage {
                    session_id: session_id.clone(),
                    artifact_id,
                    kind: ArtifactPageKind::Chunks,
                    offset: 0,
                },
            )
            .await,
            GeometryErrorCode::UnknownHandle,
        );
    }
    expect_error(
        command(
            &mut client,
            GeometryCommand::GetJob {
                session_id,
                job_id: first.unwrap(),
            },
        )
        .await,
        GeometryErrorCode::UnknownHandle,
    );
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
}
