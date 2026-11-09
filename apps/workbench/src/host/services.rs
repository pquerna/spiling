// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{ArtifactView, manufacturing::*};

impl Actor {
    fn settings_applied(
        &mut self,
        session: spiling_contracts::geometry::SessionId,
        base_revision: ProjectRevision,
        info: &ProjectInfo,
        interior_edit: Option<InteriorToolPolicyEdit>,
    ) {
        if self.state.project.as_ref().is_none_or(|project| {
            project.project_id != info.project_id || project.revision != info.revision
        }) || self
            .state
            .hello
            .as_ref()
            .is_none_or(|hello| hello.session_id != session)
        {
            return;
        }
        if let Some(intent) = self.state.manufacturing.intent.clone() {
            self.nonce += 1;
            Arc::make_mut(&mut self.state.manufacturing).applied_settings =
                Some(Arc::new(manufacture::AppliedSettings {
                    interior_edit,
                    nonce: self.nonce,
                    project: info.project_id.clone(),
                    session,
                    base_revision,
                    revision: info.revision,
                    intent,
                }));
        }
    }
    pub(super) fn alternative_capture_matches(&self, capture: &Arc<manufacture::Capture>) -> bool {
        self.state
            .project
            .as_ref()
            .zip(self.state.hello.as_ref())
            .is_some_and(|(project, hello)| {
                project.project_id == capture.provenance.project_id
                    && hello.session_id == capture.session
                    && self
                        .state
                        .manufacturing
                        .alternative
                        .as_ref()
                        .is_some_and(|evaluation| {
                            evaluation.record == capture.record
                                && evaluation.resource == capture.resource
                                && evaluation.captured_revision == capture.observed_revision
                        })
            })
    }
    pub(super) async fn alternative_evaluated(
        &mut self,
        evaluation: ManufacturingAlternativeEvaluation,
    ) {
        if let Err(error) = self.refresh().await {
            self.failure(error).await;
            return;
        }
        let Some(client) = &self.client else {
            return;
        };
        let Some(project) = &self.state.project else {
            return;
        };
        if evaluation.project_id != project.project_id {
            self.problem("Alternative result belongs to a different project");
            return;
        }
        self.nonce += 1;
        let capture = Arc::new(manufacture::Capture {
            nonce: self.nonce,
            layout_id: project.layout_id.clone(),
            session: client.hello().session_id.clone(),
            observed_revision: evaluation.captured_revision,
            record: evaluation.record.clone(),
            report: evaluation.report.clone(),
            provenance: evaluation.provenance.clone(),
            resource: evaluation.resource.clone(),
            replay: Some(evaluation.replay_display.clone()),
        });
        if let Some(task) = self.alternative_task.take() {
            task.abort();
        }
        let reserved = self
            .state
            .display
            .as_ref()
            .map_or(0, |display| display.raw_bytes * 2)
            + self.state.manufacturing.display_bytes()
            + crate::scene::CONTEXT_DISPLAY_RESERVE_BYTES
            + self.state.section.as_ref().map_or(0, |section| {
                section.summary.total_bytes as usize * crate::scene::SECTION_DISPLAY_MULTIPLIER
            })
            + if self.section_task.is_some() {
                spiling_contracts::geometry::MAX_SECTION_BYTES as usize
                    * crate::scene::SECTION_DISPLAY_MULTIPLIER
            } else {
                0
            }
            + 16 * 1024 * 1024;
        let origin = self
            .state
            .scene
            .as_ref()
            .and_then(|scene| scene.bounds_mm)
            .map_or(glam::DVec3::ZERO, |bounds| {
                (glam::DVec3::from_array(bounds.min) + glam::DVec3::from_array(bounds.max)) * 0.5
            });
        Arc::make_mut(&mut self.state.manufacturing).alternative = Some(Arc::new(evaluation));
        self.alternative_task = Some(tokio::spawn(manufacture::load_checked_review(
            client.rpc(),
            capture,
            reserved,
            origin,
        )));
    }
    pub(super) fn capture_current(&self, capture: &Arc<manufacture::Capture>) -> bool {
        self.state.phase == Phase::Running
            && self
                .state
                .project
                .as_ref()
                .zip(self.state.hello.as_ref())
                .is_some_and(|(p, h)| {
                    self.state
                        .manufacturing
                        .current_capture(p, &h.session_id)
                        .is_some_and(|c| Arc::ptr_eq(c, capture))
                })
    }
    pub(super) fn clear_review(&mut self) {
        if let Some(task) = self.review_task.take() {
            task.abort();
        }
        Arc::make_mut(&mut self.state.manufacturing).invalidate();
    }
    fn control_ready(&mut self) -> bool {
        if self.state.diagnostic.as_ref().is_some_and(|d| !d.done)
            || self.mechanics_task.is_some()
            || self.pending.is_some()
            || self.export_task.is_some()
            || self.dialog.is_some()
            || self.confirmation.is_some()
        {
            self.problem("Finish the active native action first");
            return false;
        }
        if self.client.is_none() {
            self.problem("Engine is disconnected");
            return false;
        }
        true
    }
    pub(super) async fn manufacturing(&mut self, command: ManufacturingCommand) {
        if !self.control_ready() {
            return;
        }
        if let ManufacturingCommand::Compile { base_revision, .. } = &command
            && self.state.manufacturing.preflight.as_ref().is_none_or(|p| {
                p.input_revision != *base_revision
                    || p.disposition != PreflightDisposition::Admitted
            })
        {
            self.problem("Run an admitted preflight for the current revision before Compile");
            return;
        }
        let label = match &command {
            ManufacturingCommand::ReviewMaterials { .. } => "Review material profiles",
            ManufacturingCommand::SetIntent { .. } => "Apply full manufacturing intent",
            ManufacturingCommand::EditInteriorPolicy { .. } => "Apply interior tool policy",
            ManufacturingCommand::ApplyMaterialProfile { .. } => "Apply captured material profile",
            ManufacturingCommand::Preflight { .. } => "Manufacturing preflight",
            ManufacturingCommand::Compile { .. } => "Compile manufacturing",
            ManufacturingCommand::Inspect { .. } => "Fresh actual-program verification",
            ManufacturingCommand::Get { .. } => "Manufacturing status",
            ManufacturingCommand::EvaluateInteriorPolicy { .. } => {
                "Evaluate interior policy · Current unchanged"
            }
            ManufacturingCommand::EvaluateAlternative { .. } => {
                "Evaluate alternative · Current unchanged"
            }
        };
        let applying = matches!(
            &command,
            ManufacturingCommand::SetIntent { .. }
                | ManufacturingCommand::EditInteriorPolicy { .. }
                | ManufacturingCommand::ApplyMaterialProfile { .. }
        );
        let material = matches!(&command, ManufacturingCommand::ApplyMaterialProfile { .. });
        let settings_apply = match &command {
            ManufacturingCommand::SetIntent {
                session_id,
                base_revision,
                ..
            } => Some((session_id.clone(), *base_revision, None)),
            ManufacturingCommand::EditInteriorPolicy {
                session_id,
                base_revision,
                edit,
                ..
            } => Some((session_id.clone(), *base_revision, Some(edit.clone()))),
            _ => None,
        };
        let mut settings_receipt = None;
        let review_request = match &command {
            ManufacturingCommand::ReviewMaterials {
                session_id,
                base_revision,
                request,
                ..
            } => Some((
                session_id.clone(),
                *base_revision,
                Arc::new(request.clone()),
            )),
            _ => None,
        };
        let result = self
            .client
            .as_ref()
            .expect("checked client")
            .manufacturing(command)
            .await;
        match result {
            Ok(ManufacturingResponse::OperationAccepted { operation }) => {
                self.accept(operation, Receipt::Manufacturing, label).await
            }
            Ok(ManufacturingResponse::Status { info, .. }) => {
                settings_receipt = Some(info);
                if applying {
                    Arc::make_mut(&mut self.state.manufacturing).loaded = None;
                    if material {
                        Arc::make_mut(&mut self.state.manufacturing).loaded_material = None;
                    }
                    self.state.message = format!("{label} completed");
                }
                self.refresh_after_failure().await;
            }
            Ok(ManufacturingResponse::MaterialReview { report }) => {
                if let Some((session, revision, request)) = review_request
                    && let Some(project) = &self.state.project
                    && self
                        .state
                        .hello
                        .as_ref()
                        .is_some_and(|h| h.session_id == session)
                    && project.revision == revision
                    && report.input_revision == revision
                    && report.query == request.query
                    && report.offset == request.offset
                {
                    Arc::make_mut(&mut self.state.manufacturing).material_review =
                        Some(Arc::new(manufacture::MaterialReview {
                            project: project.project_id.clone(),
                            session,
                            request,
                            report,
                        }));
                    self.state.message = "Material review ready; project unchanged".into();
                } else {
                    self.problem("Material review no longer matches the current project/request");
                }
            }
            Ok(ManufacturingResponse::Preflight { report }) => {
                Arc::make_mut(&mut self.state.manufacturing).preflight = Some(Arc::new(report))
            }
            Ok(ManufacturingResponse::Verified {
                record,
                report,
                resource,
                provenance,
                replay_display,
            }) => {
                self.verified(record, report, resource, provenance, replay_display)
                    .await
            }
            Ok(ManufacturingResponse::Error { error }) => self.problem(error.to_string()),
            Err(error) => self.failure(error).await,
        }
        if self.pending.is_none() {
            self.refresh_after_failure().await;
            if let Some((session, base_revision, interior_edit)) = settings_apply
                && let Some(info) = settings_receipt
            {
                self.settings_applied(session, base_revision, &info, interior_edit);
            }
        }
    }
    pub(super) async fn project_edit(&mut self, command: ProjectCommand) -> Option<ProjectInfo> {
        let process_edit = match &command {
            ProjectCommand::EditProcess {
                session_id,
                base_revision,
                edit,
            } => Some((session_id.clone(), *base_revision, edit.clone())),
            _ => None,
        };
        let plate_edit = match &command {
            ProjectCommand::EditPrintLayout {
                session_id,
                base_revision,
                edit,
            } => Some((session_id.clone(), *base_revision, edit.clone())),
            _ => None,
        };
        let settings_apply = match &command {
            ProjectCommand::ApplyManufacturingOption {
                session_id,
                base_revision,
                ..
            } => Some((session_id.clone(), *base_revision)),
            _ => None,
        };
        let option_metadata_edit = matches!(
            &command,
            ProjectCommand::RetainManufacturingOption { .. }
                | ProjectCommand::RemoveManufacturingOption { .. }
        );
        let option_request = match &command {
            ProjectCommand::GetManufacturingOption {
                session_id,
                base_revision,
                id,
                ..
            } => Some((session_id.clone(), *base_revision, id.clone())),
            _ => None,
        };
        if matches!(command, ProjectCommand::MatchStock { .. }) {
            self.state.stock = super::stock::State::default();
        }
        if !self.control_ready() {
            return None;
        }
        let mut receipt = None;
        match self
            .client
            .as_ref()
            .expect("checked client")
            .project(command)
            .await
        {
            Ok(ProjectResponse::OperationAccepted { operation }) => {
                self.accept(operation, Receipt::Project, "Project edit")
                    .await
            }
            Ok(ProjectResponse::Status { info }) => receipt = Some(info),
            Ok(ProjectResponse::ManufacturingOption { option }) => {
                if let Some((session, revision, id)) = option_request
                    && let Some(project) = &self.state.project
                    && project.revision == revision
                    && option.id == id
                    && self
                        .state
                        .hello
                        .as_ref()
                        .is_some_and(|hello| hello.session_id == session)
                {
                    Arc::make_mut(&mut self.state.manufacturing).inspected_option =
                        Some(Arc::new(manufacture::InspectedOption {
                            project: project.project_id.clone(),
                            revision,
                            session,
                            option,
                        }));
                }
            }
            Ok(ProjectResponse::SceneChanged { info, .. }) => receipt = Some(info),
            Ok(
                ProjectResponse::Filament { .. }
                | ProjectResponse::Filaments { .. }
                | ProjectResponse::Recipe { .. }
                | ProjectResponse::Recipes { .. },
            ) => self.problem("Unexpected material query response to project edit"),
            Ok(ProjectResponse::Error { error }) => self.problem(error.to_string()),
            Err(error) => self.failure(error).await,
        }
        if let (Some((session, base_revision, edit)), Some(info)) = (plate_edit, receipt.as_ref()) {
            self.nonce += 1;
            self.state.plate_edit = Some(Arc::new(PlateEditReceipt {
                nonce: self.nonce,
                session,
                project: info.project_id.clone(),
                base_revision,
                revision: info.revision,
                edit,
            }));
        }
        if let (Some((session, base_revision, edit)), Some(info)) = (process_edit, receipt.as_ref())
        {
            self.nonce += 1;
            self.state.process_edit = Some(Arc::new(ProcessEditReceipt {
                nonce: self.nonce,
                session,
                project: info.project_id.clone(),
                base_revision,
                revision: info.revision,
                edit,
            }));
        }
        if self.pending.is_none() {
            self.preserve_current_for_option_edit = option_metadata_edit && receipt.is_some();
            self.refresh_after_failure().await;
            self.preserve_current_for_option_edit = false;
            if let Some((session, base_revision)) = settings_apply
                && let Some(info) = &receipt
            {
                self.settings_applied(session, base_revision, info, None);
            }
        }
        receipt
    }
    pub(super) async fn verified(
        &mut self,
        record: ManufacturingArtifactRecord,
        report: VerificationReport,
        resource: ArtifactView,
        provenance: ManufacturingInspectionProvenance,
        replay: Option<ReplayDisplayDescriptor>,
    ) {
        if let Err(error) = self.refresh().await {
            self.failure(error).await;
            return;
        }
        let Some(client) = &self.client else {
            return;
        };
        self.nonce += 1;
        let capture = Arc::new(manufacture::Capture {
            nonce: self.nonce,
            layout_id: self
                .state
                .project
                .as_ref()
                .expect("refreshed project")
                .layout_id
                .clone(),
            session: client.hello().session_id.clone(),
            observed_revision: self
                .state
                .project
                .as_ref()
                .expect("refreshed project")
                .revision,
            record,
            report,
            resource,
            provenance,
            replay,
        });
        if let Err(error) = manufacture::check_current(&client.rpc(), &capture).await {
            self.failure(error).await;
            return;
        }
        let rpc = client.rpc();
        self.clear_review();
        Arc::make_mut(&mut self.state.manufacturing).capture = Some(capture.clone());
        self.review_task = Some(tokio::spawn(manufacture::load_review(
            rpc,
            capture,
            self.state.display.as_ref().map_or(0, |d| d.raw_bytes * 2)
                + self.state.section.as_ref().map_or(0, |s| {
                    s.summary.total_bytes as usize * crate::scene::SECTION_DISPLAY_MULTIPLIER
                })
                + self.state.manufacturing.display_bytes()
                + crate::scene::CONTEXT_DISPLAY_RESERVE_BYTES
                + if self.section_task.is_some() {
                    spiling_contracts::geometry::MAX_SECTION_BYTES as usize
                        * crate::scene::SECTION_DISPLAY_MULTIPLIER
                } else {
                    0
                }
                + 16 * 1024 * 1024,
            self.state
                .scene
                .as_ref()
                .and_then(|s| s.bounds_mm)
                .map_or(glam::DVec3::ZERO, |b| {
                    (glam::DVec3::from_array(b.min) + glam::DVec3::from_array(b.max)) * 0.5
                }),
        )));
    }
    pub(super) async fn file_action(&mut self, action: Action) {
        match action {
            Action::LoadAnalysisModel(path) => {
                match tokio::task::spawn_blocking(move || crate::mechanics::read_model(&path)).await
                {
                    Ok(Ok(settings)) => {
                        self.state.analysis_model = Some(Arc::new(settings));
                        self.state.message =
                            "Analysis model selected; Run evaluates the applied case".into();
                    }
                    Ok(Err(error)) => self.problem(error),
                    Err(error) => self.problem(error.to_string()),
                }
            }
            Action::LoadIntent(path) => {
                match tokio::task::spawn_blocking(move || manufacture::read_intent(&path)).await {
                    Ok(Ok((intent, label))) => {
                        self.nonce += 1;
                        Arc::make_mut(&mut self.state.manufacturing).loaded =
                            Some(Arc::new(manufacture::LoadedIntent {
                                nonce: self.nonce,
                                label,
                                intent,
                            }));
                        self.state.message =
                            "Intent loaded for review; Apply commits the full declaration".into();
                    }
                    Ok(Err(error)) => self.problem(error),
                    Err(error) => self.problem(error.to_string()),
                }
            }
            Action::SetLibraryLocation(path) => {
                self.library_action(LibraryAction::SetLocation(path)).await;
            }
            Action::LoadMaterial(path) => {
                let Some(project) = self.state.project.clone() else {
                    return;
                };
                let Some(hello) = self.state.hello.clone() else {
                    return;
                };
                match tokio::task::spawn_blocking(move || manufacture::read_material(&path)).await {
                    Ok(Ok((selection, label))) => {
                        self.nonce += 1;
                        Arc::make_mut(&mut self.state.manufacturing).loaded_material =
                            Some(Arc::new(manufacture::LoadedMaterial {
                                nonce: self.nonce,
                                label,
                                project: project.project_id.clone(),
                                revision: project.revision,
                                session: hello.session_id.clone(),
                                selection,
                            }));
                        self.state.message =
                            "Material profile loaded for review; Apply commits through the engine"
                                .into();
                    }
                    Ok(Err(error)) => self.problem(error),
                    Err(error) => self.problem(error.to_string()),
                }
            }
            Action::LoadMaterialCatalog(path) => {
                let Some(project) = self.state.project.clone() else {
                    return;
                };
                let Some(hello) = self.state.hello.clone() else {
                    return;
                };
                match tokio::task::spawn_blocking(move || manufacture::read_catalog(&path)).await {
                    Ok(Ok((catalog, label))) => {
                        self.nonce += 1;
                        let m = Arc::make_mut(&mut self.state.manufacturing);
                        m.loaded_catalog = Some(Arc::new(manufacture::LoadedCatalog {
                            nonce: self.nonce,
                            label,
                            project: project.project_id.clone(),
                            session: hello.session_id.clone(),
                            catalog,
                        }));
                        m.material_review = None;
                        self.state.message =
                            "Catalog loaded; Search reviews declarations through the engine".into();
                    }
                    Ok(Err(error)) => self.problem(error),
                    Err(error) => self.problem(error.to_string()),
                }
            }
            Action::Export(path) => {
                let Some(review) = self
                    .state
                    .manufacturing
                    .review
                    .clone()
                    .filter(|r| self.capture_current(&r.capture))
                else {
                    self.problem(
                        "Freshly verify and retrieve the current actual output before export",
                    );
                    return;
                };
                let Some(client) = &self.client else {
                    return;
                };
                self.export_task = Some(tokio::spawn(manufacture::stage_export(
                    client.rpc(),
                    review,
                    path,
                )));
            }
            _ => unreachable!(),
        }
    }
}
