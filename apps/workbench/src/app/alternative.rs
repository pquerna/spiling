// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    manufacturing::{ManufacturingCommand, ManufacturingIntent},
    project::{ManufacturingOption, ProjectCommand, ProjectId, ProjectRevision},
};
use std::sync::Arc;

pub(super) struct Trial {
    project: ProjectId,
    session: SessionId,
    revision: ProjectRevision,
    context: SourceHash,
    reference_intent: Arc<ManufacturingIntent>,
    reference_nonce: u64,
    previous_nonce: u64,
    value: ManufacturingIntent,
    option: Option<SourceHash>,
    name: String,
    candidate: bool,
    editing: bool,
    keeping: bool,
    ending: bool,
    adopting: Option<Adoption>,
    height_selection: PhysicalSelection,
}
struct Adoption {
    base_revision: ProjectRevision,
    intent: Arc<ManufacturingIntent>,
    after_nonce: u64,
    submitted_serial: u64,
}

enum TrialAction {
    Back,
    Keep,
    End,
}

fn trial_actions(ui: &mut egui::Ui, ready: bool, keep: Option<bool>) -> Option<TrialAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Back to Review")
            .on_hover_text("Keep this trial open")
            .clicked()
        {
            action = Some(TrialAction::Back);
        }
        ui.menu_button("More…", |ui| {
            if let Some(enabled) = keep
                && ui
                    .add_enabled(enabled, egui::Button::new("Keep option…"))
                    .clicked()
            {
                action = Some(TrialAction::Keep);
                ui.close();
            }
            if ui
                .add_enabled(ready, egui::Button::new("End alternative…"))
                .clicked()
            {
                action = Some(TrialAction::End);
                ui.close();
            }
        });
    });
    action
}

impl super::manufacturing::ManufacturingUi {
    fn adopt_confirmed(&mut self, receipt: Option<&crate::manufacture::AppliedSettings>) -> bool {
        let confirmed = self.trial.as_ref().is_some_and(|trial| {
            trial
                .adopting
                .as_ref()
                .zip(receipt)
                .is_some_and(|(adoption, receipt)| {
                    receipt.nonce > adoption.after_nonce
                        && receipt.project == trial.project
                        && receipt.session == trial.session
                        && receipt.base_revision == adoption.base_revision
                        && receipt.intent == adoption.intent
                })
        });
        if confirmed {
            self.trial = None;
            self.panel = 2;
            self.comparison_no_height_match = false;
        }
        confirmed
    }
}

impl Workbench {
    pub(super) fn try_alternative_entry(&mut self, ui: &mut egui::Ui) {
        if self.manufacturing.trial.is_some() {
            if ui.button("Return to alternative").clicked() {
                self.manufacturing.panel = 5;
            }
            return;
        }
        let reason = if self.state.manufacturing.intent.is_none() {
            Some("Apply Current settings before trying an alternative")
        } else if self.state.manufacturing.context_hash.is_none() {
            Some("Wait for checked Current context")
        } else if !self.manufacturing_current() || self.state.manufacturing.review.is_none() {
            Some(
                "Compile and freshly verify Current before starting a fixed actual-output comparison",
            )
        } else if self.state.busy || self.state.phase != Phase::Running {
            Some("Wait for the current operation")
        } else {
            None
        };
        if ui
            .add_enabled(reason.is_none(), egui::Button::new("Try an alternative"))
            .clicked()
        {
            self.begin_alternative(None);
        }
        if let Some(reason) = reason {
            ui.weak(reason);
            if self.state.manufacturing.intent.is_some()
                && !self.manufacturing_current()
                && ui.button("Prepare Current for comparison").clicked()
            {
                if self.state.manufacturing.artifact.is_some() {
                    self.manufacturing.panel = 2;
                } else {
                    self.show_prepare(2);
                }
            }
        }
    }

    fn begin_alternative(&mut self, option: Option<&crate::manufacture::InspectedOption>) {
        let (Some(project), Some(hello), Some(intent), Some(context)) = (
            &self.state.project,
            &self.state.hello,
            &self.state.manufacturing.intent,
            &self.state.manufacturing.context_hash,
        ) else {
            return;
        };
        if self.manufacturing.trial.is_some() {
            self.local_problem = Some(
                "End the open alternative explicitly before replacing it. Its draft is retained."
                    .into(),
            );
            self.manufacturing.panel = 5;
            return;
        }
        if !self.manufacturing_current() || self.state.manufacturing.review.is_none() {
            self.local_problem = Some("Compile and freshly verify Current before starting a comparison. The option is still available for inspection.".into());
            return;
        }
        let reference_nonce = self
            .state
            .manufacturing
            .review
            .as_ref()
            .expect("checked Current reference")
            .capture
            .nonce;
        self.queued_host = Some(Command::PinAlternativeReference {
            nonce: reference_nonce,
        });
        self.manufacturing.trial = Some(Trial {
            project: project.project_id.clone(),
            session: hello.session_id.clone(),
            revision: option.map_or(project.revision, |option| option.revision),
            context: option.map_or_else(
                || context.clone(),
                |option| option.option.context_hash.clone(),
            ),
            reference_intent: intent.clone(),
            reference_nonce,
            previous_nonce: self
                .state
                .manufacturing
                .alternative_review
                .as_ref()
                .map_or(0, |review| review.capture.nonce),
            value: option.map_or_else(
                || (**intent).clone(),
                |option| (*option.option.intent).clone(),
            ),
            option: option.map(|option| option.option.id.clone()),
            name: option.map_or_else(|| "Alternative".into(), |option| option.option.name.clone()),
            candidate: false,
            editing: true,
            keeping: false,
            ending: false,
            adopting: None,
            height_selection: PhysicalSelection::default(),
        });
        self.manufacturing.panel = 5;
        self.manufacturing.comparison_no_height_match = false;
    }

