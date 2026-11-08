// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub(super) enum Navigation {
    Action(Action),
    Face(FaceRef),
    Occurrence(OccurrenceId),
    AnalysisCase(spiling_contracts::analysis::AnalysisCaseId),
    InfillScope(bool, bool),
    ClearSelection,
    Plate(spiling_contracts::project::PrintLayoutId),
}
fn select_occurrence(
    selected: &mut Option<FaceRef>,
    occurrence: &mut Option<OccurrenceId>,
    pose_revision: &mut Option<SceneRevision>,
    subject_drafts: bool,
    target: OccurrenceId,
) -> bool {
    if *occurrence != Some(target) && subject_drafts {
        return false;
    }
    *selected = None;
    *occurrence = Some(target);
    *pose_revision = None;
    true
}
impl Workbench {
    pub(super) fn project_drafts(&self) -> bool {
        self.state.project.as_ref().is_some_and(|p| {
            self.mechanics.has_changes()
                || self.manufacturing.trial.is_some()
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
            Action::Undo
                | Action::Redo
                | Action::New
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
        let subject_drafts = self.subject_drafts();
        if !select_occurrence(
            &mut self.selected,
            &mut self.occurrence,
            &mut self.pose_revision,
            subject_drafts,
            id,
        ) {
            self.navigation = Some(Navigation::Occurrence(id));
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
            ui.label("These changes have not been applied. Keep editing, or discard the drafts to continue.");
            if self.manufacturing.trial.is_some() { ui.weak("Keep an alternative explicitly before leaving if you want to retain it."); }
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
                    self.mechanics.discard();
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
                Navigation::Plate(id) => {
                    self.mechanics.discard();
                    self.discard_preparation_drafts();
                    self.material.discard(
                        self.state
                            .manufacturing
                            .loaded_material
                            .as_ref()
                            .map(|m| m.nonce),
                    );
                    self.select_plate(id);
                }
                Navigation::Face(reference) => self.select_face(reference),
                Navigation::Occurrence(id) => self.choose_occurrence(id),
                Navigation::AnalysisCase(id) => self.choose_analysis_case(id),
                Navigation::ClearSelection => self.clear_selection(),
                Navigation::InfillScope(definition, boxed) => {
                    self.process.set_scope(definition, boxed)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected_face(occurrence_id: OccurrenceId) -> FaceRef {
        FaceRef {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(3),
            occurrence_id,
            definition_id: DefinitionId::from_source_sha256(&[42; 32]),
            face_id: FaceId::from_step_entity(1).unwrap(),
        }
    }

    #[test]
    fn analysis_case_subject_selection_invalidates_the_other_parts_pose_cache() {
        let original = OccurrenceId::new(1).unwrap();
        let target = OccurrenceId::new(2).unwrap();
        let mut selected = Some(selected_face(original));
        let mut occurrence = Some(original);
        let mut pose_revision = Some(SceneRevision(3));
        assert!(select_occurrence(
            &mut selected,
            &mut occurrence,
            &mut pose_revision,
            false,
            target,
        ));
        assert_eq!(occurrence, Some(target));
        assert!(selected.is_none());
        assert!(pose_revision.is_none());
    }

    #[test]
    fn analysis_case_subject_selection_preserves_dirty_pose_until_discard() {
        let original = OccurrenceId::new(1).unwrap();
        let target = OccurrenceId::new(2).unwrap();
        let face = selected_face(original);
        let mut selected = Some(face.clone());
        let mut occurrence = Some(original);
        let mut pose_revision = Some(SceneRevision(3));
        assert!(!select_occurrence(
            &mut selected,
            &mut occurrence,
            &mut pose_revision,
            true,
            target,
        ));
        assert_eq!(occurrence, Some(original));
        assert_eq!(selected, Some(face));
        assert_eq!(pose_revision, Some(SceneRevision(3)));
        assert!(select_occurrence(
            &mut selected,
            &mut occurrence,
            &mut pose_revision,
            false,
            target,
        ));
        assert_eq!(occurrence, Some(target));
        assert!(selected.is_none());
        assert!(pose_revision.is_none());
    }
}
