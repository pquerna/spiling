// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{NativeOperationView, geometry::*, manufacturing::*, project::*};
use spiling_engine_client::{ClientError, EngineClient};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const ENGINE: &str = env!("CARGO_BIN_EXE_spiling-engine");
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "spiling-manufacturing-{}",
            SessionId::new().as_str()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn source(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/geometry")
                .join(name),
            &path,
        )
        .unwrap();
        path
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn native(path: &Path) -> NativePath {
    NativePath::from_os_str(path.as_os_str()).unwrap()
}
fn intent() -> ManufacturingIntent {
    serde_json::from_value(serde_json::json!({
        "printer": {
            "schema_version":1,"id":"software-test","revision":"1","name":"Software only",
            "coordinate_frame":"right_handed_millimetres",
            "build_envelope":{"min":[-20.0,-20.0,0.0],"max":[200.0,200.0,100.0]},
            "components":[{"id":"bed","role":"bed","pose":{"translation_mm":[0.0,0.0,0.0],"rotation_xyzw":[0.0,0.0,0.0,1.0]},"shape":{"kind":"box","bounds_mm":{"min":[-20.0,-20.0,-1.0],"max":[200.0,200.0,0.0]}}}],
            "capabilities":{"motion_space":"cartesian_xyz","extruders":1,"output_dialect":"cartesian_absolute_gcode_v1","generated_supports":false},
            "nozzle_diameter_mm":0.4,"max_feed_mm_s":150.0,"max_volumetric_flow_mm3_s":20.0
        },
        "recipe":{"id":"solid","revision":"1","material_id":"nominal","material_revision":"1","filament_diameter_mm":1.75,"layer_height_mm":0.2,"bead_width_mm":0.4,"perimeter_count":1,"infill_fraction":1.0,"print_speed_mm_s":20.0,"travel_speed_mm_s":80.0,"flow_multiplier":1.0}
    })).unwrap()
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
        panic!("scene")
    };
    summary
}
async fn status(client: &mut EngineClient) -> (ProjectInfo, Option<ManufacturingArtifactRecord>) {
    let session_id = client.hello().session_id.clone();
    let ManufacturingResponse::Status { info, artifact, .. } = client
        .manufacturing(ManufacturingCommand::Get { session_id })
        .await
        .unwrap()
    else {
        panic!("status")
    };
    (info, artifact)
}
async fn fetch_bundle(
    client: &mut EngineClient,
    record: &ManufacturingArtifactRecord,
) -> Result<ManufacturingBundle, ClientError> {
    let session_id = client.hello().session_id.clone();
    let ManufacturingResponse::Status {
        resource: Some(resource),
        ..
    } = client
        .manufacturing(ManufacturingCommand::Get { session_id })
        .await?
    else {
        panic!("expected immutable manufacturing ByteStream descriptor");
    };
    client.fetch_manufacturing_bundle(record, &resource).await
}
async fn wait(client: &mut EngineClient, job_id: String) -> NativeOperationView {
    let start = Instant::now();
    loop {
        assert!(start.elapsed() < Duration::from_secs(70), "job deadline");
        let job = client.native_operation(&job_id).await.unwrap();
        if job.status.is_terminal() {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn import(client: &mut EngineClient, source: &Path) {
    let s = scene(client).await;
    let GeometryResponse::OperationAccepted { operation } = client
        .geometry(GeometryCommand::ImportPart {
            session_id: s.session_id,
            base_revision: s.revision,
            source: native(source),
            initial_pose: RigidPoseMm::IDENTITY,
        })
        .await
        .unwrap()
    else {
        panic!("import")
    };
    let job_id = operation.name;
    let job = wait(client, job_id).await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
}
async fn set_intent(client: &mut EngineClient, intent: ManufacturingIntent) {
    let (info, _) = status(client).await;
    let session_id = client.hello().session_id.clone();
    assert!(matches!(
        client
            .manufacturing(ManufacturingCommand::SetIntent {
                session_id,
                base_revision: info.revision,
                intent: intent.into(),
            })
            .await
            .unwrap(),
        ManufacturingResponse::Status { .. }
    ));
}
async fn start_compile(client: &mut EngineClient) -> String {
    let (info, _) = status(client).await;
    let session_id = client.hello().session_id.clone();
    let ManufacturingResponse::OperationAccepted { operation } = client
        .manufacturing(ManufacturingCommand::Compile {
            session_id,
            base_revision: info.revision,
        })
        .await
        .unwrap()
    else {
        panic!("compile")
    };
    operation.name
}
async fn compile(client: &mut EngineClient) -> ManufacturingArtifactRecord {
    let id = start_compile(client).await;
    let job = wait(client, id).await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    let Some(JobResult::ManufacturingCompiled { record, .. }) = job.result else {
        panic!("compiled")
    };
    record
}
async fn inspect(client: &mut EngineClient) -> (ManufacturingArtifactRecord, VerificationReport) {
    let (info, _) = status(client).await;
    let session_id = client.hello().session_id.clone();
    let ManufacturingResponse::OperationAccepted { operation } = client
        .manufacturing(ManufacturingCommand::Inspect {
            session_id,
            base_revision: info.revision,
        })
        .await
        .unwrap()
    else {
        panic!("inspect")
    };
    let job_id = operation.name;
    let job = wait(client, job_id).await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    let Some(JobResult::ManufacturingVerified { record, report, .. }) = job.result else {
        panic!("verified")
    };
    assert!(report.verified);
    (record, report)
}
async fn save(client: &mut EngineClient, path: &Path) {
    let s = scene(client).await;
    let ProjectResponse::OperationAccepted { operation } = client
        .project(ProjectCommand::Save {
            session_id: s.session_id,
            base_revision: s.revision,
            target: Some(native(path)),
        })
        .await
        .unwrap()
    else {
        panic!("save")
    };
    let job_id = operation.name;
    let job = wait(client, job_id).await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
}
async fn open(client: &mut EngineClient, path: &Path, read_only: bool) -> NativeOperationView {
    let s = scene(client).await;
    let ProjectResponse::OperationAccepted { operation } = client
        .project(ProjectCommand::Open {
            session_id: s.session_id,
            base_revision: s.revision,
            path: native(path),
            read_only,
            recover_previous: false,
            discard_changes: true,
        })
        .await
        .unwrap()
    else {
        panic!("open")
    };
    let job_id = operation.name;
    wait(client, job_id).await
}
fn typed<T: std::fmt::Debug>(result: Result<T, ClientError>, code: ManufacturingErrorCode) {
    assert!(
        matches!(&result, Err(ClientError::Manufacturing(error)) if error.code == code),
        "{result:?}"
    );
}
async fn history(client: &mut EngineClient, undo: bool) {
    let s = scene(client).await;
    let command = if undo {
        ProjectCommand::Undo {
            session_id: s.session_id,
            base_revision: s.revision,
        }
    } else {
        ProjectCommand::Redo {
            session_id: s.session_id,
            base_revision: s.revision,
        }
    };
    assert!(matches!(
        client.project(command).await.unwrap(),
        ProjectResponse::Status { .. }
    ));
}

#[tokio::test]
async fn intent_commit_releases_redo_native_pins_without_advancing_scene_epoch() {
    let work = Workspace::new();
    let source = work.source("box-mm.step");
    let original = std::fs::read_to_string(&source).unwrap();
    let mut client = spawn().await;
    for index in 0..MAX_DEFINITIONS {
        let name = format!("box-{index}.step");
        let path = work.0.join(&name);
        let bytes = original.replacen("FILE_NAME('box-mm.step'", &format!("FILE_NAME('{name}'"), 1);
        std::fs::write(&path, bytes).unwrap();
        import(&mut client, &path).await;
    }
    let full = scene(&mut client).await;
    assert_eq!(full.definition_count, MAX_DEFINITIONS);
    assert_eq!(full.occurrence_count, MAX_DEFINITIONS);
    for remaining in (0..MAX_DEFINITIONS).rev() {
        let current = scene(&mut client).await;
        let ProjectResponse::SceneChanged { summary, .. } = client
            .project(ProjectCommand::Undo {
                session_id: current.session_id,
                base_revision: current.revision,
            })
            .await
            .unwrap()
        else {
            panic!("undo import must change the scene")
        };
        assert_eq!(summary.definition_count, remaining);
        assert_eq!(summary.occurrence_count, remaining);
    }
    let empty = scene(&mut client).await;
    assert_eq!(empty.definition_count, 0);
    assert_eq!(empty.occurrence_count, 0);
    let (before, _) = status(&mut client).await;
    assert!(before.can_redo);
    set_intent(&mut client, intent()).await;
    let (after, _) = status(&mut client).await;
    assert_eq!(after.project_id, before.project_id);
    assert!(after.revision > before.revision);
    assert!(!after.can_redo);
    assert_eq!(scene(&mut client).await, empty);

    // A new native definition must fit after the intent edit drops all import redo.
    let name = format!("box-{}.step", MAX_DEFINITIONS);
    let path = work.0.join(&name);
    let bytes = original.replacen("FILE_NAME('box-mm.step'", &format!("FILE_NAME('{name}'"), 1);
    let expected_hash = SourceHash::from_bytes(bytes.as_bytes());
    std::fs::write(&path, bytes).unwrap();
    import(&mut client, &path).await;
    let current = scene(&mut client).await;
    assert_eq!(current.definition_count, 1);
    assert_eq!(current.occurrence_count, 1);
    assert!(current.revision > empty.revision);
    let GeometryResponse::ScenePage {
        definitions,
        next_offset,
        ..
    } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: current.session_id.clone(),
            revision: current.revision,
            kind: ScenePageKind::Definitions,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("definitions")
    };
    assert!(next_offset.is_none());
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].provenance.source_hash, expected_hash);
    assert_eq!(definitions[0].provenance.source_name, name);
    let GeometryResponse::ScenePage {
        occurrences,
        next_offset,
        ..
    } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: current.session_id,
            revision: current.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("occurrences")
    };
    assert!(next_offset.is_none());
    assert_eq!(occurrences.len(), 1);
    assert_eq!(
        occurrences[0].occurrence_id,
        OccurrenceId::new(MAX_DEFINITIONS + 1).unwrap()
    );
    assert_eq!(occurrences[0].definition_id, definitions[0].definition_id);
    assert_eq!(occurrences[0].pose, RigidPoseMm::IDENTITY);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn compiled_bundle_survives_source_deletion_and_fresh_read_only_replay() {
    let work = Workspace::new();
    let source = work.source("box-mm.step");
    let checkpoint = work.0.join("project");
    let mut client = spawn().await;
    import(&mut client, &source).await;
    let original_scene = scene(&mut client).await;
    set_intent(&mut client, intent()).await;
    assert_eq!(scene(&mut client).await, original_scene);
    let record = compile(&mut client).await;
    assert_eq!(scene(&mut client).await, original_scene);
    let bundle = fetch_bundle(&mut client, &record).await.unwrap();
    let mut missing = record.clone();
    missing.hash = SourceHash::from_bytes(b"not retained");
    typed(
        fetch_bundle(&mut client, &missing).await,
        ManufacturingErrorCode::CorruptArtifact,
    );
    let mut oversized = record.clone();
    oversized.byte_count = MAX_MANUFACTURING_BUNDLE_BYTES + 1;
    typed(
        fetch_bundle(&mut client, &oversized).await,
        ManufacturingErrorCode::ResourceLimit,
    );
    typed(
        client
            .manufacturing(ManufacturingCommand::Get {
                session_id: SessionId::new(),
            })
            .await,
        ManufacturingErrorCode::StaleRevision,
    );
    assert!(bundle.verification.verified && record.summary.software_only);
    assert!(
        bundle.program.contains("G21")
            && bundle.program.contains("G90")
            && bundle.program.contains("M82")
    );
    assert_eq!(inspect(&mut client).await.0, record);
    save(&mut client, &checkpoint).await;
    let mut changed = intent();
    changed.recipe.revision = "2".into();
    set_intent(&mut client, changed).await;
    assert!(status(&mut client).await.1.is_none());
    assert_eq!(scene(&mut client).await, original_scene);
    history(&mut client, true).await;
    assert_eq!(status(&mut client).await.1, Some(record.clone()));
    history(&mut client, false).await;
    assert!(status(&mut client).await.1.is_none());
    history(&mut client, true).await;
    assert_eq!(scene(&mut client).await, original_scene);
    let s = scene(&mut client).await;
    let GeometryResponse::ScenePage { occurrences, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: s.session_id.clone(),
            revision: s.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("occurrences")
    };
    client
        .geometry(GeometryCommand::SetInstancePose {
            session_id: s.session_id,
            base_revision: s.revision,
            occurrence_id: occurrences[0].occurrence_id,
            pose: RigidPoseMm {
                translation_mm: [1.0, 0.0, 0.0],
                ..RigidPoseMm::IDENTITY
            },
        })
        .await
        .unwrap();
    assert!(status(&mut client).await.1.is_none());
    let s = scene(&mut client).await;
    assert!(matches!(
        client
            .project(ProjectCommand::Undo {
                session_id: s.session_id,
                base_revision: s.revision,
            })
            .await
            .unwrap(),
        ProjectResponse::SceneChanged { .. }
    ));
    assert_eq!(status(&mut client).await.1, Some(record.clone()));
    client.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();
    let mut reopened = spawn().await;
    let job = open(&mut reopened, &checkpoint, true).await;
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    assert_eq!(inspect(&mut reopened).await.0, record);
    assert_eq!(fetch_bundle(&mut reopened, &record).await.unwrap(), bundle);
    let (info, _) = status(&mut reopened).await;
    let session_id = reopened.hello().session_id.clone();
    let ManufacturingResponse::OperationAccepted { operation } = reopened
        .manufacturing(ManufacturingCommand::Compile {
            session_id: session_id.clone(),
            base_revision: info.revision,
        })
        .await
        .unwrap()
    else {
        panic!("read-only compile admission")
    };
    let rejected = wait(&mut reopened, operation.name).await;
    assert_eq!(rejected.status, JobStatus::Failed, "{rejected:?}");
    assert!(
        matches!(&rejected.error, Some(JobError::Manufacturing { error }) if error.code == ManufacturingErrorCode::ReadOnly)
    );
    assert!(rejected.outputs.is_empty());
    assert_eq!(status(&mut reopened).await, (info.clone(), Some(record)));
    typed(
        reopened
            .manufacturing(ManufacturingCommand::SetIntent {
                session_id,
                base_revision: info.revision,
                intent: intent().into(),
            })
            .await,
        ManufacturingErrorCode::ReadOnly,
    );
    reopened.ping().await.unwrap();
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_holes_cylinders_and_repeated_occurrences_compile_without_display_rebuild() {
    for name in ["cylinder.step", "through-hole.step", "box-mm.step"] {
        let work = Workspace::new();
        let mut client = spawn().await;
        import(&mut client, &work.source(name)).await;
        let s = scene(&mut client).await;
        let GeometryResponse::ScenePage { occurrences, .. } = client
            .geometry(GeometryCommand::GetScenePage {
                session_id: s.session_id.clone(),
                revision: s.revision,
                kind: ScenePageKind::Occurrences,
                offset: 0,
            })
            .await
            .unwrap()
        else {
            panic!("occurrences")
        };
        let result = client
            .geometry(GeometryCommand::AddInstance {
                session_id: s.session_id,
                base_revision: s.revision,
                definition_id: occurrences[0].definition_id.clone(),
                pose: RigidPoseMm {
                    translation_mm: [40.0, 0.0, 0.0],
                    ..RigidPoseMm::IDENTITY
                },
            })
            .await
            .unwrap();
        assert!(matches!(result, GeometryResponse::SceneChanged { .. }));
        let before = scene(&mut client).await;
        set_intent(&mut client, intent()).await;
        let record = compile(&mut client).await;
        let bundle = fetch_bundle(&mut client, &record).await.unwrap();
        assert_eq!(bundle.provenance.occurrences.len(), 2);
        assert_eq!(bundle.provenance.definitions.len(), 1);
        assert_eq!(scene(&mut client).await, before);
        inspect(&mut client).await;
        client.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn stale_cancelled_and_resource_limited_compilation_never_publish() {
    let work = Workspace::new();
    let mut client = spawn().await;
    import(&mut client, &work.source("box-mm.step")).await;
    set_intent(&mut client, intent()).await;
    let diagnostic = client
        .rpc()
        .run_diagnostic(spiling_contracts::rpc::RunDiagnosticRequest {
            parent: "diagnostics/queued-manufacturing-regression".into(),
            request_id: String::new(),
            chunk_count: 4,
            delay_ms: 1000,
            chunk_bytes: 64,
            input_revision: "occupying-shared-permit".into(),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = client.rpc().get_operation(&diagnostic.name).await.unwrap();
            if spiling_contracts::metadata(&current).unwrap().state
                == spiling_contracts::rpc::DiagnosticState::Running as i32
            {
                break;
            }
            assert!(!current.done);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let stale = start_compile(&mut client).await;
    let mut changed = intent();
    changed.recipe.revision = "changed-before-native-dispatch".into();
    set_intent(&mut client, changed).await;
    let cancelled = start_compile(&mut client).await;
    client.rpc().cancel_operation(&cancelled).await.unwrap();
    let job = wait(&mut client, cancelled).await;
    assert_eq!(job.status, JobStatus::Cancelled, "{job:?}");
    assert!(
        matches!(&job.error, Some(JobError::Manufacturing { error }) if error.code == ManufacturingErrorCode::Cancelled)
    );
    assert!(job.outputs.is_empty());
    let job = wait(&mut client, stale).await;
    assert_eq!(job.status, JobStatus::Failed, "{job:?}");
    assert!(
        matches!(&job.error, Some(JobError::Manufacturing { error }) if error.code == ManufacturingErrorCode::StaleRevision),
        "{job:?}"
    );
    assert!(job.outputs.is_empty());
    assert!(status(&mut client).await.1.is_none());
    let mut too_many_layers = intent();
    too_many_layers.recipe.layer_height_mm = 0.001;
    set_intent(&mut client, too_many_layers).await;
    let id = start_compile(&mut client).await;
    let job = wait(&mut client, id).await;
    assert!(
        matches!(&job.error, Some(JobError::Manufacturing { error }) if error.code == ManufacturingErrorCode::ResourceLimit)
    );
    assert!(status(&mut client).await.1.is_none());
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn reopen_rejects_hash_corruption_and_hashed_program_tampering_without_replacing_session() {
    let work = Workspace::new();
    let checkpoint = work.0.join("project");
    let mut client = spawn().await;
    import(&mut client, &work.source("box-mm.step")).await;
    set_intent(&mut client, intent()).await;
    let record = compile(&mut client).await;
    let mut bundle = fetch_bundle(&mut client, &record).await.unwrap();
    save(&mut client, &checkpoint).await;
    client.shutdown().await.unwrap();
    let asset = checkpoint
        .join("manufacturing")
        .join(format!("{}.json", record.hash.as_str()));
    let original = std::fs::read(&asset).unwrap();
    std::fs::write(&asset, b"corrupt").unwrap();
    let mut reader = spawn().await;
    let before = scene(&mut reader).await;
    let corrupt = open(&mut reader, &checkpoint, true).await;
    assert_eq!(corrupt.status, JobStatus::Failed, "{corrupt:?}");
    assert!(
        matches!(&corrupt.error, Some(JobError::Project { error })
        if error.code == ProjectErrorCode::CorruptAsset),
        "{corrupt:?}"
    );
    assert_eq!(scene(&mut reader).await, before);
    reader.ping().await.unwrap();
    std::fs::write(&asset, original).unwrap();
    bundle.program = bundle.program.replacen("G1 ", "G0 ", 1);
    let bytes = serde_json::to_vec(&bundle).unwrap();
    let hash = SourceHash::from_bytes(&bytes);
    std::fs::write(
        checkpoint
            .join("manufacturing")
            .join(format!("{}.json", hash.as_str())),
        &bytes,
    )
    .unwrap();
    let manifest_path = checkpoint.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["manufacturing_artifact"]["hash"] = serde_json::json!(hash);
    manifest["manufacturing_artifact"]["byte_count"] = serde_json::json!(bytes.len());
    std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let tampered = open(&mut reader, &checkpoint, true).await;
    assert_eq!(tampered.status, JobStatus::Failed, "{tampered:?}");
    assert!(
        matches!(&tampered.error, Some(JobError::Manufacturing { error }) if error.code == ManufacturingErrorCode::VerificationFailed),
        "{tampered:?}"
    );
    assert_eq!(scene(&mut reader).await, before);
    reader.ping().await.unwrap();
    reader.shutdown().await.unwrap();
}

#[tokio::test]
async fn queued_save_rejects_intent_only_revision_change_before_snapshot_or_manifest_replace() {
    use spiling_contracts::{
        metadata,
        rpc::{DiagnosticState, RunDiagnosticRequest},
    };
    let work = Workspace::new();
    let mut client = spawn().await;
    import(&mut client, &work.source("box-mm.step")).await;
    set_intent(&mut client, intent()).await;
    let checkpoint = work.0.join("queued-save");
    save(&mut client, &checkpoint).await;
    let manifest_before = std::fs::read(checkpoint.join("manifest.json")).unwrap();
    let captured = status(&mut client).await.0;
    let captured_scene = scene(&mut client).await;
    let diagnostic = client
        .rpc()
        .run_diagnostic(RunDiagnosticRequest {
            parent: "diagnostics/queued-save-regression".into(),
            request_id: String::new(),
            chunk_count: 4,
            delay_ms: 1000,
            chunk_bytes: 64,
            input_revision: "occupying-shared-permit".into(),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = client.rpc().get_operation(&diagnostic.name).await.unwrap();
            if metadata(&current).unwrap().state == DiagnosticState::Running as i32 {
                break;
            }
            assert!(!current.done);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let ProjectResponse::OperationAccepted { operation } = client
        .project(ProjectCommand::Save {
            session_id: captured_scene.session_id.clone(),
            base_revision: captured_scene.revision,
            target: None,
        })
        .await
        .unwrap()
    else {
        panic!("queued save admission")
    };
    assert_eq!(operation.status, JobStatus::Queued);
    assert_eq!(operation.captured_project_revision, captured.revision);
    let mut changed = intent();
    changed.recipe.revision = "intent-only-change-after-save-admission".into();
    set_intent(&mut client, changed.clone()).await;
    let current = status(&mut client).await.0;
    assert!(current.revision > captured.revision);
    assert_eq!(
        scene(&mut client).await,
        captured_scene,
        "intent changes must not advance the scene epoch"
    );
    let result = wait(&mut client, operation.name).await;
    assert_eq!(result.status, JobStatus::Failed, "{result:?}");
    assert_eq!(result.captured_project_revision, captured.revision);
    assert!(result.result.is_none());
    assert!(
        matches!(result.error,Some(JobError::Project {error}) if error.code==ProjectErrorCode::StaleRevision)
    );
    assert_eq!(
        std::fs::read(checkpoint.join("manifest.json")).unwrap(),
        manifest_before,
        "stale queued save must not replace the manifest"
    );
    assert_eq!(
        status(&mut client).await.0,
        current,
        "rejection must not mark the new revision saved"
    );
    let ManufacturingResponse::Status { intent: stored, .. } = client
        .manufacturing(ManufacturingCommand::Get {
            session_id: captured_scene.session_id,
        })
        .await
        .unwrap()
    else {
        panic!("intent status")
    };
    assert_eq!(stored.as_deref(), Some(&changed));
    save(&mut client, &checkpoint).await;
    client.shutdown().await.unwrap();
}
