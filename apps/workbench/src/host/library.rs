// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::{geometry::SourceHash, library::*};
#[derive(Debug, Clone, Default)]
pub struct State {
    pub catalog: Option<spiling_contracts::catalog::CatalogPage>,
    pub catalog_lookup: Option<CatalogLookup>,
    pub catalog_error: Option<String>,
    pub hardware: super::hardware::State,
    pub personal_recipes: Option<LibraryRecipePage>,
    pub project_recipes: Vec<RecipeSummary>,
    pub recipe_project: Option<(ProjectId, ProjectRevision)>,
    pub recipe_selection: Option<super::recipes::Selection>,
    pub status: Option<LibraryStatus>,
    pub personal: Option<LibraryFilamentPage>,
    pub personal_query: Option<LibraryFilamentQuery>,
    pub project: Vec<LibraryFilamentSummary>,
    pub project_capture: Option<(ProjectId, ProjectRevision)>,
    pub selected: Option<(bool, FilamentRevision)>,
    pub selected_project: Option<(ProjectId, ProjectRevision)>,
    pub selected_library: Option<LibraryId>,
    pub comparison: Option<DefinitionComparison>,
    pub project_receipt: Option<ProjectMaterialReceipt>,
    pub spools: Vec<SpoolRecord>,
    pub receipt: Option<LibraryWriteReceipt>,
    pub rejected_request: Option<String>,
}
#[derive(Debug, Clone)]
pub struct CatalogLookup {
    pub key: spiling_contracts::catalog::CatalogKey,
    pub result: Result<spiling_contracts::catalog::CatalogRecord, String>,
}
#[derive(Debug, Clone)]
pub struct ProjectMaterialReceipt {
    pub project: ProjectId,
    pub before: ProjectRevision,
    pub after: ProjectRevision,
    pub reference: FilamentRef,
}
/// Exact operands retained for an explicit cross-file replacement; never a sync link.
#[derive(Debug, Clone)]
pub struct DefinitionComparison {
    pub project: (ProjectId, ProjectRevision),
    pub copy: FilamentRevision,
    pub origin: LibraryOrigin,
    pub identity: LibraryIdentity,
    pub personal: FilamentRevision,
    pub archived: bool,
}
impl DefinitionComparison {
    pub fn summary(&self) -> &'static str {
        if self.copy.definition == self.personal.definition {
            "Same content as My library"
        } else {
            match (
                self.copy.reference.revision != self.origin.filament.revision,
                self.personal.reference.revision != self.origin.filament.revision,
            ) {
                (true, true) => "Both copies changed since capture",
                (true, false) => "Edited in Project",
                (false, true) => "Update available in My library",
                (false, false) => "Different captured content",
            }
        }
    }
}
#[derive(Debug, Clone)]
pub enum LibraryAction {
    KeepStock(super::stock::Keep),
    LookupCatalog(spiling_contracts::catalog::CatalogKey),
    SearchCatalog(spiling_contracts::catalog::CatalogQuery),
    Hardware(super::hardware::HardwareAction),
    Recipe(super::recipes::RecipeAction),
    Refresh(LibraryFilamentQuery),
    Inspect {
        personal: bool,
        reference: FilamentRef,
    },
    Write(LibraryWrite),
    SetLocation(NativePath),
    PutProjectFilament {
        project_id: ProjectId,
        revision: FilamentRevision,
        origin: Option<LibraryOrigin>,
        expected: Option<SourceHash>,
        base_revision: ProjectRevision,
    },
    CompareOrigin {
        project_id: ProjectId,
        base_revision: ProjectRevision,
        reference: FilamentRef,
    },
    ReviewProfile(spiling_contracts::manufacturing::MaterialProcessProfile),
}
impl Host {
    pub fn library(&self, action: LibraryAction) -> Result<(), String> {
        match &action {
            LibraryAction::SearchCatalog(query) => query.validate()?,
            LibraryAction::LookupCatalog(key) => key.validate()?,
            LibraryAction::Recipe(action) => match action {
                super::recipes::RecipeAction::Refresh { query } => query.validate()?,
                super::recipes::RecipeAction::Inspect { reference, .. } => reference.validate()?,
                super::recipes::RecipeAction::CopyToProject { transfer, .. } => {
                    transfer.validate()?
                }
            },
            LibraryAction::Write(request) => {
                request.validate()?;
                if serde_json::to_vec(request)
                    .map_err(|e| e.to_string())?
                    .len()
                    > spiling_contracts::MAX_CONTROL_BYTES as usize
                {
                    return Err("Library draft exceeds the bounded host control limit".into());
                }
            }
            LibraryAction::PutProjectFilament { revision, .. } => {
                revision.validate()?;
            }
            LibraryAction::ReviewProfile(profile) => {
                profile.validate().map_err(|e| e.to_string())?;
            }
            LibraryAction::Refresh(query) => query.validate()?,
            LibraryAction::CompareOrigin { reference, .. } => reference.validate()?,
            LibraryAction::SetLocation(path) => path.validate().map_err(|e| e.to_string())?,
            _ => {}
        }
        self.send(Request::Library(Box::new(action)))
    }
}
impl Actor {
    pub(super) async fn library_action(&mut self, action: LibraryAction) {
        if self.client.is_none() {
            self.problem("Engine is disconnected");
            return;
        }
        let result = self.library_inner(action).await;
        if let Err(error) = result {
            self.failure(error).await;
        }
    }
    async fn library_inner(&mut self, action: LibraryAction) -> Result<(), ClientError> {
        let rpc = self.client.as_ref().expect("connected client").rpc();
        match action {
            LibraryAction::KeepStock(keep) => self.keep_stock(keep).await?,
            LibraryAction::LookupCatalog(key) => {
                let result = rpc.catalog_record(&key).await.map_err(|e| e.to_string());
                Arc::make_mut(&mut self.state.library).catalog_lookup =
                    Some(CatalogLookup { key, result });
            }
            LibraryAction::SearchCatalog(query) => {
                let result = rpc.search_catalog(&query).await;
                let state = Arc::make_mut(&mut self.state.library);
                state.catalog_error = result.as_ref().err().map(ToString::to_string);
                if let Ok(page) = result {
                    state.catalog = Some(page);
                }
            }
            LibraryAction::Hardware(action) => self.hardware_action(action).await?,
            LibraryAction::Recipe(action) => self.recipe_action(action).await?,
            LibraryAction::Refresh(query) => {
                // Project-owned content remains browsable even if personal storage fails.
                let personal = async {
                    let status = rpc.library_status().await?;
                    let page = rpc.library_filaments(&query).await?;
                    Ok::<_, ClientError>((status, page))
                }
                .await;
                let state = Arc::make_mut(&mut self.state.library);
                match &personal {
                    Ok((status, page)) => {
                        state.status = Some(status.clone());
                        state.personal = Some(page.clone());
                        state.personal_query = Some(query.clone());
                    }
                    Err(_) => {
                        state.status = None;
                        state.personal = None;
                    }
                }
                if let (Some(project), Some(hello)) = (&self.state.project, &self.state.hello) {
                    let mut offset = 0;
                    let mut rows = Vec::new();
                    loop {
                        match rpc
                            .project(ProjectCommand::ListFilaments {
                                session_id: hello.session_id.clone(),
                                base_revision: project.revision,
                                offset,
                            })
                            .await?
                        {
                            ProjectResponse::Filaments {
                                entries,
                                next_offset,
                                ..
                            } => {
                                rows.extend(entries);
                                if let Some(next) = next_offset {
                                    offset = next;
                                } else {
                                    break;
                                }
                            }
                            ProjectResponse::Error { error } => return Err(error.into()),
                            _ => {
                                return Err(ClientError::Protocol(
                                    "expected project materials".into(),
                                ));
                            }
                        }
                    }
                    state.project = rows;
                    state.project_capture = Some((project.project_id.clone(), project.revision));
                }
                personal?;
            }
            LibraryAction::Inspect {
                personal,
                reference,
            } => {
                let mut capture = None;
                let mut selected_library = None;
                let revision = if personal {
                    let status = rpc.library_status().await?;
                    let id = status
                        .identity
                        .as_ref()
                        .ok_or_else(|| ClientError::InvalidRequest("My library is empty".into()))?;
                    let revision = rpc.library_filament(&id.id, &reference).await?;
                    selected_library = Some(id.id.clone());
                    let mut query = LibrarySpoolQuery {
                        expected: Some(id.clone()),
                        filament: Some(reference.clone()),
                        ..Default::default()
                    };
                    let mut spools = Vec::new();
                    loop {
                        let page = rpc.library_spools(&query).await?;
                        spools.extend(page.spools);
                        if let Some(next) = page.next_offset {
                            query.offset = next;
                        } else {
                            break;
                        }
                    }
                    let state = Arc::make_mut(&mut self.state.library);
                    state.status = Some(status);
                    state.spools = spools;
                    revision
                } else {
                    let project = self
                        .state
                        .project
                        .as_ref()
                        .ok_or_else(|| ClientError::InvalidRequest("no project".into()))?;
                    capture = Some((project.project_id.clone(), project.revision));
                    match rpc
                        .project(ProjectCommand::GetFilament {
                            session_id: self
                                .state
                                .hello
                                .as_ref()
                                .expect("connected")
                                .session_id
                                .clone(),
                            base_revision: project.revision,
                            reference,
                        })
                        .await?
                    {
                        ProjectResponse::Filament { revision } => revision,
                        ProjectResponse::Error { error } => return Err(error.into()),
                        _ => return Err(ClientError::Protocol("expected project filament".into())),
                    }
                };
                let state = Arc::make_mut(&mut self.state.library);
                state.comparison = None;
                state.selected = Some((personal, revision));
                state.selected_project = capture;
                state.selected_library = selected_library;
            }
            LibraryAction::Write(request) => {
                Arc::make_mut(&mut self.state.library).rejected_request = None;
                let receipt = match rpc.library_write(&request).await {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        if matches!(
                            &error,
                            ClientError::Library(_) | ClientError::InvalidRequest(_)
                        ) {
                            Arc::make_mut(&mut self.state.library).rejected_request =
                                Some(request.request_id.clone());
                        }
                        return Err(error);
                    }
                };
                let confirmed = receipt.publication == LibraryPublication::Confirmed;
                if confirmed {
                    let state = Arc::make_mut(&mut self.state.library);
                    state.comparison = None;
                    state.selected = None;
                    state.selected_project = None;
                    state.selected_library = None;
                    state.spools.clear();
                }
                Arc::make_mut(&mut self.state.library).receipt = Some(receipt);
                self.state.message = if confirmed {
                    "My library saved"
                } else {
                    "Library save is uncertain; retain and resolve the original request"
                }
                .into();
            }
            LibraryAction::SetLocation(location) => {
                if self.state.library.hardware.credential_pending.is_some() {
                    return Err(ClientError::InvalidRequest(
                        "Resolve the pending credential write before changing personal libraries"
                            .into(),
                    ));
                }
                let status = rpc.library_set_location(location).await?;
                self.library_location = Some(status.location.clone());
                self.state.library = Arc::new(State {
                    status: Some(status),
                    ..Default::default()
                });
            }
            LibraryAction::PutProjectFilament {
                project_id,
                revision,
                origin,
                expected,
                base_revision,
            } => {
                if self
                    .state
                    .project
                    .as_ref()
                    .is_none_or(|p| p.project_id != project_id)
                {
                    return Err(ClientError::InvalidRequest(
                        "The material draft belongs to another project".into(),
                    ));
                }
                let session_id = self
                    .state
                    .hello
                    .as_ref()
                    .expect("connected")
                    .session_id
                    .clone();
                let reference = revision.reference.clone();
                if let Some(info) = self
                    .project_edit(ProjectCommand::PutFilament {
                        session_id,
                        base_revision,
                        revision,
                        origin,
                        expected,
                    })
                    .await
                {
                    if info.project_id != project_id || info.revision <= base_revision {
                        return Err(ClientError::Protocol(
                            "Project material receipt does not match its captured mutation".into(),
                        ));
                    }
                    let state = Arc::make_mut(&mut self.state.library);
                    state.selected = None;
                    state.selected_project = None;
                    state.selected_library = None;
                    state.comparison = None;
                    state.project_receipt = Some(ProjectMaterialReceipt {
                        project: project_id,
                        before: base_revision,
                        after: info.revision,
                        reference,
                    });
                    self.state.message =
                        "Project material saved; current job profile is unchanged".into();
                }
            }
            LibraryAction::CompareOrigin {
                project_id,
                base_revision,
                reference,
            } => {
                let capture = (project_id, base_revision);
                let project = self
                    .state
                    .project
                    .as_ref()
                    .ok_or_else(|| ClientError::InvalidRequest("no project".into()))?;
                let state = &self.state.library;
                if capture != (project.project_id.clone(), project.revision)
                    || state.project_capture.as_ref() != Some(&capture)
                    || state.selected_project.as_ref() != Some(&capture)
                {
                    return Err(ClientError::InvalidRequest(
                        "Refresh and inspect the current project material before comparison".into(),
                    ));
                }
                let copy = state
                    .selected
                    .as_ref()
                    .filter(|(personal, r)| !personal && r.reference == reference)
                    .map(|(_, r)| r.clone())
                    .ok_or_else(|| {
                        ClientError::InvalidRequest(
                            "Inspect the project material before comparison".into(),
                        )
                    })?;
                let origin = state.project.iter()
                    .find(|row| row.entry.current == reference)
                    .and_then(|row| row.entry.origin.clone())
                    .ok_or_else(|| ClientError::InvalidRequest("This project copy has no personal origin; save a new copy to My library".into()))?;
                let status = rpc.library_status().await?;
                let identity = status.identity.as_ref()
                    .filter(|id| id.id == origin.library_id)
                    .ok_or_else(|| ClientError::InvalidRequest("This project copy is independent of the selected library; save a new copy to reuse it here".into()))?
                    .clone();
                let mut query = LibraryFilamentQuery {
                    expected: Some(identity.clone()),
                    include_archived: true,
                    ..Default::default()
                };
                let row = loop {
                    let page = rpc.library_filaments(&query).await?;
                    if let Some(row) = page
                        .entries
                        .into_iter()
                        .find(|r| r.entry.current.id == origin.filament.id)
                    {
                        break row;
                    }
                    if let Some(next) = page.next_offset {
                        query.offset = next;
                    } else {
                        return Err(ClientError::InvalidRequest("The source entry is absent; the project copy remains usable and can be saved as a new item".into()));
                    }
                };
                let personal = rpc
                    .library_filament(&identity.id, &row.entry.current)
                    .await?;
                let state = Arc::make_mut(&mut self.state.library);
                state.status = Some(status);
                state.comparison = Some(DefinitionComparison {
                    project: capture,
                    copy,
                    origin,
                    identity,
                    personal,
                    archived: row.entry.archived,
                });
                self.state.message =
                    "Material comparison ready; choose an explicit replacement or keep both copies"
                        .into();
            }
            LibraryAction::ReviewProfile(profile) => {
                use spiling_contracts::manufacturing::*;
                let project = self
                    .state
                    .project
                    .as_ref()
                    .ok_or_else(|| ClientError::InvalidRequest("no project".into()))?;
                let selection = &self.state.library;
                if selection.selected_project.as_ref()
                    != Some(&(project.project_id.clone(), project.revision))
                    || !selection
                        .selected
                        .as_ref()
                        .is_some_and(|(personal, selected)| {
                            !personal && selected.definition.profiles.contains(&profile)
                        })
                {
                    return Err(ClientError::InvalidRequest(
                        "Refresh and inspect the current project material before reviewing its profile"
                            .into(),
                    ));
                }
                let setup = self
                    .state
                    .manufacturing
                    .intent
                    .as_ref()
                    .and_then(|i| i.material_profile.as_ref())
                    .map(|p| p.selection.setup.clone())
                    .unwrap_or(MaterialToolSetup {
                        hotend_id: profile.hotend_id.clone(),
                        nozzle_material: profile.nozzle_material.clone(),
                        abrasion_resistant_nozzle: false,
                        cooling_configuration_id: profile.cooling_configuration_id.clone(),
                        extruder_drive: ExtruderDrive::DirectDrive,
                    });
                self.nonce += 1;
                Arc::make_mut(&mut self.state.manufacturing).loaded_material =
                    Some(Arc::new(manufacture::LoadedMaterial {
                        nonce: self.nonce,
                        label: "Project material".into(),
                        project: project.project_id.clone(),
                        revision: project.revision,
                        session: self
                            .state
                            .hello
                            .as_ref()
                            .expect("connected")
                            .session_id
                            .clone(),
                        selection: MaterialProfileSelection {
                            profile,
                            setup,
                            overrides: MaterialProfileOverrides {
                                nozzle_temperature_c: None,
                                bed_temperature_c: None,
                                print_speed_mm_s: None,
                                flow_multiplier: None,
                                process: None,
                                first_layer_nozzle_temperature_c: None,
                                first_layer_bed_temperature_c: None,
                            },
                        },
                    }));
                self.state.message =
                    "Project profile ready in Manufacturing; inspect values and Apply".into();
            }
        }
        Ok(())
    }
}