    pub(super) fn reconcile_alternative_receipt(&mut self) {
        let receipt = self
            .state
            .manufacturing
            .applied_settings
            .as_deref()
            .filter(|receipt| {
                self.state.project.as_ref().is_some_and(|project| {
                    project.project_id == receipt.project && project.revision == receipt.revision
                }) && self.state.manufacturing.intent.as_ref() == Some(&receipt.intent)
                    && self.state.phase == Phase::Running
                    && !self.state.busy
            });
        if self.manufacturing.adopt_confirmed(receipt) {
            self.queued_host = Some(Command::DiscardAlternative);
        } else if self.state.phase == Phase::Running
            && !self.state.busy
            && self.awaiting_serial.is_none()
            && let Some(trial) = self.manufacturing.trial.as_mut()
            && trial
                .adopting
                .as_ref()
                .is_some_and(|adoption| self.state.serial > adoption.submitted_serial)
        {
            trial.adopting = None;
        }
    }

    pub(super) fn selected_manufacturing_review(&self) -> Option<Arc<crate::manufacture::Review>> {
        if self.manufacturing.panel == 5
            && let Some(trial) = &self.manufacturing.trial
        {
            if self.manufacturing.comparison_no_height_match {
                return None;
            }
            if trial.candidate {
                return self
                    .state
                    .manufacturing
                    .alternative_review
                    .clone()
                    .filter(|review| {
                        review.capture.session == trial.session
                            && review.capture.provenance.project_id == trial.project
                            && review.capture.nonce > trial.previous_nonce
                    });
            }
            return self
                .state
                .manufacturing
                .reference_review
                .clone()
                .filter(|review| review.capture.nonce == trial.reference_nonce);
        }
        self.manufacturing_current()
            .then(|| self.state.manufacturing.review.clone())
            .flatten()
    }

