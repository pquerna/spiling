// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::analysis::mechanics::AnalysisRunSettings;

fn settings_match(input: &str, captured: &AnalysisRunSettings) -> bool {
    input.len() <= 48 * 1024
        && serde_json::from_str::<AnalysisRunSettings>(input)
            .is_ok_and(|settings| settings == *captured)
}

impl Workbench {
    fn mechanics_capture(&self) -> Option<std::sync::Arc<crate::assets::MechanicsCapture>> {
        self.state.mechanics.clone().filter(|capture| {
            self.state
                .project
                .as_ref()
                .is_some_and(|project| project.project_id == capture.project)
                && self
                    .mechanics
                    .draft
                    .as_ref()
                    .is_none_or(|draft| draft.value.id == capture.summary.case.case.id)
        })
    }

    fn mechanics_current(&self, capture: &crate::assets::MechanicsCapture) -> bool {
        !self.mechanics.has_changes()
            && self.display_current()
            && settings_match(&self.mechanics.settings_json, &capture.summary.settings)
            && self.state.project.as_ref().is_some_and(|project| {
                project.layout_id == capture.summary.case.case.layout_id
                    && project.analysis.cases.contains(&capture.summary.case)
                    && project.surface_overlays.layers.is_empty()
                    && project.process_document.layers.is_empty()
            })
            && self.state.display.as_ref().is_some_and(|scene| {
                scene.occurrences.iter().any(|occurrence| {
                    occurrence.occurrence_id == capture.summary.case.case.occurrence_id
                        && occurrence.definition_id == capture.summary.case.case.definition_id
                        && occurrence.pose == capture.summary.pose
                })
            })
    }

    pub(in crate::app) fn mechanics_view(&self) -> Option<crate::viewport::mechanics::Input> {
        if self.work_area.workspace != Workspace::Analyze || !self.display_current() {
            return None;
        }
        let capture = self.mechanics_capture();
        let case = self
            .mechanics
            .draft
            .as_ref()
            .map(|draft| draft.value.clone())
            .or_else(|| {
                capture
                    .as_ref()
                    .map(|capture| capture.summary.case.case.clone())
            })?;
        let stale = capture
            .as_ref()
            .is_some_and(|capture| !self.mechanics_current(capture));
        let selected = self
            .selected
            .as_ref()
            .filter(|face| {
                face.occurrence_id == case.occurrence_id
                    && face.definition_id == case.definition_id
                    && self.state.display.as_ref().is_some_and(|scene| {
                        face.session_id == scene.summary.session_id
                            && face.scene_revision == scene.summary.revision
                    })
            })
            .map(|face| face.face_id.clone());
        Some(crate::viewport::mechanics::Input {
            editing: if self.mechanics.tool.armed {
                None
            } else {
                self.force_canvas()
                    .and_then(|canvas| canvas.active)
                    .filter(|contact| glam::DVec3::from_array(contact.load.force_n).length() > 0.0)
                    .map(|contact| contact.load.face_id)
            },
            contacts: self.state.analysis_faces.clone(),
            capture,
            case,
            selected,
            field: self.mechanics.field,
            stale,
        })
    }

