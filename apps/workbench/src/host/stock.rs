// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Capture reviewed immutable stock inputs through the ordinary project transactions.
use super::*;
pub use spiling_contracts::stock::StockChoice as Choice;
use spiling_contracts::{geometry::SourceHash, library::*};
#[derive(Debug, Clone)]
pub struct Keep {
    pub input_hash: SourceHash,
    pub choice: Choice,
    pub name: String,
}
#[derive(Debug, Clone)]
pub enum Kept {
    Filament {
        project: ProjectId,
        reference: FilamentRef,
    },
    Recipe {
        project: ProjectId,
        reference: RecipeRef,
        context: RecipeStudyContext,
    },
}
#[derive(Debug, Clone, Default)]
pub struct State {
    pub capture: Option<Arc<crate::assets::StockCapture>>,
    pub loading: bool,
    pub kept: Option<Kept>,
}
impl Actor {
    pub(super) async fn keep_stock(&mut self, keep: Keep) -> Result<(), ClientError> {
        let invalid = |e: &str| ClientError::InvalidRequest(e.into());
        let capture = self
            .state
            .stock
            .capture
            .clone()
            .ok_or_else(|| invalid("Search saved stock before keeping a result"))?;
        let p = self
            .state
            .project
            .clone()
            .ok_or_else(|| invalid("Open a writable project"))?;
        if capture.project != p.project_id
            || capture.revision != p.revision
            || capture.report.input_hash != keep.input_hash
            || p.read_only
        {
            return Err(invalid(
                "Project or search inputs changed; search again before keeping a result",
            ));
        }
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| invalid("Engine disconnected"))?;
        let rpc = client.rpc();
        let session = client.hello().session_id.clone();
        let report = &capture.report;
        let kept = match keep.choice {
            Choice::Single(index) => {
                let row = report
                    .singles
                    .get(index)
                    .ok_or_else(|| invalid("Selected filament is absent"))?;
                let candidate = &report.candidates[usize::from(row.candidate)];
                let revision = rpc
                    .stock_filament(report, usize::from(row.candidate), &session, p.revision)
                    .await?;
                if !candidate.in_project {
                    let origin = report.library.as_ref().map(|l| LibraryOrigin {
                        library_id: l.id.clone(),
                        filament: revision.reference.clone(),
                    });
                    if self
                        .project_edit(ProjectCommand::PutFilament {
                            session_id: session.clone(),
                            base_revision: p.revision,
                            revision: revision.clone(),
                            expected: None,
                            origin,
                        })
                        .await
                        .is_none()
                    {
                        return Ok(());
                    }
                }
                Kept::Filament {
                    project: p.project_id.clone(),
                    reference: revision.reference,
                }
            }
            Choice::Pair(index) => {
                let pair = report
                    .pairs
                    .get(index)
                    .ok_or_else(|| invalid("Selected recipe is absent"))?;
                let mut filaments = Vec::with_capacity(2);
                for n in pair.candidates {
                    filaments.push(
                        rpc.stock_filament(report, usize::from(n), &session, p.revision)
                            .await?,
                    );
                }
                let transfer = report
                    .recipe(
                        index,
                        RecipeId(uuid::Uuid::new_v4().to_string()),
                        keep.name,
                        filaments,
                    )
                    .map_err(ClientError::InvalidRequest)?;
                let reference = transfer.recipe.reference();
                if self
                    .project_edit(ProjectCommand::PutRecipe {
                        session_id: session,
                        base_revision: p.revision,
                        transfer,
                        expected: None,
                    })
                    .await
                    .is_none()
                {
                    return Ok(());
                }
                Kept::Recipe {
                    project: p.project_id.clone(),
                    reference,
                    context: RecipeStudyContext {
                        patch: report.query.patch,
                        cycle_origin_z_mm: report.query.patch.origin_mm[2],
                    },
                }
            }
        };
        self.state.stock.kept = Some(kept);
        self.state.message =
            "Result kept in Project. Process assignment and construction remain explicit.".into();
        Ok(())
    }
}
