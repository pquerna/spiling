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
            "schema_version":MANUFACTURING_SCHEMA_VERSION,"id":"software-test","revision":"1","name":"Software only",
            "coordinate_frame":"right_handed_millimetres",
            "build_envelope":{"min":[-20.0,-20.0,0.0],"max":[200.0,200.0,100.0]},
            "components":[{"id":"bed","role":"bed","pose":{"translation_mm":[0.0,0.0,0.0],"rotation_xyzw":[0.0,0.0,0.0,1.0]},"shape":{"kind":"box","bounds_mm":{"min":[-20.0,-20.0,-1.0],"max":[200.0,200.0,0.0]}}}],
            "capabilities":{"motion_space":"cartesian_xyz","extruders":1,"output_dialect":"cartesian_absolute_gcode_v1","generated_supports":false},
            "workflow":{"kind":"diagnostic_body_only"},
            "nozzle_diameter_mm":0.4,"max_feed_mm_s":150.0,"max_volumetric_flow_mm3_s":20.0
        },
        "recipe":{"id":"solid","revision":"1","material_id":"nominal","material_revision":"1","filament_diameter_mm":1.75,"layer_height_mm":0.2,"bead_width_mm":0.4,"perimeter_count":1,"infill_fraction":1.0,"top_skin_thickness_mm":0.2,"bottom_skin_thickness_mm":0.2,"print_speed_mm_s":20.0,"travel_speed_mm_s":80.0,"flow_multiplier":1.0,
            "process":{"first_layer_speed_factor":1.0,"first_layer_flow_factor":1.0,"fan_first_layers":0,"fan_after_first_layers":0,"bridge_speed_factor":1.0,"bridge_flow_factor":1.0,"bridge_fan_pwm":0,"max_bridge_span_mm":5.0,"retraction":{"kind":"disabled"}},
            "supports":{"kind":"disabled"}}
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
            output: VerificationOutput::ReportOnly,
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

async fn preflight(client: &mut EngineClient) -> NativeOperationView {
    let (info, _) = status(client).await;
    let session_id = client.hello().session_id.clone();
    let ManufacturingResponse::OperationAccepted { operation } = client
        .manufacturing(ManufacturingCommand::Preflight {
            session_id,
            base_revision: info.revision,
        })
        .await
        .unwrap()
    else {
        panic!("preflight must be a durable native operation")
    };
    wait(client, operation.name).await
}

#[tokio::test]
async fn preflight_is_durable_nonmutating_and_reports_software_eligibility() {
    let work = Workspace::new();
    let source = work.source("box-mm.step");
    let store = work.0.join("operations");
    let checkpoint = work.0.join("project");
    let mut client = EngineClient::spawn_in(ENGINE, &store).await.unwrap();
    import(&mut client, &source).await;
    set_intent(&mut client, intent()).await;
    let before = status(&mut client).await;
    let scene_before = scene(&mut client).await;
    let result = preflight(&mut client).await;
    assert_eq!(result.status, JobStatus::Completed, "{result:?}");
    assert!(
        result.outputs.is_empty(),
        "preflight must not publish artifacts"
    );
    let Some(JobResult::ManufacturingPreflight { report }) = result.result else {
        panic!("preflight report missing")
    };
    report.validate().unwrap();
    assert_eq!(report.input_revision, before.0.revision);
    assert!(report.software_only);
    assert!(report.eligible);
    assert_eq!(status(&mut client).await, before);
    assert_eq!(scene(&mut client).await, scene_before);
    let retained = client.native_operation(result.name.clone()).await.unwrap();
    assert!(matches!(
        retained.result,
        Some(JobResult::ManufacturingPreflight { .. })
    ));
    assert_eq!(retained.outputs, result.outputs);
    save(&mut client, &checkpoint).await;
    client.shutdown().await.unwrap();
    let mut reader = EngineClient::spawn_in(ENGINE, &store).await.unwrap();
    let historical = reader.native_operation(result.name).await.unwrap();
    assert_eq!(historical.result, retained.result);
    assert!(historical.outputs.is_empty());
    assert_eq!(
        open(&mut reader, &checkpoint, true).await.status,
        JobStatus::Completed
    );
    let read_only_before = status(&mut reader).await;
    assert!(read_only_before.0.read_only);
    let read_only_report = preflight(&mut reader).await;
    assert_eq!(read_only_report.status, JobStatus::Completed);
    assert!(read_only_report.outputs.is_empty());
    assert_eq!(status(&mut reader).await, read_only_before);
    reader.shutdown().await.unwrap();
}