    pub(in crate::app) fn mechanics_actions(&mut self, ui: &mut egui::Ui) {
        let apply_requested = std::mem::take(&mut self.mechanics.apply_requested);
        ui.heading("Analyze");
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.awaiting_serial.is_none()
            && self.state.confirmation.is_none();
        let direction_error = self
            .mechanics
            .controls
            .values()
            .find_map(|control| control.error.clone());
        let Some(draft) = &mut self.mechanics.draft else {
            ui.weak("Select a saved case or create one.");
            return;
        };
        let stale = self.state.project.as_ref().is_none_or(|project| {
            project.project_id != draft.project || project.revision != draft.base
        }) || self
            .state
            .hello
            .as_ref()
            .is_none_or(|hello| hello.session_id != draft.session)
            || self
                .state
                .project
                .as_ref()
                .is_some_and(|project| project.layout_id != draft.value.layout_id);
        if draft.dirty() || draft.submitted.is_some() || direction_error.is_some() {
            let validation = draft.value.validate().err().map(|error| error.to_string());
            let reason = if !ready || draft.submitted.is_some() {
                Some("Wait for the current operation")
            } else if stale {
                Some("Project changed; discard and reselect the case")
            } else if self
                .state
                .project
                .as_ref()
                .is_none_or(|project| project.read_only)
            {
                Some("Read-only project")
            } else {
                direction_error.as_deref().or(validation.as_deref())
            };
            if ui::primary(ui, "Apply case", reason) || (apply_requested && reason.is_none()) {
                draft.submitted = Some(self.state.serial);
                self.queued_project = Some(ProjectCommand::EditAnalysis {
                    session_id: draft.session.clone(),
                    base_revision: draft.base,
                    edit: AnalysisEdit::Put {
                        case: draft.value.clone(),
                    },
                });
            }
        } else {
            let parsed = serde_json::from_str::<AnalysisRunSettings>(&self.mechanics.settings_json)
                .ok()
                .filter(|settings| settings.validate().is_ok());
            if parsed.is_none() {
                self.primary_button(ui, Action::LoadAnalysisModel);
                ui.separator();
                return;
            }
            let reason = if !ready {
                Some("Wait for the current operation")
            } else if stale {
                Some("Project changed; reselect the case")
            } else if parsed.is_none() {
                Some("Choose a compatible assumed-material model")
            } else {
                None
            };
            if ui::primary(ui, "Run analysis", reason) {
                self.queued_project = Some(ProjectCommand::RunAnalysis {
                    session_id: draft.session.clone(),
                    base_revision: draft.base,
                    case_id: draft.value.id.clone(),
                    settings: std::sync::Arc::new(parsed.unwrap()),
                });
            }
        }
        ui.separator();
    }

