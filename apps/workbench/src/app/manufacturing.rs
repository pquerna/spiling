// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    manufacturing::*,
    project::{ProjectId, ProjectRevision},
};
use std::sync::Arc;

const BEAD_MODEL: &str = "Nominal width and physical height over decoded straight strokes. Stadium sections; bridges use elliptical envelopes without sag prediction. Flat ends preserve breaks; intersecting strokes overlap. Numerical projection ≤ 0.001 mm; section chord error ≤ 0.001 mm. Large reviews retain centerlines when the bead view reaches display limits.";

pub(super) struct IntentDraft {
    pub(super) project: ProjectId,
    pub(super) revision: ProjectRevision,
    nonce: u64,
    original: Arc<ManufacturingIntent>,
    pub(super) value: ManufacturingIntent,
}
impl IntentDraft {
    pub(super) fn apply_command(
        &mut self,
        layout_id: spiling_contracts::project::PrintLayoutId,
        session_id: SessionId,
        revision: ProjectRevision,
    ) -> Result<ManufacturingCommand, String> {
        if self.revision != revision {
            return Err("Project changed. Discard this draft and reload current settings.".into());
        }
        let value = super::policies::prepared(&self.value)?;
        self.value = value.clone();
        Ok(ManufacturingCommand::SetIntent {
            layout_id,
            session_id,
            base_revision: self.revision,
            intent: Arc::new(value),
        })
    }
}
struct BeadStatistics {
    display: std::sync::Weak<crate::replay::Replay>,
    nonce: u64,
    view: crate::replay::View,
    value: Option<crate::replay::beads::Statistics>,
}
#[derive(Default)]
pub(super) struct ManufacturingUi {
    pub(super) draft: Option<IntentDraft>,
    ignored_nonce: Option<u64>,
    pub(super) prepare_section: u8,
    pub(super) replay_record: Option<SourceHash>,
    pub replay: crate::replay::View,
    pub layers: crate::layers::Browser,
    pub review_nonce: Option<u64>,
    support_inspection: super::review::SupportInspection,
    bead_statistics: Option<BeadStatistics>,
    selected_seam: Option<u32>,
    pub(super) trial: Option<super::alternative::Trial>,
    pub(super) inspected_option: Option<spiling_contracts::geometry::SourceHash>,
    pub(super) comparison_no_height_match: bool,
}
impl ManufacturingUi {
    fn sync_replay_review(&mut self, review: &crate::manufacture::Review) {
        let Some(descriptor) = &review.capture.replay else {
            return;
        };
        if self.review_nonce != Some(review.capture.nonce) {
            self.review_nonce = Some(review.capture.nonce);
            if self.replay_record.as_ref() != Some(&review.capture.record.hash) {
                self.support_inspection =
                    super::review::SupportInspection::from_layers(&review.bundle.plan.layers);
                self.replay.events = descriptor.info.event_count;
                self.replay.layer = None;
                self.replay.layer_end = None;
                self.replay.role = None;
                self.replay.tool = None;
            }
            self.replay_record = Some(review.capture.record.hash.clone());
            self.selected_seam = None;
        }
    }

    pub(super) fn sync_intent(
        &mut self,
        project: &ProjectId,
        revision: ProjectRevision,
        current: Option<Arc<ManufacturingIntent>>,
        loaded: Option<&crate::manufacture::LoadedIntent>,
    ) {
        let (source, nonce) = loaded
            .filter(|l| self.ignored_nonce != Some(l.nonce))
            .map_or((current, 0), |l| (Some(l.intent.clone()), l.nonce));
        let Some(source) = source else {
            if self.draft.as_ref().is_some_and(|d| &d.project != project) {
                self.draft = None;
            }
            return;
        };
        let reset = self.draft.as_ref().is_none_or(|d| {
            &d.project != project
                || d.nonce != nonce
                || (d.revision != revision
                    && (d.value == *d.original
                        || super::policies::prepared(&d.value).is_ok_and(|v| v == *source)))
        });
        if reset {
            self.draft = Some(IntentDraft {
                project: project.clone(),
                revision,
                nonce,
                original: source.clone(),
                value: (*source).clone(),
            });
        }
    }
    pub(super) fn pending_loaded(&self, loaded: Option<&crate::manufacture::LoadedIntent>) -> bool {
        loaded.is_some_and(|l| self.ignored_nonce != Some(l.nonce))
    }

    pub(super) fn has_intent_draft(
        &self,
        project: &ProjectId,
        current: Option<&ManufacturingIntent>,
    ) -> bool {
        self.draft.as_ref().is_some_and(|d| {
            &d.project == project && d.value != *d.original && current != Some(&d.value)
        })
    }
}
impl Workbench {
    pub(super) fn show_prepare(&mut self, section: u8) {
        self.show_panel(Panel::Prepare);
        self.manufacturing.prepare_section = section;
    }

    pub(super) fn discard_preparation_drafts(&mut self) {
        self.manufacturing.ignored_nonce =
            self.state.manufacturing.loaded.as_ref().map(|l| l.nonce);
        self.manufacturing.draft = None;
        self.manufacturing.trial = None;
    }

    pub(super) fn manufacturing_current(&self) -> bool {
        self.state.phase == Phase::Running
            && self
                .state
                .hello
                .as_ref()
                .zip(self.state.project.as_ref())
                .is_some_and(|(h, p)| {
                    self.state
                        .manufacturing
                        .current_capture(p, &h.session_id)
                        .is_some()
                })
    }
    pub(super) fn manufacturing_verified(&self) -> bool {
        self.manufacturing_current()
            && self
                .state
                .manufacturing
                .capture
                .as_ref()
                .is_some_and(|capture| capture.report.verified)
    }

