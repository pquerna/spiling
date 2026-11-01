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
