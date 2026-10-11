// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use sha2::{Digest, Sha256};
use spiling_contracts::{
    NativeOperationView, display::validate_section_chunk, geometry::*, project::*,
};
use spiling_engine_client::{ClientError, EngineClient};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::{process::Command, time::timeout};

const ENGINE: &str = env!("CARGO_BIN_EXE_spiling-engine");

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("spiling-project-{}", SessionId::new().as_str()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn source(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        std::fs::copy(fixture(name), &path).unwrap();
        path
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name)
}
fn native(path: &Path) -> NativePath {
    NativePath::from_os_str(path.as_os_str()).unwrap()
}
fn translated(x: f64) -> RigidPoseMm {
    RigidPoseMm {
        translation_mm: [x, 0.0, 0.0],
        ..RigidPoseMm::IDENTITY
    }
}
async fn spawn() -> EngineClient {
    EngineClient::spawn(ENGINE).await.unwrap()
}
async fn scene(client: &mut EngineClient) -> SceneSummary {
    let session_id = client.hello().session_id.clone();
    let GeometryResponse::Scene { summary } = client
        .geometry(GeometryCommand::GetScene { session_id })
        .await
        .unwrap()
    else {
        panic!("expected scene");
    };
    summary.validate().unwrap();
    summary
}
async fn info(client: &mut EngineClient) -> ProjectInfo {
    let session_id = client.hello().session_id.clone();
    let ProjectResponse::Status { info } = client
        .project(ProjectCommand::Get { session_id })
        .await
        .unwrap()
    else {
        panic!("expected project info");
    };
    info
}
async fn wait_job(client: &mut EngineClient, job_id: String) -> NativeOperationView {
    let start = Instant::now();
    loop {
        assert!(
            start.elapsed() < Duration::from_secs(70),
            "job did not terminate"
        );
        let job = client.native_operation(&job_id).await.unwrap();
        if job.status.is_terminal() {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn completed_scene(job: NativeOperationView) -> SceneSummary {
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    assert!(job.error.is_none(), "{job:?}");
    let Some(JobResult::Scene { summary }) = job.result else {
        panic!("expected scene job result");
    };
    summary
}
fn project_job_error(job: &NativeOperationView, code: ProjectErrorCode) {
    assert_eq!(job.status, JobStatus::Failed, "{job:?}");
    assert!(job.result.is_none(), "{job:?}");
    assert!(
        matches!(&job.error, Some(JobError::Project { error }) if error.code == code),
        "{job:?}"
    );
}
fn project_error<T: std::fmt::Debug>(result: Result<T, ClientError>, code: ProjectErrorCode) {
    match result {
        Err(ClientError::Project(error)) => assert_eq!(error.code, code),
        other => panic!("expected {code:?}, got {other:?}"),
    }
}
async fn project_rejection(
    client: &mut EngineClient,
    command: ProjectCommand,
    code: ProjectErrorCode,
) {
    match client.project(command).await {
        Ok(ProjectResponse::OperationAccepted { operation }) => {
            let job = wait_job(client, operation.name).await;
            project_job_error(&job, code);
        }
        result => project_error(result, code),
    }
}
async fn start_open(
    client: &mut EngineClient,
    path: &Path,
    read_only: bool,
    recover_previous: bool,
    discard_changes: bool,
) -> String {
    let summary = scene(client).await;
    let ProjectResponse::OperationAccepted { operation } = client
        .project(ProjectCommand::Open {
            session_id: summary.session_id,
            base_revision: summary.revision,
            path: native(path),
            read_only,
            recover_previous,
            discard_changes,
        })
        .await
        .unwrap()
    else {
        panic!("expected asynchronous open");
    };
    operation.name
}
async fn open(
    client: &mut EngineClient,
    path: &Path,
    read_only: bool,
    recover_previous: bool,
    discard_changes: bool,
) -> SceneSummary {
    let job_id = start_open(client, path, read_only, recover_previous, discard_changes).await;
    completed_scene(wait_job(client, job_id).await)
}
async fn start_save(client: &mut EngineClient, target: Option<&Path>) -> String {
    let summary = scene(client).await;
    let ProjectResponse::OperationAccepted { operation } = client
        .project(ProjectCommand::Save {
            session_id: summary.session_id,
            base_revision: summary.revision,
            target: target.map(native),
        })
        .await
        .unwrap()
    else {
        panic!("expected asynchronous save");
    };
    operation.name
}
async fn save(client: &mut EngineClient, target: Option<&Path>) -> ProjectInfo {
    let before = scene(client).await;
    let id = start_save(client, target).await;
    let job = wait_job(client, id).await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    assert!(job.error.is_none(), "{job:?}");
    let Some(JobResult::ProjectSaved { info: saved }) = job.result else {
        panic!("expected saved project result");
    };
    assert!(!saved.dirty);
    assert_eq!(saved.saved_revision, Some(saved.revision));
    assert_eq!(
        scene(client).await,
        before,
        "save must not change the scene epoch"
    );
    assert_eq!(info(client).await, saved);
    saved
}
async fn new_project(
    client: &mut EngineClient,
    discard_changes: bool,
) -> (ProjectInfo, SceneSummary) {
    let summary = scene(client).await;
    let ProjectResponse::SceneChanged { info, summary } = client
        .project(ProjectCommand::New {
            session_id: summary.session_id,
            base_revision: summary.revision,
            discard_changes,
        })
        .await
        .unwrap()
    else {
        panic!("expected new project");
    };
    (info, summary)
}
async fn history(client: &mut EngineClient, redo: bool) -> (ProjectInfo, SceneSummary) {
    let summary = scene(client).await;
    let command = if redo {
        ProjectCommand::Redo {
            session_id: summary.session_id,
            base_revision: summary.revision,
        }
    } else {
        ProjectCommand::Undo {
            session_id: summary.session_id,
            base_revision: summary.revision,
        }
    };
    let ProjectResponse::SceneChanged { info, summary } = client.project(command).await.unwrap()
    else {
        panic!("expected history transaction");
    };
    (info, summary)
}
async fn import(client: &mut EngineClient, source: &Path, pose: RigidPoseMm) -> SceneSummary {
    let summary = scene(client).await;
    let GeometryResponse::OperationAccepted { operation } = client
        .geometry(GeometryCommand::ImportPart {
            session_id: summary.session_id,
            base_revision: summary.revision,
            source: native(source),
            initial_pose: pose,
        })
        .await
        .unwrap()
    else {
        panic!("expected import job");
    };
    let job_id = operation.name;
    completed_scene(wait_job(client, job_id).await)
}
async fn edit(
    client: &mut EngineClient,
    operation: impl FnOnce(SceneSummary) -> GeometryCommand,
) -> SceneSummary {
    let summary = scene(client).await;
    let GeometryResponse::SceneChanged { summary } =
        client.geometry(operation(summary)).await.unwrap()
    else {
        panic!("expected authoring transaction");
    };
    summary
}
async fn add(
    client: &mut EngineClient,
    definition_id: DefinitionId,
    pose: RigidPoseMm,
) -> SceneSummary {
    edit(client, |s| GeometryCommand::AddInstance {
        session_id: s.session_id,
        base_revision: s.revision,
        definition_id,
        pose,
    })
    .await
}
async fn pose(
    client: &mut EngineClient,
    occurrence_id: OccurrenceId,
    pose: RigidPoseMm,
) -> SceneSummary {
    edit(client, |s| GeometryCommand::SetInstancePose {
        session_id: s.session_id,
        base_revision: s.revision,
        occurrence_id,
        pose,
    })
    .await
}
async fn remove(client: &mut EngineClient, occurrence_id: OccurrenceId) -> SceneSummary {
    edit(client, |s| GeometryCommand::RemoveInstance {
        session_id: s.session_id,
        base_revision: s.revision,
        occurrence_id,
    })
    .await
}
async fn records(client: &mut EngineClient) -> (Vec<DefinitionRecord>, Vec<OccurrenceRecord>) {
    let summary = scene(client).await;
    let mut definitions = vec![];
    let mut occurrences = vec![];
    for kind in [ScenePageKind::Definitions, ScenePageKind::Occurrences] {
        let mut offset = 0;
        loop {
            let GeometryResponse::ScenePage {
                definitions: d,
                occurrences: o,
                next_offset,
            } = client
                .geometry(GeometryCommand::GetScenePage {
                    session_id: summary.session_id.clone(),
                    revision: summary.revision,
                    kind,
                    offset,
                })
                .await
                .unwrap()
            else {
                panic!("expected scene records");
            };
            definitions.extend(d);
            occurrences.extend(o);
            match next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
    }
    definitions.sort_by(|a, b| a.definition_id.cmp(&b.definition_id));
    occurrences.sort_by_key(|o| o.occurrence_id);
    assert_eq!(definitions.len(), summary.definition_count as usize);
    assert_eq!(occurrences.len(), summary.occurrence_count as usize);
    (definitions, occurrences)
}
async fn faces(client: &mut EngineClient, definition: &DefinitionRecord) -> Vec<FaceIndexRow> {
    let mut result = vec![];
    let mut offset = 0;
    loop {
        let GeometryResponse::FaceIndexPage { faces, next_offset } = client
            .geometry(GeometryCommand::GetFaceIndexPage {
                session_id: client.hello().session_id.clone(),
                definition_id: definition.definition_id.clone(),
                offset,
            })
            .await
            .unwrap()
        else {
            panic!("expected native face index");
        };
        result.extend(faces);
        match next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(result.len(), definition.face_count as usize);
    result
}
#[derive(Debug, PartialEq)]
struct NativeSnapshot {
    definitions: Vec<(StoredDefinition, AabbMm, Vec<FaceIndexRow>)>,
    occurrences: Vec<OccurrenceRecord>,
}
async fn native_snapshot(client: &mut EngineClient) -> NativeSnapshot {
    let (definitions, occurrences) = records(client).await;
    let mut native_definitions = vec![];
    for definition in definitions {
        let face_table = faces(client, &definition).await;
        native_definitions.push((
            StoredDefinition {
                definition_id: definition.definition_id,
                provenance: definition.provenance,
            },
            definition.bounds_mm,
            face_table,
        ));
    }
    NativeSnapshot {
        definitions: native_definitions,
        occurrences,
    }
}
async fn reference(client: &mut EngineClient) -> FaceRef {
    let summary = scene(client).await;
    let (definitions, occurrences) = records(client).await;
    let occurrence = &occurrences[0];
    let definition = definitions
        .iter()
        .find(|d| d.definition_id == occurrence.definition_id)
        .unwrap();
    let face_id = faces(client, definition).await[0].face_id.clone();
    FaceRef {
        session_id: summary.session_id,
        scene_revision: summary.revision,
        occurrence_id: occurrence.occurrence_id,
        definition_id: occurrence.definition_id.clone(),
        face_id,
    }
}
async fn inspect(client: &mut EngineClient, reference: FaceRef) -> FaceInspection {
    let GeometryResponse::FaceInspection { inspection } = client
        .geometry(GeometryCommand::InspectFace { reference })
        .await
        .unwrap()
    else {
        panic!("expected native face inspection");
    };
    inspection
}
async fn stale_reference(client: &mut EngineClient, reference: FaceRef) {
    assert!(
        matches!(client.geometry(GeometryCommand::InspectFace { reference }).await.unwrap(), GeometryResponse::Error { error } if error.code == GeometryErrorCode::StaleRevision)
    );
    client.ping().await.unwrap();
}
async fn section_areas(client: &mut EngineClient) -> BTreeMap<OccurrenceId, f64> {
    let before = scene(client).await;
    let GeometryResponse::OperationAccepted { operation } = client
        .geometry(GeometryCommand::StartSection {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            plane: PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
        })
        .await
        .unwrap()
    else {
        panic!("expected native section job");
    };
    let job_id = operation.name;
    let job = wait_job(client, job_id).await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    let Some(JobResult::Section { artifact_id }) = job.result else {
        panic!("expected section artifact");
    };
    let mut rows = vec![];
    let mut chunks = vec![];
    let mut section = None;
    for kind in [ArtifactPageKind::Loops, ArtifactPageKind::Chunks] {
        let mut offset = 0;
        loop {
            let GeometryResponse::ArtifactPage {
                summary,
                chunks: c,
                resources,
                loops,
                next_offset,
            } = client
                .geometry(GeometryCommand::GetArtifactPage {
                    session_id: before.session_id.clone(),
                    artifact_id,
                    kind,
                    offset,
                })
                .await
                .unwrap()
            else {
                panic!("expected section page");
            };
            let ArtifactSummary::Section { summary } = summary else {
                panic!("expected section summary");
            };
            assert_eq!(summary.revision, before.revision);
            section = Some(summary);
            rows.extend(loops);
            assert_eq!(resources.len(), c.len());
            chunks.extend(c.into_iter().zip(resources));
            match next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
    }
    let summary = section.unwrap();
    let mut result = BTreeMap::new();
    let mut loop_count = 0;
    let dot = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
    let artifact_summary = ArtifactSummary::Section {
        summary: summary.clone(),
    };
    for (chunk, resource) in chunks {
        let bytes = client
            .read_geometry_chunk(&resource, &chunk, &artifact_summary)
            .await
            .unwrap();
        let ArtifactChunkMetadata::Section { metadata } = chunk else {
            panic!("expected section chunk");
        };
        let decoded = validate_section_chunk(
            &bytes,
            &metadata,
            &summary,
            &before.session_id,
            artifact_id,
            metadata.chunk_index,
        )
        .unwrap();
        for local in 0..metadata.loop_count as usize {
            let start = decoded.loop_offsets.get(local).unwrap() as usize;
            let end = decoded.loop_offsets.get(local + 1).unwrap() as usize;
            let points: Vec<_> = (start..end)
                .map(|index| decoded.points_relative_mm.get(index).unwrap())
                .collect();
            let area = points
                .windows(2)
                .map(|p| {
                    dot(p[0], summary.frame.x_axis) * dot(p[1], summary.frame.y_axis)
                        - dot(p[1], summary.frame.x_axis) * dot(p[0], summary.frame.y_axis)
                })
                .sum::<f64>()
                / 2.0;
            let row = &rows[loop_count];
            assert_eq!(row.ordinal as usize, loop_count);
            assert_eq!(row.point_count as usize, points.len());
            assert_eq!(row.is_hole, area < 0.0);
            *result.entry(row.occurrence_id).or_insert(0.0) += area;
            loop_count += 1;
        }
    }
    assert_eq!(loop_count, summary.total_loop_count as usize);
    assert!(matches!(
        client
            .geometry(GeometryCommand::ReleaseArtifact {
                session_id: before.session_id,
                artifact_id
            })
            .await
            .unwrap(),
        GeometryResponse::Released {}
    ));
    result
}
fn assert_analytic_sections(snapshot: &NativeSnapshot, areas: &BTreeMap<OccurrenceId, f64>) {
    assert_eq!(areas.len(), snapshot.occurrences.len());
    for occurrence in &snapshot.occurrences {
        let definition = snapshot
            .definitions
            .iter()
            .find(|d| d.0.definition_id == occurrence.definition_id)
            .unwrap();
        let (expected, perimeter) = if definition.2.len() == 6 {
            (200.0, 60.0)
        } else {
            (25.0 * std::f64::consts::PI, 10.0 * std::f64::consts::PI)
        };
        let tolerance = perimeter * SECTION_SAMPLING_TOLERANCE_MM
            + std::f64::consts::PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2);
        assert!((areas[&occurrence.occurrence_id] - expected).abs() <= tolerance);
    }
}

#[tokio::test]
async fn durable_authoring_roundtrip_preserves_sources_occurrences_faces_and_native_sections() {
    let workspace = Workspace::new();
    let project_path = workspace.path("assembly");
    let box_path = workspace.source("box-mm.step");
    let cylinder_path = workspace.source("cylinder.step");
    let mut client = spawn().await;
    let initial = info(&mut client).await;
    assert!(!initial.read_only);
    assert!(!initial.can_undo);
    assert!(initial.path_label.is_none());
    let (created, empty) = new_project(&mut client, false).await;
    assert_ne!(created.project_id, initial.project_id);
    assert_eq!(empty.occurrence_count, 0);
    project_rejection(
        &mut client,
        ProjectCommand::Save {
            session_id: empty.session_id.clone(),
            base_revision: empty.revision,
            target: None,
        },
        ProjectErrorCode::NoSavedPath,
    )
    .await;
    assert_eq!(info(&mut client).await, created);
    import(&mut client, &box_path, RigidPoseMm::IDENTITY).await;
    import(&mut client, &cylinder_path, translated(40.0)).await;
    let (definitions, _) = records(&mut client).await;
    let box_id = definitions
        .iter()
        .find(|d| d.face_count == 6)
        .unwrap()
        .definition_id
        .clone();
    let rotated = RigidPoseMm {
        translation_mm: [80.0, 0.0, 0.0],
        rotation_xyzw: [
            0.0,
            0.0,
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
        ],
    };
    add(&mut client, box_id, rotated).await;
    let (_, occurrences) = records(&mut client).await;
    let third = occurrences[2].occurrence_id;
    let before_pose = info(&mut client).await;
    let moved = RigidPoseMm {
        translation_mm: [100.0, 0.0, 0.0],
        ..rotated
    };
    pose(&mut client, third, moved).await;
    let edited = info(&mut client).await;
    assert!(edited.revision > before_pose.revision);
    let (undone, _) = history(&mut client, false).await;
    assert!(undone.revision > edited.revision);
    assert_eq!(records(&mut client).await.1[2].pose, rotated);
    let (redone, _) = history(&mut client, true).await;
    assert!(redone.revision > undone.revision);
    assert_eq!(records(&mut client).await.1[2].pose, moved);
    let saved = save(&mut client, Some(&project_path)).await;
    assert!(saved.can_undo);
    remove(&mut client, third).await;
    assert!(info(&mut client).await.dirty);
    let (equal_to_saved, _) = history(&mut client, false).await;
    assert!(
        !equal_to_saved.dirty,
        "semantic equality, not revision equality, owns dirty"
    );
    assert!(equal_to_saved.revision > saved.revision);
    assert!(equal_to_saved.can_redo);
    let checkpoint = save(&mut client, None).await;
    assert!(
        checkpoint.can_undo && checkpoint.can_redo,
        "save retains both history stacks"
    );
    let (removed_again, _) = history(&mut client, true).await;
    assert!(removed_again.dirty);
    history(&mut client, false).await;
    assert!(!info(&mut client).await.dirty);
    let captured = native_snapshot(&mut client).await;
    assert_eq!(captured.definitions.len(), 2);
    assert_eq!(captured.occurrences.len(), 3);
    let sections = section_areas(&mut client).await;
    assert_analytic_sections(&captured, &sections);
    let old_reference = reference(&mut client).await;
    let native_face = inspect(&mut client, old_reference.clone()).await;
    let old_session = client.hello().session_id.clone();
    client.shutdown().await.unwrap();
    std::fs::remove_file(box_path).unwrap();
    std::fs::remove_file(cylinder_path).unwrap();
    let mut reopened = spawn().await;
    assert_ne!(reopened.hello().session_id, old_session);
    open(&mut reopened, &project_path, false, false, false).await;
    let loaded = info(&mut reopened).await;
    assert_eq!(loaded.project_id, checkpoint.project_id);
    assert_eq!(loaded.revision, checkpoint.revision);
    assert!(!loaded.dirty && !loaded.can_undo && !loaded.can_redo);
    assert_eq!(native_snapshot(&mut reopened).await, captured);
    let reopened_sections = section_areas(&mut reopened).await;
    assert_analytic_sections(&captured, &reopened_sections);
    for (occurrence, area) in sections {
        assert!((reopened_sections[&occurrence] - area).abs() <= SECTION_SAMPLING_TOLERANCE_MM);
    }
    stale_reference(&mut reopened, old_reference).await;
    let fresh_reference = reference(&mut reopened).await;
    let fresh_face = inspect(&mut reopened, fresh_reference).await;
    assert_eq!(fresh_face.face, native_face.face);
    assert_eq!(fresh_face.provenance, native_face.provenance);
    assert_eq!(fresh_face.pose, native_face.pose);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn dirty_replacement_requires_discard_and_same_path_writer_reopen_invalidates_old_references()
{
    let workspace = Workspace::new();
    let path = workspace.path("project");
    let mut client = spawn().await;
    import(&mut client, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
    save(&mut client, Some(&path)).await;
    let saved = native_snapshot(&mut client).await;
    let occurrence = saved.occurrences[0].occurrence_id;
    pose(&mut client, occurrence, translated(15.0)).await;
    let before = scene(&mut client).await;
    let dirty = info(&mut client).await;
    project_error(
        client
            .project(ProjectCommand::New {
                session_id: before.session_id.clone(),
                base_revision: before.revision,
                discard_changes: false,
            })
            .await,
        ProjectErrorCode::DirtyProject,
    );
    project_rejection(
        &mut client,
        ProjectCommand::Open {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            path: native(&path),
            read_only: false,
            recover_previous: false,
            discard_changes: false,
        },
        ProjectErrorCode::DirtyProject,
    )
    .await;
    assert_eq!(scene(&mut client).await, before);
    assert_eq!(info(&mut client).await, dirty);
    history(&mut client, false).await;
    let clean_epoch = scene(&mut client).await;
    let old_reference = reference(&mut client).await;
    let reopened = open(&mut client, &path, false, false, false).await;
    assert_eq!(reopened.session_id, clean_epoch.session_id);
    assert!(reopened.revision > clean_epoch.revision);
    assert_eq!(native_snapshot(&mut client).await, saved);
    stale_reference(&mut client, old_reference).await;
    import(&mut client, &fixture("cylinder.step"), translated(40.0)).await;
    let discarded_id = records(&mut client).await.1[1].occurrence_id;
    let discarded_revision = info(&mut client).await.revision;
    let discarded_reference = reference(&mut client).await;
    assert!(info(&mut client).await.dirty);
    open(&mut client, &path, false, false, true).await;
    assert_eq!(native_snapshot(&mut client).await, saved);
    assert!(
        info(&mut client).await.revision >= discarded_revision,
        "same-project replacement cannot rewind authoring revision"
    );
    stale_reference(&mut client, discarded_reference).await;
    add(
        &mut client,
        saved.occurrences[0].definition_id.clone(),
        translated(60.0),
    )
    .await;
    assert!(records(&mut client).await.1[1].occurrence_id > discarded_id);
    assert!(info(&mut client).await.revision > discarded_revision);
    open(&mut client, &path, false, false, true).await;
    let old = reference(&mut client).await;
    pose(&mut client, occurrence, translated(20.0)).await;
    let old_id = info(&mut client).await.project_id;
    let (new, empty) = new_project(&mut client, true).await;
    assert_ne!(new.project_id, old_id);
    assert_eq!(empty.occurrence_count, 0);
    assert!(!new.can_undo && !new.can_redo);
    stale_reference(&mut client, old).await;
    // Replacing the attached project releases the lock; another writer can now open it.
    let mut another = spawn().await;
    open(&mut another, &path, false, false, false).await;
    another.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_history_pins_survive_final_removal_and_history_is_bounded_without_id_reuse() {
    let workspace = Workspace::new();
    let mut client = spawn().await;
    import(&mut client, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
    save(&mut client, Some(&workspace.path("project"))).await;
    let original = native_snapshot(&mut client).await;
    let occurrence = original.occurrences[0].occurrence_id;
    let definition_id = original.occurrences[0].definition_id.clone();
    let original_areas = section_areas(&mut client).await;
    let empty = remove(&mut client, occurrence).await;
    assert_eq!(empty.occurrence_count, 0);
    assert_eq!(empty.definition_count, 0);
    let removed = info(&mut client).await;
    assert!(removed.can_undo && removed.dirty);
    let (restored, _) = history(&mut client, false).await;
    assert!(restored.revision > removed.revision);
    assert!(!restored.dirty);
    assert_eq!(native_snapshot(&mut client).await, original);
    assert_eq!(section_areas(&mut client).await, original_areas);
    let current_reference = reference(&mut client).await;
    inspect(&mut client, current_reference).await;
    history(&mut client, true).await;
    history(&mut client, false).await;
    add(&mut client, definition_id.clone(), translated(40.0)).await;
    assert!(!info(&mut client).await.can_redo);
    let allocated = records(&mut client).await.1[1].occurrence_id;
    history(&mut client, false).await;
    add(&mut client, definition_id, translated(60.0)).await;
    let next = records(&mut client).await.1[1].occurrence_id;
    assert!(
        next > allocated,
        "undo cannot rewind the occurrence allocator"
    );
    let revision = info(&mut client).await.revision;
    for index in 1..=70 {
        pose(&mut client, occurrence, translated(index as f64)).await;
    }
    let saved = save(&mut client, None).await;
    assert!(saved.can_undo);
    let mut last_revision = saved.revision;
    let mut count = 0;
    while info(&mut client).await.can_undo {
        let (undone, _) = history(&mut client, false).await;
        assert!(undone.revision > last_revision);
        last_revision = undone.revision;
        count += 1;
        assert!(count <= MAX_PROJECT_HISTORY);
    }
    assert_eq!(count, MAX_PROJECT_HISTORY);
    assert!(last_revision > revision);
    assert_eq!(records(&mut client).await.1[0].pose, translated(6.0));
    let summary = scene(&mut client).await;
    project_error(
        client
            .project(ProjectCommand::Undo {
                session_id: summary.session_id,
                base_revision: summary.revision,
            })
            .await,
        ProjectErrorCode::NoUndo,
    );
    for _ in 0..MAX_PROJECT_HISTORY {
        history(&mut client, true).await;
    }
    assert!(!info(&mut client).await.dirty);
    assert!(!info(&mut client).await.can_redo);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn writer_lock_read_only_snapshot_and_killed_writer_have_actual_process_lifetimes() {
    let workspace = Workspace::new();
    let path = workspace.path("project");
    let mut writer = spawn().await;
    import(&mut writer, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
    let saved = save(&mut writer, Some(&path)).await;
    let original = native_snapshot(&mut writer).await;
    let mut contender = spawn().await;
    import(
        &mut contender,
        &fixture("cylinder.step"),
        RigidPoseMm::IDENTITY,
    )
    .await;
    let contender_scene = scene(&mut contender).await;
    let contender_info = info(&mut contender).await;
    let id = start_open(&mut contender, &path, false, false, true).await;
    project_job_error(
        &wait_job(&mut contender, id).await,
        ProjectErrorCode::ProjectLocked,
    );
    assert_eq!(scene(&mut contender).await, contender_scene);
    assert_eq!(info(&mut contender).await, contender_info);
    contender.ping().await.unwrap();
    let mut reader = spawn().await;
    open(&mut reader, &path, true, false, false).await;
    let read_info = info(&mut reader).await;
    assert!(read_info.read_only && !read_info.dirty);
    assert_eq!(read_info.project_id, saved.project_id);
    assert_eq!(native_snapshot(&mut reader).await, original);
    assert_analytic_sections(&original, &section_areas(&mut reader).await);
    let before = scene(&mut reader).await;
    let occurrence = original.occurrences[0].occurrence_id;
    let definition = original.occurrences[0].definition_id.clone();
    for operation in [
        GeometryCommand::ImportPart {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            source: native(&fixture("cylinder.step")),
            initial_pose: RigidPoseMm::IDENTITY,
        },
        GeometryCommand::AddInstance {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            definition_id: definition,
            pose: translated(40.0),
        },
        GeometryCommand::SetInstancePose {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            occurrence_id: occurrence,
            pose: translated(20.0),
        },
        GeometryCommand::RemoveInstance {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            occurrence_id: occurrence,
        },
    ] {
        match reader.geometry(operation).await {
            Ok(GeometryResponse::OperationAccepted { operation }) => {
                let job = wait_job(&mut reader, operation.name).await;
                project_job_error(&job, ProjectErrorCode::ReadOnly);
            }
            result => project_error(result, ProjectErrorCode::ReadOnly),
        }
    }
    for operation in [
        ProjectCommand::Undo {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
        },
        ProjectCommand::Redo {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
        },
        ProjectCommand::Save {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            target: None,
        },
        ProjectCommand::Save {
            session_id: before.session_id.clone(),
            base_revision: before.revision,
            target: Some(native(&workspace.path("read-only-copy"))),
        },
    ] {
        project_rejection(&mut reader, operation, ProjectErrorCode::ReadOnly).await;
    }
    assert_eq!(scene(&mut reader).await, before);
    assert_eq!(info(&mut reader).await, read_info);
    pose(&mut writer, occurrence, translated(25.0)).await;
    let later = save(&mut writer, None).await;
    assert!(later.revision > read_info.revision);
    assert_eq!(
        native_snapshot(&mut reader).await,
        original,
        "read-only snapshot must not mix later commits"
    );
    writer.terminate().await.unwrap();
    assert!(!writer.status().await.unwrap());
    open(&mut contender, &path, false, false, true).await;
    assert_eq!(info(&mut contender).await.revision, later.revision);
    assert_eq!(records(&mut contender).await.1[0].pose, translated(25.0));
    reader.ping().await.unwrap();
    reader.shutdown().await.unwrap();
    contender.shutdown().await.unwrap();
}

fn copy_project(source: &Path, destination: &Path) {
    std::fs::create_dir(destination).unwrap();
    std::fs::copy(
        source.join("manifest.json"),
        destination.join("manifest.json"),
    )
    .unwrap();
    std::fs::copy(source.join("writer.lock"), destination.join("writer.lock")).unwrap();
    if source.join("previous.json").exists() {
        std::fs::copy(
            source.join("previous.json"),
            destination.join("previous.json"),
        )
        .unwrap();
    }
    std::fs::create_dir(destination.join("sources")).unwrap();
    for entry in std::fs::read_dir(source.join("sources")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(
            entry.path(),
            destination.join("sources").join(entry.file_name()),
        )
        .unwrap();
    }
    std::fs::create_dir(destination.join("manufacturing")).unwrap();
    for entry in std::fs::read_dir(source.join("manufacturing")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(
            entry.path(),
            destination.join("manufacturing").join(entry.file_name()),
        )
        .unwrap();
    }
}
fn manifest(path: &Path) -> ProjectManifest {
    serde_json::from_slice(&std::fs::read(path.join("manifest.json")).unwrap()).unwrap()
}
fn write_manifest(path: &Path, manifest: &ProjectManifest) {
    std::fs::write(
        path.join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
}
fn asset(path: &Path, manifest: &ProjectManifest) -> PathBuf {
    path.join("sources").join(format!(
        "{}.step",
        manifest.definitions[0].provenance.source_hash.as_str()
    ))
}
async fn failed_open_preserves(client: &mut EngineClient, path: &Path, code: ProjectErrorCode) {
    let before_scene = scene(client).await;
    let before_info = info(client).await;
    let before_native = native_snapshot(client).await;
    let old_face = reference(client).await;
    let id = start_open(client, path, false, false, true).await;
    project_job_error(&wait_job(client, id).await, code);
    assert_eq!(scene(client).await, before_scene);
    assert_eq!(info(client).await, before_info);
    assert_eq!(native_snapshot(client).await, before_native);
    inspect(client, old_face).await;
    client.ping().await.unwrap();
}

#[tokio::test]
async fn invalid_manifests_assets_and_native_admission_preserve_the_entire_live_project() {
    let workspace = Workspace::new();
    let valid = workspace.path("valid");
    let mut client = spawn().await;
    import(&mut client, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
    save(&mut client, Some(&valid)).await;
    // Keep this writer attachment and history alive across every failed replacement.
    let occurrence = records(&mut client).await.1[0].occurrence_id;
    pose(&mut client, occurrence, translated(10.0)).await;
    let baseline = manifest(&valid);
    for (name, code) in [
        ("invalid-json", ProjectErrorCode::InvalidProject),
        ("unsupported-format", ProjectErrorCode::UnsupportedFormat),
        ("invalid-identity", ProjectErrorCode::InvalidProject),
        ("duplicate-occurrence", ProjectErrorCode::InvalidProject),
        ("invalid-pose", ProjectErrorCode::InvalidProject),
        ("invalid-counter", ProjectErrorCode::InvalidProject),
        ("dangling-definition", ProjectErrorCode::InvalidProject),
        ("provenance-mismatch", ProjectErrorCode::InvalidProject),
        ("missing-asset", ProjectErrorCode::MissingAsset),
        ("corrupt-asset", ProjectErrorCode::CorruptAsset),
    ] {
        let path = workspace.path(name);
        copy_project(&valid, &path);
        let mut changed = baseline.clone();
        match name {
            "invalid-json" => {
                std::fs::write(path.join("manifest.json"), b"{\"format_version\":").unwrap();
            }
            "unsupported-format" => {
                changed.format_version += 1;
                write_manifest(&path, &changed);
            }
            "invalid-identity" => {
                changed.definitions[0].definition_id = DefinitionId::parse("a".repeat(64)).unwrap();
                write_manifest(&path, &changed);
            }
            "duplicate-occurrence" => {
                changed.occurrences.push(changed.occurrences[0].clone());
                write_manifest(&path, &changed);
            }
            "invalid-pose" => {
                changed.occurrences[0].pose.rotation_xyzw = [0.0; 4];
                write_manifest(&path, &changed);
            }
            "invalid-counter" => {
                changed.next_occurrence = changed.occurrences[0].occurrence_id.get();
                write_manifest(&path, &changed);
            }
            "dangling-definition" => {
                changed.occurrences[0].definition_id = DefinitionId::parse("a".repeat(64)).unwrap();
                write_manifest(&path, &changed);
            }
            "provenance-mismatch" => {
                changed.definitions[0].provenance.source_unit = SourceUnit::Inch;
                write_manifest(&path, &changed);
            }
            "missing-asset" => {
                std::fs::remove_file(asset(&path, &changed)).unwrap();
            }
            "corrupt-asset" => {
                std::fs::write(asset(&path, &changed), b"not the captured STEP bytes").unwrap();
            }
            _ => unreachable!(),
        }
        failed_open_preserves(&mut client, &path, code).await;
    }
    let unsupported = workspace.path("native-unsupported");
    copy_project(&valid, &unsupported);
    let bytes = std::fs::read(fixture("unsupported-nurbs.step")).unwrap();
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    let mut changed = baseline.clone();
    let definition_id = DefinitionId::from_source_sha256(&hash);
    changed.definitions[0].definition_id = definition_id.clone();
    changed.definitions[0].provenance.source_hash = SourceHash::from_bytes(&bytes);
    changed.definitions[0].provenance.source_name = "unsupported-nurbs.step".into();
    changed.occurrences[0].definition_id = definition_id;
    std::fs::write(asset(&unsupported, &changed), bytes).unwrap();
    write_manifest(&unsupported, &changed);
    let before_scene = scene(&mut client).await;
    let before_info = info(&mut client).await;
    let before_native = native_snapshot(&mut client).await;
    let id = start_open(&mut client, &unsupported, false, false, true).await;
    let job = wait_job(&mut client, id).await;
    assert_eq!(job.status, JobStatus::Failed, "{job:?}");
    assert!(
        matches!(&job.error, Some(JobError::Geometry { error }) if error.code == GeometryErrorCode::UnsupportedGeometry),
        "{job:?}"
    );
    assert_eq!(scene(&mut client).await, before_scene);
    assert_eq!(info(&mut client).await, before_info);
    assert_eq!(native_snapshot(&mut client).await, before_native);
    // The original lock survived all failures, and the attached save path still works.
    let mut contender = spawn().await;
    let id = start_open(&mut contender, &valid, false, false, false).await;
    project_job_error(
        &wait_job(&mut contender, id).await,
        ProjectErrorCode::ProjectLocked,
    );
    contender.shutdown().await.unwrap();
    save(&mut client, None).await;
    client.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn manifest_and_asset_symlinks_or_nonregular_files_are_not_project_inputs() {
    use std::os::unix::fs::symlink;
    let workspace = Workspace::new();
    let valid = workspace.path("valid");
    let mut client = spawn().await;
    import(&mut client, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
    save(&mut client, Some(&valid)).await;
    for name in ["manifest-symlink", "asset-symlink", "asset-directory"] {
        let path = workspace.path(name);
        copy_project(&valid, &path);
        let target = if name == "manifest-symlink" {
            path.join("manifest.json")
        } else {
            asset(&path, &manifest(&path))
        };
        std::fs::remove_file(&target).unwrap();
        if name == "asset-directory" {
            std::fs::create_dir(&target).unwrap();
        } else {
            let original = if name == "manifest-symlink" {
                valid.join("manifest.json")
            } else {
                asset(&valid, &manifest(&valid))
            };
            symlink(original, target).unwrap();
        }
        failed_open_preserves(&mut client, &path, ProjectErrorCode::Io).await;
    }
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_open_preserves_project_scene_history_and_writer_attachment() {
    let workspace = Workspace::new();
    let heavy_path = workspace.path("heavy");
    let mut builder = spawn().await;
    import(
        &mut builder,
        &fixture("perforated-plate.step"),
        RigidPoseMm::IDENTITY,
    )
    .await;
    save(&mut builder, Some(&heavy_path)).await;
    builder.shutdown().await.unwrap();
    let original_path = workspace.path("original");
    let mut client = spawn().await;
    import(&mut client, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
    save(&mut client, Some(&original_path)).await;
    let occurrence = records(&mut client).await.1[0].occurrence_id;
    pose(&mut client, occurrence, translated(15.0)).await;
    let before_scene = scene(&mut client).await;
    let before_info = info(&mut client).await;
    let before_native = native_snapshot(&mut client).await;
    let pid = client.pid();
    let old_face = reference(&mut client).await;
    let job_id = start_open(&mut client, &heavy_path, false, false, true).await;
    let started = Instant::now();
    client.rpc().cancel_operation(&job_id).await.unwrap();
    let job = client.native_operation(&job_id).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(matches!(
        job.status,
        JobStatus::Cancelling | JobStatus::Cancelled
    ));
    let cancelled = wait_job(&mut client, job_id).await;
    assert_eq!(cancelled.status, JobStatus::Cancelled, "{cancelled:?}");
    assert!(cancelled.result.is_none());
    assert_eq!(scene(&mut client).await, before_scene);
    assert_eq!(info(&mut client).await, before_info);
    assert_eq!(native_snapshot(&mut client).await, before_native);
    assert_eq!(client.pid(), pid);
    inspect(&mut client, old_face).await;
    let (undone, _) = history(&mut client, false).await;
    assert!(!undone.dirty);
    let mut contender = spawn().await;
    let id = start_open(&mut contender, &original_path, false, false, false).await;
    project_job_error(
        &wait_job(&mut contender, id).await,
        ProjectErrorCode::ProjectLocked,
    );
    // A cancelled staged open releases its candidate writer lock as well.
    open(&mut contender, &heavy_path, false, false, false).await;
    // Cancellation before a first-save commit preserves the attached checkpoint;
    // cancellation after commit must truthfully report the committed outcome.
    let heavy_occurrence = records(&mut contender).await.1[0].occurrence_id;
    pose(&mut contender, heavy_occurrence, translated(5.0)).await;
    let save_before = info(&mut contender).await;
    let save_scene = scene(&mut contender).await;
    let save_target = workspace.path("cancel-save");
    let save_id = start_save(&mut contender, Some(&save_target)).await;
    contender.rpc().cancel_operation(&save_id).await.unwrap();
    let outcome = wait_job(&mut contender, save_id).await;
    assert_eq!(scene(&mut contender).await, save_scene);
    assert!(
        matches!(outcome.status, JobStatus::Cancelled | JobStatus::Completed),
        "{outcome:?}"
    );
    if outcome.status == JobStatus::Cancelled {
        assert!(outcome.result.is_none());
        assert_eq!(info(&mut contender).await, save_before);
        assert!(!save_target.exists());
        save(&mut contender, None).await;
    } else {
        assert!(outcome.error.is_none());
        let Some(JobResult::ProjectSaved { info: committed }) = outcome.result else {
            panic!("a commit cannot be reported as cancellation");
        };
        assert!(!committed.dirty && committed.can_undo);
        assert_eq!(committed.revision, save_before.revision);
        assert_eq!(info(&mut contender).await, committed);
        let mut observer = spawn().await;
        open(&mut observer, &save_target, true, false, false).await;
        assert_eq!(records(&mut observer).await.1[0].pose, translated(5.0));
        observer.shutdown().await.unwrap();
    }
    // A cancellation arriving after observed native ACK/commit cannot change the
    // durable receipt or report rollback through the actual gRPC consumer surface.
    let committed_id = start_save(&mut contender, None).await;
    let committed = wait_job(&mut contender, committed_id.clone()).await;
    assert_eq!(committed.status, JobStatus::Completed, "{committed:?}");
    assert!(matches!(
        committed.result,
        Some(JobResult::ProjectSaved { .. })
    ));
    let before_late_cancel = contender.rpc().get_operation(&committed_id).await.unwrap();
    contender
        .rpc()
        .cancel_operation(&committed_id)
        .await
        .unwrap();
    let after_late_cancel = contender.rpc().get_operation(&committed_id).await.unwrap();
    assert_eq!(after_late_cancel, before_late_cancel);
    contender.shutdown().await.unwrap();
    save(&mut client, None).await;
    client.shutdown().await.unwrap();
}

// Scoped fault injection keeps process environment private to this supervised child.
// Authoring, observation and recovery use the same authenticated public gRPC API.
struct FaultChild {
    client: EngineClient,
    store: tempfile::TempDir,
}
impl FaultChild {
    async fn spawn(stage: &str) -> Self {
        let store = tempfile::tempdir().unwrap();
        let mut command = Command::new(ENGINE);
        command.env("SPILING_PROJECT_FAULT", stage);
        let client = EngineClient::spawn_command(command, store.path())
            .await
            .unwrap();
        assert_eq!(Some(client.hello().pid), client.pid());
        Self { client, store }
    }
    async fn open(&mut self, path: &Path) -> SceneSummary {
        open(&mut self.client, path, false, false, false).await
    }
    async fn import(&mut self, source: &Path) -> SceneSummary {
        import(&mut self.client, source, translated(40.0)).await
    }
    async fn crash_save(mut self, target: Option<&Path>) {
        let operation_name = start_save(&mut self.client, target).await;
        let rpc = self.client.rpc();
        let observation = async {
            // A crash may beat subscription; any delivered snapshot must remain unfinished.
            if let Ok(mut stream) = rpc.watch_operation(&operation_name).await {
                while let Ok(Some(operation)) = stream.message().await {
                    assert!(
                        !operation.done,
                        "save completion must not precede injected exit"
                    );
                    let view = spiling_contracts::native_operation_view(&operation).unwrap();
                    assert!(view.result.is_none() && view.error.is_none());
                }
            }
        };
        let exit = async {
            while self.client.status().await.unwrap() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            self.client.wait_exit().await.unwrap()
        };
        let (_, status) = timeout(Duration::from_secs(70), async {
            tokio::join!(observation, exit)
        })
        .await
        .unwrap();
        assert_eq!(
            status.code(),
            Some(86),
            "fault must kill the actual engine process"
        );
        // Restart observation proves no successful save was durably acknowledged before exit.
        let mut observer = EngineClient::spawn_in(ENGINE, self.store.path())
            .await
            .unwrap();
        let retained = observer.native_operation(&operation_name).await.unwrap();
        assert_eq!(retained.status, JobStatus::Interrupted, "{retained:?}");
        assert!(
            retained.result.is_none(),
            "save completion must not precede injected exit"
        );
        observer.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn actual_save_process_faults_publish_only_complete_current_or_explicit_previous_snapshots() {
    let workspace = Workspace::new();
    for stage in [
        "after_assets",
        "before_manifest_replace",
        "after_manifest_replace",
    ] {
        let path = workspace.path(stage);
        let mut seed = spawn().await;
        import(&mut seed, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
        let original = native_snapshot(&mut seed).await;
        let saved = save(&mut seed, Some(&path)).await;
        // Ensure a valid identical previous checkpoint exists even before the next
        // save reaches its checkpoint-publication stage.
        save(&mut seed, None).await;
        seed.shutdown().await.unwrap();
        let mut fault = FaultChild::spawn(stage).await;
        fault.open(&path).await;
        let staged = fault.import(&fixture("cylinder.step")).await;
        assert_eq!(staged.occurrence_count, 2);
        fault.crash_save(None).await;
        let mut current = spawn().await;
        let visible = open(&mut current, &path, false, false, false).await;
        let visible_info = info(&mut current).await;
        assert_eq!(visible_info.project_id, saved.project_id);
        assert!(!visible_info.dirty && !visible_info.recovered_previous);
        if stage == "after_manifest_replace" {
            assert_eq!(visible.occurrence_count, 2);
            assert_eq!(visible_info.revision.get(), saved.revision.get() + 1);
            assert_analytic_sections(
                &native_snapshot(&mut current).await,
                &section_areas(&mut current).await,
            );
        } else {
            assert_eq!(visible.occurrence_count, 1);
            assert_eq!(visible_info.revision, saved.revision);
            assert_eq!(native_snapshot(&mut current).await, original);
        }
        current.shutdown().await.unwrap();
        let mut recovered = spawn().await;
        open(&mut recovered, &path, false, true, false).await;
        let recovered_info = info(&mut recovered).await;
        assert!(recovered_info.recovered_previous && recovered_info.dirty);
        assert_eq!(recovered_info.project_id, saved.project_id);
        assert!(
            recovered_info.revision > saved.revision,
            "recovery cannot rewind the persistent revision"
        );
        assert!(!recovered_info.can_undo && !recovered_info.can_redo);
        assert_eq!(native_snapshot(&mut recovered).await, original);
        assert_analytic_sections(&original, &section_areas(&mut recovered).await);
        recovered.shutdown().await.unwrap();
        // Ordinary open cannot conceal a corrupt current manifest. Explicit
        // recovery validates the previous file independently of current bytes.
        std::fs::write(path.join("manifest.json"), b"broken current checkpoint").unwrap();
        let mut live = spawn().await;
        import(&mut live, &fixture("cylinder.step"), translated(40.0)).await;
        failed_open_preserves(&mut live, &path, ProjectErrorCode::InvalidProject).await;
        open(&mut live, &path, false, true, true).await;
        assert!(info(&mut live).await.recovered_previous);
        assert_eq!(native_snapshot(&mut live).await, original);
        let recovery_scene = scene(&mut live).await;
        let recovery_info = info(&mut live).await;
        std::fs::write(path.join("previous.json"), b"broken previous checkpoint").unwrap();
        let id = start_open(&mut live, &path, false, true, true).await;
        project_job_error(
            &wait_job(&mut live, id).await,
            ProjectErrorCode::InvalidProject,
        );
        assert_eq!(scene(&mut live).await, recovery_scene);
        assert_eq!(info(&mut live).await, recovery_info);
        assert_eq!(native_snapshot(&mut live).await, original);
        live.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn first_save_faults_do_not_strand_the_destination_or_overwrite_foreign_files() {
    let workspace = Workspace::new();
    for stage in [
        "after_assets",
        "before_manifest_replace",
        "after_manifest_replace",
    ] {
        let target = workspace.path(stage);
        let mut fault = FaultChild::spawn(stage).await;
        fault.import(&fixture("cylinder.step")).await;
        fault.crash_save(Some(&target)).await;
        let mut retry = spawn().await;
        if stage == "after_manifest_replace" {
            open(&mut retry, &target, false, false, false).await;
            assert_eq!(scene(&mut retry).await.occurrence_count, 1);
        } else {
            assert!(
                !target.exists(),
                "precommit first-save crash must not occupy the target"
            );
            import(&mut retry, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
            save(&mut retry, Some(&target)).await;
        }
        retry.shutdown().await.unwrap();
    }
    let foreign = workspace.path("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("keep"), b"foreign content").unwrap();
    let mut client = spawn().await;
    import(&mut client, &fixture("box-mm.step"), RigidPoseMm::IDENTITY).await;
    let before = info(&mut client).await;
    let job_id = start_save(&mut client, Some(&foreign)).await;
    let job = wait_job(&mut client, job_id).await;
    project_job_error(&job, ProjectErrorCode::InvalidProject);
    assert_eq!(info(&mut client).await, before);
    assert_eq!(
        std::fs::read(foreign.join("keep")).unwrap(),
        b"foreign content"
    );
    assert!(!foreign.join("manifest.json").exists());
    save(&mut client, Some(&workspace.path("retry"))).await;
    client.shutdown().await.unwrap();
}
