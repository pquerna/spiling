// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use spiling_contracts::{
    manufacturing::*,
    project::{ProjectId, ProjectRevision},
};

pub(super) struct InteriorPolicyDraft {
    project: ProjectId,
    session: SessionId,
    revision: ProjectRevision,
    pub edit: InteriorToolPolicyEdit,
    pub open: bool,
    assessment: Option<InteriorPolicyAssessment>,
    submitted: Option<(u64, u64)>,
}
impl InteriorPolicyDraft {
    pub(super) fn new(
        project: ProjectId,
        session: SessionId,
        revision: ProjectRevision,
        occurrence_id: OccurrenceId,
        definition: DefinitionId,
        expected: Option<InteriorToolOverride>,
    ) -> Self {
        Self {
            project,
            session,
            revision,
            edit: InteriorToolPolicyEdit {
                occurrence_id,
                expected_definition_id: definition,
                expected,
                value: Some(expected.unwrap_or(InteriorToolOverride {
                    enabled: true,
                    value: InteriorToolPolicy::PrimaryOnly,
                })),
            },
            open: true,
            assessment: None,
            submitted: None,
        }
    }
    fn matches(&self, receipt: &crate::manufacture::AppliedSettings) -> bool {
        self.submitted
            .is_some_and(|(_, nonce)| receipt.nonce > nonce)
            && receipt.project == self.project
            && receipt.session == self.session
            && receipt.base_revision == self.revision
            && receipt.interior_edit.as_ref() == Some(&self.edit)
    }
    pub(super) fn body(&mut self, ui: &mut egui::Ui, editable: bool) {
        if !self.open {
            return;
        }
        ui.separator();
        ui::group(ui, "Interior tool policy");
        ui.weak("Unapplied · this part");
        if let Some(assessment) = &self.assessment
            && self.edit == assessment.opportunity.edit
        {
            comparison(ui, assessment);
        }
        ui.add_enabled_ui(editable && self.submitted.is_none(), |ui| {
            let mut defaults = self.edit.value.is_none();
            if ui.checkbox(&mut defaults, "Use defaults").changed() {
                self.edit.value = if defaults {
                    None
                } else {
                    Some(self.edit.expected.unwrap_or(InteriorToolOverride {
                        enabled: true,
                        value: InteriorToolPolicy::PrimaryOnly,
                    }))
                };
            }
            if let Some(value) = &mut self.edit.value {
                ui.checkbox(&mut value.enabled, "Enabled");
                ui.selectable_value(&mut value.value, InteriorToolPolicy::Automatic, "Automatic");
                ui.selectable_value(
                    &mut value.value,
                    InteriorToolPolicy::PrimaryOnly,
                    "Primary tool only",
                );
            } else {
                ui::resolved_value(ui, "Policy", "Automatic", "Plate defaults");
            }
        });
        ui::help(
            ui,
            "interior-policy",
            "Interior tool policy",
            "Automatic compares qualified constructions using the captured job objective. Primary tool only retains the other installed tool's clearance and waste obligations. A disabled modifier reveals Automatic; an explicit Automatic pin remains a modifier.",
        );
        if ui.button("Close").clicked() {
            self.open = false;
        }
    }
}
impl Workbench {
    pub(super) fn reconcile_interior_receipt(&mut self) {
        let Some(draft) = &mut self.process.interior else {
            return;
        };
        let confirmed = self
            .state
            .manufacturing
            .applied_settings
            .as_deref()
            .is_some_and(|r| {
                draft.matches(r)
                    && self
                        .state
                        .project
                        .as_ref()
                        .is_some_and(|p| p.project_id == r.project && p.revision == r.revision)
                    && self
                        .state
                        .hello
                        .as_ref()
                        .is_some_and(|h| h.session_id == r.session)
            });
        if confirmed {
            self.process.interior = None;
        } else if draft
            .submitted
            .is_some_and(|(serial, _)| self.state.serial > serial && !self.state.busy)
        {
            draft.submitted = None;
        }
    }
    pub(super) fn interior_policy_actions(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = &mut self.process.interior else {
            return;
        };
        let current = self
            .state
            .project
            .as_ref()
            .zip(self.state.hello.as_ref())
            .is_some_and(|(p, h)| {
                p.project_id == draft.project
                    && p.revision == draft.revision
                    && h.session_id == draft.session
            });
        if !draft.open {
            if ui::primary(ui, "Resume modifier", None) {
                draft.open = true;
            }
        } else {
            let reason = if draft.submitted.is_some() {
                Some("Awaiting modifier receipt")
            } else if !current {
                Some("Project changed · discard this draft to reload")
            } else if self.state.project.as_ref().is_some_and(|p| p.read_only) {
                Some("Project is read-only")
            } else if self.state.phase != Phase::Running
                || self.state.busy
                || self.state.picker_open
                || self.awaiting_serial.is_some()
            {
                Some("Wait for the current operation")
            } else {
                None
            };
            let label = if draft.edit.value.is_none() {
                "Use defaults"
            } else if draft.edit.expected.is_some() {
                "Apply changes"
            } else {
                "Add modifier"
            };
            if ui::primary(ui, label, reason) {
                let layout_id = self
                    .state
                    .project
                    .as_ref()
                    .expect("current project")
                    .layout_id
                    .clone();
                self.queued_manufacturing = Some(ManufacturingCommand::EditInteriorPolicy {
                    layout_id,
                    session_id: draft.session.clone(),
                    base_revision: draft.revision,
                    edit: draft.edit.clone(),
                });
                draft.submitted = Some((
                    self.state.serial,
                    self.state
                        .manufacturing
                        .applied_settings
                        .as_ref()
                        .map_or(0, |r| r.nonce),
                ));
            }
        }
        if ui
            .add_enabled(
                draft.submitted.is_none(),
                egui::Button::new("Discard draft"),
            )
            .clicked()
        {
            self.process.interior = None;
        }
    }
}