    pub(super) fn manufacturing_pane(&mut self, ui: &mut egui::Ui) {
        if self.work_area.panel == Panel::Alternative {
            self.alternative_pane(ui);
            return;
        }
        if self.work_area.panel == Panel::Option {
            self.option_pane(ui);
            return;
        }
        let prepare = self.work_area.panel == Panel::Prepare;
        ui::pane_header(
            ui,
            if prepare {
                [
                    "Printer",
                    "Material",
                    "Settings",
                    "Position",
                    "Infill modifiers",
                ][usize::from(self.manufacturing.prepare_section.min(4))]
            } else {
                "Review layers"
            },
            &self
                .state
                .project
                .as_ref()
                .and_then(|p| p.layouts.iter().find(|l| l.layout_id == p.layout_id))
                .map_or_else(
                    || "Selected plate".into(),
                    |l| {
                        format!(
                            "{} · {} {}",
                            l.name,
                            l.occurrence_count,
                            if l.occurrence_count == 1 {
                                "part"
                            } else {
                                "parts"
                            }
                        )
                    },
                ),
        );
        self.manufacturing_actions(ui);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt((
                "manufacturing-body",
                self.work_area.panel,
                self.manufacturing.prepare_section,
            ))
            .show(ui, |ui| self.manufacturing_ui(ui));
    }
    fn manufacturing_actions(&mut self, ui: &mut egui::Ui) {
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        let m = self.state.manufacturing.clone();
        if self.work_area.panel == Panel::Prepare
            && matches!(self.manufacturing.prepare_section, 1 | 2)
            && self.manufacturing.trial.is_some()
        {
            ui.label("Current drafts retained · alternative open");
            return;
        }
        if self.work_area.panel == Panel::Review {
            if self.slicing() {
                ui.weak("Slicing…");
            } else if self.manufacturing_verified() {
                self.primary_button(ui, Action::Export);
                ui::verified(ui);
            } else if m.loading {
                ui.weak("Loading output…");
            } else if m.artifact.is_some() {
                self.primary_button(ui, Action::Verify);
            } else {
                ui.label("Slice the plate in Prepare.");
            }
            return;
        }
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.state.confirmation.is_none()
            && self.awaiting_serial.is_none();
        match self.manufacturing.prepare_section {
            1 if self.material.tool_id.is_some()
                || self
                    .material
                    .has_changes(&project.project_id, m.intent.as_deref()) =>
            {
                self.material_actions(ui);
                return;
            }
            1 | 2 => {
                if let Some(d) = &mut self.manufacturing.draft {
                    if d.project != project.project_id {
                        return;
                    }
                    let prepared = super::policies::prepared(&d.value);
                    let changed = m.intent.as_deref() != Some(&d.value);
                    let reason = if !ready {
                        Some("Wait for the current operation")
                    } else if project.read_only {
                        Some("Project is read-only")
                    } else if d.revision != project.revision {
                        Some("Project changed. Discard this draft and reload current settings.")
                    } else {
                        prepared.as_ref().err().map(String::as_str)
                    };
                    if changed && ui::primary(ui, "Apply settings", reason) {
                        self.queued_manufacturing = d
                            .apply_command(
                                project.layout_id.clone(),
                                hello.session_id.clone(),
                                project.revision,
                            )
                            .ok();
                    }
                    if changed {
                        if ui.button("Discard settings draft").clicked() {
                            self.manufacturing.ignored_nonce = m.loaded.as_ref().map(|l| l.nonce);
                            self.manufacturing.draft = None;
                        }
                        return;
                    }
                }
            }
            3 => {
                self.placement_actions(ui);
                return;
            }
            4 if self.process.has_changes() => {
                self.process_actions(ui);
                return;
            }
            _ => {}
        }
        // On Printer, a changed choice is sliced by the form's own action.
        let form_owns = self.manufacturing.prepare_section == 0 && self.print_settings.changed;
        if !form_owns && (m.intent.is_some() || self.slice_draft_reason().is_some()) {
            self.slice_button(ui);
        }
    }
    fn placement_actions(&mut self, ui: &mut egui::Ui) {
        let (Some(project), Some(hello), Some(scene)) = (
            self.state.project.clone(),
            self.state.hello.clone(),
            self.state.display.clone(),
        ) else {
            return;
        };
        if self.state.manufacturing.intent.is_none() {
            self.primary_button(ui, Action::LoadIntent);
            return;
        }
        let occurrence = scene
            .occurrences
            .iter()
            .find(|o| Some(o.occurrence_id) == self.occurrence);
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.awaiting_serial.is_none()
            && !project.read_only
            && self.display_current();
        let reason = (!ready).then_some("Open a writable project and wait for pending operations");
        // The engine's one placement rule, the same as the CLI's
        // --place-on-bed / --center-on-bed (selected part) and
        // --place-all-on-bed / --center-all-on-bed (no selection): rest on the
        // bed, keep XY when the printer admits it there, else centre; one
        // undo step.
        let place = |center_xy| GeometryCommand::PlaceOnBed {
            session_id: hello.session_id.clone(),
            base_revision: scene.summary.revision,
            project_revision: project.revision,
            occurrence_id: occurrence.map(|o| o.occurrence_id),
            center_xy,
        };
        let (place_label, centre_label) = if occurrence.is_some() {
            ("Place on bed", "Centre on bed")
        } else {
            ("Place all on bed", "Centre all on bed")
        };
        if ui::primary(ui, place_label, reason) {
            self.queued_geometry = Some(place(false));
        }
        if ui
            .add_enabled(reason.is_none(), egui::Button::new(centre_label))
            .on_disabled_hover_text(reason.unwrap_or_default())
            .clicked()
        {
            self.queued_geometry = Some(place(true));
        }
    }
    pub(super) fn manufacturing_ui(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.state.project.clone() else {
            return;
        };
        if self.state.hello.is_none() {
            return;
        }
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.state.confirmation.is_none()
            && self.awaiting_serial.is_none();
        let writable = ready && !project.read_only;
        let m = self.state.manufacturing.clone();
        if self.work_area.panel == Panel::Prepare {
            if matches!(self.manufacturing.prepare_section, 1 | 2)
                && self.manufacturing.trial.is_some()
            {
                ui.label(
                    "An alternative is open. Current Settings drafts are retained, not consumed.",
                );
                if ui.button("Return to alternative").clicked() {
                    self.show_panel(Panel::Alternative);
                }
                return;
            }
            if project.read_only {
                ui.label("Read-only project");
            }
            let section = self.manufacturing.prepare_section;
            if matches!(section, 1 | 2) {
                self.manufacturing.sync_intent(
                    &project.project_id,
                    project.revision,
                    m.intent.clone(),
                    m.loaded.as_deref(),
                );
            }
            if section == 0 {
                self.print_settings_ui(ui, writable);
            }
            if section == 1 {
                self.material_ui(ui, writable);
                ui.collapsing("Material region", |ui| {
                    self.functional_material_ui(ui, writable)
                });
            }
            if section == 2 {
                ui::group(ui, "Plate defaults");
                if let Some(source) = self
                    .manufacturing
                    .draft
                    .as_ref()
                    .map(|d| d.original.clone())
                {
                    let d = self
                        .manufacturing
                        .draft
                        .as_mut()
                        .expect("initialized intent");
                    let conflict = d.revision != project.revision;
                    if conflict {
                        ui.colored_label(
                            ui::Tokens::for_ui(ui).warning,
                            "Project changed · reload settings",
                        );
                    }
                    if let Some(loaded) = m
                        .loaded
                        .as_ref()
                        .filter(|l| self.manufacturing.ignored_nonce != Some(l.nonce))
                    {
                        ui.weak(format!("Selected: {}", loaded.label));
                    }
                    ui.add_enabled_ui(writable && !conflict, |ui| {
                        settings_fields(ui, &mut d.value, ui::Tokens::for_ui(ui));
                    });
                    ui.collapsing("Complete declaration", |ui| {
                        ui.label(&d.value.printer.name);
                        ui.label(&d.value.recipe.material_id);
                        ui.weak(format!(
                            "Recipe {} / {}",
                            d.value.recipe.id, d.value.recipe.revision
                        ));
                        ui.label(serde_json::to_string_pretty(&source).unwrap_or_default());
                    });
                } else {
                    ui.weak("Choose a printer and material.");
                }
                ui.collapsing("Material regions", |ui| {
                    self.functional_material_ui(ui, writable)
                });
            }
            if section == 4 {
                self.process_ui(ui, writable && self.display_current());
            }
            if let Some(preflight) = &m.preflight
                && preflight.disposition != PreflightDisposition::Admitted
                && preflight.input_revision == project.revision
            {
                use spiling_contracts::manufacturing::PreflightEvidence;
                ui.add_space(6.0);
                // Findings, not a gate: Slice still asks the engine to compile.
                ui.colored_label(ui.visuals().warn_fg_color, "Setup issues");
                let mut placement = false;
                for check in preflight.checks.iter().filter(|c| {
                    matches!(
                        c.evidence,
                        PreflightEvidence::Failed | PreflightEvidence::Unsupported
                    )
                }) {
                    placement |= matches!(
                        check.code.as_str(),
                        "hardware_placement" | "outside_envelope"
                    );
                    ui.add(egui::Label::new(&check.message).wrap());
                }
                if placement && ui.button("Arrange on bed").clicked() {
                    self.open_layout(ui.ctx());
                }
                ui.collapsing("All checks", |ui| {
                    for check in &preflight.checks {
                        ui.label(format!(
                            "{:?} · {}: {}",
                            check.evidence, check.code, check.message
                        ));
                    }
                });
            }
        } else if self.work_area.panel == Panel::Review {
            if m.loading {
                ui.spinner();
                if ui.button("Cancel layer loading").clicked() {
                    self.queued_host = Some(Command::CancelReview);
                }
            }
            self.manufacturing_options(ui);
            if let Some(review) = self.selected_manufacturing_review() {
                self.manufacturing.sync_replay_review(&review);
                if self.manufacturing.pending_loaded(m.loaded.as_deref())
                    || self
                        .manufacturing
                        .has_intent_draft(&project.project_id, m.intent.as_deref())
                {
                    ui.colored_label(
                        ui::Tokens::for_ui(ui).warning,
                        "Unapplied changes · showing committed output",
                    );
                }
                let report = &review.capture.report;
                if !report.verified {
                    ui.colored_label(ui.visuals().error_fg_color, "Verification failed");
                }
                self.job_headline(ui, report, &review.bundle.plan, &review.capture.record.hash);
                super::review::support_inspection(
                    ui,
                    &review.bundle.plan.layers,
                    review.display.as_deref(),
                    &mut self.manufacturing.replay,
                    &mut self.manufacturing.support_inspection,
                );
                let can_try_alternative = ready
                    && self.manufacturing_current()
                    && self.state.manufacturing.review.is_some();
                if let Some(action) = super::review::findings(
                    ui,
                    &review,
                    review.display.as_deref(),
                    &mut self.manufacturing.replay,
                    can_try_alternative,
                ) {
                    match action {
                        super::review::ReviewAction::InspectLayer {
                            layer: index,
                            compare_section,
                        } => {
                            self.manufacturing.replay.layer = Some(index);
                            self.manufacturing.replay.layer_end = None;
                            if compare_section {
                                self.analysis.open = true;
                                self.analysis.follow_layer = true;
                                self.analysis.view.enabled = true;
                                self.analysis.view.representation =
                                    crate::analysis::Representation::Compare;
                            }
                            self.manufacturing.replay.events = review
                                .display
                                .as_ref()
                                .and_then(|display| {
                                    display
                                        .layer_windows
                                        .get(index as usize)
                                        .map(|window| window.end)
                                })
                                .unwrap_or(u32::MAX);
                            self.manufacturing.replay.visible = true;
                            self.manufacturing.replay.source = false;
                            self.manufacturing.replay.role = None;
                            self.manufacturing.replay.tool = None;
                        }
                        super::review::ReviewAction::TryAlternative { name, intent } => {
                            self.begin_alternative_preset(name, *intent);
                        }
                        super::review::ReviewAction::OpenPosition => {
                            self.show_prepare(3);
                        }
                        super::review::ReviewAction::OpenSettings => {
                            self.show_prepare(2);
                        }
                    }
                }
                if let Some(label) = &m.last_export {
                    ui.add_space(4.0);
                    ui.label("Exported");
                    ui.add(
                        egui::Label::new(egui::RichText::new(label).small())
                            .wrap()
                            .selectable(true),
                    );
                }
                if let Some(index) = self.manufacturing.replay.layer
                    && let Some(layer) = review.bundle.plan.layers.get(index as usize)
                {
                    ui.add_space(4.0);
                    ui.label(format!(
                        "Layer {} · Z {:.2} mm · {:.2} mm thick · {} paths",
                        index + 1,
                        layer.z_mm,
                        layer.z_mm - layer.lower_z_mm,
                        layer.paths.len()
                    ));
                }
                ui.add_space(6.0);
                egui::CollapsingHeader::new("Details")
                    .id_salt("review-details")
                    .show(ui, |ui| {
                        ui.label(format!(
                            "Software verification: {}",
                            if report.verified { "Passed" } else { "Failed" }
                        ));
                        self.job_summary(ui, report);
                        ui.label(format!(
                            "{} deposition · {} travel segments",
                            report.deposition_segments, report.travel_segments
                        ));
                        ui.label(format!(
                            "{:.3} mm³ deposited · {:.3} mm filament",
                            report.deposited_volume_mm3, report.filament_length_mm
                        ));
                        let goal = &report.layer_height;
                        ui.separator();
                        ui::group(ui, "Construction goals");
                        ui.label(match goal.policy {
                            LayerHeightPolicy::Preferred => "Layer height · preferred target",
                            LayerHeightPolicy::ProtectedMaximum => {
                                "Layer height · protected maximum"
                            }
                        });
                        ui.label(format!(
                            "Requested {:.3} mm; selected nominal {:.3} mm",
                            goal.requested_mm, goal.selected_mm
                        ));
                        ui.label(format!(
                            "Actual layer heights {:.6}–{:.6} mm",
                            goal.realized_min_mm, goal.realized_max_mm
                        ));
                        if goal.reason != LayerHeightReason::Requested {
                            ui.colored_label(
                        ui::Tokens::for_ui(ui).warning,
                        match goal.reason {
                            LayerHeightReason::ProcessMinimum => {
                                "Goal adjusted · captured process minimum requires thicker layers."
                            }
                            LayerHeightReason::ProcessMaximum => {
                                "Goal adjusted · captured process maximum requires thinner layers."
                            }
                            LayerHeightReason::Requested => unreachable!(),
                        },
                    );
                        }
                        if let Some(index) = goal.first_differing_layer {
                            ui.label(format!(
                        "{} of {} layers differ from the target by more than command precision.",
                        goal.differing_layer_count, goal.layer_count
                    ));
                            if ui.button("Inspect first adjusted layer").clicked() {
                                self.manufacturing.replay.layer = Some(index);
                                self.manufacturing.replay.layer_end = None;
                                self.analysis.open = true;
                                self.analysis.follow_layer = true;
                                self.analysis.view.enabled = true;
                                self.analysis.view.representation =
                                    crate::analysis::Representation::Compare;
                                self.manufacturing.replay.events = review
                                    .display
                                    .as_ref()
                                    .and_then(|display| {
                                        display
                                            .batches
                                            .iter()
                                            .flat_map(|batch| batch.events.iter())
                                            .filter(|event| event.1 == index)
                                            .map(|event| event.0)
                                            .max()
                                            .and_then(|event| event.checked_add(1))
                                    })
                                    .unwrap_or(u32::MAX);
                                self.manufacturing.replay.visible = true;
                                self.manufacturing.replay.role = None;
                                self.manufacturing.replay.tool = None;
                            }
                        }

                        super::review::details(ui, &review);
                        ui.collapsing("Coverage", |ui| {
                            for row in &report.coverage {
                                ui.label(row);
                            }
                        });
                        ui.collapsing("Limits", |ui| {
                            for row in &report.limitations {
                                ui.label(row);
                            }
                        });
                        ui.collapsing("Captured inspection identity", |ui| {
                            ui.label(format!("{:#?}", review.capture.provenance));
                        });

                        let plan = &review.bundle.plan;
                        if let Some(index) = self.manufacturing.replay.layer
                            && let Some(layer) = plan.layers.get(index as usize)
                        {
                            ui.separator();
                            ui::group(ui, format!("Layer {}", index + 1));
                            ui.label(format!(
                                "Layer schedule interval {:.6}…{:.6} mm",
                                layer.lower_z_mm, layer.z_mm
                            ));
                            ui.label(format!(
                                "Section Z {:.6} mm · {} paths",
                                layer.section_z_mm,
                                layer.paths.len()
                            ));
                            if layer.paths.is_empty() {
                                ui.weak("Empty gap layer · no events");
                            }
                            ui.collapsing("Resolved regional coverage", |ui| {
                                super::review::layer(ui, layer);
                            });
                        }
                        ui.collapsing("Sparse band decisions", |ui| {
                            egui::ScrollArea::vertical().max_height(240.0).show_rows(
                                ui,
                                24.0,
                                plan.sparse_bands.len(),
                                |ui, range| {
                                    for index in range {
                                        let band = &plan.sparse_bands[index];
                                        ui.label(format!(
                                            "Layers {}–{} · {:?} · {:?}",
                                            band.first_layer,
                                            band.last_layer,
                                            band.owner,
                                            band.disposition
                                        ));
                                    }
                                },
                            );
                        });
                        self.construction_interfaces_ui(ui, &review);
                    });
            }
        }
    }
    pub(super) fn replay_controls(&mut self, ui: &mut egui::Ui) {
        let Some(review) = self.selected_manufacturing_review() else {
            return;
        };
        let Some(descriptor) = &review.capture.replay else {
            return;
        };
        self.manufacturing.sync_replay_review(&review);
        let section_representation = self.analysis.view.enabled;
        let mut section = self.analysis.view.clone();
        section.enabled &= section.valid()
            && (!section.local_frame || self.analysis_pose().is_some())
            && (!section.selected_only || self.occurrence.is_some());
        let plane = self.analysis_plane();
        let view = &mut self.manufacturing.replay;
        let tints = crate::replay::declared_tool_colors(&review.capture.provenance.workflow);
        if let Some(display) = &review.display
            && display.beads.is_err()
        {
            view.beads = false;
        }
        // One row: what is drawn, then a Display menu for the rest.
        ui.horizontal_wrapped(|ui| {
            if let Some(display) = &review.display {
                egui::ComboBox::from_id_salt("replay-role")
                    .selected_text(
                        view.role
                            .map_or("All roles", |role| crate::replay::ROLES[role as usize].label),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut view.role, None, "All roles");
                        let dark = ui.visuals().dark_mode;
                        for (role, entry) in crate::replay::ROLES.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 8.0), egui::Sense::hover());
                                ui.painter().rect_filled(rect, 1, entry.color(dark));
                                ui.selectable_value(&mut view.role, Some(role as u8), entry.label);
                            });
                        }
                    });
                if descriptor.info.tool_epoch_count > 0 {
                    egui::ComboBox::from_id_salt("replay-tool")
                        .selected_text(
                            view.tool
                                .map_or_else(|| "All tools".into(), |tool| format!("Tool {tool}")),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut view.tool, None, "All tools");
                            for tool in 0..2 {
                                ui.selectable_value(
                                    &mut view.tool,
                                    Some(tool),
                                    format!("Tool {tool}"),
                                );
                            }
                        });
                }
                if !section_representation {
                    ui.add_enabled_ui(display.beads.is_ok(), |ui| {
                        ui.selectable_value(&mut view.beads, true, "Beads");
                    });
                    ui.selectable_value(&mut view.beads, false, "Lines");
                }
            }
            ui.checkbox(&mut view.travels, "Travels");
            ui.menu_button("Display", |ui| {
                if section_representation {
                    ui.weak("Representation follows Section analysis");
                } else {
                    ui.checkbox(&mut view.source, "Source model");
                    ui.checkbox(&mut view.visible, "Motion");
                }
                ui.checkbox(&mut view.pressure, "Pressure events");
                if tints.is_some() {
                    ui.checkbox(&mut view.material, "Material tints");
                }
                if let Some(display) = &review.display {
                    match &display.beads {
                        Ok(beads) if view.beads => {
                            let cache = &mut self.manufacturing.bead_statistics;
                            if cache.as_ref().is_none_or(|c| {
                                c.nonce != review.capture.nonce
                                    || !c.display.ptr_eq(&Arc::downgrade(display))
                                    || c.view != *view
                            }) {
                                *cache = Some(BeadStatistics {
                                    display: Arc::downgrade(display),
                                    nonce: review.capture.nonce,
                                    view: *view,
                                    value: beads.statistics(*view),
                                });
                            }
                            ui.separator();
                            if let Some(stats) = cache.as_ref().and_then(|c| c.value) {
                                let dimension = |range: [f32; 2]| {
                                    if range[1] - range[0] < 0.000001 {
                                        format!("{:.2}", range[0])
                                    } else {
                                        format!("{:.2}–{:.2}", range[0], range[1])
                                    }
                                };
                                ui.weak(format!(
                                    "Width {} mm · height {} mm · {} strokes",
                                    dimension(stats.width),
                                    dimension(stats.height),
                                    stats.count
                                ))
                                .on_hover_text(format!("Visible nominal widths and physical stroke heights, distinct from the layer schedule. Section chord bound {:.6} mm; numerical expansion ≤ 0.001 mm.", beads.chord_error_mm));
                            }
                            ui.collapsing("Bead display model", |ui| {
                                ui.label(BEAD_MODEL);
                            });
                        }
                        Err(reason) => {
                            ui.weak(format!("Lines only · {reason}"));
                        }
                        _ => {}
                    }
                }
            });
        });
        if view.material
            && let Some(colors) = &tints
        {
            ui.collapsing("Tool colors", |ui| {
                for (tool, rgba) in colors.iter().enumerate() {
                    if let JobWorkflow::FixedDualMarlin { job } =
                        &review.capture.provenance.workflow
                    {
                        let material = &job.tools[tool].material;
                        ui.label(format!(
                            "Tool T{tool} · {} · {}",
                            material.polymer_family, material.id
                        ));
                    }
                    ui::ColorPresentation::known(
                        [rgba[0], rgba[1], rgba[2]].map(|c| (c * 255.0).round() as u8),
                        ui::ColorMeaning::CatalogEstimate,
                    )
                    .show(ui, ("tool-color", tool), ui::ColorDensity::Standard);
                }
            });
        }
        // The layer rail beside the view owns layer selection.
        if let Some(display) = &review.display {
            let bounds = self
                .state
                .display
                .as_ref()
                .and_then(|scene| scene.summary.bounds_mm);
            let radius = bounds
                .map_or(0.01, |bounds| {
                    (glam::DVec3::from_array(bounds.max) - glam::DVec3::from_array(bounds.min))
                        .length()
                        * 0.5
                })
                .max(0.01);
            let bounds = descriptor.info.bounds_mm;
            let center =
                (glam::DVec3::from_array(bounds.min) + glam::DVec3::from_array(bounds.max)) * 0.5;
            let radius = radius.max(
                (center - display.origin).length()
                    + (glam::DVec3::from_array(bounds.max) - glam::DVec3::from_array(bounds.min))
                        .length()
                        * 0.5,
            );
            let clip =
                crate::replay::seams::SectionClip::project(&section, plane, display.origin, radius);
            let mut visible_view = *view;
            if section.enabled {
                visible_view.visible =
                    section.representation != crate::analysis::Representation::Source;
            }
            ui::seam_review(
                ui,
                &display.seams,
                &mut self.manufacturing.selected_seam,
                &mut visible_view,
                clip,
                display.origin,
                &review.bundle.provenance.intent.recipe.seam_policy,
            );
            view.seams = visible_view.seams;
            view.selected_seam = view
                .seams
                .then_some(self.manufacturing.selected_seam)
                .flatten();
        }
        ui.collapsing("Command playback", |ui| {
            event_controls(ui, view, &review, descriptor);
        });
    }
}

