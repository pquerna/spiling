// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;

pub(super) async fn select(client: &EngineClient, layout_id: &PrintLayoutId) {
    let ProjectResponse::Status { info } = client
        .project(ProjectCommand::Get {
            session_id: client.hello().session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("project")
    };
    let revision = info.revision;
    let ProjectResponse::SceneChanged { info, summary } = client
        .project(ProjectCommand::SelectPrintLayout {
            session_id: client.hello().session_id.clone(),
            base_revision: revision,
            layout_id: layout_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("selected plate")
    };
    assert_eq!(info.revision, revision, "navigation changed content");
    assert_eq!(&summary.layout_id, layout_id);
}
pub(super) async fn edit(client: &EngineClient, edit: PrintLayoutEdit) -> ProjectInfo {
    let ProjectResponse::Status { info } = client
        .project(ProjectCommand::Get {
            session_id: client.hello().session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("project")
    };
    let ProjectResponse::SceneChanged { info, .. } = client
        .project(ProjectCommand::EditPrintLayout {
            session_id: client.hello().session_id.clone(),
            base_revision: info.revision,
            edit,
        })
        .await
        .unwrap()
    else {
        panic!("plate edit")
    };
    info
}
#[tokio::test]
async fn independent_plate_jobs_transfer_history_replay_and_reopen_without_source() {
    let workspace = Workspace::new();
    let source = workspace.0.join("part.step");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/independent-exporters/occt-box-mm.step"),
        &source,
    )
    .unwrap();
    let mut client = spawn().await;
    import(&mut client, &source).await;
    let profile: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    set_intent(&mut client, profile).await;
    let a = current_layout(&client).await;
    let info = edit(
        &client,
        PrintLayoutEdit::Duplicate {
            source: a.clone(),
            name: "Spare parts".into(),
        },
    )
    .await;
    let b = info.layout_id;
    assert_ne!(a, b);
    assert_eq!(scene(&mut client).await.occurrence_count, 1);
    let b_original = compile(&mut client).await;
    select(&client, &a).await;
    let a_original = compile(&mut client).await;
    assert_ne!(
        a_original.hash, b_original.hash,
        "distinct occurrence ownership lost"
    );
    select(&client, &b).await;
    assert_eq!(
        inspect(&mut client).await.0,
        b_original,
        "preparing A invalidated B"
    );
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
        panic!("occurrences")
    };
    let row = &occurrences[0];
    let mut pose = row.pose;
    pose.translation_mm[0] += 20.;
    let rev = status(&mut client).await.0.revision;
    client
        .geometry(GeometryCommand::SetInstancePose {
            session_id: before.session_id,
            base_revision: before.revision,
            project_revision: rev,
            occurrence_id: row.occurrence_id,
            pose,
        })
        .await
        .unwrap();
    assert!(status(&mut client).await.1.is_none());
    select(&client, &a).await;
    assert_eq!(
        inspect(&mut client).await.0,
        a_original,
        "editing B invalidated A"
    );
    let receipt = edit(
        &client,
        PrintLayoutEdit::Transfer {
            source: b.clone(),
            target: a.clone(),
            occurrences: vec![row.occurrence_id],
            copy: false,
        },
    )
    .await;
    assert_eq!(
        receipt
            .layouts
            .iter()
            .find(|l| l.layout_id == b)
            .unwrap()
            .occurrence_count,
        0
    );
    assert_eq!(scene(&mut client).await.occurrence_count, 2);
    assert!(status(&mut client).await.1.is_none());
    super::layout::placement_history(&mut client, true, 1).await;
    assert_eq!(inspect(&mut client).await.0, a_original);
    super::layout::placement_history(&mut client, false, 2).await;
    super::layout::placement_history(&mut client, true, 1).await;
    select(&client, &b).await;
    let b_final = compile(&mut client).await;
    assert!(inspect(&mut client).await.1.verified);
    // Wrong explicit addresses refuse even when project/session/revision otherwise match.
    let revision = status(&mut client).await.0.revision;
    let response = client
        .manufacturing(ManufacturingCommand::Compile {
            session_id: client.hello().session_id.clone(),
            layout_id: a.clone(),
            base_revision: revision,
        })
        .await
        .unwrap();
    let ManufacturingResponse::OperationAccepted { operation } = response else {
        panic!("expected operation receipt")
    };
    let refused = wait(&mut client, operation.name).await;
    assert!(
        matches!(refused.error,Some(JobError::Manufacturing {error}) if error.code==ManufacturingErrorCode::StaleRevision)
    );
    assert!(refused.result.is_none() && refused.outputs.is_empty());
    let path = workspace.0.join("two-plates.spil");
    save(&mut client, &path).await;
    client.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();
    let mut reader = spawn().await;
    assert_eq!(
        open(&mut reader, &path, true).await.status,
        JobStatus::Completed
    );
    let info = status(&mut reader).await.0;
    assert_eq!(info.layouts.len(), 2);
    let scene = scene(&mut reader).await;
    let GeometryResponse::LayoutOverview { overview } = reader
        .geometry(GeometryCommand::GetLayoutOverview {
            session_id: reader.hello().session_id.clone(),
            scene_revision: scene.revision,
            project_revision: info.revision,
        })
        .await
        .unwrap()
    else {
        panic!("overview")
    };
    assert_eq!(overview.layouts.len(), 2);
    assert_eq!(
        overview
            .layouts
            .iter()
            .map(|l| l.parts.len())
            .sum::<usize>(),
        2
    );
    assert_eq!(
        overview
            .layouts
            .iter()
            .map(|l| l.layout_id.clone())
            .collect::<Vec<_>>(),
        info.layouts
            .iter()
            .map(|l| l.layout_id.clone())
            .collect::<Vec<_>>()
    );
    assert!(overview.layouts.iter().all(|l| l.build_area_mm.is_some()));
    assert!(!info.dirty);
    select(&reader, &a).await;
    assert_eq!(inspect(&mut reader).await.0, a_original);
    select(&reader, &b).await;
    assert_eq!(inspect(&mut reader).await.0, b_final);
    let info = status(&mut reader).await.0;
    assert!(!info.dirty);
    assert!(
        matches!(reader.project(ProjectCommand::EditPrintLayout {session_id:reader.hello().session_id.clone(),base_revision:info.revision,edit:PrintLayoutEdit::Rename {layout_id:b,name:"Forbidden".into()}}).await.unwrap_err(),ClientError::Project(error) if error.code==ProjectErrorCode::ReadOnly)
    );
    reader.shutdown().await.unwrap();
}

#[tokio::test]
async fn arrangement_excludes_other_plates_occurrence_process_contributions() {
    let workspace = Workspace::new();
    let source = workspace.0.join("part.step");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/independent-exporters/occt-box-mm.step"),
        &source,
    )
    .unwrap();
    let mut client = spawn().await;
    import(&mut client, &source).await;
    set_intent(
        &mut client,
        serde_json::from_str(include_str!(
            "../../../../fixtures/manufacturing/complete-sparse.intent.json"
        ))
        .unwrap(),
    )
    .await;
    let a = current_layout(&client).await;
    edit(
        &client,
        PrintLayoutEdit::Duplicate {
            source: a.clone(),
            name: "Other process".into(),
        },
    )
    .await;
    let s = scene(&mut client).await;
    let GeometryResponse::ScenePage { occurrences, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: s.session_id,
            revision: s.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("parts")
    };
    let part = &occurrences[0];
    edit_process(
        &mut client,
        spiling_contracts::process::ProcessEdit::SetSparseFraction {
            scope: spiling_contracts::surface::SurfaceScope::Occurrence {
                occurrence_id: part.occurrence_id,
                expected_definition_id: part.definition_id.clone(),
            },
            fraction: 0.6,
        },
    )
    .await;
    select(&client, &a).await;
    let s = scene(&mut client).await;
    let revision = status(&mut client).await.0.revision;
    let GeometryResponse::OperationAccepted { operation } = client
        .geometry(GeometryCommand::PrepareLayout {
            session_id: s.session_id,
            base_revision: s.revision,
            project_revision: revision,
            request: LayoutRequest {
                objective: LayoutObjective::Copies,
                template: Some(OccurrenceId::new(1).unwrap()),
                quantity: 2,
                use_additional_plates: false,
                pinned: vec![],
                spacing_mm: 3.0,
                style: LayoutStyle::Balanced,
            },
        })
        .await
        .unwrap()
    else {
        panic!("layout accepted")
    };
    let terminal = wait(&mut client, operation.name).await;
    assert_eq!(terminal.status, JobStatus::Completed, "{terminal:?}");
    client.shutdown().await.unwrap();
}
