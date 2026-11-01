// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::library::*;
#[derive(Debug, Clone)]
pub struct Selection {
    pub personal: bool,
    pub transfer: RecipeTransfer,
    pub project: Option<(ProjectId, ProjectRevision)>,
    pub library: Option<LibraryIdentity>,
}
#[expect(
    clippy::large_enum_variant,
    reason = "The host admits bounded complete controls inline; recipe transfer adds no per-call boxing"
)]
#[derive(Debug, Clone)]
pub enum RecipeAction {
    Refresh {
        query: LibraryRecipeQuery,
    },
    Inspect {
        personal: bool,
        reference: RecipeRef,
    },
    CopyToProject {
        project: (ProjectId, ProjectRevision),
        transfer: RecipeTransfer,
    },
}
impl Actor {
    pub(super) async fn recipe_action(&mut self, action: RecipeAction) -> Result<(), ClientError> {
        let rpc = self.client.as_ref().expect("connected").rpc();
        match action {
            RecipeAction::Refresh { query } => {
                // A failed personal store must not hide standalone project recipes.
                let personal = async {
                    let status = rpc.library_status().await?;
                    let mut query = query;
                    if query.expected.is_none() {
                        query.expected = status.identity.clone();
                    }
                    Ok::<_, ClientError>((status, rpc.library_recipes(&query).await?))
                }
                .await;
                let state = Arc::make_mut(&mut self.state.library);
                state.personal_recipes = personal.as_ref().ok().map(|(_, page)| page.clone());
                state.status = personal.as_ref().ok().map(|(status, _)| status.clone());
                state.project_recipes.clear();
                state.recipe_project = None;
                if let (Some(p), Some(h)) = (&self.state.project, &self.state.hello) {
                    let mut offset = 0;
                    loop {
                        match rpc
                            .project(ProjectCommand::ListRecipes {
                                session_id: h.session_id.clone(),
                                base_revision: p.revision,
                                offset,
                            })
                            .await?
                        {
                            ProjectResponse::Recipes {
                                entries,
                                next_offset,
                                ..
                            } => {
                                state.project_recipes.extend(entries);
                                if let Some(next) = next_offset {
                                    offset = next;
                                } else {
                                    break;
                                }
                            }
                            ProjectResponse::Error { error } => return Err(error.into()),
                            _ => {
                                return Err(ClientError::Protocol(
                                    "expected project recipes".into(),
                                ));
                            }
                        }
                    }
                    state.recipe_project = Some((p.project_id.clone(), p.revision));
                }
                personal?;
            }
            RecipeAction::Inspect {
                personal,
                reference,
            } => {
                let (transfer, project, library) = if personal {
                    let status = rpc.library_status().await?;
                    let identity = status
                        .identity
                        .as_ref()
                        .ok_or_else(|| ClientError::InvalidRequest("no personal recipes".into()))?;
                    let transfer = rpc.library_recipe(&identity.id, &reference).await?;
                    let identity = identity.clone();
                    Arc::make_mut(&mut self.state.library).status = Some(status);
                    (transfer, None, Some(identity))
                } else {
                    let p = self
                        .state
                        .project
                        .as_ref()
                        .ok_or_else(|| ClientError::InvalidRequest("no project".into()))?;
                    let h = self.state.hello.as_ref().expect("connected");
                    let transfer = match rpc
                        .project(ProjectCommand::GetRecipe {
                            session_id: h.session_id.clone(),
                            base_revision: p.revision,
                            reference,
                        })
                        .await?
                    {
                        ProjectResponse::Recipe { transfer } => transfer,
                        ProjectResponse::Error { error } => return Err(error.into()),
                        _ => return Err(ClientError::Protocol("expected complete recipe".into())),
                    };
                    (transfer, Some((p.project_id.clone(), p.revision)), None)
                };
                Arc::make_mut(&mut self.state.library).recipe_selection = Some(Selection {
                    personal,
                    transfer,
                    project,
                    library,
                });
            }
            RecipeAction::CopyToProject { project, transfer } => {
                if self
                    .state
                    .project
                    .as_ref()
                    .is_none_or(|p| (p.project_id.clone(), p.revision) != project)
                {
                    return Err(ClientError::InvalidRequest(
                        "project changed; review recipe reuse again".into(),
                    ));
                }
                let session_id = self
                    .state
                    .hello
                    .as_ref()
                    .expect("connected")
                    .session_id
                    .clone();
                if self
                    .project_edit(ProjectCommand::PutRecipe {
                        session_id,
                        base_revision: project.1,
                        transfer,
                        expected: None,
                    })
                    .await
                    .is_some()
                {
                    self.state.message = "Complete recipe copied to Project. Inspect it and choose a fresh study context before assessment.".into();
                    Arc::make_mut(&mut self.state.library).recipe_selection = None;
                }
            }
        }
        Ok(())
    }
}