pub(super) fn settings_fields(
    ui: &mut egui::Ui,
    intent: &mut ManufacturingIntent,
    tokens: ui::Tokens,
) {
    super::policies::seam_fields(ui, intent);
    ui::group(ui, "Construction goals");
    ui::layer_height_goal(ui, intent, tokens);
    for (label, value) in [
        ("Bead width (mm)", &mut intent.recipe.bead_width_mm),
        ("Top skin (mm)", &mut intent.recipe.top_skin_thickness_mm),
        (
            "Bottom skin (mm)",
            &mut intent.recipe.bottom_skin_thickness_mm,
        ),
        ("Travel speed (mm/s)", &mut intent.recipe.travel_speed_mm_s),
    ] {
        ui::number(ui, label, value, 0.01);
    }
    ui::percent(
        ui,
        if matches!(
            intent.recipe.sparse_infill,
            SparseInfillPolicy::WebRoof { .. }
        ) {
            "Base webs (%)"
        } else {
            "Infill (%)"
        },
        &mut intent.recipe.infill_fraction,
    );
    ui.add_enabled_ui(intent.material_profile.is_none(), |ui| {
        ui::number(
            ui,
            "Print speed (mm/s)",
            &mut intent.recipe.print_speed_mm_s,
            0.1,
        );
        ui::number(
            ui,
            "Flow multiplier",
            &mut intent.recipe.flow_multiplier,
            0.01,
        );
    });
    if intent.material_profile.is_some() {
        ui.weak("Speed, flow and process overrides: Material section");
    }
    ui.horizontal(|ui| {
        ui.label("Perimeters");
        ui.add(egui::DragValue::new(&mut intent.recipe.perimeter_count).range(1..=8));
    });
    super::policies::fields(ui, intent);
    ui.collapsing("Geometry tolerance", |ui| {
        ui.label(format!(
            "Shape approximation limit · {:.3} mm",
            intent.recipe.native_shape_error_mm()
        ));
    });
}

