// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::{
    analysis::*,
    project::{ProjectCommand, ProjectId, ProjectRevision},
};
mod results;

#[derive(Default)]
pub(super) struct MechanicsUi {
    settings_json: String,
    settings_initialized: bool,
    field: usize,
    slice_z: Option<f64>,
    draft: Option<Draft>,
    force_n: [f64; 3],
    moment_n_mm: [f64; 3],
    wrench_dirty: bool,
}
struct Draft {
    project: ProjectId,
    session: SessionId,
    base: ProjectRevision,
    original: Option<AnalysisCase>,
    value: AnalysisCase,
    submitted: Option<u64>,
    removing: bool,
}
impl Draft {
    fn dirty(&self) -> bool {
        self.original.as_ref() != Some(&self.value)
    }
    fn confirmed(&self, document: &AnalysisDocument) -> bool {
        !self.removing
            && self.submitted.is_some()
            && document.cases.iter().any(|record| {
                self.base.next().ok() == Some(record.edited_revision) && record.case == self.value
            })
    }
}
impl MechanicsUi {
    fn hydrate_settings(
        &mut self,
        settings: &spiling_contracts::analysis::mechanics::AnalysisRunSettings,
    ) {
        if !self.settings_initialized {
            self.settings_json = serde_json::to_string_pretty(settings).unwrap_or_default();
            self.settings_initialized = true;
        }
    }
    pub(super) fn has_changes(&self) -> bool {
        self.wrench_dirty
            || self
                .draft
                .as_ref()
                .is_some_and(|draft| draft.dirty() || draft.submitted.is_some())
    }
    pub(super) fn discard(&mut self) {
        *self = Self::default();
    }
}
impl Workbench {
    pub(super) fn choose_analysis_case(&mut self, id: AnalysisCaseId) {
        if self.mechanics.has_changes() {
            return;
        }
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        let Some(record) = project
            .analysis
            .cases
            .iter()
            .find(|record| record.case.id == id && record.case.layout_id == project.layout_id)
        else {
            return;
        };
        if self.subject_drafts() {
            self.navigation = Some(navigation::Navigation::AnalysisCase(id));
            return;
        }
        self.choose_occurrence(record.case.occurrence_id);
        self.mechanics.draft = Some(Draft {
            project: project.project_id.clone(),
            session: hello.session_id.clone(),
            base: project.revision,
            original: Some(record.case.clone()),
            value: record.case.clone(),
            submitted: None,
            removing: false,
        });
        if let Some(result) = project
            .analysis
            .results
            .iter()
            .find(|result| result.case_id == id)
            && self
                .state
                .mechanics
                .as_ref()
                .is_none_or(|capture| capture.resource.sha256 != result.hash.as_str())
        {
            self.queued_project = Some(ProjectCommand::GetAnalysisResult {
                session_id: hello.session_id.clone(),
                base_revision: project.revision,
                case_id: result.case_id.clone(),
            });
        }
    }
    pub(super) fn reconcile_mechanics(&mut self) {
        if let Some(capture) = &self.state.mechanics
            && self
                .state
                .project
                .as_ref()
                .is_some_and(|project| project.project_id == capture.project)
        {
            self.mechanics.hydrate_settings(&capture.summary.settings);
        }
        let Some(draft) = &mut self.mechanics.draft else {
            return;
        };
        let Some(project) = &self.state.project else {
            return;
        };
        let current = draft.project == project.project_id
            && self
                .state
                .hello
                .as_ref()
                .is_some_and(|hello| hello.session_id == draft.session);
        if current
            && draft.removing
            && draft.submitted.is_some()
            && draft.base.next().ok() == Some(project.revision)
            && !project
                .analysis
                .cases
                .iter()
                .any(|record| record.case.id == draft.value.id)
        {
            self.mechanics.draft = None;
            return;
        }
        if current && draft.confirmed(&project.analysis) {
            draft.original = Some(draft.value.clone());
            draft.submitted = None;
            draft.base = project.revision;
        } else if draft
            .submitted
            .is_some_and(|serial| self.state.serial > serial && !self.state.busy)
        {
            draft.submitted = None;
            draft.removing = false;
        }
        if !draft.dirty() && draft.submitted.is_none() && !self.mechanics.wrench_dirty {
            if current
                && let Some(record) = project
                    .analysis
                    .cases
                    .iter()
                    .find(|record| record.case.id == draft.value.id)
            {
                draft.value = record.case.clone();
                draft.original = Some(record.case.clone());
                draft.base = project.revision;
            } else {
                self.mechanics.draft = None;
            }
        }
    }
    pub(super) fn mechanics_pane(&mut self, ui: &mut egui::Ui) {
        ui::pane_header(ui, "Analyze", "FDM · source-bound load cases");
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            ui.label("Connect the native engine to author a case.");
            return;
        };
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.awaiting_serial.is_none()
            && self.state.confirmation.is_none();
        let saved: Vec<_> = project
            .analysis
            .cases
            .iter()
            .filter(|record| record.case.layout_id == project.layout_id)
            .collect();
        ui::group(ui, "Saved cases");
        if saved.is_empty() {
            ui.weak("No saved load cases on this plate.");
        }
        for record in &saved {
            let selected = self
                .mechanics
                .draft
                .as_ref()
                .is_some_and(|draft| draft.value.id == record.case.id);
            if ui
                .add_enabled(
                    !self.mechanics.has_changes(),
                    egui::Button::new(&record.case.name)
                        .selected(selected)
                        .truncate(),
                )
                .clicked()
            {
                self.choose_analysis_case(record.case.id.clone());
            }
        }
        self.mechanics_results(ui, ready);
        let subject = self.state.display.as_ref().and_then(|scene| {
            let occurrence = scene
                .occurrences
                .iter()
                .find(|occurrence| Some(occurrence.occurrence_id) == self.occurrence)?;
            let definition = scene.definition(&occurrence.definition_id)?;
            Some((occurrence.clone(), definition.record.clone()))
        });
        let can_create = ready
            && self.display_current()
            && !project.read_only
            && !self.mechanics.has_changes()
            && subject.is_some()
            && project.analysis.cases.len() < MAX_ANALYSIS_CASES;
        if ui
            .add_enabled(can_create, egui::Button::new("New case for selected part"))
            .clicked()
        {
            let (occurrence, definition) = subject.expect("selected native subject");
            let name = (1..=MAX_ANALYSIS_CASES + 1)
                .map(|index| format!("Load case {index}"))
                .find(|name| !saved.iter().any(|record| &record.case.name == name))
                .expect("bounded unique case name");
            self.mechanics.draft = Some(Draft {
                project: project.project_id.clone(),
                session: hello.session_id.clone(),
                base: project.revision,
                original: None,
                submitted: None,
                removing: false,
                value: AnalysisCase {
                    id: AnalysisCaseId::new(),
                    name,
                    layout_id: project.layout_id.clone(),
                    occurrence_id: occurrence.occurrence_id,
                    definition_id: definition.definition_id,
                    source_hash: definition.provenance.source_hash,
                    mode: AnalysisMode::Basic,
                    loads: vec![],
                    restraints: vec![],
                },
            });
        }
        if self.mechanics.draft.is_none() {
            ui.weak("Select a native part, create a case, then select finite source faces for loads and restraints.");
            return;
        }
        ui.separator();
        let draft = self.mechanics.draft.as_ref().expect("case draft");
        let invalid_context = draft.project != project.project_id
            || draft.session != hello.session_id
            || draft.value.layout_id != project.layout_id;
        let stale = draft.base != project.revision || invalid_context;
        let validation = draft.value.validate().err().map(|error| error.to_string());
        let reason = if !ready || draft.submitted.is_some() {
            Some("Wait for the native operation")
        } else if project.read_only {
            Some("This project is read-only")
        } else if stale {
            Some("Draft is stale; discard it and reopen the saved case")
        } else if self.mechanics.wrench_dirty {
            Some("Set the selected face load or discard its wrench values first")
        } else if !draft.dirty() {
            Some("No unapplied case changes")
        } else {
            validation.as_deref()
        };
        if ui::primary(ui, "Apply case", reason) {
            let draft = self.mechanics.draft.as_mut().expect("case draft");
            draft.submitted = Some(self.state.serial);
            self.queued_project = Some(ProjectCommand::EditAnalysis {
                session_id: draft.session.clone(),
                base_revision: draft.base,
                edit: AnalysisEdit::Put {
                    case: draft.value.clone(),
                },
            });
        }
        if ui
            .add_enabled(
                self.mechanics
                    .draft
                    .as_ref()
                    .is_none_or(|draft| draft.submitted.is_none()),
                egui::Button::new("Discard draft / close case"),
            )
            .clicked()
        {
            self.mechanics.discard();
            return;
        }
        ui.weak("Apply changes project content; Save writes the .spil checkpoint.");
        if let Some(draft) = &mut self.mechanics.draft
            && draft.original.is_some()
            && ui
                .add_enabled(
                    ready
                        && !project.read_only
                        && !stale
                        && !draft.dirty()
                        && !self.mechanics.wrench_dirty
                        && draft.submitted.is_none(),
                    egui::Button::new("Delete saved case"),
                )
                .clicked()
        {
            draft.removing = true;
            draft.submitted = Some(self.state.serial);
            self.queued_project = Some(ProjectCommand::EditAnalysis {
                session_id: draft.session.clone(),
                base_revision: draft.base,
                edit: AnalysisEdit::Remove {
                    case_id: draft.value.id.clone(),
                },
            });
        }
        egui::ScrollArea::vertical().id_salt("mechanics-case-body").show(ui, |ui| {
            let draft = self.mechanics.draft.as_mut().expect("case draft");
            ui.add_enabled_ui(ready && !project.read_only && !stale && draft.submitted.is_none(), |ui| {
                ui.label(format!("Part {} · source-local service frame", draft.value.occurrence_id.get()));
                ui.label("Case name");
                ui.text_edit_singleline(&mut draft.value.name);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut draft.value.mode, AnalysisMode::Basic, "Basic");
                    ui.selectable_value(&mut draft.value.mode, AnalysisMode::Enhanced, "Enhanced");
                });
                ui.weak("Both modes describe printed FDM construction, not a bulk isotropic solid. Reference is an advanced numerical setting, not a third mode.");
                ui::group(ui, "Finite source-face patches");
                ui.weak("Whole trimmed faces include their native holes. Components use source-local axes. Moments are total resultants about the source origin, not a point force or an inferred lever arm.");
                let patch = self.selected.as_ref().filter(|reference| {
                    reference.session_id == hello.session_id
                        && self.state.scene.as_ref().is_some_and(|scene| reference.scene_revision == scene.revision)
                        && reference.occurrence_id == draft.value.occurrence_id && reference.definition_id == draft.value.definition_id
                });
                if let Some(reference) = patch { ui.label(format!("Selected patch: {}", reference.face_id.as_str())); }
                else { ui.weak("Select a face on this case's part in the viewport or source tree."); }
                for (axis, value) in self.mechanics.force_n.iter_mut().enumerate() {
                    self.mechanics.wrench_dirty |= ui::number(ui, &format!("Force {} (N)", ["X", "Y", "Z"][axis]), value, 0.1).changed();
                }
                for (axis, value) in self.mechanics.moment_n_mm.iter_mut().enumerate() {
                    self.mechanics.wrench_dirty |= ui::number(ui, &format!("Moment {} (N·mm)", ["X", "Y", "Z"][axis]), value, 0.1).changed();
                }
                let has_wrench = self.mechanics.force_n.iter().chain(&self.mechanics.moment_n_mm).all(|value| value.is_finite())
                    && self.mechanics.force_n.iter().chain(&self.mechanics.moment_n_mm).any(|value| *value != 0.0);
                let can_load = patch.is_some_and(|reference| !draft.value.restraints.iter().any(|restraint| restraint.face_id == reference.face_id)) && has_wrench;
                if ui.add_enabled(can_load, egui::Button::new("Set selected face load")).clicked() {
                    let load = AnalysisFaceLoad { face_id: patch.expect("selected patch").face_id.clone(), force_n: self.mechanics.force_n, moment_n_mm: self.mechanics.moment_n_mm };
                    if let Some(existing) = draft.value.loads.iter_mut().find(|existing| existing.face_id == load.face_id) { *existing = load; }
                    else { draft.value.loads.push(load); }
                    self.mechanics.wrench_dirty = false;
                }
                if self.mechanics.wrench_dirty && ui.button("Discard wrench values").clicked() {
                    self.mechanics.force_n = [0.0; 3]; self.mechanics.moment_n_mm = [0.0; 3]; self.mechanics.wrench_dirty = false;
                }
                let can_fix = patch.is_some_and(|reference| !draft.value.loads.iter().any(|load| load.face_id == reference.face_id));
                if ui.add_enabled(can_fix, egui::Button::new("Fix selected face · XYZ")).clicked() {
                    let restraint = AnalysisFaceRestraint { face_id: patch.expect("selected patch").face_id.clone(), fixed_translation: [true; 3] };
                    if let Some(existing) = draft.value.restraints.iter_mut().find(|existing| existing.face_id == restraint.face_id) { *existing = restraint; }
                    else { draft.value.restraints.push(restraint); }
                }
                let mut remove_load = None;
                for (index, load) in draft.value.loads.iter().enumerate() {
                    ui.push_id(("load", index), |ui| {
                        ui.label(format!("{} · force {:?} N · moment {:?} N·mm", load.face_id.as_str(), load.force_n, load.moment_n_mm));
                        if ui.button("Copy wrench to load editor").clicked() { self.mechanics.force_n = load.force_n; self.mechanics.moment_n_mm = load.moment_n_mm; self.mechanics.wrench_dirty = true; }
                        if ui.button("Remove load").clicked() { remove_load = Some(index); }
                    });
                }
                if let Some(index) = remove_load { draft.value.loads.remove(index); }
                let mut remove_restraint = None;
                for (index, restraint) in draft.value.restraints.iter_mut().enumerate() {
                    ui.push_id(("restraint", index), |ui| {
                        ui.label(restraint.face_id.as_str());
                        ui.horizontal(|ui| { for (axis, fixed) in restraint.fixed_translation.iter_mut().enumerate() { ui.checkbox(fixed, ["Fix X", "Fix Y", "Fix Z"][axis]); } });
                        if ui.button("Remove restraint").clicked() { remove_restraint = Some(index); }
                    });
                }
                if let Some(index) = remove_restraint { draft.value.restraints.remove(index); }
            });
            ui.separator();
            ui::group(ui, "Model and evidence");
            if let Some(intent) = &self.state.manufacturing.intent {
                ui.label(format!("Current print settings: layer {:.3} mm · bead {:.3} mm · sparse {:.1}%", intent.recipe.layer_height_mm, intent.recipe.bead_width_mm, intent.recipe.infill_fraction * 100.0));
                ui.weak("Settings remain owned by Prepare. No construction/operator has been captured or solved for this case.");
            } else { ui.label("Apply explicit print settings in Prepare before constructing an FDM model."); }
            ui.label("Authored boundary intent only · no equilibrium result");
            ui.weak("Finite-patch area/moment transfer, directional wall/skin/core laws, bending-qualified whole-part solve and independent field assessment are not yet available. No stress, displacement, strength or breaking-load result is inferred.");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_settings_hydrate_once_and_preserve_deliberately_empty_drafts() {
        let settings = serde_json::from_str(include_str!(
            "../../../../fixtures/analysis/hook-assumptions.json"
        ))
        .unwrap();
        let mut ui = MechanicsUi::default();
        ui.hydrate_settings(&settings);
        assert!(!ui.settings_json.is_empty());
        for empty in ["", "  \n"] {
            ui.settings_json = empty.into();
            ui.hydrate_settings(&settings);
            assert_eq!(ui.settings_json, empty);
        }
        ui.discard();
        ui.hydrate_settings(&settings);
        assert!(!ui.settings_json.is_empty());
        let mut edited = MechanicsUi {
            settings_json: "explicit changed settings".into(),
            settings_initialized: true,
            ..Default::default()
        };
        edited.hydrate_settings(&settings);
        assert_eq!(edited.settings_json, "explicit changed settings");
    }
    fn draft() -> Draft {
        Draft {
            project: ProjectId::new(),
            session: SessionId::new(),
            base: ProjectRevision(3),
            original: None,
            submitted: Some(1),
            removing: false,
            value: AnalysisCase {
                id: AnalysisCaseId::new(),
                name: "Hook load".into(),
                layout_id: spiling_contracts::project::PrintLayoutId::new(),
                occurrence_id: OccurrenceId::new(1).unwrap(),
                definition_id: DefinitionId::from_source_sha256(&[42; 32]),
                source_hash: SourceHash::from_digest(&[42; 32]),
                mode: AnalysisMode::Basic,
                loads: vec![],
                restraints: vec![],
            },
        }
    }
    #[test]
    fn analysis_draft_confirmation_requires_exact_case_and_its_own_edit_revision() {
        let mut draft = draft();
        let mut document = AnalysisDocument {
            results: vec![],
            cases: vec![AnalysisCaseRecord {
                case: draft.value.clone(),
                edited_revision: ProjectRevision(3),
            }],
        };
        assert!(!draft.confirmed(&document));
        document.cases[0].edited_revision = ProjectRevision(4);
        assert!(draft.confirmed(&document));
        document.cases[0].case.mode = AnalysisMode::Enhanced;
        assert!(!draft.confirmed(&document));
        draft.value.mode = AnalysisMode::Enhanced;
        draft.removing = true;
        assert!(!draft.confirmed(&document));
        draft.removing = false;
        draft.submitted = None;
        assert!(!draft.confirmed(&document));
    }
    #[test]
    fn analysis_unset_wrench_values_are_guarded_drafts_not_saved_case_content() {
        let mut ui = MechanicsUi {
            draft: Some(draft()),
            ..Default::default()
        };
        let draft = ui.draft.as_mut().unwrap();
        draft.original = Some(draft.value.clone());
        draft.submitted = None;
        assert!(!ui.has_changes());
        ui.force_n = [0.0, -12.0, 0.0];
        ui.wrench_dirty = true;
        assert!(ui.has_changes());
        ui.discard();
        assert!(!ui.has_changes());
    }
}
