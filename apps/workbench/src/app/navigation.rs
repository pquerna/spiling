// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub(super) enum Navigation {
    Action(Action),
    Face(FaceRef),
    Occurrence(OccurrenceId),
    InfillScope(bool, bool),
    ClearSelection,
}
impl Workbench {
    pub(super) fn project_drafts(&self) -> bool {
        self.state.project.as_ref().is_some_and(|p| {
            self.manufacturing.trial.is_some()
                || self
                    .manufacturing
                    .has_intent_draft(&p.project_id, self.state.manufacturing.intent.as_deref())
                || self
                    .manufacturing
                    .pending_loaded(self.state.manufacturing.loaded.as_deref())
                || self
                    .material
                    .has_changes(&p.project_id, self.state.manufacturing.intent.as_deref())
                || self.state.hello.as_ref().is_some_and(|hello| {
                    self.material.pending_loaded(
                        &p.project_id,
                        &hello.session_id,
                        self.state.manufacturing.loaded_material.as_deref(),
                    )
                })
        })
    }
    pub(super) fn guard_action(&mut self, action: Action) -> bool {
        if action == Action::Close
            && (self.library.pending_save()
                || self.state.library.hardware.credential_pending.is_some())
        {
            self.library.open = true;
            self.local_problem = Some("Resolve the original library or credential save before closing; its outcome is unconfirmed.".into());
            return true;
        }
        if action == Action::Close && self.library.has_drafts(&self.state.library) {
            self.navigation = Some(Navigation::Action(action));
            return true;
        }
        if matches!(
            action,
            Action::New
                | Action::Open
                | Action::OpenReadOnly
                | Action::Recover
                | Action::Restart
                | Action::Close
        ) && (self.project_drafts() || self.subject_drafts())
        {
            self.navigation = Some(Navigation::Action(action));
            true
        } else {
            false
        }
    }
    pub(super) fn subject_drafts(&self) -> bool {
        self.surface.has_changes() || self.process.has_changes() || self.pose != self.pose_original
    }
    pub(super) fn choose_occurrence(&mut self, id: OccurrenceId) {
        if self.occurrence != Some(id) && self.subject_drafts() {
            self.navigation = Some(Navigation::Occurrence(id));
        } else {
            self.occurrence = Some(id);
            self.pose_revision = None;
        }
    }
    pub(super) fn navigation_dialog(&mut self, ctx: &egui::Context) {
        if self.navigation.is_none() {
            return;
        }
        if self.navigation_focus.is_none() {
            self.navigation_focus = ctx.memory(|m| m.focused());
        }
        let mut discard = false;
        let mut keep = false;
        let modal = egui::Modal::new(egui::Id::unique("draft-navigation")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading("Leave unapplied changes?");
            ui.label("Back retains drafts. Apply normal drafts or explicitly Keep an evaluated alternative before leaving. Discarding leaves Current settings and kept options unchanged.");
            if let Some(Navigation::Action(action)) = &self.navigation { ui.label(format!("Next: {}", action.label())); }
            keep = ui::primary(ui, "Keep editing", None);
            discard = ui.button(egui::RichText::new("Discard drafts and continue").color(self.prefs.theme.tokens().warning)).clicked();
        });
        if keep || modal.should_close() {
            self.navigation = None;
            if let Some(id) = self.navigation_focus.take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        } else if discard && let Some(next) = self.navigation.take() {
            if let Some(id) = self.navigation_focus.take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
            self.surface.discard();
            self.process.discard();
            self.pose_revision = None;
            self.pose = self.pose_original;
            match next {
                Navigation::Action(action) => {
                    if action == Action::Close {
                        self.library.discard_drafts();
                    }
                    self.discard_preparation_drafts();
                    self.material.discard(
                        self.state
                            .manufacturing
                            .loaded_material
                            .as_ref()
                            .map(|m| m.nonce),
                    );
                    self.queued = Some(action);
                }
                Navigation::Face(reference) => self.select_face(reference),
                Navigation::Occurrence(id) => self.choose_occurrence(id),
                Navigation::ClearSelection => self.clear_selection(),
                Navigation::InfillScope(definition, boxed) => {
                    self.process.set_scope(definition, boxed)
                }
            }
        }
    }
}