    pub(super) fn alternative_pane(&mut self, ui: &mut egui::Ui) {
        let unchanged = self.manufacturing.trial.as_ref().is_some_and(|trial| {
            self.state.manufacturing.intent.as_deref() == Some(&trial.reference_intent)
                && self.state.manufacturing.context_hash.as_ref() == Some(&trial.context)
        });
        ui::pane_header(
            ui,
            "Trying an alternative",
            if unchanged {
                "Current unchanged · source and setup fixed"
            } else {
                "Current changed · earlier fixed comparison, not current output"
            },
        );
        let Some(mut trial) = self.manufacturing.trial.take() else {
            self.manufacturing.panel = 2;
            return;
        };
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            self.manufacturing.trial = Some(trial);
            return;
        };
        let m = self.state.manufacturing.clone();
        let reference = m
            .reference_review
            .as_ref()
            .filter(|review| review.capture.nonce == trial.reference_nonce);
        let prepared = super::policies::prepared(&trial.value);
        let context_matches = trial.project == project.project_id
            && trial.session == hello.session_id
            && m.context_hash.as_ref() == Some(&trial.context)
            && m.intent.as_deref() == Some(&trial.reference_intent);
        let evaluated = m.alternative.as_ref().filter(|evaluation| {
            context_matches
                && evaluation.project_id == trial.project
                && evaluation.context_hash == trial.context
                && prepared.as_ref().is_ok_and(|intent| {
                    same_input_capture(
                        evaluation.captured_revision,
                        project.revision,
                        &evaluation.intent,
                        intent,
                    )
                })
                && m.alternative_review.as_ref().is_some_and(|review| {
                    review.capture.session == trial.session
                        && review.capture.record == evaluation.record
                        && review.capture.nonce > trial.previous_nonce
                        && review.capture.report.verified
                })
        });
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.state.confirmation.is_none()
            && self.awaiting_serial.is_none()
            && !m.alternative_loading;
        let current_drafts = self
            .manufacturing
            .has_intent_draft(&project.project_id, m.intent.as_deref())
            || self.manufacturing.pending_loaded(m.loaded.as_deref())
            || self.material.pending_loaded(
                &project.project_id,
                &hello.session_id,
                m.loaded_material.as_deref(),
            )
            || self
                .material
                .has_changes(&project.project_id, m.intent.as_deref())
            || self.process.has_changes();
        let blocked = if !ready {
            Some(
                "Operation pending or outcome unconfirmed. Resolve it in Activity before continuing.",
            )
        } else if reference.is_none() {
            Some(
                "Current reference reservation is unavailable. End this trial and freshly verify Current before trying again.",
            )
        } else if !context_matches {
            Some(
                "Source, setup or Current settings changed. This fixed comparison cannot be rebased; end it and start a new trial.",
            )
        } else {
            None
        };
        if evaluated.is_some() {
            let reason = blocked.or_else(|| {
                if project.read_only { Some("Read-only project: inspection and evaluation are allowed; Use and Keep are disabled") }
                else if current_drafts { Some("Apply or explicitly discard retained Current Settings/Material/Infill drafts before Use") }
                else { None }
            });
            if ui::primary(ui, "Use these settings", reason) {
                trial.adopting = Some(Adoption {
                    submitted_serial: self.state.serial,
                    base_revision: project.revision,
                    intent: Arc::new(prepared.as_ref().expect("evaluated intent").clone()),
                    after_nonce: m
                        .applied_settings
                        .as_ref()
                        .map_or(0, |receipt| receipt.nonce),
                });
                if let Some(id) = &trial.option
                    && project
                        .manufacturing_options
                        .iter()
                        .any(|option| &option.id == id)
                    && m.inspected_option.as_ref().is_some_and(|option| {
                        option.option.id == *id && option.option.intent.as_ref() == &trial.value
                    })
                {
                    self.queued_project = Some(ProjectCommand::ApplyManufacturingOption {
                        layout_id: project.layout_id.clone(),
                        session_id: trial.session.clone(),
                        base_revision: project.revision,
                        id: id.clone(),
                    });
                } else {
                    self.queued_manufacturing = Some(ManufacturingCommand::SetIntent {
                        layout_id: project.layout_id.clone(),
                        session_id: trial.session.clone(),
                        base_revision: project.revision,
                        intent: Arc::new(prepared.as_ref().expect("evaluated intent").clone()),
                    });
                }
            }
        } else {
            let reason = blocked.or_else(|| prepared.as_ref().err().map(String::as_str));
            if ui::primary(ui, "Evaluate alternative", reason) {
                trial.revision = project.revision;
                if trial.candidate
                    && let (Some(before), Some(after)) = (m.alternative_review.as_ref(), reference)
                {
                    self.manufacturing.comparison_no_height_match = !remap_height(
                        &mut trial.height_selection,
                        &mut self.manufacturing.replay,
                        before,
                        after,
                        !self.manufacturing.comparison_no_height_match,
                    );
                    self.manufacturing.review_nonce = Some(after.capture.nonce);
                }
                trial.candidate = false;
                trial.editing = false;
                self.queued_manufacturing = Some(ManufacturingCommand::EvaluateAlternative {
                    layout_id: project.layout_id.clone(),
                    session_id: trial.session.clone(),
                    base_revision: project.revision,
                    expected_context_hash: trial.context.clone(),
                    intent: Arc::new(prepared.as_ref().expect("validated intent").clone()),
                });
            }
            if m.alternative.is_some() {
                ui.label("Previous result · not the current draft or revision. Evaluate again before Use or Keep.");
            }
        }
        if m.alternative_loading {
            ui.spinner();
            ui.label("Retrieving checked alternative output…");
        }
        if self.state.active_operation {
            ui.label(&self.state.message);
            self.button(ui, Action::Cancel);
            self.button(ui, Action::Resume);
        }
        if project.revision != trial.revision {
            ui.label("Project revision changed · explicitly evaluate again for this revision");
        }
        if current_drafts {
            ui.weak("Unapplied Current drafts are retained separately. This trial did not consume them.");
        }
        let action = trial_actions(ui, ready, evaluated.map(|_| ready && !project.read_only));
        let reveal_decision = matches!(action, Some(TrialAction::Keep | TrialAction::End));
        match action {
            Some(TrialAction::Back) => self.manufacturing.panel = 2,
            Some(TrialAction::Keep) => trial.keeping = true,
            Some(TrialAction::End) => trial.ending = true,
            None => {}
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut trial.editing, true, "Settings");
            ui.selectable_value(&mut trial.editing, false, "Compare");
        });
        let mut discarded = false;
        egui::ScrollArea::vertical().id_salt("alternative-body").show(ui, |ui| {
        if reveal_decision {
            ui.scroll_to_cursor(Some(egui::Align::Min));
        }
        if trial.keeping {
            ui::text(ui, "Option name", &mut trial.name);
            let option = evaluated.and_then(|evaluation| {
                ManufacturingOption::new(
                    trial.name.clone(),
                    trial.context.clone(),
                    evaluation.intent.clone(),
                )
                .ok()
            });
            let room = project.manufacturing_options.len()
                < spiling_contracts::project::MAX_MANUFACTURING_OPTIONS;
            if ui
                .add_enabled(
                    ready && !project.read_only && room && option.is_some(),
                    egui::Button::new("Keep named option"),
                )
                .clicked()
            {
                self.queued_project = Some(ProjectCommand::RetainManufacturingOption {
                    layout_id: project.layout_id.clone(),
                    session_id: trial.session.clone(),
                    base_revision: project.revision,
                    option: option.expect("checked option"),
                });
                trial.keeping = false;
            }
            if !room {
                ui.label("Option limit reached. Remove an existing option first.");
            }
            if option_name_problem(&trial.name) {
                ui.label("Use 1–128 bytes, no surrounding whitespace or control characters.");
            }
            ui.weak("Keep is an explicit, undoable project change. It does not apply settings or replace Current output.");
            if ui.button("Cancel Keep").clicked() {
                trial.keeping = false;
            }
        }
        if trial.ending {
            ui.label("Discard this trial and its comparison? Current settings and retained options are unchanged.");
            if ui.button("Discard trial and return to Current").clicked() {
                self.manufacturing.panel = 2;
                self.queued_host = Some(Command::DiscardAlternative);
                discarded = true;
                return;
            }
            if ui.button("Keep trying").clicked() {
                trial.ending = false;
            }
        }
            if trial.editing {
                ui.push_id("alternative-settings", |ui| {
                    ui.add_enabled_ui(ready, |ui| super::manufacturing::settings_fields(ui, &mut trial.value, self.prefs.theme.tokens()));
                });
            } else {
                ui::group(ui, "Shared canvas");
                let previous = trial.candidate;
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut trial.candidate, false, "A · Current reference");
                    ui.selectable_value(&mut trial.candidate, true, "B · Alternative");
                });
                if previous != trial.candidate {
                    let before = if previous { m.alternative_review.as_ref() } else { reference };
                    let after = if trial.candidate { m.alternative_review.as_ref() } else { reference };
                    if let (Some(before), Some(after)) = (before, after) {
                        self.manufacturing.comparison_no_height_match = !remap_height(
                            &mut trial.height_selection,
                            &mut self.manufacturing.replay,
                            before,
                            after,
                            !self.manufacturing.comparison_no_height_match,
                        );
                        self.manufacturing.review_nonce = Some(after.capture.nonce);
                    }
                }
                ui.weak("One camera and visibility filters. A/B carries physical-height selection, not a layer-number match.");
                if let Some((lower, upper)) = trial.height_selection.interval {
                    ui.label(format!("Comparison height {lower:.4}–{upper:.4} mm · showing overlapping layers"));
                }
                if self.manufacturing.comparison_no_height_match {
                    ui.label("No corresponding deposition height in this result. Motion is hidden, not expanded to All.");
                    if ui.button("Explicitly show all heights").clicked() {
                        self.manufacturing.comparison_no_height_match = false;
                        self.manufacturing.replay.layer = None;
                        self.manufacturing.replay.layer_end = None;
                        self.manufacturing.replay.events = u32::MAX;
                        trial.height_selection = PhysicalSelection::default();
                    }
                }
                actual_stats(ui, "A · Fixed Current reference", reference.map(Arc::as_ref));
                actual_stats(ui, "B · Alternative result", m.alternative_review.as_deref());
                ui::group(ui, "Exact changed settings");
                for change in settings_changes(&trial.reference_intent, &trial.value) { ui.label(change); }
                if let Some(evaluation) = &m.alternative {
                    ui.collapsing("Captured input and software evidence", |ui| {
                        ui.label(format!("Project revision {} · context {}", evaluation.captured_revision.0, evaluation.context_hash.as_str()));
                        ui.label(format!("{:#?}", evaluation.provenance));
                        ui.label(if evaluation.changed_inputs_complete { "Complete engine input-change receipt" } else { "Engine input-change receipt is truncated; inspect complete settings below" });
                        for change in &evaluation.changed_inputs {
                            ui.label(format!("{}: {} → {}", change.path, change.before.as_deref().unwrap_or("absent"), change.after.as_deref().unwrap_or("absent")));
                        }
                        ui.label(format!("{:#?}", evaluation.report));
                    });
                }
            }
        });
        if !discarded {
            self.manufacturing.trial = Some(trial);
        }
    }

    pub(super) fn manufacturing_options(&mut self, ui: &mut egui::Ui) {
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        if project.manufacturing_options.is_empty() {
            return;
        }
        ui::group(ui, "Kept options");
        for option in &project.manufacturing_options {
            ui.push_id(option.id.as_str(), |ui| {
                if ui.button(format!("Inspect {}", option.name)).clicked() {
                    self.manufacturing.inspected_option = Some(option.id.clone());
                    self.manufacturing.panel = 6;
                    self.queued_project = Some(ProjectCommand::GetManufacturingOption {
                        layout_id: project.layout_id.clone(),
                        session_id: hello.session_id.clone(),
                        base_revision: project.revision,
                        id: option.id.clone(),
                    });
                }
                if self.state.manufacturing.context_hash.as_ref() != Some(&option.context_hash) {
                    ui.weak("Different source/setup context · inspect only");
                }
            });
        }
        ui.weak("Inspection never applies an option. Saved results require fresh evaluation after reopening.");
    }

    pub(super) fn option_pane(&mut self, ui: &mut egui::Ui) {
        ui::pane_header(
            ui,
            "Inspect kept option",
            "Read-only inspection · Current unchanged",
        );
        if ui::close(ui, "Back to Review") {
            self.manufacturing.panel = 2;
        }
        let selected = self
            .state
            .manufacturing
            .inspected_option
            .clone()
            .filter(|option| {
                self.manufacturing.inspected_option.as_ref() == Some(&option.option.id)
            });
        let Some(option) = selected else {
            ui.label("Loading the selected option. If the request failed, return to Review and select it again.");
            return;
        };
        ui.heading(&option.option.name);
        let current = self
            .state
            .project
            .as_ref()
            .zip(self.state.hello.as_ref())
            .is_some_and(|(project, hello)| {
                project.project_id == option.project
                    && project.revision == option.revision
                    && hello.session_id == option.session
            });
        if !current {
            ui.label("Inspection is from an earlier revision. Return to Review and inspect again before editing or removing.");
        }
        if self.state.manufacturing.context_hash.as_ref() != Some(&option.option.context_hash) {
            ui.label("Source/setup context mismatch. This option remains inspectable; it cannot be rebound to the new context.");
        }
        let ready = current
            && !self.state.busy
            && self.state.phase == Phase::Running
            && self.awaiting_serial.is_none();
        let reason = if !ready {
            Some("Wait for the operation, or fetch this option at the current revision")
        } else if self.manufacturing.trial.is_some() {
            Some("End the open alternative before replacing its draft")
        } else if !self.manufacturing_current() || self.state.manufacturing.review.is_none() {
            Some("Compile and freshly verify Current before comparing this option")
        } else {
            None
        };
        if ui::primary(ui, "Edit a copy in alternative", reason) {
            self.begin_alternative(Some(&option));
        }
        if !self.manufacturing_current() && ui.button("Prepare Current for comparison").clicked() {
            if self.state.manufacturing.artifact.is_some() {
                self.manufacturing.panel = 2;
            } else {
                self.show_prepare(2);
            }
        }
        let writable = ready
            && self
                .state
                .project
                .as_ref()
                .is_some_and(|project| !project.read_only);
        if ui
            .add_enabled(writable, egui::Button::new("Remove kept option"))
            .clicked()
        {
            self.queued_project = Some(ProjectCommand::RemoveManufacturingOption {
                layout_id: self
                    .state
                    .project
                    .as_ref()
                    .expect("writable project")
                    .layout_id
                    .clone(),
                session_id: option.session.clone(),
                base_revision: option.revision,
                id: option.option.id.clone(),
            });
            self.manufacturing.panel = 2;
        }
        if !writable {
            ui.weak("Remove requires a writable project and current inspection");
        }
        egui::ScrollArea::vertical()
            .id_salt("option-inspection")
            .show(ui, |ui| {
                ui.label("Stored settings only · no fresh manufacturing authority");
                ui.label(serde_json::to_string_pretty(&option.option.intent).unwrap_or_default());
            });
    }
}