pub(super) fn policy_name(value: InteriorToolPolicy) -> &'static str {
    match value {
        InteriorToolPolicy::Automatic => "Automatic",
        InteriorToolPolicy::PrimaryOnly => "Primary tool only",
    }
}

fn comparison(ui: &mut egui::Ui, assessment: &InteriorPolicyAssessment) {
    ui.collapsing("Comparison", |ui| {
        for (label, work) in [
            ("Primary only", &assessment.primary),
            ("Automatic", &assessment.automatic),
        ] {
            ui::group(ui, label);
            if let Some(time) = &work.time {
                ui.label(format!("{:.1} min modeled", time.modeled_seconds() / 60.0));
            }
            ui.label(format!(
                "Model {:.1} mm³ · Waste {:.1} mm³",
                work.model_volume_mm3, work.waste_volume_mm3
            ));
        }
        ui.weak("Declared model · shared unknown startup");
    });
}
impl Workbench {
    pub(super) fn interior_suggestion_ui(&mut self, ui: &mut egui::Ui, occurrence: OccurrenceId) {
        let (Some(project), Some(hello), Some(opportunity)) = (
            self.state.project.clone(),
            self.state.hello.clone(),
            self.state.manufacturing.interior_opportunity.clone(),
        ) else {
            return;
        };
        if opportunity.edit.occurrence_id != occurrence
            || self
                .prefs
                .dismissed_policy_contexts
                .contains(&opportunity.input_hash)
            || self.process.has_changes()
        {
            return;
        }
        let assessment = self
            .state
            .manufacturing
            .alternative
            .as_ref()
            .filter(|evaluation| {
                evaluation.project_id == project.project_id
                    && evaluation.captured_revision == project.revision
            })
            .and_then(|evaluation| evaluation.interior_policy.as_deref())
            .filter(|a| a.opportunity == opportunity);
        // A proposal row requires the checked immutable bundle to agree with the
        // typed receipt, not just a terminal operation flag.
        let assessment = assessment.filter(|assessment| {
            self.state
                .manufacturing
                .alternative_review
                .as_ref()
                .is_some_and(|review| {
                    review
                        .bundle
                        .plan
                        .interior_selection
                        .as_deref()
                        .is_some_and(|selection| {
                            InteriorPolicyAssessment::from_selection(opportunity.clone(), selection)
                                .as_ref()
                                == Ok(*assessment)
                        })
                })
        });
        ui.separator();
        if let Some(assessment) = assessment {
            if assessment.qualified() {
                ui::group(ui, "Suggested change");
                ui::modifier_delta(ui, "Interior tool policy", "Primary only", "Automatic");
                if let (Some(before), Some(after)) = (&assessment.primary.time, &assessment.automatic.time) {
                    ui.weak(format!("{:.1} min less modeled work", (before.modeled_seconds() - after.modeled_seconds()) / 60.0));
                }
                if ui.add_enabled(!self.state.busy, egui::Button::new("Review")).clicked() {
                    let mut draft = InteriorPolicyDraft::new(project.project_id.clone(), hello.session_id.clone(), project.revision, occurrence, opportunity.edit.expected_definition_id.clone(), opportunity.edit.expected);
                    draft.edit = opportunity.edit.clone();
                    draft.assessment = Some(assessment.clone());
                    self.process.interior = Some(draft);
                }
            } else { ui.weak("Current pin retained · objective not met"); }
        } else if ui.add_enabled(self.state.phase == Phase::Running && self.manufacturing.trial.is_none() && !self.state.busy && !self.state.manufacturing.alternative_loading, egui::Button::new("Check default")).on_hover_text("Compare releasing this pin after the captured setup or goal changed. Current stays unchanged.").clicked() {
            self.queued_manufacturing = Some(ManufacturingCommand::EvaluateInteriorPolicy { layout_id: project.layout_id.clone(), session_id: hello.session_id.clone(), base_revision: project.revision, opportunity: opportunity.clone() });
        }
        if ui.button("Dismiss").clicked() {
            if self.prefs.dismissed_policy_contexts.len() == 32 {
                self.prefs.dismissed_policy_contexts.remove(0);
            }
            self.prefs
                .dismissed_policy_contexts
                .push(opportunity.input_hash);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scoped_receipt_cannot_be_confirmed_by_full_settings_or_another_edit() {
        let mut draft = InteriorPolicyDraft::new(
            ProjectId::new(),
            SessionId::new(),
            ProjectRevision(4),
            OccurrenceId::new(1).unwrap(),
            DefinitionId::from_source_sha256(&[8; 32]),
            None,
        );
        draft.submitted = Some((7, 9));
        let intent: ManufacturingIntent = serde_json::from_str(include_str!(
            "../../../../fixtures/manufacturing/mixed-width-combined.intent.json"
        ))
        .unwrap();
        let receipt = crate::manufacture::AppliedSettings {
            nonce: 10,
            project: draft.project.clone(),
            session: draft.session.clone(),
            base_revision: draft.revision,
            revision: ProjectRevision(5),
            intent: std::sync::Arc::new(intent),
            interior_edit: Some(draft.edit.clone()),
        };
        assert!(draft.matches(&receipt));
        let mut wrong = receipt.clone();
        wrong.interior_edit = None;
        assert!(!draft.matches(&wrong));
        let mut wrong = receipt.clone();
        wrong.interior_edit.as_mut().unwrap().value = None;
        assert!(!draft.matches(&wrong));
        let mut wrong = receipt.clone();
        wrong.session = SessionId::new();
        assert!(!draft.matches(&wrong));
        let mut wrong = receipt.clone();
        wrong.base_revision = ProjectRevision(3);
        assert!(!draft.matches(&wrong));
        let mut wrong = receipt;
        wrong.nonce = 9;
        assert!(!draft.matches(&wrong));
    }
}