fn event_controls(
    ui: &mut egui::Ui,
    view: &mut crate::replay::View,
    review: &crate::manufacture::Review,
    _descriptor: &ReplayDisplayDescriptor,
) {
    let window = crate::layers::event_window(review, *view);
    let mut progress = view
        .events
        .saturating_sub(window.start)
        .min(window.end - window.start);
    if ui
        .add(
            egui::Slider::new(&mut progress, 0..=window.end - window.start)
                .text("Commands in selection"),
        )
        .changed()
    {
        view.events = window.start + progress;
    }
    if view.events > window.start
        && view.events <= window.end
        && let Some(index) = view.events.checked_sub(1)
        && let Some(bytes) = &review.motion
    {
        let start = REPLAY_HEADER_BYTES + index as usize * REPLAY_RECORD_BYTES;
        use std::io::{Read, Seek, SeekFrom};
        let mut reader = bytes.reader();
        let mut r = [0; REPLAY_RECORD_BYTES];
        if reader
            .seek(SeekFrom::Start(start as u64))
            .and_then(|_| reader.read_exact(&mut r))
            .is_err()
        {
            ui.weak("Command detail unavailable");
            return;
        }
        let number = |offset| {
            f64::from_le_bytes(
                r[offset..offset + 8]
                    .try_into()
                    .expect("checked SPLR7 record"),
            )
        };
        let tool = u16::from_le_bytes(
            r[REPLAY_TOOL_OFFSET..REPLAY_TOOL_OFFSET + 2]
                .try_into()
                .expect("checked active tool"),
        );
        ui.add(
            egui::Label::new(format!(
                "{} · tool {} · XYZ {:.6}, {:.6}, {:.6} mm · E {:.8} mm · F {:.1} mm/min",
                crate::replay::ROLES[r[46] as usize].label,
                tool,
                number(0),
                number(8),
                number(16),
                number(24),
                number(32)
            ))
            .wrap(),
        );
        if r[47] & 2 != 0 {
            ui.weak(format!(
                "Nominal association: width {:.6} mm · physical Z {:.6}…{:.6} mm · height {:.6} mm",
                number(48),
                number(56),
                number(64),
                number(64) - number(56)
            ));
        }
    }
    ui.collapsing("Actual motion legend",|ui|{ui.label("Perimeter · solid · top/bottom · sparse · bridge · support/interface · prime · waste · travel · stationary retract/unretract. Tool epochs change state without a connecting segment. Dimensions and tool colors are nominal declarations.");});
}

