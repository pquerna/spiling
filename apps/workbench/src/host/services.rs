// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{ArtifactView, manufacturing::*};

impl Actor {
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
            ManufacturingCommand::SetIntent { .. } => "Apply full manufacturing intent",
            ManufacturingCommand::ApplyMaterialProfile { .. } => "Apply captured material profile",
            ManufacturingCommand::Preflight { .. } => "Manufacturing preflight",
            ManufacturingCommand::Compile { .. } => "Compile manufacturing",
            ManufacturingCommand::Inspect { .. } => "Fresh actual-program verification",
            ManufacturingCommand::Get { .. } => "Manufacturing status",
        };
        let applying = matches!(
            &command,
            ManufacturingCommand::SetIntent { .. }
                | ManufacturingCommand::ApplyMaterialProfile { .. }
        );
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
            Ok(ManufacturingResponse::Status { .. }) => {
                if applying {
                    Arc::make_mut(&mut self.state.manufacturing).loaded = None;
                    self.state.message = format!("{label} completed");
                }
                self.refresh_after_failure().await;
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
        }
    }
    pub(super) async fn project_edit(&mut self, command: ProjectCommand) {
        if !self.control_ready() {
            return;
        }
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
            Ok(ProjectResponse::SceneChanged { .. } | ProjectResponse::Status { .. }) => {}
            Ok(ProjectResponse::Error { error }) => self.problem(error.to_string()),
            Err(error) => self.failure(error).await,
        }
        if self.pending.is_none() {
            self.refresh_after_failure().await;
        }
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
                + self
                    .state
                    .section
                    .as_ref()
                    .map_or(0, |s| s.summary.total_bytes as usize * 6)
                + self.state.manufacturing.display_bytes()
                + if self.section_task.is_some() {
                    spiling_contracts::geometry::MAX_SECTION_BYTES as usize * 6
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