fn option_name_problem(name: &str) -> bool {
    name.is_empty() || name.len() > 128 || name.trim() != name || name.chars().any(char::is_control)
}

pub(crate) fn gallery(ui: &mut egui::Ui, tokens: ui::Tokens) {
    ui::pane_header(
        ui,
        "Trying an alternative",
        "Current unchanged · illustrative values, no engine requests",
    );
    let state_id = ui.make_persistent_id("alternative-gallery-state");
    let mut state = ui.data_mut(|data| data.get_temp::<u8>(state_id).unwrap_or(0));
    ui.horizontal_wrapped(|ui| {
        for (index, label) in [
            "Draft",
            "Evaluated",
            "Stale",
            "Pending",
            "Uncertain",
            "Failed",
            "Read-only",
            "No reference",
            "Context mismatch",
        ]
        .iter()
        .enumerate()
        {
            ui.selectable_value(&mut state, index as u8, *label);
        }
    });
    ui.data_mut(|data| data.insert_temp(state_id, state));
    let reason = match state {
        3 => Some("Evaluation pending. Current remains applied; Cancel is explicit."),
        4 => Some(
            "Outcome unconfirmed. Resume observation in Activity; never retry Use automatically.",
        ),
        6 => Some("Read-only project: evaluation and inspection allowed; Keep and Use disabled."),
        7 => Some("Compile and freshly verify Current before starting a comparison."),
        8 => Some(
            "Source/setup context mismatch. End this trial and start another; no automatic rebase.",
        ),
        _ => None,
    };
    ui::primary(
        ui,
        if matches!(state, 1 | 6) {
            "Use these settings"
        } else {
            "Evaluate alternative"
        },
        reason,
    );
    if state == 7 {
        let _ = ui.button("Prepare Current for comparison");
    }
    if state == 3 {
        let _ = ui.button("Cancel operation");
    }
    if state == 4 {
        let _ = ui.button("Resume observation");
    }
    if state == 2 {
        ui.label("Previous result · draft or revision changed. Evaluate again before Use or Keep.");
    }
    if state == 5 {
        ui::problem(
            ui,
            "Alternative could not be evaluated",
            "Native refusal · previous candidate and fixed Current reference retained. Correct the draft and Evaluate again.",
        );
    }
    let _ = trial_actions(ui, state != 3, matches!(state, 1 | 6).then_some(state != 6));
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("alternative-gallery-body")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let _ = ui.selectable_label(true, "A · Fixed Current reference");
                let _ = ui.selectable_label(false, "B · Alternative");
            });
            ui.label("Shared camera, visibility filters and physical-height navigation");
            ui.label("Illustrative actual replay: 31 layers · 0.2–0.24 mm physical heights");
            ui.label("Exact changed settings · recipe.infill_fraction: 0.2 → 0.31");
            ui.weak(
                "Actual seam markers are decoded emitted starts, not physical-print guarantees.",
            );
            let draft_id = ui.make_persistent_id("alternative-gallery-draft");
            let mut draft = ui
                .data_mut(|data| data.get_temp::<ManufacturingIntent>(draft_id))
                .unwrap_or_else(|| {
                    let mut intent =
                        spiling_contracts::manufacturing::decode_intent(include_bytes!(
                            "../../../../fixtures/manufacturing/complete-sparse.intent.json"
                        ))
                        .expect("gallery intent");
                    intent.recipe.infill_fraction = 0.31;
                    intent
                });
            ui.push_id("alternative-settings", |ui| {
                super::manufacturing::settings_fields(ui, &mut draft, tokens)
            });
            ui.data_mut(|data| data.insert_temp(draft_id, draft));
            ui.weak("NOT MACHINE READY · only illustrative software evidence");
        });
}