    pub(super) fn mechanics_results(&mut self, ui: &mut egui::Ui, ready: bool) {
        if let Some(capture) = self.mechanics_capture() {
            let current = self.mechanics_current(&capture);
            let summary = &capture.summary;
            ui.horizontal(|ui| {
                ui.strong(if current { "Result" } else { "Stale result" });
                ui.weak("Assumed materials");
            });
            egui::Grid::new("mechanics-metrics")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Max deflection");
                    ui.label(format!("{:.4} mm", summary.maximum_displacement_mm));
                    ui.end_row();
                    ui.label("Sampled macro stress");
                    ui.label(format!("{:.3} MPa", summary.maximum_macro_von_mises_mpa));
                    ui.end_row();
                });
            egui::ComboBox::from_id_salt("mechanical-field")
                .selected_text(crate::mechanics::FIELD_NAMES[self.mechanics.field.min(3)])
                .show_ui(ui, |ui| {
                    for (index, name) in crate::mechanics::FIELD_NAMES.iter().enumerate() {
                        ui.selectable_value(&mut self.mechanics.field, index, *name);
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("Evidence…").clicked() {
                    self.mechanics.evidence_open = true;
                }
                let attached = self.state.project.as_ref().is_some_and(|project| {
                    project
                        .analysis
                        .results
                        .iter()
                        .any(|result| result.hash.as_str() == capture.resource.sha256)
                });
                if attached {
                    ui.weak("Attached");
                } else if ui
                    .add_enabled(
                        ready
                            && current
                            && self.state.project.as_ref().is_some_and(|project| {
                                !project.read_only && project.revision == summary.captured_revision
                            }),
                        egui::Button::new("Attach result"),
                    )
                    .clicked()
                    && let (Some(project), Some(hello)) = (&self.state.project, &self.state.hello)
                {
                    self.queued_project = Some(ProjectCommand::AttachAnalysisResult {
                        session_id: hello.session_id.clone(),
                        base_revision: project.revision,
                        input_hash: summary.input_hash.clone(),
                    });
                }
            });
            let mut open = self.mechanics.evidence_open;
            egui::Window::new("Analysis evidence").open(&mut open).default_width(500.0)
                .max_height(540.0).vscroll(true).show(ui.ctx(), |ui| {
                    ui.strong("Assumed directional wall / skin / effective core");
                    ui.label("Tied interfaces. Not calibrated strength, resolved roads or qualified Basic/Enhanced. Algebraic checks do not establish discretization or material accuracy.");
                    ui.label(format!("{} cells · {} DOFs · CPU FP64", summary.elements, summary.dofs));
                    ui.label("The 3D surface displays constant cell-centroid values on the exterior finite-element faces. No stress smoothing, road maxima or deformed geometry is inferred. Hover a cell to inspect its sample. Layer-normal stress is signed.");
                    ui.label(format!("Concentration radius {:.3} mm; nominal deposited-volume normalization", summary.concentration_radius_mm));
                    ui.label(format!("Residual {:.3e} N · force balance {:.3e} N · moment balance {:.3e} N·mm", summary.assessment.free_residual_n, summary.assessment.force_imbalance_n, summary.assessment.moment_imbalance_n_mm));
                    ui.label(format!("Work/energy relative error {:.3e}", summary.assessment.work_energy_relative_error));
                    let costs = &summary.costs;
                    ui.label(format!("Capture {:.2} ms · construction {:.2} ms · setup {:.2} ms", costs.native_capture_ms, costs.construction_ms, costs.operator_setup_ms));
                    ui.label(format!("RHS {:.2} ms · solve {:.2} ms · assessment {:.2} ms · fields {:.2} ms · reuse {}", costs.rhs_ms, costs.solve_ms, costs.assessment_ms, costs.fields_ms, costs.operator_reused));
                    ui.label("Engine phases only; not total UI latency or a performance cohort.");
                    ui.label(format!("Region volumes {:?} mm³ · energies {:?} N·mm", summary.region_volume_mm3, summary.region_energy_n_mm));
                    ui.label(format!("Native shape allowance {:.6} mm", summary.native_shape_error_mm));
                    ui.label(&summary.settings.laws.name); ui.label(&summary.settings.laws.justification);
                    ui.monospace(format!("input {}", summary.input_hash.as_str()));
                    ui.monospace(format!("operator {}", summary.operator_hash.as_str()));
                    ui.label("Attach, then Save retains these fields and assumptions in .spil. Reopen restores without solving; changed inputs retain stale evidence.");
                    if let Some(previous) = self.state.mechanics_previous.as_ref().filter(|previous| previous.project == capture.project) {
                        ui.separator(); ui.label("Previous result (check captured loads/model before comparing)");
                        ui.label(format!("Deflection {:.4} → {:.4} mm", previous.summary.maximum_displacement_mm, summary.maximum_displacement_mm));
                    }
                });
            self.mechanics.evidence_open = open;
        } else {
            ui.weak("Not evaluated");
        }
        ui.separator();
    }

    pub(super) fn mechanics_model(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Model", |ui| {
            if let Ok(mut settings) =
                serde_json::from_str::<AnalysisRunSettings>(&self.mechanics.settings_json)
            {
                ui.add(egui::Label::new(&settings.laws.name).truncate());
                ui.label(format!(
                    "{} walls · {:.0}% infill · {:.2} mm layers",
                    settings.recipe.perimeter_count,
                    settings.recipe.infill_fraction * 100.0,
                    settings.recipe.layer_height_mm
                ));
                if ui::number(
                    ui,
                    "Max cell span (mm)",
                    &mut settings.maximum_span_mm,
                    0.25,
                )
                .changed()
                {
                    self.mechanics.settings_json = serde_json::to_string_pretty(&settings).unwrap();
                    self.mechanics.settings_initialized = true;
                }
            } else {
                ui.weak("No valid model selected");
            }
            self.button(ui, Action::LoadAnalysisModel);
        });
        ui.collapsing("Advanced model data", |ui| {
            if ui
                .add(
                    egui::TextEdit::multiline(&mut self.mechanics.settings_json)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                self.mechanics.settings_initialized = true;
            }
            if self.mechanics.settings_json.len() > 48 * 1024 {
                ui.colored_label(ui.visuals().error_fg_color, "Model exceeds 48 KiB");
            }
        });
    }
}
