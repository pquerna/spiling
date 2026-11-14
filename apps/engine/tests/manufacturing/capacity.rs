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