fn same_input_capture(
    captured_revision: ProjectRevision,
    current_revision: ProjectRevision,
    captured: &ManufacturingIntent,
    draft: &ManufacturingIntent,
) -> bool {
    captured_revision == current_revision && captured == draft
}

fn actual_stats(ui: &mut egui::Ui, title: &str, review: Option<&crate::manufacture::Review>) {
    ui::group(ui, title);
    let Some(review) = review else {
        ui.weak("No fresh actual output captured for this reference");
        return;
    };
    let report = &review.capture.report;
    ui.label(format!(
        "Software verification: {} · {} actual layers",
        if report.verified { "passed" } else { "failed" },
        review.bundle.plan.layers.len()
    ));
    ui.label(format!(
        "{} deposition / {} travel segments",
        report.deposition_segments, report.travel_segments
    ));
    ui.label(format!(
        "{:.3} mm³ deposited · {:.3} mm filament",
        report.deposited_volume_mm3, report.filament_length_mm
    ));
    ui.label(format!(
        "Physical layer heights {:.4}–{:.4} mm",
        report.layer_height.realized_min_mm, report.layer_height.realized_max_mm
    ));
    ui.weak("Decoded emitted program · not measured print time, physical mass, printability or machine readiness");
}

fn settings_changes(current: &ManufacturingIntent, candidate: &ManufacturingIntent) -> Vec<String> {
    fn compare(
        path: &str,
        before: &serde_json::Value,
        after: &serde_json::Value,
        changes: &mut Vec<String>,
    ) {
        if before == after {
            return;
        }
        if let (Some(before), Some(after)) = (before.as_object(), after.as_object()) {
            for key in before
                .keys()
                .chain(after.keys())
                .collect::<std::collections::BTreeSet<_>>()
            {
                compare(
                    &format!("{path}{key}."),
                    before.get(key).unwrap_or(&serde_json::Value::Null),
                    after.get(key).unwrap_or(&serde_json::Value::Null),
                    changes,
                );
            }
        } else {
            changes.push(format!(
                "{}: {before} → {after}",
                path.trim_end_matches('.')
            ));
        }
    }
    let mut changes = Vec::new();
    compare(
        "",
        &serde_json::to_value(current).expect("typed intent"),
        &serde_json::to_value(candidate).expect("typed intent"),
        &mut changes,
    );
    if changes.is_empty() {
        changes.push("No settings changed".into());
    }
    changes
}

