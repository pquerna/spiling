// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    manufacturing::*,
    optical::OpticalSelection,
    project::{ProjectCommand, ProjectId, ProjectRevision},
};
#[derive(Default)]
pub(super) struct StudyUi {
    project: Option<ProjectId>,
    revision: Option<ProjectRevision>,
    selection: Option<OpticalSelection>,
    bindings: [Option<usize>; 2],
    construction: Option<usize>,
}
impl Workbench {
    pub(super) fn study_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Color study");
            if ui::close(ui, "Back to workbench") {
                self.manufacturing.panel = 0;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.stock.matching, true, "Match target");
            ui.selectable_value(&mut self.stock.matching, false, "Construction study");
        });
        ui.separator();
        if self.stock.matching {
            self.stock_page(ui);
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("study-page")
            .show(ui, |ui| self.study_ui(ui));
    }
    pub(super) fn study_ui(&mut self, ui: &mut egui::Ui) {
        ui.set_max_width(ui.available_width().min(850.0));
        ui.collapsing("Color reference", |ui| self.colors.show(ui));
        ui.separator();
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.awaiting_serial.is_none();
        let writable = ready && !project.read_only;
        if self.study.project.as_ref() != Some(&project.project_id)
            || self.study.revision != Some(project.revision)
        {
            self.study = StudyUi {
                project: Some(project.project_id.clone()),
                revision: Some(project.revision),
                selection: project
                    .appearance_study
                    .optical
                    .as_ref()
                    .and_then(|s| s.selection),
                ..Default::default()
            };
        }
        ui::pane_header(
            ui,
            "Reference study",
            "Project · estimates, not calibrated color",
        );
        ui.label("Physical color is unqualified");
        ui::help(
            ui,
            "reference-scope",
            "Reference estimates",
            "Backing optics and calibration are unmodelled. Manufacturing independently admits every construction; accepting a reference estimate does not qualify physical color.",
        );
        let Some(study) = &project.appearance_study.optical else {
            ui::pane_header(
                ui,
                "No color study",
                "Choose a Project recipe and set its study context",
            );
            if ui::primary(ui, "Choose recipe", None) {
                self.choose_recipe();
            }
            return;
        };
        ui.label(format!(
            "{} material declarations",
            study.request.materials.len()
        ));
        ui.collapsing("Study inputs", |ui| {
            ui.add(
                egui::Label::new(format!("{:#?}", study.request))
                    .wrap()
                    .selectable(true),
            );
        });
        ui.label(format!(
            "Requested patch: {:.3} × {:.3} mm",
            study.request.patch.width_mm, study.request.patch.height_mm
        ));
        let has_report = self
            .state
            .optical_report
            .as_ref()
            .is_some_and(|c| c.is_current(&project));
        if !has_report {
            if study.report.is_some() {
                if ui::primary(
                    ui,
                    "Load report",
                    (!ready || self.state.optical_loading)
                        .then_some("Wait for the current operation"),
                ) {
                    self.queued_project = Some(ProjectCommand::GetOpticalReport {
                        session_id: hello.session_id.clone(),
                        base_revision: project.revision,
                    });
                }
            } else if ui::primary(
                ui,
                "Compare",
                (!writable).then_some("Open a writable project and wait for pending operations"),
            ) {
                self.queued_project = Some(ProjectCommand::CompareOptical {
                    session_id: hello.session_id.clone(),
                    base_revision: project.revision,
                });
            }
        } else if ui
            .add_enabled(writable, egui::Button::new("Compare again"))
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
        if !capture.is_current(&project) {
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
        super::review::target(ui, capture.report.request.target.color);
        super::review::assessment(ui, "Requested cycle", &capture.report.requested);
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
        if let Some(OpticalSelection::Candidate { index }) = self.study.selection
            && let Some(visit) = capture
                .report
                .visits
                .iter()
                .find(|visit| visit.index == index)
        {
            ui.push_id(("selected-candidate", index), |ui| {
                super::review::assessment(ui, "Selected candidate", &visit.assessment);
            });
        }
        let accepted = study.selection.is_some()
            && study.selection == self.study.selection
            && study.report.as_ref() == Some(&capture.record);
        let reason = if !writable {
            Some("Open a writable project and wait for pending operations")
        } else if !capture.record.complete {
            Some("This report is incomplete. Compare again before accepting.")
        } else if !self
            .study
            .selection
            .as_ref()
            .is_some_and(|s| capture.report.has_selection(s))
        {
            Some("Choose a reported cycle")
        } else {
            None
        };
        if !accepted && ui::primary(ui, "Accept selection", reason) {
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
            .zip(
                study
                    .selection
                    .filter(|_| study.report.as_ref() == Some(&capture.record)),
            )
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
        let reason = if !writable {
            Some("Open a writable project and wait for pending operations")
        } else if !application.as_ref().is_some_and(|a| a.validate().is_ok()) {
            Some("Choose two distinct loaded materials and an existing construction")
        } else {
            None
        };
        if accepted && ui::primary(ui, "Apply construction", reason) {
            self.queued_project = Some(ProjectCommand::ApplyOpticalConstruction {
                session_id: hello.session_id.clone(),
                base_revision: project.revision,
                application: application.expect("checked explicit binding"),
            });
        }
    }
}
