// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;

fn native_box(workspace: &Workspace) -> PathBuf {
    let target = workspace.0.join("part.step");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/independent-exporters/occt-box-mm.step"),
        &target,
    )
    .unwrap();
    target
}
fn request(objective: LayoutObjective, quantity: u32) -> LayoutRequest {
    LayoutRequest {
        objective,
        template: (objective != LayoutObjective::Arrange).then(|| OccurrenceId::new(1).unwrap()),
        quantity,
        use_additional_plates: false,
        pinned: vec![],
        spacing_mm: 3.0,
        style: LayoutStyle::Balanced,
    }
}
async fn prepare(client: &mut EngineClient, request: LayoutRequest) -> LayoutProposal {
    let s = scene(client).await;
    let revision = status(client).await.0.revision;
    let GeometryResponse::OperationAccepted { operation } = client
        .geometry(GeometryCommand::PrepareLayout {
            session_id: s.session_id,
            base_revision: s.revision,
            project_revision: revision,
            request,
        })
        .await
        .unwrap()
    else {
        panic!("prepare layout")
    };
    let terminal = wait(client, operation.name).await;
    assert_eq!(terminal.status, JobStatus::Completed, "{terminal:?}");
    let Some(JobResult::LayoutPrepared { proposal }) = terminal.result else {
        panic!("layout proposal")
    };
    proposal.validate().unwrap();
    proposal
}
fn apply(proposal: &LayoutProposal) -> GeometryCommand {
    GeometryCommand::ApplyLayout {
        session_id: proposal.session_id.clone(),
        base_revision: proposal.scene_revision,
        project_revision: proposal.project_revision,
        proposal_id: proposal.id.clone(),
    }
}
pub(super) async fn placement_history(client: &mut EngineClient, undo: bool, count: u32) {
    let scene = scene(client).await;
    let command = if undo {
        ProjectCommand::Undo {
            session_id: scene.session_id,
            base_revision: scene.revision,
        }
    } else {
        ProjectCommand::Redo {
            session_id: scene.session_id,
            base_revision: scene.revision,
        }
    };
    let response = client.project(command).await.unwrap();
    assert!(
        matches!(
            response,
            ProjectResponse::Status { .. } | ProjectResponse::SceneChanged { .. }
        ),
        "{response:?}"
    );
    assert_eq!(super::scene(client).await.occurrence_count, count);
}
#[tokio::test]
async fn layout_proposals_preserve_current_apply_atomically_and_reopen_without_original_source() {
    let workspace = Workspace::new();
    let source = native_box(&workspace);
    let mut client = spawn().await;
    import(&mut client, &source).await;
    let profile: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    set_intent(&mut client, profile).await;
    let original = compile(&mut client).await;
    let before = status(&mut client).await;
    let proposal = prepare(&mut client, request(LayoutObjective::Copies, 2)).await;
    assert_eq!(proposal.placed_count, 2);
    assert_eq!(proposal.requested_count, 2);
    assert!(proposal.verification.verified);
    assert_eq!(
        status(&mut client).await,
        before,
        "read-only search changed Current"
    );
    assert_eq!(scene(&mut client).await.occurrence_count, 1);
    assert!(matches!(
        client.geometry(apply(&proposal)).await.unwrap(),
        GeometryResponse::SceneChanged { .. }
    ));
    assert_eq!(scene(&mut client).await.occurrence_count, 2);
    assert!(status(&mut client).await.1.is_none());
    let stale = client.geometry(apply(&proposal)).await.unwrap();
    assert!(
        matches!(stale,GeometryResponse::Error { error } if error.code == GeometryErrorCode::StaleRevision)
    );
    placement_history(&mut client, true, 1).await;
    assert_eq!(scene(&mut client).await.occurrence_count, 1);
    assert_eq!(status(&mut client).await.1, Some(original));
    placement_history(&mut client, false, 2).await;
    let copies = compile(&mut client).await;
    assert!(inspect(&mut client).await.1.verified);
    let path = workspace.0.join("copies.spil");
    save(&mut client, &path).await;
    client.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();
    let mut reader = spawn().await;
    assert_eq!(
        open(&mut reader, &path, true).await.status,
        JobStatus::Completed
    );
    assert_eq!(scene(&mut reader).await.occurrence_count, 2);
    assert_eq!(inspect(&mut reader).await.0, copies);
    let before = status(&mut reader).await;
    let proposal = prepare(&mut reader, request(LayoutObjective::Arrange, 1)).await;
    assert_eq!(status(&mut reader).await, before);
    assert!(
        matches!(reader.geometry(apply(&proposal)).await.unwrap_err(),ClientError::Project(e) if e.code == ProjectErrorCode::ReadOnly)
    );
    reader.shutdown().await.unwrap();
}
#[tokio::test]
async fn layout_partial_fit_keeps_existing_parts_and_pin_conflicts_are_explicit() {
    let workspace = Workspace::new();
    let mut client = spawn().await;
    import(&mut client, &native_box(&workspace)).await;
    let mut profile: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    profile.printer.build_envelope.max = [50.0, 30.0, 220.0];
    if let JobWorkflow::CompleteMarlin { job } = &mut profile.printer.workflow {
        job.prime_start_mm = [10.0, 29.0, 0.2];
        job.prime_end_mm = [30.0, 29.0, 0.2];
        job.park_xy_mm = [0.0, 29.0];
    }
    profile.validate().unwrap();
    set_intent(&mut client, profile).await;
    let before = status(&mut client).await;
    let mut q = request(LayoutObjective::Fill, 12);
    q.style = LayoutStyle::Dense;
    let proposal = prepare(&mut client, q).await;
    assert!(proposal.placed_count >= 1 && proposal.placed_count < proposal.requested_count);
    assert_eq!(proposal.requested_count, 12);
    assert_eq!(status(&mut client).await, before);
    assert!(matches!(
        client.geometry(apply(&proposal)).await.unwrap(),
        GeometryResponse::SceneChanged { .. }
    ));
    assert_eq!(
        scene(&mut client).await.occurrence_count,
        proposal.placed_count
    );
    let before = status(&mut client).await;
    let mut unchanged = request(LayoutObjective::Arrange, 1);
    unchanged.pinned = (1..=proposal.placed_count)
        .map(|id| OccurrenceId::new(id).unwrap())
        .collect();
    let unchanged = prepare(&mut client, unchanged).await;
    assert!(unchanged.placements.is_empty());
    assert!(matches!(
        client.geometry(apply(&unchanged)).await.unwrap(),
        GeometryResponse::Error { .. }
    ));
    assert_eq!(status(&mut client).await, before);
    let s = scene(&mut client).await;
    let mut q = request(LayoutObjective::Arrange, 1);
    q.pinned.push(OccurrenceId::new(1).unwrap());
    q.spacing_mm = 100.0; // The pinned part's requested clearance extends beyond the bed.
    let GeometryResponse::OperationAccepted { operation } = client
        .geometry(GeometryCommand::PrepareLayout {
            session_id: s.session_id,
            base_revision: s.revision,
            project_revision: before.0.revision,
            request: q,
        })
        .await
        .unwrap()
    else {
        panic!("pin admission")
    };
    let failed = wait(&mut client, operation.name).await;
    assert_eq!(failed.status, JobStatus::Failed);
    assert!(matches!(failed.error, Some(JobError::Manufacturing { .. })));
    assert_eq!(status(&mut client).await, before);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn quantity_overflow_is_atomic_scoped_and_independently_replayed_per_plate() {
    let workspace = Workspace::new();
    let source = native_box(&workspace);
    let mut client = spawn().await;
    import(&mut client, &source).await;
    let mut profile: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    profile.printer.build_envelope.max = [30.0, 24.0, 220.0];
    if let JobWorkflow::CompleteMarlin { job } = &mut profile.printer.workflow {
        job.prime_start_mm = [5.0, 23.0, 0.2];
        job.prime_end_mm = [25.0, 23.0, 0.2];
        job.park_xy_mm = [0.0, 23.0];
    }
    profile.validate().unwrap();
    set_intent(&mut client, profile).await;
    let part = instances(&mut client).await.remove(0);
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
    let before = status(&mut client).await;
    let mut q = request(LayoutObjective::Copies, 4);
    q.use_additional_plates = true;
    let proposal = prepare(&mut client, q).await;
    assert_eq!(proposal.placed_count, 1);
    assert_eq!(proposal.total_placed(), 4);
    assert_eq!(proposal.copied_count(), 3);
    assert_eq!(proposal.additional_plates.len(), 3);
    assert_eq!(proposal.limited_by, LayoutLimit::RequestedQuantity);
    let wire: spiling_contracts::rpc::LayoutProposal = proposal.clone().try_into().unwrap();
    assert_eq!(LayoutProposal::try_from(wire).unwrap(), proposal);
    for fault in 0..4 {
        let mut invalid = proposal.clone();
        match fault {
            0 => invalid.additional_plates[0].placements[0].copy = false,
            1 => invalid.additional_plates[0].verification.verified = false,
            2 => invalid.additional_plates[0].manufacturing_attempts = 5,
            _ => invalid.placed_count += 1,
        }
        assert!(invalid.validate().is_err());
    }
    assert!(
        proposal
            .additional_plates
            .iter()
            .all(|p| p.verification.verified)
    );
    assert_eq!(
        status(&mut client).await,
        before,
        "search must not mutate any plate"
    );
    // Freeze the reviewed original pose, then request overflow with the only
    // existing part pinned. Apply must work even with no current-plate placements.
    assert!(matches!(
        client
            .geometry(GeometryCommand::SetInstancePose {
                session_id: proposal.session_id.clone(),
                base_revision: proposal.scene_revision,
                project_revision: proposal.project_revision,
                occurrence_id: part.occurrence_id,
                pose: proposal.placements[0].pose,
            })
            .await
            .unwrap(),
        GeometryResponse::SceneChanged { .. }
    ));
    assert!(
        matches!(client.geometry(apply(&proposal)).await.unwrap(), GeometryResponse::Error { error } if error.code == GeometryErrorCode::StaleRevision)
    );
    let before = status(&mut client).await;
    let mut pinned = proposal.request.clone();
    pinned.pinned = vec![part.occurrence_id];
    let proposal = prepare(&mut client, pinned).await;
    assert!(proposal.placements.is_empty());
    assert_eq!(proposal.additional_plates.len(), 3);
    assert_eq!(status(&mut client).await, before);
    assert!(matches!(
        client.geometry(apply(&proposal)).await.unwrap(),
        GeometryResponse::SceneChanged { .. }
    ));
    let info = status(&mut client).await.0;
    assert_eq!(info.layouts.len(), 4);
    assert_eq!(
        info.layouts.iter().map(|p| p.occurrence_count).sum::<u32>(),
        4
    );
    assert!(info.layouts.iter().all(|p| !p.has_result));
    placement_history(&mut client, true, 1).await;
    assert_eq!(
        status(&mut client).await.0.layouts.len(),
        1,
        "one Undo must remove the entire distribution"
    );
    placement_history(&mut client, false, 1).await;
    let layouts = status(&mut client).await.0.layouts;
    let mut programs = Vec::new();
    for layout in &layouts {
        super::plates::select(&client, &layout.layout_id).await;
        assert_eq!(instances(&mut client).await.len(), 1);
        let info = status(&mut client).await.0;
        assert_eq!(info.resolved_process[0].value.fraction, 0.6);
        programs.push(compile(&mut client).await);
        assert!(inspect(&mut client).await.1.verified);
    }
    let path = workspace.0.join("four-plates.spil");
    save(&mut client, &path).await;
    client.shutdown().await.unwrap();
    std::fs::remove_file(source).unwrap();
    let mut reader = spawn().await;
    assert_eq!(
        open(&mut reader, &path, true).await.status,
        JobStatus::Completed
    );
    assert_eq!(status(&mut reader).await.0.layouts.len(), 4);
    for (layout, program) in layouts.iter().zip(programs) {
        super::plates::select(&reader, &layout.layout_id).await;
        let (actual, verification) = inspect(&mut reader).await;
        assert_eq!(actual, program);
        assert!(verification.verified);
    }
    reader.shutdown().await.unwrap();
}

async fn instances(client: &mut EngineClient) -> Vec<OccurrenceRecord> {
    let scene = scene(client).await;
    let GeometryResponse::ScenePage { occurrences, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: scene.session_id,
            revision: scene.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("occurrences")
    };
    occurrences
}

#[tokio::test]
async fn overflow_reports_plate_limit_without_mutating_or_omitting_requested_quantity() {
    let workspace = Workspace::new();
    let mut client = spawn().await;
    import(&mut client, &native_box(&workspace)).await;
    let mut profile = intent();
    profile.printer.build_envelope.min = [0.0, 0.0, 0.0];
    profile.printer.build_envelope.max = [30.0, 24.0, 100.0];
    set_intent(&mut client, profile).await;
    let original = status(&mut client).await.0.layout_id;
    for n in 2..=MAX_PRINT_LAYOUTS {
        super::plates::edit(
            &client,
            PrintLayoutEdit::Create {
                name: format!("Plate {n}"),
                copy_setup_from: Some(original.clone()),
            },
        )
        .await;
    }
    super::plates::select(&client, &original).await;
    let before = status(&mut client).await;
    let mut q = request(LayoutObjective::Copies, 4);
    q.use_additional_plates = true;
    let proposal = prepare(&mut client, q).await;
    assert_eq!(proposal.request.quantity, 4);
    assert_eq!(proposal.requested_count, 4);
    assert_eq!(proposal.total_placed(), 1);
    assert!(proposal.additional_plates.is_empty());
    assert_eq!(proposal.limited_by, LayoutLimit::PlateBudget);
    assert_eq!(status(&mut client).await, before);
    client.shutdown().await.unwrap();
}
