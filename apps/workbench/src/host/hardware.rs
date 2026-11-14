// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Private hardware actions; no transport or secret operations run in a redraw.
use super::*;
use spiling_contracts::{hardware::*, library::*};
use spiling_engine_client::hardware::credential_journal::PendingCredential;
use spiling_engine_client::hardware::credentials::{CredentialInput, CredentialStore};
#[derive(Debug, Clone, Default)]
pub struct State {
    pub personal: Option<LibraryHardware>,
    pub catalog: Option<HardwareCatalog>,
    pub local_selection: Option<Selection>,
    pub credential_pending: Option<PendingCredential>,
    pub error: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Selection {
    pub project: ProjectId,
    pub layout_id: spiling_contracts::project::PrintLayoutId,
    pub capture_hash: spiling_contracts::geometry::SourceHash,
    pub machine_id: String,
    pub plate_id: String,
    pub generation: u64,
    pub matches: bool,
}
#[derive(Debug, Clone)]
pub enum HardwareAction {
    Refresh,
    Select {
        project: ProjectId,
        layout_id: spiling_contracts::project::PrintLayoutId,
        base: ProjectRevision,
        expected: Option<LibraryIdentity>,
        machine: String,
        plate: String,
        face: String,
    },
    RememberCredential {
        route: String,
        expected: LibraryIdentity,
        role: CredentialRole,
        secret: CredentialInput,
    },
    ResolveCredential,
    /// One step: select the chosen printer when it differs from the project's,
    /// then create the default intent through the engine.
    DefaultSettings {
        project: ProjectId,
        layout_id: spiling_contracts::project::PrintLayoutId,
        base: ProjectRevision,
        printer: DefaultPrinter,
        request: spiling_contracts::manufacturing::DefaultIntentRequest,
    },
    ForgetCredential {
        route: String,
        expected: LibraryIdentity,
    },
}
/// The printer a one-step settings request uses.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultPrinter {
    /// The project's current hardware capture.
    Current,
    /// A shipped setup by exact id, on its stock plate.
    Shipped(String),
    /// A configured My hardware machine on one of its plates.
    Machine {
        expected: Option<LibraryIdentity>,
        machine: String,
        plate: String,
        face: String,
    },
}
impl Actor {
    pub(super) async fn hardware_action(
        &mut self,
        action: HardwareAction,
    ) -> Result<(), ClientError> {
        match action {
            HardwareAction::Refresh => self.refresh_hardware().await?,
            HardwareAction::DefaultSettings {
                project,
                layout_id,
                base,
                printer,
                request,
            } => {
                self.default_settings(project, layout_id, base, printer, request)
                    .await?
            }
            HardwareAction::Select {
                project,
                layout_id,
                base,
                expected,
                machine,
                plate,
                face,
            } => {
                let client = self.client.as_ref().expect("connected client");
                let rpc = client.rpc();
                let mut personal = rpc.library_hardware().await?;
                if personal.identity != expected
                    || self.state.project.as_ref().is_none_or(|p| {
                        p.project_id != project || p.layout_id != layout_id || p.revision != base
                    })
                {
                    return Err(ClientError::InvalidRequest(
                        "Hardware selection changed; refresh and review the retained choice".into(),
                    ));
                }
                let capture = personal
                    .hardware
                    .capture(&machine, &plate, &face)
                    .map_err(ClientError::InvalidRequest)?;
                let hash = capture.hash().map_err(ClientError::InvalidRequest)?;
                let generation = personal
                    .hardware
                    .machines
                    .iter()
                    .find(|m| m.id == machine)
                    .expect("capture checked")
                    .generation;
                if self
                    .project_edit(ProjectCommand::SetHardware {
                        layout_id: layout_id.clone(),
                        session_id: client.hello().session_id.clone(),
                        base_revision: base,
                        capture: Some(Arc::new(capture)),
                    })
                    .await
                    .is_none()
                {
                    return Ok(());
                }
                Arc::make_mut(&mut self.state.library)
                    .hardware
                    .local_selection = Some(Selection {
                    project: project.clone(),
                    layout_id: layout_id.clone(),
                    capture_hash: hash.clone(),
                    machine_id: machine.clone(),
                    plate_id: plate.clone(),
                    generation,
                    matches: true,
                });
                // A private association failure cannot undo or misreport the committed project edit.
                if let Some(route) = self.route.as_ref().filter(|r| !r.uncertain) {
                    personal.hardware.bindings.retain(|b| {
                        b.project_id != project.as_str()
                            || b.layout_id != layout_id
                            || b.attachment != route.path
                    });
                    personal
                        .hardware
                        .bindings
                        .push(LocalProjectHardwareBinding {
                            project_id: project.as_str().into(),
                            layout_id: layout_id.clone(),
                            attachment: route.path.clone(),
                            capture_hash: hash,
                            machine_id: machine,
                            plate_id: plate,
                            machine_generation: generation,
                        });
                    let request = hardware_write(&personal, personal.hardware.clone());
                    match rpc.library_write(&request).await {
                        Ok(receipt) if receipt.publication == LibraryPublication::Confirmed => {}
                        Ok(_) => {
                            self.state.message="Project hardware applied; remembering the local selection is uncertain".into();
                        }
                        Err(e) => {
                            self.problem(format!(
                                "Project hardware applied; local selection was not saved: {e}"
                            ));
                        }
                    }
                }
                self.refresh_hardware().await?;
            }
            HardwareAction::RememberCredential {
                route,
                expected,
                role,
                secret,
            } => {
                let rpc = self.client.as_ref().expect("connected client").rpc();
                let mut p = rpc.library_hardware().await?;
                if p.identity.as_ref() != Some(&expected) {
                    return Err(ClientError::InvalidRequest(
                        "Connection changed; refresh before saving credentials".into(),
                    ));
                }
                let connection = p
                    .hardware
                    .routes
                    .iter_mut()
                    .find(|r| r.id == route)
                    .ok_or_else(|| {
                        ClientError::InvalidRequest("Connection no longer exists".into())
                    })?;
                let previous = connection.credential.clone();
                if previous.as_ref().is_some_and(|r| r.role != role) {
                    return Err(ClientError::InvalidRequest(
                        "Credential role differs from the saved connection".into(),
                    ));
                }
                if self.state.library.hardware.credential_pending.is_some() {
                    return Err(ClientError::InvalidRequest(
                        "Resolve the previous credential write first".into(),
                    ));
                }
                // A library CAS failure must never rotate a credential still used by the old route.
                let rotation = previous.clone();
                let reference = tokio::task::spawn_blocking(move || {
                    let store = CredentialStore::user()?;
                    match rotation {
                        Some(previous) => store.stage_rotation(&previous, secret.into_secret()),
                        None => store.remember(role, secret.into_secret()),
                    }
                })
                .await
                .map_err(|_| ClientError::InvalidRequest("Credential task failed".into()))?
                .map_err(|e| ClientError::InvalidRequest(e.to_string()))?;
                connection.credential = Some(reference.clone());
                connection.generation = connection.generation.checked_add(1).ok_or_else(|| {
                    ClientError::InvalidRequest("Connection generation exhausted".into())
                })?;
                let request = hardware_write(&p, p.hardware.clone());
                let location = rpc.library_status().await?.location;
                Arc::make_mut(&mut self.state.library)
                    .hardware
                    .credential_pending = Some(PendingCredential {
                    location,
                    request,
                    delete_after: previous,
                    created: Some(reference),
                });
                self.resolve_credential_write().await?;
            }
            HardwareAction::ResolveCredential => self.resolve_credential_write().await?,
            HardwareAction::ForgetCredential { route, expected } => {
                let rpc = self.client.as_ref().expect("connected client").rpc();
                let mut p = rpc.library_hardware().await?;
                if p.identity.as_ref() != Some(&expected) {
                    return Err(ClientError::InvalidRequest(
                        "Connection changed; refresh before forgetting credentials".into(),
                    ));
                }
                let connection = p
                    .hardware
                    .routes
                    .iter_mut()
                    .find(|r| r.id == route)
                    .ok_or_else(|| {
                        ClientError::InvalidRequest("Connection no longer exists".into())
                    })?;
                let reference = connection.credential.take();
                connection.generation = connection.generation.checked_add(1).ok_or_else(|| {
                    ClientError::InvalidRequest("Connection generation exhausted".into())
                })?;
                if self.state.library.hardware.credential_pending.is_some() {
                    return Err(ClientError::InvalidRequest(
                        "Resolve the previous credential write first".into(),
                    ));
                }
                let request = hardware_write(&p, p.hardware.clone());
                let location = rpc.library_status().await?.location;
                Arc::make_mut(&mut self.state.library)
                    .hardware
                    .credential_pending = Some(PendingCredential {
                    location,
                    request,
                    delete_after: reference,
                    created: None,
                });
                self.resolve_credential_write().await?;
            }
        }
        Ok(())
    }
    async fn resolve_credential_write(&mut self) -> Result<(), ClientError> {
        let pending = self
            .state
            .library
            .hardware
            .credential_pending
            .clone()
            .ok_or_else(|| ClientError::InvalidRequest("No unresolved credential write".into()))?;
        let rpc = self.client.as_ref().expect("connected client").rpc();
        let current = rpc.library_hardware().await?;
        if rpc.library_status().await?.location != pending.location
            || current.identity.as_ref().map(|i| &i.id) != pending.request.library_id.as_ref()
        {
            return Err(ClientError::InvalidRequest(
                "Select the original personal library before resolving its credential write".into(),
            ));
        }
        let journal = pending.clone();
        credential_task(move |store| store.save_pending(&journal)).await?;
        let receipt = match rpc.library_write(&pending.request).await {
            Ok(receipt) => receipt,
            Err(error @ ClientError::Library(_)) if matches!(&error, ClientError::Library(e) if matches!(e.code, LibraryErrorCode::StaleRevision | LibraryErrorCode::InvalidData)) =>
            {
                // A definitive refusal keeps the saved credential. Remove only a fresh
                // slot that the currently observed original library does not reference.
                if let Some(created) = pending.created.clone() {
                    let current = rpc.library_hardware().await?;
                    delete_unreferenced_credential(&current, created).await?;
                }
                let journal = pending.clone();
                credential_task(move |store| store.remove_pending(&journal)).await?;
                Arc::make_mut(&mut self.state.library)
                    .hardware
                    .credential_pending = None;
                self.refresh_hardware().await?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if receipt.publication != LibraryPublication::Confirmed {
            self.state.message =
                "Credential reference save uncertain; resolve the original request".into();
            return Ok(());
        }
        if let Some(reference) = pending.delete_after.clone() {
            let current = rpc.library_hardware().await?;
            delete_unreferenced_credential(&current, reference).await?;
        }
        let journal = pending.clone();
        credential_task(move |store| store.remove_pending(&journal)).await?;
        Arc::make_mut(&mut self.state.library)
            .hardware
            .credential_pending = None;
        self.state.message = "Credential reference saved".into();
        self.refresh_hardware().await
    }
    pub(super) async fn remember_hardware_selection(&mut self) {
        let Some(selection) = self.state.library.hardware.local_selection.clone() else {
            return;
        };
        let Some(route) = self.route.clone().filter(|r| !r.uncertain) else {
            return;
        };
        let Some(project) = self.state.project.clone() else {
            return;
        };
        if project.project_id != selection.project
            || project.layout_id != selection.layout_id
            || project
                .hardware
                .as_ref()
                .and_then(|h| h.hash().ok())
                .as_ref()
                != Some(&selection.capture_hash)
        {
            return;
        }
        let rpc = self.client.as_ref().expect("connected client").rpc();
        let result = async {
            let mut p = rpc.library_hardware().await?;
            p.hardware.bindings.retain(|b| {
                b.project_id != project.project_id.as_str()
                    || b.layout_id != project.layout_id
                    || b.attachment != route.path
            });
            p.hardware.bindings.push(LocalProjectHardwareBinding {
                project_id: project.project_id.as_str().into(),
                layout_id: project.layout_id.clone(),
                attachment: route.path,
                capture_hash: selection.capture_hash,
                machine_id: selection.machine_id,
                plate_id: selection.plate_id,
                machine_generation: selection.generation,
            });
            let request = hardware_write(&p, p.hardware.clone());
            rpc.library_write(&request).await
        }
        .await;
        match result {
            Ok(r) if r.publication == LibraryPublication::Confirmed => {}
            Ok(_) => self.problem("Project saved; private hardware association save is uncertain"),
            Err(e) => self.problem(format!(
                "Project saved; private hardware association was not saved: {e}"
            )),
        }
    }
    async fn default_settings(
        &mut self,
        project: ProjectId,
        layout_id: spiling_contracts::project::PrintLayoutId,
        base: ProjectRevision,
        printer: DefaultPrinter,
        request: spiling_contracts::manufacturing::DefaultIntentRequest,
    ) -> Result<(), ClientError> {
        let stale = || {
            ClientError::InvalidRequest(
                "The project changed; review the print settings choice again".into(),
            )
        };
        if self.state.project.as_ref().is_none_or(|p| {
            p.project_id != project || p.layout_id != layout_id || p.revision != base
        }) {
            return Err(stale());
        }
        let session = self
            .client
            .as_ref()
            .expect("connected client")
            .hello()
            .session_id
            .clone();
        match printer {
            DefaultPrinter::Current => {}
            DefaultPrinter::Shipped(id) => {
                let catalog = self
                    .client
                    .as_ref()
                    .expect("connected client")
                    .rpc()
                    .hardware_catalog()
                    .await?;
                let setup =
                    catalog.setups.iter().find(|s| s.id == id).ok_or_else(|| {
                        ClientError::InvalidRequest(format!("no shipped setup {id}"))
                    })?;
                let capture = catalog
                    .stock_capture(setup)
                    .map_err(ClientError::InvalidRequest)?;
                let current = self.state.project.as_ref().and_then(|p| p.hardware.clone());
                if current.as_deref() != Some(&capture) {
                    if self
                        .project_edit(ProjectCommand::SetHardware {
                            layout_id: layout_id.clone(),
                            session_id: session.clone(),
                            base_revision: base,
                            capture: Some(Arc::new(capture)),
                        })
                        .await
                        .is_none()
                    {
                        return Ok(());
                    }
                    Arc::make_mut(&mut self.state.library)
                        .hardware
                        .local_selection = None;
                }
            }
            DefaultPrinter::Machine {
                expected,
                machine,
                plate,
                face,
            } => {
                Box::pin(self.hardware_action(HardwareAction::Select {
                    project: project.clone(),
                    layout_id: layout_id.clone(),
                    base,
                    expected,
                    machine,
                    plate,
                    face,
                }))
                .await?;
            }
        }
        let Some(current) = self.state.project.as_ref().filter(|p| {
            p.project_id == project && p.layout_id == layout_id && p.hardware.is_some()
        }) else {
            return Err(stale());
        };
        let command = spiling_contracts::manufacturing::ManufacturingCommand::CreateDefaultIntent {
            layout_id,
            session_id: session,
            base_revision: current.revision,
            request,
        };
        self.manufacturing(command).await;
        Ok(())
    }
    pub(super) async fn refresh_hardware(&mut self) -> Result<(), ClientError> {
        let rpc = self.client.as_ref().expect("connected client").rpc();
        let personal = rpc.library_hardware().await?;
        let catalog = rpc.hardware_catalog().await?;
        let status = rpc.library_status().await?;
        if self.state.library.hardware.credential_pending.is_none()
            && let Some(identity) = &personal.identity
        {
            let location = status.location.clone();
            let library_id = identity.id.clone();
            let pending =
                credential_task(move |store| store.pending(&location, &library_id)).await?;
            Arc::make_mut(&mut self.state.library)
                .hardware
                .credential_pending = pending;
        }
        let local = self.state.project.as_ref().and_then(|project| {
            let capture = project.hardware.as_ref()?;
            let hash = capture.hash().ok()?;
            let b = self.route.as_ref().and_then(|route| {
                personal.hardware.bindings.iter().find(|b| {
                    b.project_id == project.project_id.as_str()
                        && b.layout_id == project.layout_id
                        && b.attachment == route.path
                })
            });
            b.map(|b| Selection {
                project: project.project_id.clone(),
                layout_id: project.layout_id.clone(),
                capture_hash: b.capture_hash.clone(),
                machine_id: b.machine_id.clone(),
                plate_id: b.plate_id.clone(),
                generation: b.machine_generation,
                matches: b.capture_hash == hash
                    && personal
                        .hardware
                        .machines
                        .iter()
                        .any(|m| m.id == b.machine_id && m.generation == b.machine_generation)
                    && personal
                        .hardware
                        .capture(&b.machine_id, &b.plate_id, &capture.face)
                        .and_then(|c| c.hash())
                        .is_ok_and(|installed| installed == hash),
            })
        });
        let state = Arc::make_mut(&mut self.state.library);
        state.status = Some(status);
        state.hardware.personal = Some(personal);
        state.hardware.catalog = Some(catalog);
        state.hardware.error = None;
        if self.route.is_some() || local.is_some() {
            state.hardware.local_selection = local;
        } else if state.hardware.local_selection.as_ref().is_some_and(|s| {
            self.state.project.as_ref().is_none_or(|p| {
                p.project_id != s.project
                    || p.hardware.as_ref().and_then(|c| c.hash().ok()).as_ref()
                        != Some(&s.capture_hash)
            })
        }) {
            state.hardware.local_selection = None;
        } else if let Some(selection) = &mut state.hardware.local_selection {
            let project = self
                .state
                .project
                .as_ref()
                .expect("retained matching project");
            let capture = project
                .hardware
                .as_ref()
                .expect("retained matching capture");
            let personal = state.hardware.personal.as_ref().expect("fresh inventory");
            selection.matches = personal
                .hardware
                .machines
                .iter()
                .any(|m| m.id == selection.machine_id && m.generation == selection.generation)
                && personal
                    .hardware
                    .capture(&selection.machine_id, &selection.plate_id, &capture.face)
                    .and_then(|c| c.hash())
                    .is_ok_and(|hash| hash == selection.capture_hash);
        }
        Ok(())
    }
}
fn hardware_write(personal: &LibraryHardware, hardware: HardwareLibrary) -> LibraryWrite {
    LibraryWrite {
        library_id: personal.identity.as_ref().map(|i| i.id.clone()),
        base_revision: personal.identity.as_ref().map_or(0, |i| i.revision),
        request_id: uuid::Uuid::new_v4().to_string(),
        mutation: LibraryMutation::ReplaceHardware { hardware },
    }
}

async fn delete_unreferenced_credential(
    current: &LibraryHardware,
    reference: CredentialRef,
) -> Result<(), ClientError> {
    if current
        .hardware
        .routes
        .iter()
        .any(|r| r.credential.as_ref() == Some(&reference))
    {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || match CredentialStore::user()?.delete(&reference) {
        Ok(()) | Err(spiling_engine_client::hardware::credentials::CredentialError::Missing) => {
            Ok(())
        }
        Err(e) => Err(e),
    })
    .await
    .map_err(|_| ClientError::InvalidRequest("Credential task failed".into()))?
    .map_err(|e| ClientError::InvalidRequest(e.to_string()))
}

async fn credential_task<T: Send + 'static>(
    task: impl FnOnce(
        CredentialStore,
    ) -> Result<T, spiling_engine_client::hardware::credentials::CredentialError>
    + Send
    + 'static,
) -> Result<T, ClientError> {
    tokio::task::spawn_blocking(move || task(CredentialStore::user()?))
        .await
        .map_err(|_| ClientError::InvalidRequest("Credential task failed".into()))?
        .map_err(|e| ClientError::InvalidRequest(e.to_string()))
}
