// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use spiling_contracts::{NativeOperationView, geometry::*, library::*, project::*};
use spiling_engine_client::{ClientError, EngineClient};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;
const ENGINE: &str = env!("CARGO_BIN_EXE_spiling-engine");
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("spiling-library-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
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
async fn spawn(root: &Path, store: &str, library: &str) -> EngineClient {
    let mut command = Command::new(ENGINE);
    command.arg("--library").arg(root.join(library));
    EngineClient::spawn_command(command, root.join(store))
        .await
        .unwrap()
}
fn definition(name: String) -> FilamentDefinition {
    FilamentDefinition {
        declaration: Default::default(),
        catalog_origin: None,
        name,
        material: None,
        color_name: None,
        appearance: None,
        profiles: vec![],
        source: DefinitionSource::Operator,
    }
}
async fn write(client: &EngineClient, mutation: LibraryMutation) -> LibraryWrite {
    let status = client.rpc().library_status().await.unwrap();
    let request = LibraryWrite {
        library_id: status.identity.as_ref().map(|i| i.id.clone()),
        base_revision: status.identity.map_or(0, |i| i.revision),
        request_id: uuid::Uuid::new_v4().to_string(),
        mutation,
    };
    let result = client.rpc().library_write(&request).await.unwrap();
    assert_eq!(result.publication, LibraryPublication::Confirmed);
    request
}
async fn info(client: &EngineClient) -> ProjectInfo {
    match client
        .rpc()
        .project(ProjectCommand::Get {
            session_id: client.hello().session_id.clone(),
        })
        .await
        .unwrap()
    {
        ProjectResponse::Status { info } => info,
        _ => panic!("project info"),
    }
}
async fn scene_revision(client: &EngineClient) -> SceneRevision {
    match client
        .rpc()
        .geometry(GeometryCommand::GetScene {
            session_id: client.hello().session_id.clone(),
        })
        .await
        .unwrap()
    {
        GeometryResponse::Scene { summary } => summary.revision,
        _ => panic!("scene info"),
    }
}
async fn wait(client: &EngineClient, operation: NativeOperationView) -> JobResult {
    tokio::time::timeout(Duration::from_secs(60), async {
        let mut operation = operation;
        loop {
            if operation.status.is_terminal() {
                assert_eq!(operation.status, JobStatus::Completed, "{operation:?}");
                return operation.result.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            operation = client
                .rpc()
                .native_operation(&operation.name)
                .await
                .unwrap();
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn personal_inventory_is_shared_but_project_copies_reopen_without_it() {
    let workspace = Workspace::new();
    let mut first = spawn(&workspace.0, "one", "My library.spil").await;
    let mut second = spawn(&workspace.0, "two", "My library.spil").await;
    assert!(
        first
            .rpc()
            .library_status()
            .await
            .unwrap()
            .identity
            .is_none()
    );
    assert!(!workspace.0.join("My library.spil").exists());
    for i in 0..21 {
        write(
            &first,
            LibraryMutation::PutFilament {
                id: FilamentId(uuid::Uuid::new_v4().to_string()),
                expected: None,
                definition: definition(format!("Filament {i:02}")),
            },
        )
        .await;
    }
    let page = second
        .rpc()
        .library_filaments(&LibraryFilamentQuery::default())
        .await
        .unwrap();
    assert_eq!(page.total_matches, 21);
    assert_eq!(page.entries.len(), 20);
    assert_eq!(page.next_offset, Some(20));
    let tail = second
        .rpc()
        .library_filaments(&LibraryFilamentQuery {
            expected: page.identity.clone(),
            offset: 20,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tail.entries.len(), 1);
    let reference = page.entries[0].entry.current.clone();
    let library = page.identity.unwrap();
    let revision = first
        .rpc()
        .library_filament(&library.id, &reference)
        .await
        .unwrap();
    for i in 0..2 {
        write(
            &first,
            LibraryMutation::PutSpool {
                spool: SpoolRecord {
                    id: SpoolId(uuid::Uuid::new_v4().to_string()),
                    filament: reference.clone(),
                    name: format!("Spool {i}"),
                    state: StockState::OnHand,
                    quantity: None,
                    lot: None,
                    location: Some("Dry box".into()),
                },
            },
        )
        .await;
    }
    let stale = LibraryWrite {
        library_id: Some(library.id.clone()),
        base_revision: library.revision,
        request_id: uuid::Uuid::new_v4().to_string(),
        mutation: LibraryMutation::PutFilament {
            id: reference.id.clone(),
            expected: Some(reference.revision.clone()),
            definition: definition("Lost concurrent update".into()),
        },
    };
    assert!(matches!(
        second.rpc().library_write(&stale).await,
        Err(ClientError::Library(LibraryError {
            code: LibraryErrorCode::StaleRevision,
            ..
        }))
    ));
    let request = write(
        &first,
        LibraryMutation::PutFilament {
            id: reference.id.clone(),
            expected: Some(reference.revision.clone()),
            definition: definition("Updated name".into()),
        },
    )
    .await;
    assert!(second.rpc().library_write(&request).await.unwrap().replayed);
    let on_hand = second
        .rpc()
        .library_filaments(&LibraryFilamentQuery {
            on_hand_only: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(on_hand.entries.len(), 1);
    assert_eq!(on_hand.entries[0].on_hand_spools, 2);
    let original = first
        .rpc()
        .library_filament(&library.id, &reference)
        .await
        .unwrap();
    assert_eq!(
        original, revision,
        "stock registration retains the old exact revision"
    );
    let before = info(&first).await;
    assert!(matches!(
        first
            .rpc()
            .project(ProjectCommand::PutFilament {
                session_id: first.hello().session_id.clone(),
                base_revision: before.revision,
                revision: revision.clone(),
                origin: Some(LibraryOrigin {
                    library_id: library.id,
                    filament: reference.clone()
                }),
                expected: None
            })
            .await
            .unwrap(),
        ProjectResponse::Status { .. }
    ));
    let ProjectResponse::OperationAccepted { operation } = first
        .rpc()
        .project(ProjectCommand::Save {
            session_id: first.hello().session_id.clone(),
            base_revision: scene_revision(&first).await,
            target: Some(native(&workspace.0.join("Job.spil"))),
        })
        .await
        .unwrap()
    else {
        panic!("save")
    };
    assert!(matches!(
        wait(&first, operation).await,
        JobResult::ProjectSaved { .. }
    ));
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
    std::fs::rename(
        workspace.0.join("My library.spil"),
        workspace.0.join("Unavailable.spil"),
    )
    .unwrap();
    let mut reopened = spawn(&workspace.0, "three", "Missing.spil").await;
    let ProjectResponse::OperationAccepted { operation } = reopened
        .rpc()
        .project(ProjectCommand::Open {
            session_id: reopened.hello().session_id.clone(),
            base_revision: scene_revision(&reopened).await,
            path: native(&workspace.0.join("Job.spil")),
            read_only: true,
            recover_previous: false,
            discard_changes: false,
        })
        .await
        .unwrap()
    else {
        panic!("open")
    };
    wait(&reopened, operation).await;
    let current = info(&reopened).await;
    assert!(current.read_only);
    assert_eq!(current.materials.unwrap().filament_count, 1);
    let response = reopened
        .rpc()
        .project(ProjectCommand::GetFilament {
            session_id: reopened.hello().session_id.clone(),
            base_revision: current.revision,
            reference,
        })
        .await
        .unwrap();
    assert_eq!(response, ProjectResponse::Filament { revision });
    assert!(!workspace.0.join("Missing.spil").exists());
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn complete_recipes_copy_atomically_reassess_and_reopen_without_personal_storage() {
    use spiling_contracts::{appearance::AppearanceColor, optical::OpticalSelection};
    let workspace = Workspace::new();
    let mut client = spawn(&workspace.0, "recipe-one", "My library.spil").await;
    assert!(
        client
            .rpc()
            .library_recipes(&LibraryRecipeQuery::default())
            .await
            .unwrap()
            .entries
            .is_empty()
    );
    assert!(!workspace.0.join("My library.spil").exists());
    for count in [2usize, 3] {
        let filaments = (0..count)
            .map(|i| {
                let mut definition = definition(format!("Recipe {count} ingredient {i}"));
                definition.appearance = Some(DeclaredAppearance {
                    color: AppearanceColor::Srgb {
                        channels: [0.1 + i as f64 * 0.3, 0.3, 0.5],
                    },
                    source: DefinitionSource::Operator,
                });
                FilamentRevision {
                    reference: FilamentRef {
                        id: FilamentId(uuid::Uuid::new_v4().to_string()),
                        revision: definition.content_hash().unwrap(),
                    },
                    definition,
                }
            })
            .collect();
        let transfer = RecipeTransfer::from_declarations(
            RecipeId(uuid::Uuid::new_v4().to_string()),
            format!("{count}-ingredient recipe"),
            filaments,
            AppearanceColor::Srgb { channels: [0.5; 3] },
            vec![0.2; count],
        )
        .unwrap();
        let request = write(
            &client,
            LibraryMutation::PutRecipe {
                transfer: transfer.clone(),
                expected: None,
            },
        )
        .await;
        assert!(client.rpc().library_write(&request).await.unwrap().replayed);
        let identity = client
            .rpc()
            .library_status()
            .await
            .unwrap()
            .identity
            .unwrap();
        assert_eq!(
            client
                .rpc()
                .library_recipe(&identity.id, &transfer.recipe.reference())
                .await
                .unwrap(),
            transfer
        );
        assert_eq!(
            client
                .rpc()
                .library_spools(&LibrarySpoolQuery::default())
                .await
                .unwrap()
                .total_matches,
            0
        );
        let before = info(&client).await;
        let session_id = client.hello().session_id.clone();
        assert!(matches!(
            client
                .rpc()
                .project(ProjectCommand::PutRecipe {
                    session_id: session_id.clone(),
                    base_revision: before.revision,
                    transfer: transfer.clone(),
                    expected: None
                })
                .await
                .unwrap(),
            ProjectResponse::Status { .. }
        ));
        let copied = info(&client).await;
        assert_eq!(copied.revision.0, before.revision.0 + 1);
        assert!(
            copied.appearance_study.optical.is_none()
                || copied
                    .appearance_study
                    .optical
                    .as_ref()
                    .unwrap()
                    .report
                    .is_some()
        );
        assert!(matches!(
            client
                .rpc()
                .project(ProjectCommand::PutRecipe {
                    session_id: session_id.clone(),
                    base_revision: before.revision,
                    transfer: transfer.clone(),
                    expected: None
                })
                .await,
            Err(ClientError::Project(ProjectError {
                code: ProjectErrorCode::StaleRevision,
                ..
            }))
        ));
        assert_eq!(info(&client).await.revision, copied.revision);
        let context = RecipeStudyContext {
            patch: transfer.recipe.definition.request.patch,
            cycle_origin_z_mm: 0.03,
        };
        assert!(matches!(
            client
                .rpc()
                .project(ProjectCommand::LoadRecipeStudy {
                    session_id: session_id.clone(),
                    base_revision: copied.revision,
                    reference: transfer.recipe.reference(),
                    context: context.clone()
                })
                .await
                .unwrap(),
            ProjectResponse::Status { .. }
        ));
        let study_info = info(&client).await;
        let study = study_info.appearance_study.optical.unwrap();
        assert!(study.report.is_none() && study.selection.is_none());
        assert_eq!(study.request.recipe.origin_z_mm, 0.03);
        let ProjectResponse::OperationAccepted { operation } = client
            .rpc()
            .project(ProjectCommand::CompareOptical {
                session_id: session_id.clone(),
                base_revision: study_info.revision,
            })
            .await
            .unwrap()
        else {
            panic!("compare")
        };
        let JobResult::OpticalCompared {
            record, resource, ..
        } = wait(&client, operation).await
        else {
            panic!("compared result")
        };
        let report = client
            .fetch_optical_report(&record, &resource)
            .await
            .unwrap();
        assert!(report.is_complete());
        assert!(report.has_selection(&OpticalSelection::Requested));
        assert!(matches!(
            client
                .rpc()
                .project(ProjectCommand::AcceptOpticalReport {
                    session_id: session_id.clone(),
                    base_revision: study_info.revision,
                    report_hash: record.hash,
                    selection: Some(OpticalSelection::Requested)
                })
                .await
                .unwrap(),
            ProjectResponse::Status { .. }
        ));
        let current = info(&client).await;
        assert!(
            current
                .appearance_study
                .optical
                .unwrap()
                .selection
                .is_some()
        );
        assert!(matches!(
            client
                .rpc()
                .project(ProjectCommand::LoadRecipeStudy {
                    session_id,
                    base_revision: current.revision,
                    reference: transfer.recipe.reference(),
                    context
                })
                .await
                .unwrap(),
            ProjectResponse::Status { .. }
        ));
        assert!(
            info(&client)
                .await
                .appearance_study
                .optical
                .unwrap()
                .report
                .is_none()
        );
        if count == 2 {
            let current = info(&client).await;
            let ProjectResponse::OperationAccepted { operation } = client
                .rpc()
                .project(ProjectCommand::CompareOptical {
                    session_id: client.hello().session_id.clone(),
                    base_revision: current.revision,
                })
                .await
                .unwrap()
            else {
                panic!("compare")
            };
            let JobResult::OpticalCompared { record, .. } = wait(&client, operation).await else {
                panic!("result")
            };
            client
                .rpc()
                .project(ProjectCommand::AcceptOpticalReport {
                    session_id: client.hello().session_id.clone(),
                    base_revision: current.revision,
                    report_hash: record.hash,
                    selection: Some(OpticalSelection::Requested),
                })
                .await
                .unwrap();
        }
    }
    let ProjectResponse::OperationAccepted { operation } = client
        .rpc()
        .project(ProjectCommand::Save {
            session_id: client.hello().session_id.clone(),
            base_revision: scene_revision(&client).await,
            target: Some(native(&workspace.0.join("Recipes.spil"))),
        })
        .await
        .unwrap()
    else {
        panic!("save")
    };
    wait(&client, operation).await;
    client.shutdown().await.unwrap();
    std::fs::rename(
        workspace.0.join("My library.spil"),
        workspace.0.join("Unavailable.spil"),
    )
    .unwrap();
    let mut reopened = spawn(&workspace.0, "recipe-two", "Missing.spil").await;
    let ProjectResponse::OperationAccepted { operation } = reopened
        .rpc()
        .project(ProjectCommand::Open {
            session_id: reopened.hello().session_id.clone(),
            base_revision: scene_revision(&reopened).await,
            path: native(&workspace.0.join("Recipes.spil")),
            read_only: true,
            recover_previous: false,
            discard_changes: false,
        })
        .await
        .unwrap()
    else {
        panic!("open")
    };
    wait(&reopened, operation).await;
    let current = info(&reopened).await;
    assert!(current.read_only);
    let ProjectResponse::Recipes { entries, total, .. } = reopened
        .rpc()
        .project(ProjectCommand::ListRecipes {
            session_id: reopened.hello().session_id.clone(),
            base_revision: current.revision,
            offset: 0,
        })
        .await
        .unwrap()
    else {
        panic!("recipes")
    };
    assert_eq!(total, 2);
    for entry in entries {
        let ProjectResponse::Recipe { transfer } = reopened
            .rpc()
            .project(ProjectCommand::GetRecipe {
                session_id: reopened.hello().session_id.clone(),
                base_revision: current.revision,
                reference: entry.reference,
            })
            .await
            .unwrap()
        else {
            panic!("transfer")
        };
        assert_eq!(transfer.filaments.len(), entry.ingredient_count as usize);
        transfer.validate().unwrap();
    }
    assert!(!workspace.0.join("Missing.spil").exists());
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn hardware_capture_compiles_replays_and_reopens_without_private_inventory() {
    use spiling_contracts::{hardware::*, manufacturing::*};
    use std::sync::Arc;
    let workspace = Workspace::new();
    let mut client = spawn(&workspace.0, "hardware-one", "My library.spil").await;
    let catalog = client.rpc().hardware_catalog().await.unwrap();
    let machine = uuid::Uuid::new_v4().to_string();
    let plate = uuid::Uuid::new_v4().to_string();
    let slot = uuid::Uuid::new_v4().to_string();
    let mut h = HardwareLibrary::default();
    for id in [machine.clone(), uuid::Uuid::new_v4().to_string()] {
        h.machines.push(OwnedMachine {
            id,
            nickname: "PRIVATE Workshop sentinel".into(),
            serial: Some("PRIVATE-SERIAL-SENTINEL".into()),
            location: Some("PRIVATE-ROOM-SENTINEL".into()),
            identity: PrinterIdentity::from_setup(&catalog.setups[0]),
            configuration: MachineConfiguration::Configured {
                setup: Box::new(catalog.setups[0].clone()),
            },
            generation: 1,
        });
    }
    for id in [plate.clone(), uuid::Uuid::new_v4().to_string()] {
        h.plates.push(OwnedPlate {
            id,
            nickname: "PRIVATE-SHEET-SENTINEL".into(),
            definition: catalog.plates[0].clone(),
            installed_machine: None,
            notes: "PRIVATE-CONDITION-SENTINEL".into(),
        });
    }
    h.routes.push(EndpointRoute {
        id: uuid::Uuid::new_v4().to_string(),
        machine_id: machine.clone(),
        family: ConnectionFamily::Moonraker,
        endpoint: "http://private-endpoint.invalid".into(),
        expected_identity: "PRIVATE-DEVICE-SENTINEL".into(),
        generation: 1,
        credential: Some(CredentialRef {
            slot_id: slot.clone(),
            role: CredentialRole::ApiKey,
            backend: CredentialBackend::PlaintextFile,
        }),
    });
    write(
        &client,
        LibraryMutation::ReplaceHardware {
            hardware: h.clone(),
        },
    )
    .await;
    let capture = Arc::new(
        client
            .rpc()
            .library_hardware()
            .await
            .unwrap()
            .hardware
            .capture(&machine, &plate, "textured")
            .unwrap(),
    );
    let original = info(&client).await;
    client
        .rpc()
        .project(ProjectCommand::SetHardware {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
            base_revision: original.revision,
            capture: Some(capture.clone()),
        })
        .await
        .unwrap();
    assert_eq!(info(&client).await.hardware, Some(capture.clone()));
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry/box-mm.step");
    let GeometryResponse::OperationAccepted { operation } = client
        .rpc()
        .geometry(GeometryCommand::ImportPart {
            session_id: client.hello().session_id.clone(),
            base_revision: scene_revision(&client).await,
            source: native(&source),
            options: spiling_contracts::geometry::ModelImportOptions::default(),
            initial_pose: RigidPoseMm::IDENTITY,
        })
        .await
        .unwrap()
    else {
        panic!("import")
    };
    wait(&client, operation).await;
    let intent: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../fixtures/manufacturing/solid-fill.intent.json"
    ))
    .unwrap();
    client
        .rpc()
        .manufacturing(ManufacturingCommand::SetIntent {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
            base_revision: info(&client).await.revision,
            intent: Arc::new(intent),
        })
        .await
        .unwrap();
    let ManufacturingResponse::OperationAccepted { operation } = client
        .rpc()
        .manufacturing(ManufacturingCommand::Compile {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
            base_revision: info(&client).await.revision,
        })
        .await
        .unwrap()
    else {
        panic!("compile")
    };
    let JobResult::ManufacturingCompiled { record, .. } = wait(&client, operation).await else {
        panic!("compiled")
    };
    let ManufacturingResponse::OperationAccepted { operation } = client
        .rpc()
        .manufacturing(ManufacturingCommand::Inspect {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
            base_revision: info(&client).await.revision,
            output: VerificationOutput::ReportOnly,
        })
        .await
        .unwrap()
    else {
        panic!("inspect")
    };
    let JobResult::ManufacturingVerified { report, .. } = wait(&client, operation).await else {
        panic!("verified")
    };
    assert!(report.verified);
    let before = info(&client).await;
    h.machines[0].nickname = "RENAMED-PRIVATE-SENTINEL".into();
    write(&client, LibraryMutation::ReplaceHardware { hardware: h }).await;
    assert_eq!(info(&client).await.revision, before.revision);
    client
        .rpc()
        .project(ProjectCommand::SetHardware {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
            base_revision: before.revision,
            capture: Some(capture.clone()),
        })
        .await
        .unwrap();
    assert_eq!(
        info(&client).await.revision,
        before.revision,
        "same setup/local instance change must preserve project and artifact"
    );
    let mut changed = (*capture).clone();
    changed.face = "smooth".into();
    client
        .rpc()
        .project(ProjectCommand::SetHardware {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
            base_revision: before.revision,
            capture: Some(Arc::new(changed)),
        })
        .await
        .unwrap();
    let ManufacturingResponse::Status { artifact, .. } = client
        .rpc()
        .manufacturing(ManufacturingCommand::Get {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("status")
    };
    assert!(artifact.is_none());
    client
        .rpc()
        .project(ProjectCommand::Undo {
            session_id: client.hello().session_id.clone(),
            base_revision: scene_revision(&client).await,
        })
        .await
        .unwrap();
    let ManufacturingResponse::Status { artifact, .. } = client
        .rpc()
        .manufacturing(ManufacturingCommand::Get {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("status")
    };
    assert_eq!(artifact.as_ref().map(|r| &r.hash), Some(&record.hash));
    let mut blocked = (*capture).clone();
    blocked.plate.faces[0].keepouts_mm.push(AabbMm {
        min: [-1.; 3],
        max: [220.; 3],
    });
    client
        .rpc()
        .project(ProjectCommand::SetHardware {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
            base_revision: info(&client).await.revision,
            capture: Some(Arc::new(blocked)),
        })
        .await
        .unwrap();
    let ManufacturingResponse::OperationAccepted { operation } = client
        .rpc()
        .manufacturing(ManufacturingCommand::Preflight {
            layout_id: info(&client).await.layout_id,
            session_id: client.hello().session_id.clone(),
            base_revision: info(&client).await.revision,
        })
        .await
        .unwrap()
    else {
        panic!("preflight")
    };
    let JobResult::ManufacturingPreflight { report } = wait(&client, operation).await else {
        panic!("preflight report")
    };
    assert_eq!(report.disposition, PreflightDisposition::Refused);
    assert!(report.checks.iter().any(|c| c.code == "hardware_placement"));
    client
        .rpc()
        .project(ProjectCommand::Undo {
            session_id: client.hello().session_id.clone(),
            base_revision: scene_revision(&client).await,
        })
        .await
        .unwrap();
    let path = workspace.0.join("Hardware job.spil");
    let ProjectResponse::OperationAccepted { operation } = client
        .rpc()
        .project(ProjectCommand::Save {
            session_id: client.hello().session_id.clone(),
            base_revision: scene_revision(&client).await,
            target: Some(native(&path)),
        })
        .await
        .unwrap()
    else {
        panic!("save")
    };
    wait(&client, operation).await;
    client.shutdown().await.unwrap();
    let bytes = std::fs::read(&path).unwrap();
    for sentinel in [
        "PRIVATE",
        "private-endpoint.invalid",
        slot.as_str(),
        machine.as_str(),
        plate.as_str(),
    ] {
        assert!(
            !bytes
                .windows(sentinel.len())
                .any(|w| w == sentinel.as_bytes()),
            "private hardware data leaked: {sentinel}"
        );
    }
    let mut reopened = spawn(&workspace.0, "hardware-two", "Absent library.spil").await;
    let ProjectResponse::OperationAccepted { operation } = reopened
        .rpc()
        .project(ProjectCommand::Open {
            session_id: reopened.hello().session_id.clone(),
            base_revision: scene_revision(&reopened).await,
            path: native(&path),
            read_only: true,
            recover_previous: false,
            discard_changes: false,
        })
        .await
        .unwrap()
    else {
        panic!("open")
    };
    wait(&reopened, operation).await;
    assert_eq!(info(&reopened).await.hardware, Some(capture));
    assert!(
        reopened
            .rpc()
            .library_hardware()
            .await
            .unwrap()
            .identity
            .is_none()
    );
    let ManufacturingResponse::OperationAccepted { operation } = reopened
        .rpc()
        .manufacturing(ManufacturingCommand::Inspect {
            layout_id: info(&reopened).await.layout_id,
            session_id: reopened.hello().session_id.clone(),
            base_revision: info(&reopened).await.revision,
            output: VerificationOutput::ReportOnly,
        })
        .await
        .unwrap()
    else {
        panic!("fresh replay")
    };
    let JobResult::ManufacturingVerified { report, .. } = wait(&reopened, operation).await else {
        panic!("replayed")
    };
    assert!(report.verified);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn bundled_reference_search_captures_declarations_stock_and_portable_project() {
    use spiling_contracts::catalog::*;
    let workspace = Workspace::new();
    let mut client = spawn(&workspace.0, "catalog-one", "Catalog library.spil").await;
    let query = CatalogQuery {
        kind: CatalogKind::Filament,
        query: "3d fuel".into(),
        offset: 0,
        snapshot: None,
    };
    let started = std::time::Instant::now();
    let page = client.rpc().search_catalog(&query).await.unwrap();
    eprintln!(
        "cold bundled search: {:?}; {} matches",
        started.elapsed(),
        page.total
    );
    assert!(page.total > 20);
    let captured = page.entries.first().unwrap();
    assert_eq!(
        client.rpc().catalog_record(&captured.key()).await.unwrap(),
        *captured
    );
    let mut missing = captured.key();
    missing.id = "not-a-bundled-product".into();
    assert!(client.rpc().catalog_record(&missing).await.is_err());
    // A missing source is a local lookup failure, not a lost engine session.
    assert_eq!(
        client.rpc().catalog_record(&captured.key()).await.unwrap(),
        *captured
    );

    for text in ["prusament pla", "ovrture", "sunlu", "#8bd8c5"] {
        let query = CatalogQuery {
            query: text.into(),
            ..query.clone()
        };
        let started = std::time::Instant::now();
        let result = client.rpc().search_catalog(&query).await.unwrap();
        eprintln!(
            "warm search {text}: {:?}; {} matches",
            started.elapsed(),
            result.total
        );
        assert!(result.total > 0);
    }
    assert!(
        !workspace.0.join("Catalog library.spil").exists(),
        "discovery is read-only"
    );
    let continued = CatalogQuery {
        offset: page.next.unwrap(),
        snapshot: Some(page.snapshot.clone()),
        ..query.clone()
    };
    let next = client.rpc().search_catalog(&continued).await.unwrap();
    assert!(
        page.entries
            .iter()
            .all(|a| !next.entries.iter().any(|b| a.origin == b.origin))
    );
    let source=page.entries.iter().find(|r|matches!(&r.item,CatalogItem::Filament{declaration,..} if declaration.hueforge_td_mm.is_some())).unwrap().clone();
    let definition = source.filament_definition().unwrap();
    assert!(definition.material.is_none() && definition.profiles.is_empty());
    assert!(definition.declaration.diameter_mm.is_none());
    let reference = FilamentRef {
        id: FilamentId(uuid::Uuid::new_v4().to_string()),
        revision: definition.content_hash().unwrap(),
    };
    write(
        &client,
        LibraryMutation::PutFilament {
            id: reference.id.clone(),
            expected: None,
            definition: definition.clone(),
        },
    )
    .await;
    for _ in 0..2 {
        write(
            &client,
            LibraryMutation::PutSpool {
                spool: SpoolRecord {
                    id: SpoolId(uuid::Uuid::new_v4().to_string()),
                    filament: reference.clone(),
                    name: "Actual spool".into(),
                    state: StockState::OnHand,
                    quantity: None,
                    lot: None,
                    location: None,
                },
            },
        )
        .await;
    }
    let personal = client
        .rpc()
        .library_filaments(&LibraryFilamentQuery {
            query: "3d-fuel".into(),
            on_hand_only: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(personal.entries.len(), 1);
    assert_eq!(personal.entries[0].on_hand_spools, 2);
    assert_eq!(personal.entries[0].declaration, definition.declaration);
    let mut wrong = definition.clone();
    wrong.declaration.brand = Some("Different product brand".into());
    let status = client.rpc().library_status().await.unwrap();
    assert!(
        client
            .rpc()
            .library_write(&LibraryWrite {
                library_id: status.identity.as_ref().map(|i| i.id.clone()),
                base_revision: status.identity.unwrap().revision,
                request_id: uuid::Uuid::new_v4().to_string(),
                mutation: LibraryMutation::PutFilament {
                    id: reference.id.clone(),
                    expected: Some(reference.revision.clone()),
                    definition: wrong
                }
            })
            .await
            .is_err()
    );
    let revision = FilamentRevision {
        reference: reference.clone(),
        definition,
    };
    let response = client
        .rpc()
        .project(ProjectCommand::PutFilament {
            session_id: client.hello().session_id.clone(),
            base_revision: info(&client).await.revision,
            revision: revision.clone(),
            origin: None,
            expected: None,
        })
        .await
        .unwrap();
    assert!(matches!(response, ProjectResponse::Status { .. }));
    let path = workspace.0.join("Catalog job.spil");
    let ProjectResponse::OperationAccepted { operation } = client
        .rpc()
        .project(ProjectCommand::Save {
            session_id: client.hello().session_id.clone(),
            base_revision: scene_revision(&client).await,
            target: Some(native(&path)),
        })
        .await
        .unwrap()
    else {
        panic!("save")
    };
    wait(&client, operation).await;
    client.shutdown().await.unwrap();
    std::fs::rename(
        workspace.0.join("Catalog library.spil"),
        workspace.0.join("Unavailable.spil"),
    )
    .unwrap();
    let mut reopened = spawn(&workspace.0, "catalog-two", "Missing.spil").await;
    let ProjectResponse::OperationAccepted { operation } = reopened
        .rpc()
        .project(ProjectCommand::Open {
            session_id: reopened.hello().session_id.clone(),
            base_revision: scene_revision(&reopened).await,
            path: native(&path),
            read_only: true,
            recover_previous: false,
            discard_changes: false,
        })
        .await
        .unwrap()
    else {
        panic!("open")
    };
    wait(&reopened, operation).await;
    assert_eq!(
        reopened
            .rpc()
            .project(ProjectCommand::GetFilament {
                session_id: reopened.hello().session_id.clone(),
                base_revision: info(&reopened).await.revision,
                reference
            })
            .await
            .unwrap(),
        ProjectResponse::Filament { revision }
    );
    assert!(!workspace.0.join("Missing.spil").exists());
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn printer_discovery_preserves_incomplete_inventory_and_requires_deliberate_setup() {
    use spiling_contracts::{catalog::*, hardware::*};
    let workspace = Workspace::new();
    let mut client = spawn(&workspace.0, "printers-one", "Printers.spil").await;
    let query = CatalogQuery {
        kind: CatalogKind::Printer,
        query: "a1".into(),
        offset: 0,
        snapshot: None,
    };
    let page = client.rpc().search_catalog(&query).await.unwrap();
    let reference = page
        .entries
        .iter()
        .find(|r| matches!(&r.item,CatalogItem::Printer {model,..} if model=="A1"))
        .unwrap()
        .clone();
    assert!(
        matches!(&page.entries[0].item,CatalogItem::Printer{model,..} if model=="A1"),
        "exact model precedes its longer variant"
    );
    let mini = client
        .rpc()
        .search_catalog(&CatalogQuery {
            query: "a1 mini".into(),
            ..query.clone()
        })
        .await
        .unwrap();
    assert!(
        mini.entries
            .iter()
            .all(|r| matches!(&r.item,CatalogItem::Printer{model,..} if model=="A1 mini"))
    );
    assert!(!workspace.0.join("Printers.spil").exists());
    let known = PrinterIdentity::from_reference(reference.clone()).unwrap();
    let machine = |identity: PrinterIdentity| OwnedMachine {
        id: uuid::Uuid::new_v4().to_string(),
        nickname: "Workshop".into(),
        serial: Some("PRIVATE".into()),
        location: None,
        identity,
        configuration: MachineConfiguration::Unconfigured,
        generation: 1,
    };
    let mut h = HardwareLibrary::default();
    h.machines.push(machine(known.clone()));
    h.machines.push(machine(known));
    h.machines.push(machine(PrinterIdentity {
        manufacturer: "Custom".into(),
        model: "Workshop printer".into(),
        catalog_origin: None,
    }));
    write(
        &client,
        LibraryMutation::ReplaceHardware {
            hardware: h.clone(),
        },
    )
    .await;
    let saved = client.rpc().library_hardware().await.unwrap();
    assert_eq!(saved.hardware, h);
    assert!(
        h.capture(
            &h.machines[0].id,
            &uuid::Uuid::new_v4().to_string(),
            "smooth"
        )
        .unwrap_err()
        .contains("Setup needed")
    );
    let c = client.rpc().hardware_catalog().await.unwrap();
    let mut wrong = h.clone();
    wrong.machines[0].configuration = MachineConfiguration::Configured {
        setup: Box::new(c.setups[0].clone()),
    };
    assert!(
        wrong
            .validate()
            .unwrap_err()
            .contains("different printer model")
    );
    let mut custom = c.setups[0].clone();
    custom.id = "custom.operator-declaration".into();
    custom.model.vendor = "Custom".into();
    custom.model.model = "Workshop printer".into();
    custom.model.source = "Operator declaration from explicit software settings".into();
    h.machines[2].configuration = MachineConfiguration::Configured {
        setup: Box::new(custom),
    };
    write(&client, LibraryMutation::ReplaceHardware { hardware: h }).await;
    let saved = client.rpc().library_hardware().await.unwrap();
    assert_eq!(saved.hardware.machines[2].generation, 2);
    assert_eq!(saved.hardware.machines[0].generation, 1);
    client.shutdown().await.unwrap();
    let mut reopened = spawn(&workspace.0, "printers-two", "Printers.spil").await;
    let persisted = reopened.rpc().library_hardware().await.unwrap();
    assert_eq!(persisted, saved);
    assert_eq!(
        persisted.hardware.machines[0]
            .identity
            .catalog_origin
            .as_deref(),
        Some(&reference)
    );
    assert_ne!(
        persisted.hardware.machines[0].id,
        persisted.hardware.machines[1].id
    );
    reopened.shutdown().await.unwrap();
}