#[tokio::test]
async fn queued_preflight_cancellation_and_revision_staleness_do_not_publish() {
    use spiling_contracts::rpc::{DiagnosticState, RunDiagnosticRequest};
    let work = Workspace::new();
    let source = work.source("box-mm.step");
    let mut client = spawn().await;
    import(&mut client, &source).await;
    set_intent(&mut client, intent()).await;
    let rpc = client.rpc();
    let blocker = rpc
        .run_diagnostic(RunDiagnosticRequest {
            parent: "diagnostics/preflight-boundary".into(),
            request_id: String::new(),
            chunk_count: 4,
            delay_ms: 1000,
            chunk_bytes: 64,
            input_revision: "hold-native-permit".into(),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let operation = rpc.get_operation(&blocker.name).await.unwrap();
            if spiling_contracts::metadata(&operation).unwrap().state
                == DiagnosticState::Running as i32
            {
                break;
            }
            assert!(!operation.done);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let before = status(&mut client).await;
    let command = ManufacturingCommand::Preflight {
        session_id: client.hello().session_id.clone(),
        base_revision: before.0.revision,
    };
    let ManufacturingResponse::OperationAccepted { operation } =
        client.manufacturing(command.clone()).await.unwrap()
    else {
        panic!("queued preflight")
    };
    client.cancel_operation(&operation.name).await.unwrap();
    let cancelled = wait(&mut client, operation.name).await;
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert!(cancelled.result.is_none() && cancelled.outputs.is_empty());
    assert_eq!(status(&mut client).await, before);
    let ManufacturingResponse::OperationAccepted { operation } =
        client.manufacturing(command).await.unwrap()
    else {
        panic!("queued preflight")
    };
    let mut changed = intent();
    changed.recipe.revision = "2".into();
    set_intent(&mut client, changed).await;
    let changed_state = status(&mut client).await;
    rpc.cancel_operation(&blocker.name).await.unwrap();
    let stale = wait(&mut client, operation.name).await;
    assert_eq!(stale.status, JobStatus::Failed);
    assert!(
        matches!(stale.error, Some(JobError::Manufacturing { error }) if error.code == ManufacturingErrorCode::StaleRevision)
    );
    assert!(stale.result.is_none() && stale.outputs.is_empty());
    assert_eq!(status(&mut client).await, changed_state);
    client.shutdown().await.unwrap();
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
    let before_invalid = status(&mut client).await;
    let mut invalid_inputs = Vec::new();
    for fraction in [0.0, f64::NAN, 1.01] {
        let mut invalid = intent();
        invalid.recipe.infill_fraction = fraction;
        invalid_inputs.push(invalid);
    }
    let mut invalid = intent();
    invalid.recipe.top_skin_thickness_mm = 0.0;
    invalid_inputs.push(invalid);
    let mut invalid = intent();
    invalid.recipe.bottom_skin_thickness_mm = f64::INFINITY;
    invalid_inputs.push(invalid);
    let mut invalid = intent();
    invalid.printer.capabilities.output_dialect = OutputDialect::Marlin2SingleExtruderGcodeV1;
    invalid_inputs.push(invalid);
    for invalid in invalid_inputs {
        typed(
            client
                .manufacturing(ManufacturingCommand::SetIntent {
                    session_id: client.hello().session_id.clone(),
                    base_revision: before_invalid.0.revision,
                    intent: invalid.into(),
                })
                .await,
            ManufacturingErrorCode::InvalidSpecification,
        );
        assert_eq!(status(&mut client).await, before_invalid);
        assert_eq!(scene(&mut client).await, original_scene);
        client.ping().await.unwrap();
    }
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
    let project_revision = status(&mut client).await.0.revision;
    client
        .geometry(GeometryCommand::SetInstancePose {
            session_id: s.session_id,
            base_revision: s.revision,
            project_revision,
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

#[tokio::test]
async fn actual_motion_read_only_publication_cancel_and_corrupt_transfer_boundaries() {
    use spiling_contracts::rpc::{DiagnosticState, RunDiagnosticRequest};
    let work = Workspace::new();
    let source = work.source("box-mm.step");
    let checkpoint = work.0.join("motion-project");
    let store = work.0.join("reader-operations");
    let mut writer = spawn().await;
    import(&mut writer, &source).await;
    let mut requested = intent();
    requested.printer.capabilities.output_dialect = OutputDialect::Marlin2SingleExtruderGcodeV1;
    requested.printer.workflow = JobWorkflow::CompleteMarlin {
        job: SingleExtruderJob {
            machine_id: "software-declaration-not-qualified".into(),
            firmware_version: "2.1.2.5".into(),
            configuration_hash: SourceHash::from_bytes(b"software fixture configuration"),
            calibration_record_hash: SourceHash::from_bytes(
                b"software fixture declaration, not measured calibration",
            ),
            homing_contract: HomingCoordinateContract::MarlinNativeCartesian,
            home_position_mm: [0.0, 0.0, 0.0],
            nozzle_temperature_c: 200.0,
            bed_temperature_c: 60.0,
            minimum_extrusion_temperature_c: 170.0,
            maximum_nozzle_temperature_c: 250.0,
            maximum_bed_temperature_c: 100.0,
            prime_start_mm: [-10.0, -10.0, 0.2],
            prime_end_mm: [-5.0, -10.0, 0.2],
            prime_filament_length_mm: 1.0,
            prime_speed_mm_s: 5.0,
            park_position_mm: [0.0, 0.0, 30.0],
        },
    };
    requested.recipe.infill_fraction = 0.35;
    requested.recipe.top_skin_thickness_mm = 0.4;
    requested.recipe.bottom_skin_thickness_mm = 0.6;
    set_intent(&mut writer, requested.clone()).await;
    let record = compile(&mut writer).await;
    let bundle = fetch_bundle(&mut writer, &record).await.unwrap();
    assert_eq!(bundle.provenance.intent, requested);
    assert_eq!(bundle.verification.lifecycle_motion_count, 4);
    assert_eq!(bundle.verification.priming_filament_length_mm, 1.0);
    assert!(bundle.plan.layers.iter().any(|layer| {
        layer.region_coverage.intentional_void_area_mm2 > 0.0
            && layer
                .paths
                .iter()
                .any(|path| path.kind == DepositionKind::SparseFill)
    }));
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .any(|layer| { layer.region_coverage.bottom_skin_area_mm2 > 0.0 })
    );
    assert!(
        bundle
            .plan
            .layers
            .iter()
            .any(|layer| { layer.region_coverage.top_skin_area_mm2 > 0.0 })
    );
    let bead_area = ((requested.recipe.bead_width_mm - requested.recipe.layer_height_mm)
        * requested.recipe.layer_height_mm
        + std::f64::consts::PI * (requested.recipe.layer_height_mm / 2.0).powi(2))
        * requested.recipe.flow_multiplier;
    let nominal_volume: f64 = bundle
        .plan
        .layers
        .iter()
        .map(|layer| {
            let path_length: f64 = layer
                .paths
                .iter()
                .flat_map(|path| path.points_mm.windows(2))
                .map(|points| (points[1][0] - points[0][0]).hypot(points[1][1] - points[0][1]))
                .sum();
            let volume = path_length * bead_area;
            assert!((layer.region_coverage.nominal_deposition_volume_mm3 - volume).abs() < 1e-8);
            volume
        })
        .sum();
    assert!((bundle.verification.deposited_volume_mm3 - nominal_volume).abs() < 0.001);
    assert!(record.summary.software_only);
    save(&mut writer, &checkpoint).await;
    writer.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();

    let mut reader = EngineClient::spawn_in(ENGINE, &store).await.unwrap();
    assert_eq!(
        open(&mut reader, &checkpoint, true).await.status,
        JobStatus::Completed
    );
    let before = status(&mut reader).await;
    let original_scene = scene(&mut reader).await;
    let session = reader.hello().session_id.clone();
    let command = ManufacturingCommand::Inspect {
        session_id: session.clone(),
        base_revision: before.0.revision,
        output: VerificationOutput::ReportAndMotion,
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    let rpc = reader.rpc();
    let ManufacturingResponse::OperationAccepted { operation } = rpc
        .manufacturing_with_request_id(command.clone(), &request_id)
        .await
        .unwrap()
    else {
        panic!("motion admission")
    };
    let completed = wait(&mut reader, operation.name.clone()).await;
    assert_eq!(completed.status, JobStatus::Completed, "{completed:?}");
    let Some(JobResult::ManufacturingVerified {
        record: inspected,
        report,
        provenance,
        replay_display: Some(display),
        resource,
    }) = &completed.result
    else {
        panic!("complete actual-program display")
    };
    assert_eq!(inspected, &record);
    assert!(report.verified);
    assert_eq!(report.lifecycle_motion_count, 4);
    assert_eq!(report.priming_filament_length_mm, 1.0);
    assert_eq!(display.info.lifecycle_count, 4);
    assert_eq!(
        display.info.event_count,
        report.deposition_segments + report.travel_segments + report.lifecycle_motion_count
    );
    assert_eq!(provenance.input_hash, record.input_hash);
    assert_eq!(provenance.input_revision, bundle.provenance.input_revision);
    assert_eq!(
        display.info.program_hash,
        SourceHash::from_bytes(bundle.program.as_bytes())
    );
    assert!(completed.outputs.iter().any(|out| out == &display.resource));
    assert!(completed.outputs.iter().any(|out| out == resource));
    let bytes = reader.fetch_replay_display(&record, display).await.unwrap();
    assert_eq!(&bytes[..4], b"SPLR");
    assert_eq!(bytes.len(), 96 + 48 * display.info.event_count as usize);
    assert_eq!(
        bytes[96 + 46],
        6,
        "first event is endpoint-only lifecycle travel"
    );
    assert_eq!(bytes[96 + 47], 0, "no invented origin");
    let lifecycle: Vec<_> = bytes[96..]
        .as_chunks::<48>()
        .0
        .iter()
        .filter(|event| event[46] >= 6)
        .collect();
    assert_eq!(lifecycle.len(), 4);
    assert!(
        lifecycle
            .iter()
            .all(|event| { u16::from_le_bytes(event[44..46].try_into().unwrap()) == u16::MAX })
    );
    assert_eq!(
        lifecycle[1][46], 7,
        "priming is separate from object deposition"
    );
    for event in bytes[96..].as_chunks::<48>().0 {
        let source_line = u32::from_le_bytes(event[40..44].try_into().unwrap());
        let actual_line = bundle
            .program
            .lines()
            .nth(source_line as usize - 1)
            .unwrap();
        assert!(actual_line.starts_with("G0 ") || actual_line.starts_with("G1 "));
        for (axis, offset) in [('X', 0), ('Y', 8), ('Z', 16), ('E', 24), ('F', 32)] {
            if let Some(value) = actual_line
                .split_ascii_whitespace()
                .find_map(|word| word.strip_prefix(axis).and_then(|v| v.parse::<f64>().ok()))
            {
                assert_eq!(
                    f64::from_le_bytes(event[offset..offset + 8].try_into().unwrap()),
                    value,
                    "display must reflect the emitted bytes at line {source_line}, not plan coordinates"
                );
            }
        }
    }
    assert_eq!(
        status(&mut reader).await,
        before,
        "inspection never mutates Core"
    );
    assert_eq!(scene(&mut reader).await, original_scene);
    let duplicate = rpc
        .manufacturing_with_request_id(command.clone(), &request_id)
        .await
        .unwrap();
    assert!(
        matches!(duplicate, ManufacturingResponse::OperationAccepted { operation } if operation == completed)
    );
    let mut report_only = command.clone();
    let ManufacturingCommand::Inspect { output, .. } = &mut report_only else {
        unreachable!()
    };
    *output = VerificationOutput::ReportOnly;
    assert!(
        matches!(rpc.manufacturing_with_request_id(report_only, &request_id).await,
        Err(ClientError::Rpc(error)) if error.code() == tonic::Code::AlreadyExists)
    );
    reader.cancel_operation(&completed.name).await.unwrap();
    assert_eq!(
        reader.native_operation(&completed.name).await.unwrap(),
        completed,
        "late cancellation cannot turn an ACK-winning result into fake cancellation"
    );

    let blocking = rpc
        .run_diagnostic(RunDiagnosticRequest {
            parent: "diagnostics/motion-cancel-boundary".into(),
            request_id: String::new(),
            chunk_count: 4,
            delay_ms: 1000,
            chunk_bytes: 64,
            input_revision: "hold-native-permit".into(),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let op = rpc.get_operation(&blocking.name).await.unwrap();
            if spiling_contracts::metadata(&op).unwrap().state == DiagnosticState::Running as i32 {
                break;
            }
            assert!(!op.done);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let ManufacturingResponse::OperationAccepted { operation: queued } =
        reader.manufacturing(command).await.unwrap()
    else {
        panic!("queued motion")
    };
    reader.cancel_operation(&queued.name).await.unwrap();
    let cancelled = wait(&mut reader, queued.name).await;
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert!(cancelled.result.is_none() && cancelled.outputs.is_empty());
    rpc.cancel_operation(&blocking.name).await.unwrap();
    assert_eq!(status(&mut reader).await, before);

    let mut mismatch = display.clone();
    mismatch.info.bundle_hash = SourceHash::from_bytes(b"another bundle");
    typed(
        reader.fetch_replay_display(&record, &mismatch).await,
        ManufacturingErrorCode::CorruptArtifact,
    );
    let mut corrupt = bytes.clone();
    corrupt[96 + 47] = 0x80;
    let database = rusqlite::Connection::open(store.join("operations.sqlite")).unwrap();
    database
        .execute(
            "UPDATE artifacts SET bytes=? WHERE name=?",
            rusqlite::params![corrupt, display.resource.name],
        )
        .unwrap();
    let error = reader
        .fetch_replay_display(&record, display)
        .await
        .unwrap_err();
    assert!(matches!(&error, ClientError::Manufacturing(error)
        if error.code == ManufacturingErrorCode::CorruptArtifact));
    assert!(
        !error.is_fatal(),
        "fully received corrupt bytes leave the child usable"
    );
    database
        .execute(
            "UPDATE artifacts SET bytes=? WHERE name=?",
            rusqlite::params![bytes, display.resource.name],
        )
        .unwrap();
    reader.ping().await.unwrap();
    reader.shutdown().await.unwrap();
}

#[tokio::test]
async fn surface_metadata_invalidates_active_bundle_without_scene_transfer_and_history_restores_it()
{
    use spiling_contracts::surface::{SurfaceFinish, SurfaceScopeKind};
    let workspace = Workspace::new();
    let mut client = spawn().await;
    import(&mut client, &workspace.source("box-mm.step")).await;
    set_intent(&mut client, intent()).await;
    let original = compile(&mut client).await;
    let before = scene(&mut client).await;
    let GeometryResponse::ScenePage { occurrences, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: before.session_id.clone(),
            revision: before.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("occurrences");
    };
    let occurrence = &occurrences[0];
    let GeometryResponse::FaceIndexPage { faces, .. } = client
        .geometry(GeometryCommand::GetFaceIndexPage {
            session_id: before.session_id.clone(),
            definition_id: occurrence.definition_id.clone(),
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("source faces");
    };
    let (input, _) = status(&mut client).await;
    let ProjectResponse::Status { info: authored } = client
        .project(ProjectCommand::AddSurfaceFinish {
            session_id: before.session_id.clone(),
            base_revision: input.revision,
            face_ref: FaceRef {
                session_id: before.session_id.clone(),
                scene_revision: before.revision,
                occurrence_id: occurrence.occurrence_id,
                definition_id: occurrence.definition_id.clone(),
                face_id: faces[0].face_id.clone(),
            },
            scope: SurfaceScopeKind::Definition,
            finish: SurfaceFinish::Smooth,
        })
        .await
        .unwrap()
    else {
        panic!("metadata status");
    };
    assert!(status(&mut client).await.1.is_none());
    assert_eq!(scene(&mut client).await, before);
    history(&mut client, true).await;
    assert_eq!(status(&mut client).await.1, Some(original.clone()));
    assert_eq!(scene(&mut client).await, before);
    history(&mut client, false).await;
    assert!(status(&mut client).await.1.is_none());
    let recaptured = compile(&mut client).await;
    assert_ne!(recaptured.input_hash, original.input_hash);
    let bundle = fetch_bundle(&mut client, &recaptured).await.unwrap();
    assert_eq!(
        bundle.provenance.surface_overlays,
        authored.surface_overlays
    );
    assert_eq!(scene(&mut client).await, before);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn spatial_image_compilation_captures_self_contained_content_and_fresh_replay() {
    use spiling_contracts::surface::{ImageReliefParameters, SurfaceScopeKind};
    let workspace = Workspace::new();
    let source = workspace.source("box-mm.step");
    let image_path = workspace.0.join("field.pgm");
    let canonical = b"P5\n3 2\n255\n\x00\x40\xff\x80\x20\x00";
    std::fs::write(&image_path, canonical).unwrap();
    let checkpoint = workspace.0.join("image-bundle-project");
    let mut client = spawn().await;
    import(&mut client, &source).await;
    set_intent(&mut client, intent()).await;
    let original = compile(&mut client).await;
    let before_scene = scene(&mut client).await;
    let GeometryResponse::ScenePage { occurrences, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: before_scene.session_id.clone(),
            revision: before_scene.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("occurrences");
    };
    let occurrence = &occurrences[0];
    let GeometryResponse::FaceIndexPage { faces, .. } = client
        .geometry(GeometryCommand::GetFaceIndexPage {
            session_id: before_scene.session_id.clone(),
            definition_id: occurrence.definition_id.clone(),
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("faces");
    };
    let mut target = None;
    for face in faces {
        let reference = FaceRef {
            session_id: before_scene.session_id.clone(),
            scene_revision: before_scene.revision,
            occurrence_id: occurrence.occurrence_id,
            definition_id: occurrence.definition_id.clone(),
            face_id: face.face_id,
        };
        let GeometryResponse::FaceInspection { inspection } = client
            .geometry(GeometryCommand::InspectFace {
                reference: reference.clone(),
            })
            .await
            .unwrap()
        else {
            panic!("face inspection");
        };
        if matches!(inspection.face.geometry.analytic, Some(AnalyticSurfaceDescription::Plane { normal, .. }) if normal[2].abs() < 1e-12)
        {
            target = Some(reference);
            break;
        }
    }
    let before = status(&mut client).await.0;
    let ProjectResponse::OperationAccepted { operation } = client
        .project(ProjectCommand::ImportSurfaceImageFinish {
            session_id: before_scene.session_id.clone(),
            base_revision: before.revision,
            reference: target.expect("native vertical wall"),
            scope: SurfaceScopeKind::Definition,
            source: native(&image_path),
            parameters: ImageReliefParameters {
                depth_mm: 0.04,
                u_repeats: 1,
                u_phase_fraction: 0.125,
                z_period_mm: 2.0,
                z_phase_mm: 0.25,
                edge_margin_mm: 1.0,
                vertical_error_mm: 0.02,
            },
        })
        .await
        .unwrap()
    else {
        panic!("image import");
    };
    let imported = wait(&mut client, operation.name).await;
    assert_eq!(imported.status, JobStatus::Completed, "{imported:?}");
    let Some(JobResult::SurfaceImageImported { image, .. }) = imported.result else {
        panic!("image receipt");
    };
    assert!(
        status(&mut client).await.1.is_none(),
        "atomic image authoring invalidates the old bundle"
    );
    assert_eq!(scene(&mut client).await, before_scene);
    history(&mut client, true).await;
    assert_eq!(status(&mut client).await.1, Some(original.clone()));
    history(&mut client, false).await;
    assert!(status(&mut client).await.1.is_none());
    let compiled = compile(&mut client).await;
    assert_ne!(compiled.input_hash, original.input_hash);
    let bundle = fetch_bundle(&mut client, &compiled).await.unwrap();
    assert_eq!(bundle.provenance.surface_images.len(), 1);
    assert_eq!(
        bundle.provenance.surface_images[0].image_hash,
        image.image_hash
    );
    assert_eq!(
        bundle.provenance.surface_images[0].bytes.as_slice(),
        canonical
    );
    assert_eq!(bundle.provenance.surface_overlays.images, vec![image]);
    assert_eq!(
        bundle.provenance.surface_overlays,
        status(&mut client).await.0.surface_overlays
    );
    assert!(bundle.verification.verified);
    assert_eq!(scene(&mut client).await, before_scene);
    save(&mut client, &checkpoint).await;
    client.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(image_path).unwrap();
    let mut reader = spawn().await;
    let opened = open(&mut reader, &checkpoint, true).await;
    assert_eq!(opened.status, JobStatus::Completed, "{opened:?}");
    assert_eq!(inspect(&mut reader).await.0, compiled);
    assert_eq!(fetch_bundle(&mut reader, &compiled).await.unwrap(), bundle);
    reader.shutdown().await.unwrap();
}
