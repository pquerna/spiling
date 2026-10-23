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
        "recipe":{"id":"solid","revision":"1","material_id":"nominal","material_revision":"1","filament_diameter_mm":1.75,"layer_height_mm":0.2,"native_shape_error_mm":0.1,"bead_width_mm":0.4,"perimeter_count":1,"infill_fraction":1.0,"top_skin_thickness_mm":0.2,"bottom_skin_thickness_mm":0.2,"print_speed_mm_s":20.0,"travel_speed_mm_s":80.0,"flow_multiplier":1.0,
            "process":{"first_layer_speed_factor":1.0,"first_layer_flow_factor":1.0,"fan_first_layers":0,"fan_after_first_layers":0,"bridge_speed_factor":1.0,"bridge_flow_factor":1.0,"bridge_fan_pwm":0,"max_bridge_span_mm":5.0,"retraction":{"kind":"disabled"}},
            "supports":{"kind":"disabled"},
            "seam_policy":{"kind":"nearest"},"rectilinear_axis":"alternating_xy"}
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
    assert_eq!(report.disposition, PreflightDisposition::Admitted);
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
    for xy in [
        [0.0, 0.0],
        [2.0, 0.0],
        [f64::NAN, 1.0],
        [1.0, f64::INFINITY],
    ] {
        let mut invalid = intent();
        invalid.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy };
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
    let nominal_volume: f64 = bundle
        .plan
        .layers
        .iter()
        .map(|layer| {
            let first_layer_factor = if layer.index == 0 {
                requested.recipe.process.first_layer_flow_factor
            } else {
                1.0
            };
            let volume: f64 = layer
                .paths
                .iter()
                .map(|path| {
                    let height = path.height_mm();
                    let mut factor = requested.recipe.flow_multiplier;
                    factor *= first_layer_factor;
                    if path.kind == DepositionKind::Bridge {
                        factor *= requested.recipe.process.bridge_flow_factor;
                    }
                    path.checked_edges()
                        .unwrap()
                        .map(|edge| {
                            let length = (edge.points_mm[1][0] - edge.points_mm[0][0])
                                .hypot(edge.points_mm[1][1] - edge.points_mm[0][1]);
                            let area = (edge.width_mm - height) * height
                                + std::f64::consts::PI * (height * 0.5).powi(2);
                            length * area * factor
                        })
                        .sum::<f64>()
                })
                .sum();
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

async fn edit_process(
    client: &mut EngineClient,
    edit: spiling_contracts::process::ProcessEdit,
) -> ProjectInfo {
    let input = status(client).await.0;
    let ProjectResponse::Status { info } = client
        .project(ProjectCommand::EditProcess {
            session_id: client.hello().session_id.clone(),
            base_revision: input.revision,
            edit,
        })
        .await
        .unwrap()
    else {
        panic!("process edit must return authoritative project status")
    };
    info
}

fn sparse_layer(
    scope: spiling_contracts::surface::SurfaceScope,
    fraction: f64,
) -> spiling_contracts::process::ProcessEdit {
    use spiling_contracts::process::{ProcessAction, ProcessEdit, ProcessSelector, ProcessValue};
    ProcessEdit::AddLayer {
        scope,
        selector: ProcessSelector::WholeOwnedVolume,
        interface: None,
        action: ProcessAction::Replace {
            value: ProcessValue::SparseFraction { fraction },
        },
    }
}

fn effective_process(
    info: &ProjectInfo,
    occurrence_id: OccurrenceId,
) -> &spiling_contracts::process::ResolvedProcessValue {
    &info
        .resolved_process
        .iter()
        .find(|resolved| resolved.occurrence_id == occurrence_id)
        .expect("current occurrence has an authoritative effective process")
        .value
}

#[tokio::test]
async fn rounded_housing_two_zone_separator_survives_native_bed_placement_and_fresh_export() {
    use spiling_contracts::{
        process::{ProcessEdit, ProcessInterfacePermission, ProcessProvenance, ProcessSelector},
        surface::SurfaceScope,
    };
    use spiling_engine_client::export::OutputDirectory;

    let work = Workspace::new();
    std::fs::create_dir(work.0.join("independent-exporters")).unwrap();
    let source = work.source("independent-exporters/occt-rounded-housing-mm.step");
    let checkpoint = work.0.join("housing-project");
    let mut client = spawn().await;
    import(&mut client, &source).await;
    let mut requested = decode_intent(include_bytes!(
        "../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    requested.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    set_intent(&mut client, requested.clone()).await;

    let captured = scene(&mut client).await;
    let GeometryResponse::ScenePage { occurrences, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: captured.session_id.clone(),
            revision: captured.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("housing occurrence")
    };
    assert_eq!(occurrences.len(), 1);
    let occurrence = &occurrences[0];
    let revision = status(&mut client).await.0.revision;
    // Operator placement must use native support/contact, not a guessed Z offset.
    let prepared = client
        .prepare_bed_pose(
            &captured,
            revision,
            occurrence,
            requested.printer.build_envelope,
            true,
        )
        .await
        .unwrap();
    assert_eq!(scene(&mut client).await, captured);
    assert!(matches!(
        client
            .geometry(GeometryCommand::SetInstancePose {
                session_id: captured.session_id,
                base_revision: prepared.scene_revision,
                project_revision: prepared.project_revision,
                occurrence_id: occurrence.occurrence_id,
                pose: prepared.pose,
            })
            .await
            .unwrap(),
        GeometryResponse::SceneChanged { .. }
    ));
    let placed_scene = scene(&mut client).await;
    let scope = SurfaceScope::Definition {
        definition_id: occurrence.definition_id.clone(),
    };
    let whole = edit_process(
        &mut client,
        ProcessEdit::SetSparseFraction {
            scope: scope.clone(),
            fraction: 0.20,
        },
    )
    .await;
    let whole_provenance = effective_process(&whole, occurrence.occurrence_id)
        .provenance
        .unwrap();
    let bounds = ProcessSelector::SourceBoxMm {
        min: [-1.0, -1.0, -1.0],
        max: [8.0, 19.0, 11.0],
    };
    let authored = edit_process(
        &mut client,
        ProcessEdit::SetBoxSparseFraction {
            scope: scope.clone(),
            min: [-1.0, -1.0, -1.0],
            max: [8.0, 19.0, 11.0],
            fraction: 0.60,
            interface: Some(ProcessInterfacePermission::SeparatorWall),
        },
    )
    .await;
    let (box_layer, box_operation) = authored
        .process_document
        .layers
        .iter()
        .find_map(|layer| {
            layer
                .operations
                .iter()
                .find(|operation| operation.selector == bounds)
                .map(|operation| (layer.layer_id, operation.operation_id))
        })
        .expect("authored source box");
    let box_provenance = ProcessProvenance {
        layer_id: box_layer,
        operation_id: box_operation,
    };
    let eligibility = preflight(&mut client).await;
    assert_eq!(eligibility.status, JobStatus::Completed, "{eligibility:?}");
    let Some(JobResult::ManufacturingPreflight { report }) = eligibility.result else {
        panic!("housing preflight report")
    };
    assert_eq!(report.disposition, PreflightDisposition::Admitted);
    assert!(report.software_only);
    let record = compile(&mut client).await;
    let bundle = fetch_bundle(&mut client, &record).await.unwrap();
    assert!(record.summary.software_only && bundle.verification.verified);
    assert_eq!(bundle.provenance.intent, requested);
    assert_eq!(
        bundle.provenance.process_document,
        authored.process_document
    );
    assert_eq!(bundle.provenance.definitions.len(), 1);
    assert_eq!(bundle.provenance.occurrences.len(), 1);
    assert_eq!(bundle.provenance.occurrences[0].pose, prepared.pose);
    assert_eq!(scene(&mut client).await, placed_scene);
    // Finite-selection outcomes describe boxes. The whole-owned base remains
    // independently attributed on the actual light region, not another box.
    assert!(record.summary.process_resolution.iter().any(|resolution| {
        resolution.occurrence_id == occurrence.occurrence_id
            && resolution.definition_id == occurrence.definition_id
            && resolution.provenance == box_provenance
            && resolution.status == ProcessRegionStatus::Applied
    }));
    for (fraction, provenance) in [(0.20, whole_provenance), (0.60, box_provenance)] {
        assert!(bundle.plan.layers.iter().any(|layer| {
            layer.paths.iter().any(|path| {
                let Some(DepositionOwner::Regional { region_index }) = path.owner else {
                    return false;
                };
                let region = &layer.region_coverage.process_regions[usize::from(region_index)];
                region.fraction == fraction
                    && region.area_mm2 > 0.0
                    && region.contributions.iter().any(|contribution| {
                        contribution.occurrence_id == occurrence.occurrence_id
                            && contribution.definition_id == occurrence.definition_id
                            && contribution.provenance == Some(provenance)
                    })
            })
        }));
    }
    let separator_segments: usize = bundle
        .plan
        .layers
        .iter()
        .flat_map(|layer| &layer.paths)
        .filter(|path| path.kind == DepositionKind::SeparatorWall)
        .map(|path| path.points_mm.len() - 1)
        .sum();
    assert!(separator_segments > 0);
    let mut interfaces = 0;
    for layer in &bundle.plan.layers {
        for interface in &layer.region_coverage.process_interfaces {
            interfaces += 1;
            assert_eq!(interface.occurrence_id, occurrence.occurrence_id);
            assert_eq!(interface.definition_id, occurrence.definition_id);
            assert_eq!(interface.provenance, box_provenance);
            assert_eq!(interface.axis, 0);
            assert_eq!(interface.thickness_mm, requested.recipe.bead_width_mm);
            assert!(interface.area_mm2 > 0.0 && !interface.centerlines_mm.is_empty());
            let fractions = interface
                .region_indices
                .map(|index| layer.region_coverage.process_regions[usize::from(index)].fraction);
            assert!(fractions == [0.20, 0.60] || fractions == [0.60, 0.20]);
        }
    }
    assert!(interfaces > 0);

    // A bounds adjustment keeps authoring identity and invalidates the old result;
    // undo restores exactly the successful source-owned authority.
    let adjusted = edit_process(
        &mut client,
        ProcessEdit::SetBoxSparseFraction {
            scope,
            min: [-1.0, -1.0, -1.0],
            max: [7.0, 19.0, 11.0],
            fraction: 0.60,
            interface: Some(ProcessInterfacePermission::SeparatorWall),
        },
    )
    .await;
    let adjusted_layer = adjusted
        .process_document
        .layers
        .iter()
        .find(|layer| layer.layer_id == box_layer)
        .unwrap();
    let adjusted_operation = adjusted_layer
        .operations
        .iter()
        .find(|operation| operation.operation_id == box_operation)
        .unwrap();
    assert_eq!(
        adjusted_operation.selector,
        ProcessSelector::SourceBoxMm {
            min: [-1.0, -1.0, -1.0],
            max: [7.0, 19.0, 11.0],
        }
    );
    assert!(status(&mut client).await.1.is_none());
    assert_eq!(scene(&mut client).await, placed_scene);
    history(&mut client, true).await;
    let restored = status(&mut client).await;
    assert_eq!(restored.0.process_document, authored.process_document);
    assert_eq!(restored.1, Some(record.clone()));

    let mut original_motion = None;
    for phase in ["live", "fresh-read-only"] {
        let before = status(&mut client).await;
        assert_eq!(before.0.read_only, phase == "fresh-read-only");
        assert_eq!(before.0.process_document, authored.process_document);
        assert_eq!(before.1, Some(record.clone()));
        let ManufacturingResponse::OperationAccepted { operation } = client
            .manufacturing(ManufacturingCommand::Inspect {
                session_id: client.hello().session_id.clone(),
                base_revision: before.0.revision,
                output: VerificationOutput::ReportAndMotion,
            })
            .await
            .unwrap()
        else {
            panic!("housing actual-program inspection")
        };
        let inspected = wait(&mut client, operation.name).await;
        assert_eq!(inspected.status, JobStatus::Completed, "{inspected:?}");
        let Some(JobResult::ManufacturingVerified {
            record: verified_record,
            report,
            provenance,
            replay_display: Some(display),
            ..
        }) = inspected.result
        else {
            panic!("housing actual-program report and motion")
        };
        assert_eq!(verified_record, record);
        assert!(report.verified);
        assert_eq!(provenance.input_hash, record.input_hash);
        assert_eq!(provenance.process_document, authored.process_document);
        assert_eq!(
            display.info.program_hash,
            SourceHash::from_bytes(bundle.program.as_bytes())
        );
        let motion = client
            .fetch_replay_display(&record, &display)
            .await
            .unwrap();
        let mut program_lines = bundle.program.lines();
        let mut previous_line = 0;
        let mut actual_separators = 0;
        for event in motion[REPLAY_HEADER_BYTES..]
            .as_chunks::<REPLAY_RECORD_BYTES>()
            .0
        {
            let source_line = u32::from_le_bytes(event[40..44].try_into().unwrap());
            let line = program_lines
                .nth((source_line - previous_line - 1) as usize)
                .unwrap();
            previous_line = source_line;
            assert!(line.starts_with("G0 ") || line.starts_with("G1 "));
            for (axis, offset) in [('X', 0), ('Y', 8), ('Z', 16), ('E', 24), ('F', 32)] {
                if let Some(value) = line
                    .split_ascii_whitespace()
                    .find_map(|word| word.strip_prefix(axis).and_then(|v| v.parse::<f64>().ok()))
                {
                    assert_eq!(
                        f64::from_le_bytes(event[offset..offset + 8].try_into().unwrap()),
                        value
                    );
                }
            }
            // SPLR's documented separator-wall role, not a perimeter/bridge alias.
            if event[46] == 13 {
                actual_separators += 1;
                assert!(line.starts_with("G1 "));
            }
        }
        assert_eq!(actual_separators, separator_segments);
        if let Some(original) = &original_motion {
            assert_eq!(&motion, original);
        }
        let fetched = fetch_bundle(&mut client, &record).await.unwrap();
        assert_eq!(fetched, bundle);
        let destination = work.0.join(phase);
        let mut output = OutputDirectory::create(&destination).unwrap();
        output
            .write_manufacturing_bundle(&fetched, &report)
            .unwrap();
        output.write("motion.splr", &motion).unwrap();
        assert_eq!(status(&mut client).await, before);
        output.commit();
        drop(output);
        assert_eq!(
            std::fs::read(destination.join("program.gcode")).unwrap(),
            bundle.program.as_bytes()
        );
        assert_eq!(
            serde_json::from_slice::<VerificationReport>(
                &std::fs::read(destination.join("verification.json")).unwrap()
            )
            .unwrap(),
            report
        );
        if phase == "live" {
            original_motion = Some(motion);
            save(&mut client, &checkpoint).await;
            client.shutdown().await.unwrap();
            std::fs::remove_file(&source).unwrap();
            client = spawn().await;
            let opened = open(&mut client, &checkpoint, true).await;
            assert_eq!(opened.status, JobStatus::Completed, "{opened:?}");
        }
    }
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn process_overrides_resolve_inherit_reset_and_survive_history_and_read_only_reopen() {
    use spiling_contracts::{
        process::{ProcessAction, ProcessEdit, ProcessSelector},
        surface::SurfaceScope,
    };
    let workspace = Workspace::new();
    let source = workspace.source("box-mm.step");
    let checkpoint = workspace.0.join("process-project");
    let mut client = spawn().await;
    import(&mut client, &source).await;
    set_intent(&mut client, intent()).await;
    let original_scene = scene(&mut client).await;
    let GeometryResponse::ScenePage { occurrences, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: original_scene.session_id.clone(),
            revision: original_scene.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("source occurrences")
    };
    let first = occurrences[0].clone();
    let GeometryResponse::SceneChanged { .. } = client
        .geometry(GeometryCommand::AddInstance {
            session_id: original_scene.session_id,
            base_revision: original_scene.revision,
            definition_id: first.definition_id.clone(),
            pose: RigidPoseMm {
                translation_mm: [40.0, 0.0, 0.0],
                ..RigidPoseMm::IDENTITY
            },
        })
        .await
        .unwrap()
    else {
        panic!("repeated source occurrence")
    };
    let before_scene = scene(&mut client).await;
    let original = compile(&mut client).await;
    let original_bundle = fetch_bundle(&mut client, &original).await.unwrap();
    let definition_scope = SurfaceScope::Definition {
        definition_id: first.definition_id.clone(),
    };
    let occurrence_scope = SurfaceScope::Occurrence {
        occurrence_id: first.occurrence_id,
        expected_definition_id: first.definition_id.clone(),
    };
    let before = status(&mut client).await.0;
    let inherited = edit_process(&mut client, sparse_layer(definition_scope.clone(), 0.35)).await;
    let definition_layer = &inherited.process_document.layers[0];
    let definition_operation = &definition_layer.operations[0];
    for resolved in &inherited.resolved_process {
        assert_eq!(resolved.value.fraction, 0.35);
        let provenance = resolved.value.provenance.as_ref().unwrap();
        assert_eq!(provenance.layer_id, definition_layer.layer_id);
        assert_eq!(provenance.operation_id, definition_operation.operation_id);
    }
    assert!(status(&mut client).await.1.is_none());
    assert_eq!(scene(&mut client).await, before_scene);
    history(&mut client, true).await;
    assert_eq!(status(&mut client).await.1, Some(original));
    assert_eq!(
        effective_process(&status(&mut client).await.0, first.occurrence_id).fraction,
        1.0
    );
    history(&mut client, false).await;
    assert_eq!(
        status(&mut client).await.0.resolved_process,
        inherited.resolved_process
    );
    assert!(status(&mut client).await.1.is_none());

    // Every mutation binds the persistent input revision and exact source identity.
    let result = client
        .project(ProjectCommand::EditProcess {
            session_id: before_scene.session_id.clone(),
            base_revision: before.revision,
            edit: sparse_layer(occurrence_scope.clone(), 0.6),
        })
        .await;
    assert!(
        matches!(result, Err(ClientError::Project(error)) if error.code == ProjectErrorCode::StaleRevision)
    );
    let current = status(&mut client).await.0;
    let result = client
        .project(ProjectCommand::EditProcess {
            session_id: before_scene.session_id.clone(),
            base_revision: current.revision,
            edit: sparse_layer(
                SurfaceScope::Occurrence {
                    occurrence_id: first.occurrence_id,
                    expected_definition_id: DefinitionId::parse("0".repeat(64)).unwrap(),
                },
                0.6,
            ),
        })
        .await;
    assert!(
        matches!(result, Err(ClientError::Project(error)) if error.code == ProjectErrorCode::InvalidProject)
    );
    assert_eq!(status(&mut client).await.0, current);

    let local = edit_process(&mut client, sparse_layer(occurrence_scope.clone(), 0.6)).await;
    let local_value = effective_process(&local, first.occurrence_id);
    assert_eq!(local_value.fraction, 0.6);
    assert_ne!(
        local_value.provenance,
        effective_process(&inherited, first.occurrence_id).provenance
    );
    for sibling in &local.resolved_process {
        assert_eq!(sibling.global_fraction, 1.0);
        assert_eq!(
            sibling.inherited,
            *effective_process(&inherited, sibling.occurrence_id)
        );
        if sibling.occurrence_id != first.occurrence_id {
            assert_eq!(sibling.value.fraction, 0.35);
            assert_eq!(
                sibling.value.provenance,
                effective_process(&inherited, sibling.occurrence_id).provenance
            );
        }
    }
    let local_layer = local_value.provenance.as_ref().unwrap().layer_id;
    let erased = edit_process(
        &mut client,
        ProcessEdit::AddOperation {
            layer_id: local_layer,
            selector: ProcessSelector::WholeOwnedVolume,
            interface: None,
            action: ProcessAction::Erase,
        },
    )
    .await;
    assert_eq!(erased.resolved_process, inherited.resolved_process);
    history(&mut client, true).await;
    assert_eq!(
        status(&mut client).await.0.resolved_process,
        local.resolved_process
    );
    let latest = edit_process(&mut client, sparse_layer(occurrence_scope.clone(), 0.7)).await;
    let latest_layer = effective_process(&latest, first.occurrence_id)
        .provenance
        .as_ref()
        .unwrap()
        .layer_id;
    let disabled = edit_process(
        &mut client,
        ProcessEdit::SetLayerEnabled {
            layer_id: latest_layer,
            enabled: false,
        },
    )
    .await;
    assert_eq!(disabled.resolved_process, local.resolved_process);
    let reset = edit_process(
        &mut client,
        ProcessEdit::Reset {
            scope: occurrence_scope.clone(),
        },
    )
    .await;
    assert_eq!(reset.resolved_process, inherited.resolved_process);
    history(&mut client, true).await;
    assert_eq!(
        status(&mut client).await.0.resolved_process,
        disabled.resolved_process
    );
    history(&mut client, false).await;
    assert_eq!(
        status(&mut client).await.0.resolved_process,
        reset.resolved_process
    );
    assert_eq!(scene(&mut client).await, before_scene);
    let preflight_input = status(&mut client).await;
    let eligibility = preflight(&mut client).await;
    assert_eq!(eligibility.status, JobStatus::Completed, "{eligibility:?}");
    assert_eq!(status(&mut client).await, preflight_input);
    let compiled = compile(&mut client).await;
    assert_ne!(compiled.input_hash, original_bundle.provenance.input_hash);
    let bundle = fetch_bundle(&mut client, &compiled).await.unwrap();
    assert_eq!(bundle.provenance.process_document, reset.process_document);
    assert!(bundle.verification.verified);
    assert!(
        bundle.verification.deposited_volume_mm3
            < original_bundle.verification.deposited_volume_mm3,
        "effective local density must change actual deposited output"
    );
    assert_ne!(bundle.program, original_bundle.program);
    save(&mut client, &checkpoint).await;
    let saved = status(&mut client).await.0;
    client.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();

    let mut reader = spawn().await;
    let opened = open(&mut reader, &checkpoint, true).await;
    assert_eq!(opened.status, JobStatus::Completed, "{opened:?}");
    let reopened = status(&mut reader).await.0;
    assert!(reopened.read_only);
    assert_eq!(reopened.process_document, saved.process_document);
    assert_eq!(reopened.resolved_process, saved.resolved_process);
    assert_eq!(inspect(&mut reader).await.0, compiled);
    assert_eq!(fetch_bundle(&mut reader, &compiled).await.unwrap(), bundle);
    for edit in [
        sparse_layer(definition_scope, 0.5),
        ProcessEdit::Reset {
            scope: occurrence_scope,
        },
        ProcessEdit::SetLayerEnabled {
            layer_id: definition_layer.layer_id,
            enabled: false,
        },
    ] {
        let result = reader
            .project(ProjectCommand::EditProcess {
                session_id: reader.hello().session_id.clone(),
                base_revision: reopened.revision,
                edit,
            })
            .await;
        assert!(
            matches!(result, Err(ClientError::Project(error)) if error.code == ProjectErrorCode::ReadOnly)
        );
        assert_eq!(status(&mut reader).await.0, reopened);
    }
    reader.shutdown().await.unwrap();
}

#[tokio::test]
async fn queued_process_pose_and_source_changes_reject_stale_compile_and_preflight() {
    use spiling_contracts::{
        metadata,
        rpc::{DiagnosticState, RunDiagnosticRequest},
        surface::SurfaceScope,
    };
    let workspace = Workspace::new();
    let mut client = spawn().await;
    import(&mut client, &workspace.source("box-mm.step")).await;
    set_intent(&mut client, intent()).await;
    let initial_scene = scene(&mut client).await;
    let GeometryResponse::ScenePage { occurrences, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: initial_scene.session_id,
            revision: initial_scene.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("source occurrence")
    };
    let occurrence = occurrences[0].clone();
    for preflight_only in [false, true] {
        for mutation in ["process", "pose", "source"] {
            let diagnostic = client
                .rpc()
                .run_diagnostic(RunDiagnosticRequest {
                    parent: "diagnostics/queued-process-input".into(),
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
            let input = status(&mut client).await.0;
            let session_id = client.hello().session_id.clone();
            let command = if preflight_only {
                ManufacturingCommand::Preflight {
                    session_id,
                    base_revision: input.revision,
                }
            } else {
                ManufacturingCommand::Compile {
                    session_id,
                    base_revision: input.revision,
                }
            };
            let ManufacturingResponse::OperationAccepted { operation } =
                client.manufacturing(command).await.unwrap()
            else {
                panic!("queued manufacturing operation")
            };
            match mutation {
                "process" => {
                    edit_process(
                        &mut client,
                        sparse_layer(
                            SurfaceScope::Definition {
                                definition_id: occurrence.definition_id.clone(),
                            },
                            0.35,
                        ),
                    )
                    .await;
                }
                "pose" => {
                    let scene = scene(&mut client).await;
                    client
                        .geometry(GeometryCommand::SetInstancePose {
                            session_id: scene.session_id,
                            base_revision: scene.revision,
                            project_revision: input.revision,
                            occurrence_id: occurrence.occurrence_id,
                            pose: RigidPoseMm {
                                translation_mm: [if preflight_only { 2.0 } else { 1.0 }, 0.0, 0.0],
                                ..RigidPoseMm::IDENTITY
                            },
                        })
                        .await
                        .unwrap();
                }
                "source" => {
                    let scene = scene(&mut client).await;
                    client
                        .geometry(GeometryCommand::RemoveInstance {
                            session_id: scene.session_id,
                            base_revision: scene.revision,
                            occurrence_id: occurrence.occurrence_id,
                        })
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            client
                .rpc()
                .cancel_operation(&diagnostic.name)
                .await
                .unwrap();
            let stale = wait(&mut client, operation.name).await;
            assert_eq!(stale.status, JobStatus::Failed, "{stale:?}");
            assert!(
                matches!(&stale.error, Some(JobError::Manufacturing { error }) if error.code == ManufacturingErrorCode::StaleRevision),
                "{stale:?}"
            );
            assert!(stale.outputs.is_empty());
            assert!(status(&mut client).await.1.is_none());
            if mutation == "source" {
                let scene = scene(&mut client).await;
                assert!(matches!(
                    client
                        .project(ProjectCommand::Undo {
                            session_id: scene.session_id,
                            base_revision: scene.revision,
                        })
                        .await
                        .unwrap(),
                    ProjectResponse::SceneChanged { .. }
                ));
            }
        }
    }
    client.shutdown().await.unwrap();
}