#[derive(Default)]
struct PhysicalSelection {
    interval: Option<(f64, f64)>,
    projected: Option<(Option<u16>, Option<u16>)>,
}

impl PhysicalSelection {
    fn remap(
        &mut self,
        view: &mut crate::replay::View,
        before: impl Iterator<Item = (f64, f64)>,
        after: impl Iterator<Item = (f64, f64)>,
        observe_selection: bool,
    ) -> bool {
        let selected = (view.layer, view.layer_end);
        if observe_selection && self.projected != Some(selected) {
            self.interval = None;
            if let Some(first) = view.layer {
                let last = view.layer_end.unwrap_or(first);
                let mut lower = None;
                let mut upper = None;
                for (index, (bottom, top)) in before.enumerate() {
                    if index == usize::from(first) {
                        lower = Some(bottom);
                    }
                    if index == usize::from(last) {
                        upper = Some(top);
                    }
                }
                self.interval = lower.zip(upper);
            }
        }
        if let Some((lower, upper)) = self.interval {
            let Some((first, last)) = overlapping_layers(after, lower, upper) else {
                return false;
            };
            view.layer = Some(first as u16);
            view.layer_end = (first != last).then_some(last as u16);
        } else {
            view.layer = None;
            view.layer_end = None;
        }
        self.projected = Some((view.layer, view.layer_end));
        true
    }
}

