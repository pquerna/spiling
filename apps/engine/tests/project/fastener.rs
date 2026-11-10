// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::analysis::{
    fastener::{
        AnalysisFastener, AnalysisFastenerPreparation, AnalysisMeshGeometry, FastenerAttachment,
        HoleEnd,
    },
    fields::AnalysisFields,
    load::ForceFootprint,
    mechanics::{AnalysisRunSettings, AnalysisRunSummary},
    *,
};

#[test]
fn native_fastener_service_reactions_reuse_refusals_and_portable_result() {
    std::thread::Builder::new()
        .name("native-fastener-service".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(Box::pin(fastener_service_check()));
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn prepare(client: &mut EngineClient, reference: FaceRef) -> NativeOperationView {
    let captured = info(client).await;
    let ProjectResponse::OperationAccepted { operation } = client
        .project(ProjectCommand::PrepareAnalysisFastener {
            session_id: client.hello().session_id.clone(),
            base_revision: captured.revision,
            reference,
        })
        .await
        .unwrap()
    else {
        panic!("typed fastener preparation operation expected");
    };
    let result = wait_job(client, operation.name).await;
    assert_eq!(info(client).await, captured);
    result
}

async fn put(client: &mut EngineClient, case: &AnalysisCase) {
    let captured = info(client).await;
    client
        .project(ProjectCommand::EditAnalysis {
            session_id: client.hello().session_id.clone(),
            base_revision: captured.revision,
            edit: AnalysisEdit::Put { case: case.clone() },
        })
        .await
        .unwrap();
    assert_eq!(info(client).await.analysis.cases[0].case, *case);
}

async fn run(client: &mut EngineClient, case: &AnalysisCase) -> NativeOperationView {
    let captured = info(client).await;
    let settings: AnalysisRunSettings = serde_json::from_slice(include_bytes!(
        "../../../../fixtures/analysis/hook-assumptions.json"
    ))
    .unwrap();
    assert_eq!(settings.maximum_span_mm, 12.0);
    let ProjectResponse::OperationAccepted { operation } = client
        .project(ProjectCommand::RunAnalysis {
            session_id: client.hello().session_id.clone(),
            base_revision: captured.revision,
            case_id: case.id.clone(),
            settings: std::sync::Arc::new(settings),
        })
        .await
        .unwrap()
    else {
        panic!("typed analysis operation expected");
    };
    let result = wait_job(client, operation.name).await;
    assert_eq!(info(client).await, captured);
    result
}

async fn fields(client: &mut EngineClient, operation: &NativeOperationView) -> Vec<u8> {
    assert_eq!(operation.status, JobStatus::Completed, "{operation:?}");
    let Some(JobResult::AnalysisCompleted {
        input_hash,
        captured_revision,
        resource,
        ..
    }) = &operation.result
    else {
        panic!("completed numerical fields expected");
    };
    client
        .fetch_analysis_fields(input_hash, *captured_revision, resource)
        .await
        .unwrap()
}

fn check_reactions(bytes: &[u8], case: &AnalysisCase) {
    assert_eq!(&bytes[..8], b"SPLA\x02\0\0\0");
    let fields = AnalysisFields::decode(bytes).unwrap();
    assert_eq!(fields.summary.case.case, *case);
    assert!(fields.summary.case.case.restraints.is_empty());
    assert_eq!(fields.element_count, 144);
    assert_eq!(fields.summary.dofs, 4704);
    assert_eq!(fields.summary.assessment.rigid_constraint_ranks, vec![6]);
    assert!(matches!(
        fields.summary.mesh_geometry,
        AnalysisMeshGeometry::RationalBoredPlate { .. }
    ));
    assert!(fields.summary.maximum_displacement_mm > 0.0);
    let response = &fields.summary.fastener_responses[0];
    assert_eq!(response.fastener_id, case.fasteners[0].id);
    assert!((response.area_mm2 / (std::f64::consts::PI * 7.0) - 1.0).abs() < 1e-9);
    let mut force = [0.0; 3];
    let mut moment = [0.0; 3];
    let mut area = 0.0;
    for index in 0..fields.patch_count {
        let sample = fields.patch(index).unwrap();
        if sample.patch_index as usize != case.loads.len() {
            continue;
        }
        let sample_force = sample.traction_mpa.map(|value| value * sample.area_mm2);
        let [point_x, point_y, point_z] = sample.point_mm;
        let [force_x, force_y, force_z] = sample_force;
        let sample_moment = [
            point_y * force_z - point_z * force_y,
            point_z * force_x - point_x * force_z,
            point_x * force_y - point_y * force_x,
        ];
        area += sample.area_mm2;
        for axis in 0..3 {
            force[axis] += sample_force[axis];
            moment[axis] += sample_moment[axis];
        }
    }
    assert!((area / response.area_mm2 - 1.0).abs() < 1e-9);
    for axis in 0..3 {
        assert!((force[axis] - response.reaction_force_n[axis]).abs() < 1e-6);
        assert!((moment[axis] - response.reaction_moment_n_mm[axis]).abs() < 1e-5);
        assert!((force[axis] + case.loads[0].force_n[axis]).abs() < 1e-6);
        assert!((moment[axis] + case.loads[0].moment_n_mm[axis]).abs() < 1e-5);
    }
}

async fn fastener_service_check() {
    let workspace = Workspace::new();
    let store = tempfile::tempdir().unwrap();
    let compute = spiling_contracts::compute::ComputeSettings {
        default_backend: spiling_contracts::compute::ComputeBackend::Cpu,
        fea_backend: None,
    };
    let command = || {
        let mut command = Command::new(ENGINE);
        command
            .env("TOKIO_WORKER_THREADS", "2")
            .env("RAYON_NUM_THREADS", "2");
        command
    };
    let mut client = EngineClient::spawn_command_with_compute(command(), store.path(), compute)
        .await
        .unwrap();
    let solid_source = workspace.source("box-mm.step");
    import(&mut client, &solid_source, RigidPoseMm::IDENTITY).await;
    let solid_face = reference(&mut client).await;
    let missing_bore = prepare(&mut client, solid_face).await;
    assert_eq!(missing_bore.status, JobStatus::Failed, "{missing_bore:?}");
    assert!(missing_bore.result.is_none() && missing_bore.outputs.is_empty());
    new_project(&mut client, true).await;
    let source = workspace.path("plate.step");
    std::fs::copy(
        fixture("independent-exporters/occt-through-hole-mm.step"),
        &source,
    )
    .unwrap();
    let current_scene = import(&mut client, &source, RigidPoseMm::IDENTITY).await;
    let (definitions, occurrences) = records(&mut client).await;
    let mut bore = None;
    let mut loaded = None;
    for face in faces(&mut client, &definitions[0]).await {
        let reference = FaceRef {
            session_id: current_scene.session_id.clone(),
            scene_revision: current_scene.revision,
            occurrence_id: occurrences[0].occurrence_id,
            definition_id: definitions[0].definition_id.clone(),
            face_id: face.face_id,
        };
        match inspect(&mut client, reference.clone())
            .await
            .face
            .geometry
            .analytic
        {
            Some(AnalyticSurfaceDescription::Cylinder { radius_mm, .. })
                if (radius_mm - 3.0).abs() < 1e-7 =>
            {
                bore = Some(reference);
            }
            Some(AnalyticSurfaceDescription::Plane { origin_mm, normal })
                if normal[0].abs() > 0.99 && (origin_mm[0] - 20.0).abs() < 1e-7 =>
            {
                loaded = Some(reference);
            }
            _ => {}
        }
    }
    let loaded = loaded.expect("native x=20 load face");
    let wrong_face = prepare(&mut client, loaded.clone()).await;
    assert_eq!(wrong_face.status, JobStatus::Failed, "{wrong_face:?}");
    assert!(wrong_face.result.is_none() && wrong_face.outputs.is_empty());
    let prepared = prepare(&mut client, bore.expect("native cylindrical bore")).await;
    assert_eq!(prepared.status, JobStatus::Completed, "{prepared:?}");
    let Some(JobResult::AnalysisFastenerPrepared { preparation }) = prepared.result else {
        panic!("typed complete native hole expected");
    };
    let preparation: AnalysisFastenerPreparation = preparation;
    preparation.validate().unwrap();
    assert_eq!(preparation.hole.center_mm, [10.0, 10.0, 0.0]);
    assert_eq!(preparation.hole.radius_mm, 3.0);
    assert_eq!(preparation.hole.depth_mm, 8.0);
    let captured = info(&mut client).await;
    let mut case = AnalysisCase {
        id: AnalysisCaseId::new(),
        name: "ideal native clamp".into(),
        layout_id: captured.layout_id,
        occurrence_id: loaded.occurrence_id,
        definition_id: loaded.definition_id,
        source_hash: preparation.source_hash,
        mode: AnalysisMode::Basic,
        loads: vec![AnalysisFaceLoad {
            face_id: loaded.face_id,
            footprint: ForceFootprint::WholeFace {},
            force_n: [0.0, 0.0, -10.0],
            moment_n_mm: [-100.0, 200.0, 0.0],
        }],
        restraints: vec![],
        fasteners: vec![AnalysisFastener {
            id: 1,
            name: "ideal mounting connection".into(),
            hole: preparation.hole,
            attachment: FastenerAttachment::Clamp {
                end: HoleEnd::Start,
                washer_diameter_mm: 8.0,
            },
        }],
    };
    for wrong_hole in [true, false] {
        let mut invalid = case.clone();
        if wrong_hole {
            invalid.fasteners[0].hole.radius_mm = 3.1;
        } else {
            invalid.fasteners[0].attachment = FastenerAttachment::Clamp {
                end: HoleEnd::End,
                washer_diameter_mm: 21.0,
            };
        }
        let before = info(&mut client).await;
        let session_id = client.hello().session_id.clone();
        project_rejection(
            &mut client,
            ProjectCommand::EditAnalysis {
                session_id,
                base_revision: before.revision,
                edit: AnalysisEdit::Put { case: invalid },
            },
            ProjectErrorCode::InvalidProject,
        )
        .await;
        assert_eq!(info(&mut client).await, before);
    }
    put(&mut client, &case).await;
    assert_eq!(scene(&mut client).await, current_scene);
    let cold_job = run(&mut client, &case).await;
    let cold = fields(&mut client, &cold_job).await;
    check_reactions(&cold, &case);
    assert!(
        !AnalysisFields::decode(&cold)
            .unwrap()
            .summary
            .costs
            .operator_reused
    );
    case.loads[0].force_n = case.loads[0].force_n.map(|value| 2.0 * value);
    case.loads[0].moment_n_mm = case.loads[0].moment_n_mm.map(|value| 2.0 * value);
    put(&mut client, &case).await;
    let warm_job = run(&mut client, &case).await;
    let warm = fields(&mut client, &warm_job).await;
    check_reactions(&warm, &case);
    let first = AnalysisFields::decode(&cold).unwrap();
    let second = AnalysisFields::decode(&warm).unwrap();
    assert!(second.summary.costs.operator_reused);
    assert_eq!(second.summary.operator_hash, first.summary.operator_hash);
    assert_ne!(second.summary.input_hash, first.summary.input_hash);
    assert!(
        (second.summary.strain_energy_n_mm / first.summary.strain_energy_n_mm - 4.0).abs() < 1e-7
    );
    for index in 0..first.node_count {
        for axis in 0..3 {
            assert!(
                (second.node(index).unwrap().displacement_mm[axis]
                    - 2.0 * first.node(index).unwrap().displacement_mm[axis])
                    .abs()
                    < 1e-9
            );
        }
    }
    let before = info(&mut client).await;
    client
        .project(ProjectCommand::AttachAnalysisResult {
            session_id: client.hello().session_id.clone(),
            base_revision: before.revision,
            input_hash: second.summary.input_hash.clone(),
        })
        .await
        .unwrap();
    let path = workspace.path("clamp.spil");
    let attached = save(&mut client, Some(&path)).await;
    assert_eq!(attached.analysis.results.len(), 1);
    let portable_directory = tempfile::tempdir().unwrap();
    let portable_path = portable_directory.path().join("only.spil");
    copy_project(&path, &portable_path);
    for allow_axial_slide in [false, true] {
        case.fasteners[0].attachment = FastenerAttachment::Pin {
            allow_axial_slide,
            allow_spin: true,
        };
        put(&mut client, &case).await;
        let mechanism = run(&mut client, &case).await;
        assert_eq!(mechanism.status, JobStatus::Failed, "{mechanism:?}");
        assert!(mechanism.result.is_none() && mechanism.outputs.is_empty());
        assert!(format!("{:?}", mechanism.error).contains("DisconnectedOrMechanism"));
    }
    for end in [HoleEnd::Start, HoleEnd::End] {
        for attachment in [
            FastenerAttachment::Clamp {
                end,
                washer_diameter_mm: 8.0,
            },
            FastenerAttachment::BondedEngagement {
                end,
                engagement_mm: 3.1,
            },
        ] {
            case.fasteners[0].attachment = attachment;
            put(&mut client, &case).await;
            assert_eq!(info(&mut client).await.analysis.cases[0].case, case);
        }
    }
    client.shutdown().await.unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::remove_file(&solid_source).unwrap();
    std::fs::remove_file(&path).unwrap();
    drop(store);
    assert_eq!(
        std::fs::read_dir(portable_directory.path())
            .unwrap()
            .count(),
        1
    );
    let empty_store = tempfile::tempdir().unwrap();
    assert_eq!(std::fs::read_dir(empty_store.path()).unwrap().count(), 0);
    let mut portable =
        EngineClient::spawn_command_with_compute(command(), empty_store.path(), compute)
            .await
            .unwrap();
    assert!(portable.native_operation(&warm_job.name).await.is_err());
    open(&mut portable, &portable_path, true, false, false).await;
    let before_read = info(&mut portable).await;
    assert!(before_read.read_only && !before_read.dirty);
    assert_eq!(before_read.analysis.results, attached.analysis.results);
    let ProjectResponse::OperationAccepted { operation } = portable
        .project(ProjectCommand::GetAnalysisResult {
            session_id: portable.hello().session_id.clone(),
            base_revision: before_read.revision,
            case_id: second.summary.case.case.id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("saved result operation expected");
    };
    let retrieved = wait_job(&mut portable, operation.name).await;
    assert_eq!(retrieved.status, JobStatus::Completed, "{retrieved:?}");
    let Some(JobResult::AnalysisResultPrepared {
        project_id,
        read_revision,
        record,
        resource,
    }) = retrieved.result
    else {
        panic!("portable result read, not a new numerical solve, expected");
    };
    assert_eq!(project_id, before_read.project_id);
    assert_eq!(read_revision, before_read.revision);
    assert_eq!(record, attached.analysis.results[0]);
    let returned = portable
        .fetch_analysis_fields(&record.input_hash, record.captured_revision, &resource)
        .await
        .unwrap();
    assert_eq!(returned, warm);
    check_reactions(&returned, &second.summary.case.case);
    assert_eq!(info(&mut portable).await, before_read);
    check_tampered_results(&workspace, &portable_path, &warm, &mut portable).await;
    portable.shutdown().await.unwrap();
}

async fn check_tampered_results(
    workspace: &Workspace,
    saved: &Path,
    bytes: &[u8],
    client: &mut EngineClient,
) {
    let original = AnalysisFields::decode(bytes).unwrap();
    let metadata_bytes = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    for ownership in [false, true] {
        let target = workspace.path(if ownership {
            "wrong-curved-owner.spil"
        } else {
            "wrong-reaction.spil"
        });
        copy_project(saved, &target);
        let mut summary: AnalysisRunSummary = original.summary.clone();
        if ownership {
            let AnalysisMeshGeometry::RationalBoredPlate { profile } = &mut summary.mesh_geometry
            else {
                panic!("curved profile expected");
            };
            profile.outer_faces.swap(0, 2);
        } else {
            summary.fastener_responses[0].reaction_moment_n_mm[0] += 1.0;
        }
        let metadata = serde_json::to_vec(&summary).unwrap();
        let mut changed = bytes[..32].to_vec();
        changed[8..12].copy_from_slice(&(metadata.len() as u32).to_le_bytes());
        changed.extend_from_slice(&metadata);
        changed.extend_from_slice(&bytes[32 + metadata_bytes..]);
        assert_eq!(AnalysisFields::decode(&changed).is_ok(), ownership);
        let mut root = manifest(&target);
        let old_hash = root.analysis.results[0].hash.clone();
        root.analysis.results[0].hash = SourceHash::from_bytes(&changed);
        root.analysis.results[0].byte_count = changed.len() as u32;
        support::write_root(
            &target,
            &serde_json::to_vec(&root).unwrap(),
            &[(old_hash, changed)],
        );
        let before = info(client).await;
        let rejected = start_open(client, &target, true, false, false).await;
        let rejected = wait_job(client, rejected).await;
        assert_eq!(rejected.status, JobStatus::Failed, "{rejected:?}");
        assert!(rejected.result.is_none() && rejected.outputs.is_empty());
        assert_eq!(info(client).await, before);
    }
}