#[cfg(test)]
mod goal_draft_tests {
    use super::*;

    fn inspection_layers() -> Vec<PrintLayer> {
        let coverage = PlanarRegionCoverage {
            section_area_mm2: 0.0,
            source_section_area_mm2: 0.0,
            surface_added_area_mm2: 0.0,
            wall_area_mm2: 0.0,
            bottom_skin_area_mm2: 0.0,
            top_skin_area_mm2: 0.0,
            solid_interior_area_mm2: 0.0,
            sparse_interior_area_mm2: 0.0,
            intentional_void_area_mm2: 0.0,
            protected_hole_area_mm2: 0.0,
            nominal_deposition_volume_mm3: 0.0,
            support_area_mm2: 0.0,
            support_nominal_deposition_volume_mm3: 0.0,
            unsupported_overhang_mm: 0.0,
            support_omitted_wall_mm: 0.0,
            thin_widened_mm: 0.0,
            thin_omitted_area_mm2: 0.0,
            process_regions: Vec::new(),
            process_interfaces: Vec::new(),
            separator_area_mm2: 0.0,
            process_diaphragms: Vec::new(),
            diaphragm_area_mm2: 0.0,
            skin_bridge_domains: Vec::new(),
            web_roof_domains: Vec::new(),
        };
        (0..4)
            .map(|index| PrintLayer {
                index,
                hatch_course: index,
                lower_z_mm: f64::from(index) * 0.2,
                z_mm: f64::from(index + 1) * 0.2,
                height_mm: 0.2,
                section_z_mm: f64::from(index) * 0.2 + 0.1,
                paths: Vec::new(),
                region_coverage: PlanarRegionCoverage {
                    unsupported_overhang_mm: if index == 2 {
                        0.5
                    } else if index == 3 {
                        1.25
                    } else {
                        0.0
                    },
                    ..coverage.clone()
                },
                skin_command_fits: Vec::new(),
            })
            .collect()
    }