fn remap_height(
    selection: &mut PhysicalSelection,
    view: &mut crate::replay::View,
    before: &crate::manufacture::Review,
    after: &crate::manufacture::Review,
    observe_selection: bool,
) -> bool {
    if !selection.remap(
        view,
        before
            .bundle
            .plan
            .layers
            .iter()
            .map(|layer| (layer.lower_z_mm, layer.z_mm)),
        after
            .bundle
            .plan
            .layers
            .iter()
            .map(|layer| (layer.lower_z_mm, layer.z_mm)),
        observe_selection,
    ) {
        return false;
    }
    view.events = crate::layers::event_window(after, *view).end;
    view.selected_seam = None;
    true
}

fn overlapping_layers(
    layers: impl Iterator<Item = (f64, f64)>,
    lower: f64,
    upper: f64,
) -> Option<(usize, usize)> {
    let mut range = None;
    for (index, (bottom, top)) in layers.enumerate() {
        if top > lower && bottom < upper {
            range = Some((range.map_or(index, |(first, _)| first), index));
        }
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> ManufacturingIntent {
        spiling_contracts::manufacturing::decode_intent(include_bytes!(
            "../../../../fixtures/manufacturing/complete-sparse.intent.json"
        ))
        .unwrap()
    }

    #[test]
    fn alternative_changes_never_reuse_a_successful_old_capture() {
        let current = intent();
        let mut candidate = current.clone();
        let revision = ProjectRevision(7);
        assert!(same_input_capture(revision, revision, &current, &candidate));
        candidate.recipe.infill_fraction = 0.31;
        assert!(!same_input_capture(
            revision, revision, &current, &candidate
        ));
        assert!(!same_input_capture(
            revision,
            ProjectRevision(8),
            &candidate,
            &candidate
        ));
        candidate = current.clone();
        candidate.recipe.seam_policy = spiling_contracts::manufacturing::SeamPolicy::Distributed;
        assert!(!same_input_capture(
            revision, revision, &current, &candidate
        ));
        assert_eq!(current, intent());
    }

    #[test]
    fn alternative_summary_retains_exact_changed_values_and_full_intent() {
        let current = intent();
        let mut candidate = current.clone();
        candidate.recipe.infill_fraction = 0.310000000001;
        let changes = settings_changes(&current, &candidate);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("recipe.infill_fraction"));
        assert!(changes[0].contains("0.310000000001"));
        assert_eq!(candidate.printer, current.printer);
        assert_eq!(candidate.material_profile, current.material_profile);
        assert_eq!(candidate.hardware, current.hardware);
        for name in ["", " ends ", "bad\nname", &"界".repeat(43)] {
            assert!(option_name_problem(name));
        }
        assert!(!option_name_problem("Thicker walls — 試行"));
    }

    #[test]
    fn alternative_physical_height_without_overlap_never_selects_all_or_nearest() {
        let layers = [(0.0, 0.2), (0.6, 0.8)];
        assert_eq!(overlapping_layers(layers.into_iter(), 0.3, 0.5), None);
        assert_eq!(overlapping_layers(layers.into_iter(), 1.0, 1.2), None);
        assert_eq!(
            overlapping_layers(layers.into_iter(), 0.1, 0.7),
            Some((0, 1))
        );
        assert_eq!(overlapping_layers(layers.into_iter(), 0.2, 0.6), None);
    }

    #[test]
    fn alternative_physical_selection_does_not_expand_on_repeated_switches() {
        let reference = [(0.0, 0.2), (0.2, 0.4), (0.4, 0.6), (0.6, 0.8)];
        let candidate = [(0.0, 0.24), (0.24, 0.48), (0.48, 0.72), (0.72, 0.96)];
        for end in [None, Some(2)] {
            let mut selection = PhysicalSelection::default();
            let mut view = crate::replay::View {
                layer: Some(1),
                layer_end: end,
                ..Default::default()
            };
            for _ in 0..20 {
                assert!(selection.remap(
                    &mut view,
                    reference.into_iter(),
                    candidate.into_iter(),
                    true
                ));
                assert_eq!(
                    (view.layer, view.layer_end),
                    (Some(0), Some(end.unwrap_or(1)))
                );
                assert!(selection.remap(
                    &mut view,
                    candidate.into_iter(),
                    reference.into_iter(),
                    true
                ));
                assert_eq!((view.layer, view.layer_end), (Some(1), end));
            }
            view.layer = Some(3);
            view.layer_end = None;
            assert!(selection.remap(
                &mut view,
                reference.into_iter(),
                candidate.into_iter(),
                true
            ));
            assert_eq!((view.layer, view.layer_end), (Some(2), Some(3)));
            assert_eq!(selection.interval, Some((0.6, 0.8)));
        }
    }

    #[test]
    fn alternative_missing_height_retains_original_selection_when_switching_back() {
        let reference = [(0.0, 0.2), (0.2, 0.4)];
        let candidate = [(0.6, 0.8)];
        let mut selection = PhysicalSelection::default();
        let mut view = crate::replay::View {
            layer: Some(1),
            ..Default::default()
        };
        assert!(!selection.remap(
            &mut view,
            reference.into_iter(),
            candidate.into_iter(),
            true
        ));
        assert_eq!(selection.interval, Some((0.2, 0.4)));
        assert!(selection.remap(
            &mut view,
            candidate.into_iter(),
            reference.into_iter(),
            false
        ));
        assert_eq!((view.layer, view.layer_end), (Some(1), None));
    }

    #[test]
    fn alternative_use_only_adopts_exact_confirmed_receipt_and_returns_to_current() {
        let intent = Arc::new(intent());
        let project = ProjectId::new();
        let session = SessionId::new();
        let mut ui = super::super::manufacturing::ManufacturingUi::default();
        ui.panel = 5;
        ui.trial = Some(Trial {
            project: project.clone(),
            session: session.clone(),
            revision: ProjectRevision(7),
            context: SourceHash::from_bytes(b"fixed context"),
            reference_intent: intent.clone(),
            reference_nonce: 1,
            previous_nonce: 0,
            value: (*intent).clone(),
            option: None,
            name: "Trial".into(),
            candidate: true,
            editing: false,
            keeping: false,
            ending: false,
            height_selection: PhysicalSelection::default(),
            adopting: Some(Adoption {
                base_revision: ProjectRevision(8),
                intent: intent.clone(),
                after_nonce: 4,
                submitted_serial: 20,
            }),
        });
        assert!(!ui.adopt_confirmed(None));
        let mut receipt = crate::manufacture::AppliedSettings {
            nonce: 4,
            project,
            session,
            base_revision: ProjectRevision(8),
            revision: ProjectRevision(9),
            intent,
        };
        assert!(!ui.adopt_confirmed(Some(&receipt)));
        receipt.nonce = 5;
        receipt.base_revision = ProjectRevision(7);
        assert!(!ui.adopt_confirmed(Some(&receipt)));
        receipt.base_revision = ProjectRevision(8);
        assert!(ui.adopt_confirmed(Some(&receipt)));
        assert!(ui.trial.is_none());
        assert_eq!(ui.panel, 2);
    }

    #[test]
    fn alternative_gallery_reflows_complete_states_in_both_themes() {
        for theme in [ui::Theme::Graphite, ui::Theme::Paper] {
            for width in [240.0, 320.0, 400.0] {
                for scale in [1.0, 1.5, 2.0] {
                    let context = egui::Context::default();
                    theme.apply(&context);
                    context.set_zoom_factor(scale);
                    for state in 0..9u8 {
                        let mut overflow = false;
                        context
                            .run_ui(
                                egui::RawInput {
                                    screen_rect: Some(egui::Rect::from_min_size(
                                        egui::Pos2::ZERO,
                                        egui::vec2(width * scale, 1000.0 * scale),
                                    )),
                                    ..Default::default()
                                },
                                |ui| {
                                    let right = ui.max_rect().right();
                                    let id = ui.make_persistent_id("alternative-gallery-state");
                                    ui.data_mut(|data| data.insert_temp(id, state));
                                    gallery(ui, theme.tokens());
                                    overflow = ui.min_rect().right() > right + 1.0;
                                },
                            )
                            .drop_without_applying_deltas();
                        assert!(!overflow, "{theme:?}, {width}, {scale}, {state}");
                    }
                }
            }
        }
    }
}
