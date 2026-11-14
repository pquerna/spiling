// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Project size is admitted by bytes and work, never a definition, occurrence
//! or placement-candidate count. These exceed every former fixed count.
use super::*;

/// The controlled box fixture rescaled to `x` by `y` by `z` mm. Distinct sizes
/// give distinct source bytes and therefore distinct definitions.
fn small_box(workspace: &Workspace, name: &str, size: [f64; 3]) -> PathBuf {
    let original = include_str!("../../../../fixtures/geometry/box-mm.step");
    let mut text = String::with_capacity(original.len());
    for line in original.lines() {
        if line.contains("CARTESIAN_POINT") {
            let line = line
                .replace("20.0", &format!("{:?}", size[0]))
                .replace("10.0", &format!("{:?}", size[1]))
                .replace("8.0", &format!("{:?}", size[2]));
            text.push_str(&line);
        } else {
            text.push_str(&line.replace("box-mm.step", name));
        }
        text.push('\n');
    }
    let path = workspace.0.join(name);
    std::fs::write(&path, text).unwrap();
    path
}
fn layout_request(objective: LayoutObjective, quantity: u32) -> LayoutRequest {
    LayoutRequest {
        objective,
        template: (objective != LayoutObjective::Arrange).then(|| OccurrenceId::new(1).unwrap()),
        quantity,
        use_additional_plates: false,
        pinned: vec![],
        spacing_mm: 1.0,
        style: LayoutStyle::Dense,
    }
}
async fn prepare_layout(client: &mut EngineClient, request: LayoutRequest) -> LayoutProposal {
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
async fn apply_layout(client: &mut EngineClient, proposal: &LayoutProposal) {
    assert!(matches!(
        client
            .geometry(GeometryCommand::ApplyLayout {
                session_id: proposal.session_id.clone(),
                base_revision: proposal.scene_revision,
                project_revision: proposal.project_revision,
                proposal_id: proposal.id.clone(),
            })
            .await
            .unwrap(),
        GeometryResponse::SceneChanged { .. }
    ));
}
fn small_bed_intent(side_mm: f64) -> ManufacturingIntent {
    let mut profile = intent();
    profile.printer.build_envelope =
        AabbMm::new([0.0, 0.0, 0.0], [side_mm, side_mm, 100.0]).unwrap();
    for component in &mut profile.printer.components {
        component.shape = MachineComponentShape::Box {
            bounds_mm: AabbMm::new([0.0, 0.0, -1.0], [side_mm, side_mm, 0.0]).unwrap(),
        };
    }
    profile.validate().unwrap();
    profile
}

#[tokio::test]
async fn definitions_and_occurrences_beyond_former_caps_page_lay_out_and_compile() {
    const DEFINITIONS: u32 = 70; // more than one overview/scene definition page
    const OCCURRENCES: u32 = 300;
    let workspace = Workspace::new();
    let mut client = spawn().await;
    for index in 0..DEFINITIONS {
        let size = [2.0 + 0.02 * f64::from(index), 1.5, 0.4];
        import(
            &mut client,
            &small_box(&workspace, &format!("box-{index}.step"), size),
        )
        .await;
    }
    set_intent(&mut client, intent()).await;
    let mut summary = scene(&mut client).await;
    assert_eq!(summary.definition_count, DEFINITIONS);
    let GeometryResponse::ScenePage { definitions, .. } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: summary.session_id.clone(),
            revision: summary.revision,
            kind: ScenePageKind::Definitions,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("definitions")
    };
    let repeated = definitions[0].definition_id.clone();
    while summary.occurrence_count < OCCURRENCES {
        let GeometryResponse::SceneChanged { summary: changed } = client
            .geometry(GeometryCommand::AddInstance {
                session_id: summary.session_id.clone(),
                base_revision: summary.revision,
                definition_id: repeated.clone(),
                pose: RigidPoseMm::IDENTITY,
            })
            .await
            .unwrap()
        else {
            panic!("instance beyond the former occurrence cap")
        };
        summary = changed;
    }
    let proposal = prepare_layout(&mut client, layout_request(LayoutObjective::Arrange, 1)).await;
    assert_eq!(proposal.placed_count, OCCURRENCES);
    assert_eq!(proposal.limited_by, LayoutLimit::RequestedQuantity);
    assert!(proposal.verification.verified);
    apply_layout(&mut client, &proposal).await;

    let info = status(&mut client).await.0;
    let summary = scene(&mut client).await;
    let overview = client
        .layout_overview(&summary.session_id, summary.revision, info.revision)
        .await
        .unwrap();
    assert_eq!(overview.definitions.len(), DEFINITIONS as usize);
    assert_eq!(overview.layouts.len(), 1);
    assert_eq!(overview.layouts[0].parts.len(), OCCURRENCES as usize);

    compile(&mut client).await;
    let (_, report) = inspect(&mut client).await;
    assert!(report.verified);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn fill_plate_places_every_small_part_that_fits_and_reports_the_rest() {
    let workspace = Workspace::new();
    let mut client = spawn().await;
    import(
        &mut client,
        &small_box(&workspace, "tiny.step", [2.0, 2.0, 0.4]),
    )
    .await;
    set_intent(&mut client, small_bed_intent(80.0)).await;
    let requested = 100_000;
    let proposal = prepare_layout(
        &mut client,
        layout_request(LayoutObjective::Fill, requested),
    )
    .await;
    // Every candidate position was decided: the copies that were not placed
    // are reported as not fitting the build area, never as a search limit.
    assert_eq!(proposal.requested_count, requested);
    assert_eq!(proposal.limited_by, LayoutLimit::BuildArea);
    assert!(proposal.placed_count > 256, "{}", proposal.placed_count);
    assert!(proposal.verification.verified);
    for (index, a) in proposal.footprints_mm.iter().enumerate() {
        assert!((0..2).all(|axis| a.min[axis] >= 0.0 && a.max[axis] <= 80.0));
        for b in &proposal.footprints_mm[index + 1..] {
            assert!(!(0..2).all(|axis| a.min[axis] < b.max[axis] && b.min[axis] < a.max[axis]));
        }
    }
    apply_layout(&mut client, &proposal).await;
    assert_eq!(
        scene(&mut client).await.occurrence_count,
        proposal.placed_count
    );
    client.shutdown().await.unwrap();
}

/// Grid origins whose +25 mm hotend offset never lands on another part: four
/// 20 mm X windows 50 mm apart, rows below the waste lines at Y 160.
fn fixed_dual_origins(count: usize) -> Vec<[f64; 2]> {
    let mut origins = Vec::new();
    for row in 0..43 {
        for window in 0..4 {
            for column in 0..6 {
                origins.push([
                    20.0 + 50.0 * f64::from(window) + 3.0 * f64::from(column),
                    20.0 + 3.0 * f64::from(row),
                ]);
            }
        }
    }
    origins.truncate(count);
    origins
}

#[tokio::test]
async fn fixed_dual_plate_beyond_former_caps_stages_its_intent_compiles_and_verifies() {
    const PARTS: usize = 900; // far beyond the former 32 fixed-dual occurrences
    let workspace = Workspace::new();
    let mut client = spawn().await;
    import(
        &mut client,
        &small_box(&workspace, "dual.step", [1.5, 1.5, 0.4]),
    )
    .await;
    let origins = fixed_dual_origins(PARTS);
    assert_eq!(origins.len(), PARTS);
    let mut summary = scene(&mut client).await;
    let GeometryResponse::ScenePage {
        definitions,
        occurrences,
        ..
    } = client
        .geometry(GeometryCommand::GetScenePage {
            session_id: summary.session_id.clone(),
            revision: summary.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("occurrences")
    };
    let _ = definitions;
    let first = occurrences[0].clone();
    // Move the imported part to the first origin, then add the rest.
    let project_revision = status(&mut client).await.0.revision;
    let GeometryResponse::SceneChanged { summary: moved } = client
        .geometry(GeometryCommand::SetInstancePose {
            session_id: summary.session_id.clone(),
            base_revision: summary.revision,
            project_revision,
            occurrence_id: first.occurrence_id,
            pose: RigidPoseMm {
                translation_mm: [origins[0][0], origins[0][1], 0.0],
                ..RigidPoseMm::IDENTITY
            },
        })
        .await
        .unwrap()
    else {
        panic!("move")
    };
    summary = moved;
    for origin in &origins[1..] {
        let GeometryResponse::SceneChanged { summary: changed } = client
            .geometry(GeometryCommand::AddInstance {
                session_id: summary.session_id.clone(),
                base_revision: summary.revision,
                definition_id: first.definition_id.clone(),
                pose: RigidPoseMm {
                    translation_mm: [origin[0], origin[1], 0.0],
                    ..RigidPoseMm::IDENTITY
                },
            })
            .await
            .unwrap()
        else {
            panic!("instance")
        };
        summary = changed;
    }
    assert_eq!(summary.occurrence_count as usize, PARTS);
    let mut placed = Vec::new();
    let mut offset = 0;
    loop {
        let GeometryResponse::ScenePage {
            occurrences,
            next_offset,
            ..
        } = client
            .geometry(GeometryCommand::GetScenePage {
                session_id: summary.session_id.clone(),
                revision: summary.revision,
                kind: ScenePageKind::Occurrences,
                offset,
            })
            .await
            .unwrap()
        else {
            panic!("occurrence page")
        };
        placed.extend(occurrences);
        match next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    let mut intent: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../../fixtures/manufacturing/fixed-dual.intent.json"
    ))
    .unwrap();
    let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
        unreachable!()
    };
    let template = job.assignments[0].clone();
    job.assignments = placed
        .iter()
        .map(|occurrence| {
            let [x, y, _] = occurrence.pose.translation_mm;
            OccurrenceMaterialAssignment {
                occurrence_id: occurrence.occurrence_id,
                work_envelope_mm: AabbMm::new([x - 0.7, y - 0.7, -1.0], [x + 2.2, y + 2.2, 9.0])
                    .unwrap(),
                ..template.clone()
            }
        })
        .collect();
    intent.validate().unwrap();
    let wire = prost::Message::encoded_len(
        &spiling_contracts::rpc::ManufacturingIntent::try_from(intent.clone()).unwrap(),
    );
    assert!(
        wire > spiling_contracts::MAX_CONTROL_BYTES as usize,
        "{wire}"
    );
    // Larger than one control frame: staged through ByteStream both ways.
    set_intent(&mut client, intent.clone()).await;
    let ManufacturingResponse::Status {
        intent: Some(stored),
        ..
    } = client
        .manufacturing(ManufacturingCommand::Get {
            layout_id: current_layout(&client).await,
            session_id: client.hello().session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("status")
    };
    assert_eq!(*stored, intent);
    let record = compile(&mut client).await;
    // Every part's perimeter and fill is planned, not a truncated plate.
    assert!(
        record.summary.paths as usize >= 2 * PARTS,
        "{:?}",
        record.summary
    );
    let (_, report) = inspect(&mut client).await;
    assert!(report.verified);
    client.shutdown().await.unwrap();
}