    fn inspection_display() -> crate::replay::Replay {
        crate::replay::Replay {
            origin: DVec3::ZERO,
            layer_windows: vec![2..6, 6..11, 11..20, 20..28],
            event_count: 30,
            batches: Vec::new(),
            seams: Vec::new(),
            owned_bytes: 0,
            beads: Err("Centerlines only".into()),
        }
    }

    #[test]
    fn support_inspection_reveals_actual_bands_and_handles_missing_preview() {
        for theme in [Theme::Graphite, Theme::Paper] {
            for width in [240.0, 400.0] {
                let context = egui::Context::default();
                theme.apply(&context);
                context.global_style_mut(|style| style.animation_time = 0.0);
                let mut layers = inspection_layers();
                for index in [0, 2, 3] {
                    let layer = &mut layers[index];
                    layer.paths.push(DepositionPath {
                        material_domain: None,
                        kind: DepositionKind::SupportInterface,
                        width: WidthProfile::Constant { width_mm: 0.45 },
                        physical_z_mm: [layer.lower_z_mm, layer.z_mm],
                        owner: None,
                        points_mm: vec![[0.0, 0.0, layer.z_mm], [1.0, 0.0, layer.z_mm]],
                        arcs: Vec::new(),
                        exposed: false,
                        connectors: Vec::new(),
                    });
                }
                let display = inspection_display();
                let inspection = std::cell::RefCell::new(
                    super::super::review::SupportInspection::from_layers(&layers),
                );
                let before = crate::replay::View {
                    tool: Some(1),
                    source: true,
                    travels: false,
                    ..Default::default()
                };
                let mut view = before;
                let revealed = crate::replay::View {
                    visible: true,
                    source: false,
                    tool: None,
                    ..before
                };
                let frame = |events, view: &mut crate::replay::View, available: bool| {
                    context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 400.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            super::super::review::support_inspection(
                                ui,
                                &layers,
                                available.then_some(&display),
                                view,
                                &mut inspection.borrow_mut(),
                            )
                        },
                    )
                };
                let click_label = |label: &str, view: &mut crate::replay::View, available| {
                    let output = frame(Vec::new(), view, available);
                    let point = output
                        .shapes
                        .iter()
                        .find_map(|shape| {
                            if let egui::Shape::Text(text) = &shape.shape {
                                assert!(
                                    shape.clip_rect.contains_rect(text.visual_bounding_rect()),
                                    "{theme:?} {width}: {}",
                                    text.galley.text()
                                );
                                (text.galley.text() == label)
                                    .then(|| text.visual_bounding_rect().center())
                            } else {
                                None
                            }
                        })
                        .expect("visible support action");
                    output.drop_without_applying_deltas();
                    for pressed in [true, false] {
                        frame(
                            vec![
                                egui::Event::PointerMoved(point),
                                egui::Event::PointerButton {
                                    pos: point,
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ],
                            view,
                            available,
                        )
                        .drop_without_applying_deltas();
                    }
                };
                click_label("Interfaces", &mut view, false);
                assert_eq!(view, before);
                click_label("Interfaces", &mut view, true);
                assert_eq!(
                    view,
                    crate::replay::View {
                        role: Some(10),
                        events: display.event_count,
                        ..revealed
                    }
                );
                click_label("Next band", &mut view, true);
                assert_eq!(
                    (view.layer, view.layer_end, view.events),
                    (Some(0), None, 6)
                );
                click_label("Next band", &mut view, true);
                assert_eq!(
                    (view.layer, view.layer_end, view.events),
                    (Some(2), Some(3), 28)
                );
                click_label("Next band", &mut view, true);
                assert_eq!((view.layer, view.layer_end), (Some(2), Some(3)));
                click_label("Previous band", &mut view, true);
                assert_eq!(
                    (view.layer, view.layer_end, view.events),
                    (Some(0), None, 6)
                );
                click_label("Scaffold", &mut view, true);
                assert_eq!(
                    view,
                    crate::replay::View {
                        role: Some(9),
                        events: display.event_count,
                        ..revealed
                    }
                );
                click_label("All paths", &mut view, true);
                assert_eq!(
                    view,
                    crate::replay::View {
                        events: display.event_count,
                        ..revealed
                    }
                );
            }
        }
    }

    #[test]
    fn largest_recorded_issue_reveals_actual_paths_without_changing_drafts() {
        for theme in [Theme::Graphite, Theme::Paper] {
            for width in [240.0, 400.0] {
                let context = egui::Context::default();
                theme.apply(&context);
                context.global_style_mut(|style| style.animation_time = 0.0);
                let layers = inspection_layers();
                let captured_layers = layers.clone();
                let display = inspection_display();
                let original: ManufacturingIntent = serde_json::from_str(include_str!(
                    "../../../../fixtures/manufacturing/preferred-layers.intent.json"
                ))
                .unwrap();
                let mut edited = original.clone();
                edited.recipe.layer_height_mm = 0.15;
                let project = ProjectId::default();
                let revision = ProjectRevision(3);
                let mut manufacturing = ManufacturingUi {
                    draft: Some(IntentDraft {
                        project: project.clone(),
                        revision,
                        nonce: 7,
                        original: Arc::new(original.clone()),
                        value: edited.clone(),
                    }),
                    replay: crate::replay::View {
                        layer: Some(0),
                        layer_end: Some(1),
                        events: 7,
                        role: Some(9),
                        tool: Some(1),
                        source: true,
                        visible: false,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let before = manufacturing.replay;
                let frame = |events, manufacturing: &mut ManufacturingUi| {
                    context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 600.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            super::super::review::unsupported_layers(
                                ui,
                                &layers,
                                Some(&display),
                                &mut manufacturing.replay,
                            );
                        },
                    )
                };
                let click = |point, manufacturing: &mut ManufacturingUi| {
                    for pressed in [true, false] {
                        frame(
                            vec![
                                egui::Event::PointerMoved(point),
                                egui::Event::PointerButton {
                                    pos: point,
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ],
                            manufacturing,
                        )
                        .drop_without_applying_deltas();
                    }
                };
                let output = frame(vec![], &mut manufacturing);
                let details = output.shapes.iter().find_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape
                        && text.galley.text().starts_with("Support details")
                    {
                        Some(text.visual_bounding_rect().center())
                    } else {
                        None
                    }
                });
                output.drop_without_applying_deltas();
                click(
                    details.expect("support evidence can be disclosed"),
                    &mut manufacturing,
                );
                assert_eq!(manufacturing.replay, before);
                let output = frame(vec![], &mut manufacturing);
                let mut target = None;
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape {
                        assert!(
                            shape.clip_rect.contains_rect(text.visual_bounding_rect()),
                            "{theme:?} {width}: {}",
                            text.galley.text()
                        );
                        if text.galley.text() == "Show layer 4" {
                            target = Some(text.visual_bounding_rect().center());
                        }
                    }
                }
                output.drop_without_applying_deltas();
                assert_eq!(manufacturing.replay, before);
                let point = target.expect("affected layer has a reachable inspection action");
                click(point, &mut manufacturing);
                assert_eq!(
                    manufacturing.replay,
                    crate::replay::View {
                        layer: Some(layers[3].index),
                        layer_end: None,
                        events: display.layer_windows[3].end,
                        role: None,
                        tool: None,
                        source: false,
                        visible: true,
                        ..before
                    }
                );
                let draft = manufacturing.draft.as_ref().unwrap();
                assert_eq!(draft.project, project);
                assert_eq!(draft.revision, revision);
                assert_eq!(draft.nonce, 7);
                assert_eq!(*draft.original, original);
                assert_eq!(draft.value, edited);
                assert!(manufacturing.has_intent_draft(&project, Some(&original)));
                assert_eq!(manufacturing.review_nonce, None);
                assert_eq!(manufacturing.replay_record, None);
                assert_eq!(layers, captured_layers);
            }
        }
    }

    #[test]
    fn unsupported_layers_without_actual_preview_leave_inspection_unchanged() {
        let context = egui::Context::default();
        let layers = inspection_layers();
        let mut view = crate::replay::View {
            layer: Some(0),
            layer_end: Some(1),
            events: 7,
            ..Default::default()
        };
        let before = view;
        context
            .run_ui(egui::RawInput::default(), |ui| {
                super::super::review::unsupported_layers(ui, &layers, None, &mut view);
            })
            .drop_without_applying_deltas();
        assert_eq!(view, before);
    }

    #[test]
    fn acknowledged_intent_is_not_pending_before_prepare_refreshes_its_draft() {
        let original: ManufacturingIntent = serde_json::from_str(include_str!(
            "../../../../fixtures/manufacturing/preferred-layers.intent.json"
        ))
        .unwrap();
        let mut edited = original.clone();
        edited.recipe.layer_height_mm = 0.15;
        let project = ProjectId::default();
        let view = ManufacturingUi {
            draft: Some(IntentDraft {
                project: project.clone(),
                revision: ProjectRevision(3),
                nonce: 0,
                original: Arc::new(original.clone()),
                value: edited.clone(),
            }),
            ..Default::default()
        };
        assert!(
            view.has_intent_draft(&project, Some(&original)),
            "unacknowledged edits remain visible"
        );
        assert!(
            !view.has_intent_draft(&project, Some(&edited)),
            "a matching engine receipt clears pending status without rendering Prepare"
        );
        let mut different = edited.clone();
        different.recipe.bead_width_mm += 0.01;
        assert!(
            view.has_intent_draft(&project, Some(&different)),
            "another authoritative result cannot acknowledge this draft"
        );
    }
}
