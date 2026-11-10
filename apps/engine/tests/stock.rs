// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Real-child saved-stock search, portable capture and independent replay.
use spiling_contracts::{
    NativeOperationView, appearance::*, geometry::*, library::*, optical::*, project::*, stock::*,
};
use spiling_engine_client::EngineClient;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
const ENGINE: &str = env!("CARGO_BIN_EXE_spiling-engine");
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("spiling-stock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn native(p: &Path) -> NativePath {
    NativePath::from_os_str(p.as_os_str()).unwrap()
}
async fn spawn(root: &Path, store: &str, library: &str) -> EngineClient {
    let mut cmd = tokio::process::Command::new(ENGINE);
    cmd.arg("--library").arg(root.join(library));
    EngineClient::spawn_command(cmd, root.join(store))
        .await
        .unwrap()
}
async fn info(c: &EngineClient) -> ProjectInfo {
    let ProjectResponse::Status { info } = c
        .rpc()
        .project(ProjectCommand::Get {
            session_id: c.hello().session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("status")
    };
    info
}
async fn scene(c: &EngineClient) -> SceneSummary {
    let GeometryResponse::Scene { summary } = c
        .rpc()
        .geometry(GeometryCommand::GetScene {
            session_id: c.hello().session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("scene")
    };
    summary
}
async fn wait(c: &EngineClient, op: NativeOperationView) -> NativeOperationView {
    tokio::time::timeout(Duration::from_secs(90), async {
        let mut op = op;
        while !op.done {
            tokio::time::sleep(Duration::from_millis(10)).await;
            op = c.rpc().native_operation(&op.name).await.unwrap();
        }
        op
    })
    .await
    .unwrap()
}
async fn done(c: &EngineClient, op: NativeOperationView) -> JobResult {
    let op = wait(c, op).await;
    assert_eq!(op.status, JobStatus::Completed, "{op:?}");
    op.result.unwrap()
}
async fn write(c: &EngineClient, mutation: LibraryMutation) {
    let status = c.rpc().library_status().await.unwrap();
    let receipt = c
        .rpc()
        .library_write(&LibraryWrite {
            library_id: status.identity.as_ref().map(|i| i.id.clone()),
            base_revision: status.identity.map_or(0, |i| i.revision),
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation,
        })
        .await
        .unwrap();
    assert_eq!(receipt.publication, LibraryPublication::Confirmed);
}
fn filament(n: u8, color: Option<[f64; 3]>) -> FilamentRevision {
    let definition = FilamentDefinition {
        name: format!("Stock {n}"),
        declaration: FilamentDeclaration {
            brand: Some("Software declarations".into()),
            material_label: Some("PLA".into()),
            diameter_mm: Some(1.75),
            hueforge_td_mm: None,
        },
        catalog_origin: None,
        material: None,
        color_name: None,
        appearance: color.map(|channels| DeclaredAppearance {
            color: AppearanceColor::Srgb { channels },
            source: DefinitionSource::Operator,
        }),
        profiles: vec![],
        source: DefinitionSource::Operator,
    };
    FilamentRevision {
        reference: FilamentRef {
            id: FilamentId(format!("00000000-0000-4000-a000-{n:012}")),
            revision: definition.content_hash().unwrap(),
        },
        definition,
    }
}
fn query(scope: StockScope) -> StockQuery {
    StockQuery {
        scope,
        on_hand_only: false,
        target: OpticalTarget {
            color: AppearanceColor::Srgb {
                channels: [0.4, 0.5, 0.8],
            },
            delta_e00_tolerance: 2.0,
            lightness_tolerance: None,
        },
        patch: OpticalPatch {
            origin_mm: [0.0; 3],
            normal_xy: [0.0, -1.0],
            width_mm: 10.0,
            height_mm: 10.0,
        },
        slot_height_mm: 0.2,
        max_evaluations: 65536,
    }
}
async fn search(c: &EngineClient, q: StockQuery) -> StockMatchReport {
    let before = info(c).await;
    let ProjectResponse::OperationAccepted { operation } = c
        .rpc()
        .project(ProjectCommand::MatchStock {
            session_id: c.hello().session_id.clone(),
            base_revision: before.revision,
            query: q.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("search admission")
    };
    let JobResult::StockMatched {
        project_id,
        captured_revision,
        record,
        resource,
    } = done(c, operation).await
    else {
        panic!("stock result")
    };
    assert_eq!(
        (project_id, captured_revision),
        (before.project_id.clone(), before.revision)
    );
    let report = c
        .rpc()
        .fetch_stock_report(&record, &resource)
        .await
        .unwrap();
    assert_eq!(report.query, q);
    assert_eq!(info(c).await, before);
    report
}
async fn compare(c: &EngineClient) -> (OpticalReportRecord, OpticalReport) {
    let p = info(c).await;
    let ProjectResponse::OperationAccepted { operation } = c
        .rpc()
        .project(ProjectCommand::CompareOptical {
            session_id: c.hello().session_id.clone(),
            base_revision: p.revision,
        })
        .await
        .unwrap()
    else {
        panic!("compare")
    };
    let JobResult::OpticalCompared {
        record, resource, ..
    } = done(c, operation).await
    else {
        panic!("report")
    };
    let report = c
        .rpc()
        .fetch_optical_report(&record, &resource)
        .await
        .unwrap();
    assert!(report.is_complete());
    (record, report)
}
async fn keep(c: &EngineClient, report: &StockMatchReport, index: usize) -> RecipeTransfer {
    let base = info(c).await.revision;
    let session = c.hello().session_id.clone();
    let mut filaments = Vec::new();
    for n in report.pairs[index].candidates {
        filaments.push(
            c.rpc()
                .stock_filament(report, usize::from(n), &session, base)
                .await
                .unwrap(),
        );
    }
    let transfer = report
        .recipe(
            index,
            RecipeId(uuid::Uuid::new_v4().to_string()),
            "Saved stock result".into(),
            filaments,
        )
        .unwrap();
    c.rpc()
        .project(ProjectCommand::PutRecipe {
            session_id: session,
            base_revision: base,
            transfer: transfer.clone(),
            expected: None,
        })
        .await
        .unwrap();
    transfer
}
async fn study(c: &EngineClient, transfer: &RecipeTransfer) {
    c.rpc()
        .project(ProjectCommand::LoadRecipeStudy {
            session_id: c.hello().session_id.clone(),
            base_revision: info(c).await.revision,
            reference: transfer.recipe.reference(),
            context: RecipeStudyContext {
                patch: transfer.recipe.definition.request.patch,
                cycle_origin_z_mm: transfer.recipe.definition.request.recipe.origin_z_mm,
            },
        })
        .await
        .unwrap();
}
async fn save(c: &EngineClient, path: &Path) {
    let ProjectResponse::OperationAccepted { operation } = c
        .rpc()
        .project(ProjectCommand::Save {
            session_id: c.hello().session_id.clone(),
            base_revision: scene(c).await.revision,
            target: Some(native(path)),
        })
        .await
        .unwrap()
    else {
        panic!("save")
    };
    done(c, operation).await;
}
async fn open(c: &EngineClient, path: &Path, read_only: bool) {
    let ProjectResponse::OperationAccepted { operation } = c
        .rpc()
        .project(ProjectCommand::Open {
            session_id: c.hello().session_id.clone(),
            base_revision: scene(c).await.revision,
            path: native(path),
            read_only,
            recover_previous: false,
            discard_changes: false,
        })
        .await
        .unwrap()
    else {
        panic!("open")
    };
    done(c, operation).await;
}
#[tokio::test]
async fn stock_search_keeps_distinct_products_and_spools_then_reassesses_without_library() {
    let workspace = Workspace::new();
    let mut c = spawn(&workspace.0, "one", "My library.spil").await;
    let a = filament(1, Some([0.1, 0.3, 0.9]));
    let b = filament(2, Some([1.0; 3]));
    let same_name = filament(3, Some([0.3; 3]));
    let unknown = filament(4, None);
    for f in [&a, &b, &same_name, &unknown] {
        write(
            &c,
            LibraryMutation::PutFilament {
                id: f.reference.id.clone(),
                expected: None,
                definition: f.definition.clone(),
            },
        )
        .await;
    }
    for (n, f) in [
        a.reference.clone(),
        a.reference.clone(),
        b.reference.clone(),
    ]
    .into_iter()
    .enumerate()
    {
        write(
            &c,
            LibraryMutation::PutSpool {
                spool: SpoolRecord {
                    id: SpoolId(uuid::Uuid::new_v4().to_string()),
                    filament: f,
                    name: format!("Physical spool {n}"),
                    state: StockState::OnHand,
                    quantity: None,
                    lot: None,
                    location: None,
                },
            },
        )
        .await;
    }
    c.rpc()
        .project(ProjectCommand::PutFilament {
            session_id: c.hello().session_id.clone(),
            base_revision: info(&c).await.revision,
            revision: a.clone(),
            origin: None,
            expected: None,
        })
        .await
        .unwrap();
    let report = search(&c, query(StockScope::Both)).await;
    assert_eq!(report.candidates.len(), 4);
    assert_eq!(report.total_pairs, 6);
    assert_eq!(report.status, StockSearchStatus::Complete);
    assert_eq!(report.unassessed_singles, 1);
    assert_eq!(report.unassessed_pairs, 3);
    let first = &report.candidates[0];
    assert!(first.in_project && first.in_library);
    assert_eq!(first.on_hand_spools, 2);
    let mut owned = query(StockScope::Both);
    owned.on_hand_only = true;
    let report = search(&c, owned).await;
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(report.total_pairs, 1);
    let current = info(&c).await;
    let session = c.hello().session_id.clone();
    assert!(
        c.rpc()
            .project(ProjectCommand::MatchStock {
                session_id: session.clone(),
                base_revision: ProjectRevision(current.revision.0 - 1),
                query: query(StockScope::Both)
            })
            .await
            .is_err()
    );
    assert_eq!(info(&c).await, current);
    let mut bounded = query(StockScope::Both);
    bounded.max_evaluations = 32;
    assert_eq!(
        search(&c, bounded).await.status,
        StockSearchStatus::WorkLimit
    );
    let transfer = keep(&c, &report, 0).await;
    assert_eq!(transfer.filaments.len(), 2);
    study(&c, &transfer).await;
    let (_, fresh) = compare(&c).await;
    assert_eq!(fresh.requested, report.pairs[0].assessment);
    let path = workspace.0.join("Result.spil");
    save(&c, &path).await;
    c.shutdown().await.unwrap();
    std::fs::rename(
        workspace.0.join("My library.spil"),
        workspace.0.join("Hidden.spil"),
    )
    .unwrap();
    let mut reopened = spawn(&workspace.0, "two", "Missing.spil").await;
    open(&reopened, &path, true).await;
    let current = info(&reopened).await;
    let ProjectResponse::Recipe { transfer: captured } = reopened
        .rpc()
        .project(ProjectCommand::GetRecipe {
            session_id: reopened.hello().session_id.clone(),
            base_revision: current.revision,
            reference: transfer.recipe.reference(),
        })
        .await
        .unwrap()
    else {
        panic!("portable recipe")
    };
    assert_eq!(captured, transfer);
    let standalone = search(&reopened, query(StockScope::Project)).await;
    assert_eq!(standalone.candidates.len(), 2);
    assert!(standalone.library.is_none());
    let (_, again) = compare(&reopened).await;
    assert_eq!(again.requested, fresh.requested);
    assert_eq!(
        again.material_evidence_hashes,
        fresh.material_evidence_hashes
    );
    assert!(!workspace.0.join("Missing.spil").exists());
    reopened.shutdown().await.unwrap();
}

async fn compile_export(c: &EngineClient, path: &Path) -> String {
    use spiling_contracts::manufacturing::*;
    let session = c.hello().session_id.clone();
    let ManufacturingResponse::OperationAccepted { operation } = c
        .rpc()
        .manufacturing(ManufacturingCommand::Compile {
            layout_id: info(c).await.layout_id,
            session_id: session.clone(),
            base_revision: info(c).await.revision,
        })
        .await
        .unwrap()
    else {
        panic!("compile")
    };
    let JobResult::ManufacturingCompiled { .. } = done(c, operation).await else {
        panic!("compiled")
    };
    let ManufacturingResponse::OperationAccepted { operation } = c
        .rpc()
        .manufacturing(ManufacturingCommand::Inspect {
            layout_id: info(c).await.layout_id,
            session_id: session,
            base_revision: info(c).await.revision,
            output: VerificationOutput::ReportAndMotion,
        })
        .await
        .unwrap()
    else {
        panic!("inspect")
    };
    let JobResult::ManufacturingVerified {
        record,
        report,
        resource,
        provenance,
        replay_display,
    } = done(c, operation).await
    else {
        panic!("fresh actual replay")
    };
    assert!(report.verified);
    assert_eq!(provenance.surface_color.len(), 1);
    let display = c
        .rpc()
        .fetch_replay_display(&record, &replay_display.unwrap())
        .await
        .unwrap();
    assert!(!display.is_empty());
    let bundle = c
        .rpc()
        .fetch_manufacturing_bundle(&record, &resource)
        .await
        .unwrap();
    let program = c.rpc().fetch_manufacturing_program(&record).await.unwrap();
    let mut output = spiling_engine_client::export::OutputDirectory::create(path).unwrap();
    output
        .write_manufacturing_bundle(&bundle, &program, &report, &provenance)
        .unwrap();
    output.commit();
    assert_eq!(std::fs::read_dir(path).unwrap().count(), 4);
    let exported = std::fs::read_to_string(path.join("program.gcode")).unwrap();
    assert_eq!(exported.as_bytes(), program.to_vec().unwrap());
    assert!(spiling_engine_client::export::OutputDirectory::create(path).is_err());
    exported
}
#[tokio::test]
async fn matched_stock_recipe_reaches_actual_replay_export_and_source_free_recompile() {
    use spiling_contracts::manufacturing::*;
    let workspace = Workspace::new();
    let mut c = spawn(&workspace.0, "surface-one", "My library.spil").await;
    let source = workspace.0.join("Source.step");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry/box-mm.step"),
        &source,
    )
    .unwrap();
    let session = c.hello().session_id.clone();
    let GeometryResponse::OperationAccepted { operation } = c
        .rpc()
        .geometry(GeometryCommand::ImportPart {
            session_id: session.clone(),
            base_revision: scene(&c).await.revision,
            source: native(&source),
            initial_pose: RigidPoseMm {
                translation_mm: [20.0, 20.0, 0.0],
                ..RigidPoseMm::IDENTITY
            },
        })
        .await
        .unwrap()
    else {
        panic!("import")
    };
    done(&c, operation).await;
    let current = scene(&c).await;
    let GeometryResponse::ScenePage { definitions, .. } = c
        .rpc()
        .geometry(GeometryCommand::GetScenePage {
            session_id: session.clone(),
            revision: current.revision,
            kind: ScenePageKind::Definitions,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("definitions")
    };
    let GeometryResponse::ScenePage { occurrences, .. } = c
        .rpc()
        .geometry(GeometryCommand::GetScenePage {
            session_id: session.clone(),
            revision: current.revision,
            kind: ScenePageKind::Occurrences,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("occurrences")
    };
    let mut intent: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../fixtures/manufacturing/surface-color-box.intent.json"
    ))
    .unwrap();
    let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
        panic!("fixed dual")
    };
    job.assignments[0].occurrence_id = occurrences[0].occurrence_id;
    let construction = &mut job.surface_constructions[0];
    construction.occurrence_id = occurrences[0].occurrence_id;
    construction.definition_id = definitions[0].definition_id.clone();
    construction.source_hash = definitions[0].provenance.source_hash.clone();
    let tools = job.tools.clone();
    for (i, color) in [[0.1, 0.3, 0.9], [1.0; 3]].into_iter().enumerate() {
        let f = filament(i as u8 + 1, Some(color));
        write(
            &c,
            LibraryMutation::PutFilament {
                id: f.reference.id,
                expected: None,
                definition: f.definition,
            },
        )
        .await;
    }
    c.rpc()
        .manufacturing(ManufacturingCommand::SetIntent {
            layout_id: info(&c).await.layout_id,
            session_id: session.clone(),
            base_revision: info(&c).await.revision,
            intent: std::sync::Arc::new(intent),
        })
        .await
        .unwrap();
    let report = search(&c, query(StockScope::MyLibrary)).await;
    assert_eq!(report.pairs.len(), 1);
    let transfer = keep(&c, &report, 0).await;
    study(&c, &transfer).await;
    let (record, fresh) = compare(&c).await;
    assert_eq!(fresh.requested, report.pairs[0].assessment);
    let base = info(&c).await.revision;
    c.rpc()
        .project(ProjectCommand::AcceptOpticalReport {
            session_id: session.clone(),
            base_revision: base,
            report_hash: record.hash.clone(),
            selection: Some(OpticalSelection::Requested),
        })
        .await
        .unwrap();
    let application = ApplyOpticalConstruction {
        construction_index: 0,
        report_hash: record.hash,
        selection: OpticalSelection::Requested,
        bindings: std::array::from_fn(|i| OpticalMaterialBinding {
            evidence_key: fresh.request.materials[i].clone(),
            evidence_hash: fresh.material_evidence_hashes[i].clone(),
            loaded_material_id: tools[i].material.id.clone(),
            loaded_declaration_hash: tools[i].material.declaration_hash.clone(),
        }),
    };
    let ProjectResponse::OperationAccepted { operation } = c
        .rpc()
        .project(ProjectCommand::ApplyOpticalConstruction {
            session_id: session,
            base_revision: info(&c).await.revision,
            application,
        })
        .await
        .unwrap()
    else {
        panic!("apply retained stock recipe")
    };
    done(&c, operation).await;
    let program = compile_export(&c, &workspace.0.join("First export")).await;
    let path = workspace.0.join("Manufacturing.spil");
    save(&c, &path).await;
    c.shutdown().await.unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::rename(
        workspace.0.join("My library.spil"),
        workspace.0.join("Hidden.spil"),
    )
    .unwrap();
    let mut reopened = spawn(&workspace.0, "surface-two", "Missing.spil").await;
    open(&reopened, &path, false).await;
    assert_eq!(
        compile_export(&reopened, &workspace.0.join("Reopened export")).await,
        program
    );
    assert!(!workspace.0.join("Missing.spil").exists());
    reopened.shutdown().await.unwrap();
}
#[tokio::test]
async fn queued_stock_search_cancels_and_rejects_stale_project_without_mutation() {
    use spiling_contracts::rpc;
    let workspace = Workspace::new();
    let mut c = spawn(&workspace.0, "cancel-one", "My library.spil").await;
    let f = filament(1, Some([0.5; 3]));
    write(
        &c,
        LibraryMutation::PutFilament {
            id: f.reference.id,
            expected: None,
            definition: f.definition,
        },
    )
    .await;
    for stale in [false, true] {
        let before = info(&c).await;
        let blocker = c
            .rpc()
            .run_diagnostic(rpc::RunDiagnosticRequest {
                parent: "diagnostics/stock-lease".into(),
                request_id: String::new(),
                chunk_count: 4,
                delay_ms: 500,
                chunk_bytes: 64,
                input_revision: "stock-cancel-fence".into(),
            })
            .await
            .unwrap();
        let ProjectResponse::OperationAccepted { operation } = c
            .rpc()
            .project(ProjectCommand::MatchStock {
                session_id: c.hello().session_id.clone(),
                base_revision: before.revision,
                query: query(StockScope::MyLibrary),
            })
            .await
            .unwrap()
        else {
            panic!("queued search")
        };
        if stale {
            let f = filament(2, None);
            c.rpc()
                .project(ProjectCommand::PutFilament {
                    session_id: c.hello().session_id.clone(),
                    base_revision: before.revision,
                    revision: f,
                    origin: None,
                    expected: None,
                })
                .await
                .unwrap();
        } else {
            c.rpc().cancel_operation(&operation.name).await.unwrap();
        }
        c.rpc().cancel_operation(&blocker.name).await.unwrap();
        let terminal = wait(&c, operation).await;
        if stale {
            assert_eq!(terminal.status, JobStatus::Failed);
            assert!(terminal.outputs.is_empty());
        } else {
            assert!(matches!(
                terminal.status,
                JobStatus::Cancelled | JobStatus::Completed
            ));
            if terminal.status == JobStatus::Completed {
                let Some(JobResult::StockMatched {
                    record, resource, ..
                }) = terminal.result
                else {
                    panic!("cancelled prefix")
                };
                assert_eq!(
                    c.rpc()
                        .fetch_stock_report(&record, &resource)
                        .await
                        .unwrap()
                        .status,
                    StockSearchStatus::Cancelled
                );
            }
        }
        assert_eq!(
            info(&c).await.revision.0,
            before.revision.0 + u32::from(stale)
        );
    }
    c.shutdown().await.unwrap();
}
