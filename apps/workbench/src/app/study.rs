// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    manufacturing::*,
    optical::OpticalSelection,
    project::{ProjectCommand, ProjectId},
};
#[derive(Default)]
pub(super) struct StudyUi {
    project: Option<ProjectId>,
    selection: Option<OpticalSelection>,
    bindings: [Option<usize>; 2],
    construction: Option<usize>,
}
impl Workbench {
    pub(super) fn study_ui(&mut self, ui: &mut egui::Ui) {
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.awaiting_serial.is_none();
        let writable = ready && !project.read_only;
        if self.study.project.as_ref() != Some(&project.project_id) {
            self.study = StudyUi {
                project: Some(project.project_id.clone()),
                ..Default::default()
            };
        }
        ui.heading("Retained optical study");
        ui.weak("Named reference estimates. Backing optics, calibration and physical color remain unqualified. Manufacturing independently admits every construction.");
        let Some(study) = &project.appearance_study.optical else {
            ui.label("No retained optical study in this project. Studies and palette authoring are available through the shared CLI.");
            return;
        };
        ui.label(format!(
            "Materials: {}",
            study.request.materials.join(" / ")
        ));
        ui.label(format!(
            "Requested patch: {:.3} × {:.3} mm",
            study.request.patch.width_mm, study.request.patch.height_mm
        ));
        if ui
            .add_enabled(
                ready && !self.state.optical_loading,
                egui::Button::new("Load retained reference report"),
            )
            .clicked()
        {
            self.queued_project = Some(ProjectCommand::GetOpticalReport {
                session_id: hello.session_id.clone(),
                base_revision: project.revision,
            });
        }
        if ui
            .add_enabled(writable, egui::Button::new("Recompare current study"))
            .clicked()
        {
            self.queued_project = Some(ProjectCommand::CompareOptical {
                session_id: hello.session_id.clone(),
                base_revision: project.revision,
            });
        }
        if self.state.optical_loading {
            ui.spinner();
        }
        let Some(capture) = &self.state.optical_report else {
            return;
        };
        if capture.project != project.project_id || study.report.as_ref() != Some(&capture.record) {
            return;
        }
        let status = match capture.report.status {
            spiling_contracts::appearance::AppearanceStatus::Complete { outcome } => {
                format!("Traversal complete · {outcome:?}")
            }
            spiling_contracts::appearance::AppearanceStatus::WorkLimit => {
                "Work limit reached · incomplete".into()
            }
            spiling_contracts::appearance::AppearanceStatus::Cancelled => {
                "Cancelled · incomplete".into()
            }
        };
        ui.label(format!(
            "{status} · {} candidates · {} retained visits",
            capture.report.total_candidates,
            capture.report.visits.len()
        ));
        super::review::assessment(ui, "Requested reference", &capture.report.requested);
        ui.selectable_value(
            &mut self.study.selection,
            Some(OpticalSelection::Requested),
            "Select requested cycle",
        );
        egui::ScrollArea::vertical()
            .id_salt("retained alternatives")
            .max_height(180.0)
            .show_rows(ui, 26.0, capture.report.visits.len(), |ui, range| {
                for index in range {
                    let visit = &capture.report.visits[index];
                    let selection = OpticalSelection::Candidate { index: visit.index };
                    let label = format!(
                        "Candidate {} · {}",
                        visit.index,
                        super::review::assessment_label(&visit.assessment)
                    );
                    if ui
                        .add_sized(
                            [ui.available_width(), 24.0],
                            egui::Button::selectable(
                                self.study.selection == Some(selection),
                                label,
                            )
                            .truncate(),
                        )
                        .clicked()
                    {
                        self.study.selection = Some(selection);
                    }
                }
            });
        if ui
            .add_enabled(
                writable
                    && capture.record.complete
                    && self
                        .study
                        .selection
                        .as_ref()
                        .is_some_and(|s| capture.report.has_selection(s))
                    && study.selection != self.study.selection,
                egui::Button::new("Accept selected complete report"),
            )
            .clicked()
        {
            self.queued_project = Some(ProjectCommand::AcceptOpticalReport {
                session_id: hello.session_id.clone(),
                base_revision: project.revision,
                report_hash: capture.record.hash.clone(),
                selection: self.study.selection,
            });
        }
        ui.separator();
        let Some(intent) = &self.state.manufacturing.intent else {
            return;
        };
        let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
            ui.weak("Applying this selection requires an existing fixed-dual source construction and explicit loaded declarations.");
            return;
        };
        if study.request.materials.len() != 2 || capture.report.material_evidence_hashes.len() != 2
        {
            return;
        }
        for i in 0..2 {
            egui::ComboBox::from_id_salt(("loaded material binding", i))
                .selected_text(
                    self.study.bindings[i]
                        .and_then(|i| job.tools.get(i))
                        .map_or("Choose loaded material", |t| t.material.id.as_str()),
                )
                .show_ui(ui, |ui| {
                    ui.label(format!("Evidence {}", study.request.materials[i]));
                    for (index, tool) in job.tools.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.study.bindings[i],
                            Some(index),
                            format!(
                                "{} · declaration {}",
                                tool.material.id,
                                tool.material.declaration_hash.as_str()
                            ),
                        );
                    }
                });
        }
        egui::ComboBox::from_id_salt("existing source construction")
            .selected_text(
                self.study
                    .construction
                    .map_or("Choose existing construction".into(), |i| {
                        format!("Construction {i}")
                    }),
            )
            .show_ui(ui, |ui| {
                for (index, construction) in job.surface_constructions.iter().enumerate() {
                    ui.selectable_value(
                        &mut self.study.construction,
                        Some(index),
                        format!(
                            "Construction {index} · occurrence {}",
                            construction.occurrence_id.get()
                        ),
                    );
                }
            });
        let application = self.study.bindings[0]
            .zip(self.study.bindings[1])
            .filter(|(a, b)| a != b)
            .zip(
                self.study
                    .construction
                    .filter(|i| *i < job.surface_constructions.len()),
            )
            .zip(study.selection)
            .map(|(((a, b), index), selection)| ApplyOpticalConstruction {
                construction_index: index as u16,
                report_hash: capture.record.hash.clone(),
                selection,
                bindings: std::array::from_fn(|i| {
                    let tool = &job.tools[[a, b][i]];
                    OpticalMaterialBinding {
                        evidence_key: study.request.materials[i].clone(),
                        evidence_hash: capture.report.material_evidence_hashes[i].clone(),
                        loaded_material_id: tool.material.id.clone(),
                        loaded_declaration_hash: tool.material.declaration_hash.clone(),
                    }
                }),
            });
        if ui
            .add_enabled(
                writable && application.as_ref().is_some_and(|a| a.validate().is_ok()),
                egui::Button::new("Apply retained selection to source construction"),
            )
            .clicked()
        {
            self.queued_project = Some(ProjectCommand::ApplyOpticalConstruction {
                session_id: hello.session_id.clone(),
                base_revision: project.revision,
                application: application.expect("checked explicit binding"),
            });
        }
    }
}
