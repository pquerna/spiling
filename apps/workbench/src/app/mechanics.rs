// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::{
    analysis::*,
    project::{ProjectCommand, ProjectId, ProjectRevision},
};
mod fastener;
mod load;
mod results;
mod tool;

#[derive(Default)]
pub(super) struct MechanicsUi {
    tool: tool::Tool,
    fastener: fastener::Tool,
    apply_requested: bool,
    controls: std::collections::BTreeMap<FaceId, load::Control>,
    settings_json: String,
    settings_initialized: bool,
    field: usize,
    pub(super) evidence_open: bool,
    imported_settings:
        Option<std::sync::Arc<spiling_contracts::analysis::mechanics::AnalysisRunSettings>>,
    seen_capture: Option<String>,
    draft: Option<Draft>,
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
    fn select_reference_model(&mut self) -> Result<(), String> {
        self.settings_json = serde_json::to_string_pretty(&crate::mechanics::reference_model()?)
            .map_err(|error| error.to_string())?;
        self.settings_initialized = true;
        Ok(())
    }

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
        self.controls
            .values()
            .any(|control| control.error.is_some())
            || self
                .draft
                .as_ref()
                .is_some_and(|draft| draft.dirty() || draft.submitted.is_some())
    }
    pub(super) fn discard(&mut self) {
        let seen_capture = self.seen_capture.take();
        *self = Self {
            seen_capture,
            ..Default::default()
        };
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
        self.mechanics.controls.clear();
        self.mechanics.tool = Default::default();
        self.mechanics.fastener.reset_selection();
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
        self.finish_force_placement();
        self.reconcile_fastener();
        let adopted = self
            .state
            .mechanics
            .as_ref()
            .filter(|_| self.work_area.workspace == Workspace::Analyze && !self.subject_drafts())
            .filter(|capture| {
                self.state
                    .project
                    .as_ref()
                    .is_some_and(|project| project.project_id == capture.project)
            })
            .filter(|capture| {
                self.mechanics.seen_capture.as_deref() != Some(capture.resource.sha256.as_str())
            })
            .map(|capture| {
                (
                    capture.resource.sha256.clone(),
                    capture.summary.case.case.id.clone(),
                )
            });
        if let Some((hash, case_id)) = adopted {
            self.mechanics.seen_capture = Some(hash);
            if self.mechanics.draft.is_none() {
                self.choose_analysis_case(case_id);
            }
        }
        if let Some(settings) = &self.state.analysis_model
            && self
                .mechanics
                .imported_settings
                .as_ref()
                .is_none_or(|previous| !std::sync::Arc::ptr_eq(previous, settings))
        {
            self.mechanics.settings_json =
                serde_json::to_string_pretty(settings.as_ref()).unwrap_or_default();
            self.mechanics.settings_initialized = true;
            self.mechanics.imported_settings = Some(settings.clone());
        }
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
        if !draft.dirty()
            && draft.submitted.is_none()
            && !self
                .mechanics
                .controls
                .values()
                .any(|control| control.error.is_some())
        {
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
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            ui.label("Connect the engine to begin.");
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
        let mut create_case = false;
        ui.horizontal(|ui| {
            ui::group(ui, "Case");
            create_case = ui
                .add_enabled(can_create, egui::Button::new("+"))
                .on_hover_text("New case for selected part")
                .clicked();
        });
        if saved.is_empty() {
            ui.weak("No saved cases");
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
        let has_result = self.state.mechanics.as_ref().is_some_and(|capture| {
            self.mechanics.draft.as_ref().is_some_and(|draft| {
                draft.project == capture.project && draft.value.id == capture.summary.case.case.id
            })
        });
        if has_result
            && (self.mechanics.tool.selected.is_some()
                || self.mechanics.tool.armed
                || self.mechanics.fastener.selected.is_some()
                || self.mechanics.fastener.armed)
        {
            ui.collapsing("Result · assumed materials", |ui| {
                self.mechanics_results(ui, ready)
            });
        } else {
            self.mechanics_results(ui, ready);
        }
        if can_create && create_case {
            self.mechanics.controls.clear();
            self.mechanics.tool = Default::default();
            self.mechanics.fastener.reset_selection();
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
                    fasteners: vec![],
                    restraints: vec![],
                },
            });
        }
        if self.mechanics.draft.is_none() {
            ui.weak("Select a native part, create a case, then select finite source faces for loads and restraints.");
            return;
        }
        let draft = self.mechanics.draft.as_ref().expect("case draft");
        let invalid_context = draft.project != project.project_id
            || draft.session != hello.session_id
            || draft.value.layout_id != project.layout_id;
        let stale = draft.base != project.revision || invalid_context;
        self.fastener_inspector(
            ui,
            ready && !project.read_only && !stale && draft.submitted.is_none(),
        );
        ui.push_id("mechanics-case-body", |ui| {
            let draft = self.mechanics.draft.as_mut().expect("case draft");
            ui.add_enabled_ui(
                ready && !project.read_only && !stale && draft.submitted.is_none(),
                |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .toggle_value(&mut self.mechanics.tool.armed, "Apply force")
                            .on_hover_text("Click a face to place a force")
                            .changed()
                            && self.mechanics.tool.armed
                        {
                            self.mechanics.fastener.cancel();
                            self.mechanics.fastener.selected = None;
                        }
                        if self.mechanics.tool.armed && ui.small_button("Cancel").clicked() {
                            self.mechanics.tool.armed = false;
                            self.mechanics.tool.pending = None;
                        }
                    });
                    if self.mechanics.tool.armed {
                        ui::number(ui, "Force (N)", &mut self.mechanics.tool.magnitude, 0.1);
                        ui.checkbox(&mut self.mechanics.tool.whole_face, "Whole face");
                        if !self.mechanics.tool.whole_face {
                            ui::number(
                                ui,
                                "Contact diameter (mm)",
                                &mut self.mechanics.tool.diameter,
                                0.1,
                            );
                        }
                        ui.horizontal(|ui| {
                            const DIRECTIONS: [&str; 8] = [
                                "Into face",
                                "Out of face",
                                "Part X",
                                "Part Y",
                                "Part Z",
                                "Plate X",
                                "Plate Y",
                                "Plate Z",
                            ];
                            egui::ComboBox::from_id_salt("placement-direction")
                                .selected_text(DIRECTIONS[self.mechanics.tool.direction])
                                .show_ui(ui, |ui| {
                                    for (index, label) in DIRECTIONS.iter().enumerate() {
                                        ui.selectable_value(
                                            &mut self.mechanics.tool.direction,
                                            index,
                                            *label,
                                        );
                                    }
                                });
                            ui.toggle_value(&mut self.mechanics.tool.reversed, "Flip");
                        });
                    }
                    if let Some(error) = &self.mechanics.tool.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    ui::group(ui, "Loads & supports");
                    let patch = self.selected.as_ref().filter(|reference| {
                        reference.session_id == hello.session_id
                            && self
                                .state
                                .scene
                                .as_ref()
                                .is_some_and(|scene| reference.scene_revision == scene.revision)
                            && reference.occurrence_id == draft.value.occurrence_id
                            && reference.definition_id == draft.value.definition_id
                    });
                    egui::CollapsingHeader::new("Add support")
                        .default_open(
                            draft.value.loads.is_empty() && draft.value.restraints.is_empty(),
                        )
                        .show(ui, |ui| {
                            if patch.is_none() {
                                ui.weak("Select a face to fix it.");
                            }
                            let can_fix = patch.is_some_and(|reference| {
                                !draft
                                    .value
                                    .loads
                                    .iter()
                                    .any(|load| load.face_id == reference.face_id)
                            });
                            if ui
                                .add_enabled(can_fix, egui::Button::new("Fix selected face · XYZ"))
                                .clicked()
                            {
                                let restraint = AnalysisFaceRestraint {
                                    face_id: patch.expect("selected patch").face_id.clone(),
                                    fixed_translation: [true; 3],
                                };
                                if let Some(existing) = draft
                                    .value
                                    .restraints
                                    .iter_mut()
                                    .find(|existing| existing.face_id == restraint.face_id)
                                {
                                    *existing = restraint;
                                } else {
                                    draft.value.restraints.push(restraint);
                                }
                            }
                        });
                    let mut remove_load = None;
                    for (index, load) in draft.value.loads.iter_mut().enumerate() {
                        ui.push_id(("load", index), |ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(
                                        self.mechanics.tool.selected.as_ref()
                                            == Some(&load.face_id),
                                        format!("Force {}", index + 1),
                                    )
                                    .on_hover_text(load.face_id.as_str())
                                    .clicked()
                                {
                                    self.mechanics.tool.selected = Some(load.face_id.clone());
                                    self.mechanics.fastener.cancel();
                                    self.mechanics.fastener.selected = None;
                                    if let Some(scene) = &self.state.scene {
                                        self.queued_project =
                                            Some(ProjectCommand::PrepareAnalysisFace {
                                                session_id: hello.session_id.clone(),
                                                base_revision: project.revision,
                                                reference: FaceRef {
                                                    session_id: hello.session_id.clone(),
                                                    scene_revision: scene.revision,
                                                    occurrence_id: draft.value.occurrence_id,
                                                    definition_id: draft
                                                        .value
                                                        .definition_id
                                                        .clone(),
                                                    face_id: load.face_id.clone(),
                                                },
                                            });
                                    }
                                }
                                if ui.small_button("−").on_hover_text("Remove load").clicked() {
                                    remove_load = Some(index);
                                }
                            });
                            if self.mechanics.tool.selected.as_ref() != Some(&load.face_id) {
                                ui.weak(format!(
                                    "{:.2} N",
                                    glam::DVec3::from_array(load.force_n).length()
                                ));
                                return;
                            }
                            let preparation = self.state.analysis_faces.iter().find(|face| {
                                face.project_id == project.project_id
                                    && face.source_hash == draft.value.source_hash
                                    && face.reference.face_id == load.face_id
                                    && face.reference.definition_id == draft.value.definition_id
                                    && face.reference.occurrence_id == draft.value.occurrence_id
                                    && face.reference.session_id == hello.session_id
                                    && self.state.scene.as_ref().is_some_and(|scene| {
                                        scene.revision == face.reference.scene_revision
                                    })
                            });
                            if let Some(preparation) = preparation {
                                self.mechanics.apply_requested |= self
                                    .mechanics
                                    .controls
                                    .entry(load.face_id.clone())
                                    .or_insert_with(|| load::Control::new(preparation, load))
                                    .show(ui, preparation, load);
                            } else {
                                ui.weak(format!(
                                    "{:.3} N",
                                    glam::DVec3::from_array(load.force_n).length()
                                ));
                                if ui.button("Direction & force…").clicked()
                                    && let Some(scene) = &self.state.scene
                                {
                                    self.queued_project =
                                        Some(ProjectCommand::PrepareAnalysisFace {
                                            session_id: hello.session_id.clone(),
                                            base_revision: project.revision,
                                            reference: FaceRef {
                                                session_id: hello.session_id.clone(),
                                                scene_revision: scene.revision,
                                                occurrence_id: draft.value.occurrence_id,
                                                definition_id: draft.value.definition_id.clone(),
                                                face_id: load.face_id.clone(),
                                            },
                                        });
                                }
                            }
                            ui.collapsing("Advanced source wrench", |ui| {
                                for (axis, value) in load.force_n.iter_mut().enumerate() {
                                    ui::number(
                                        ui,
                                        &format!("{} (N)", ["X", "Y", "Z"][axis]),
                                        value,
                                        0.1,
                                    );
                                }
                                for (axis, value) in load.moment_n_mm.iter_mut().enumerate() {
                                    ui::number(
                                        ui,
                                        &format!("Source M{} (N·mm)", ["X", "Y", "Z"][axis]),
                                        value,
                                        0.1,
                                    );
                                }
                            });
                        });
                    }
                    if let Some(index) = remove_load {
                        let removed = draft.value.loads.remove(index);
                        self.mechanics.controls.remove(&removed.face_id);
                    }
                    let mut remove_restraint = None;
                    for (index, restraint) in draft.value.restraints.iter_mut().enumerate() {
                        ui.push_id(("restraint", index), |ui| {
                            ui.horizontal(|ui| {
                                ui.strong(format!("Support {}", index + 1))
                                    .on_hover_text(restraint.face_id.as_str());
                                if ui
                                    .small_button("−")
                                    .on_hover_text("Remove support")
                                    .clicked()
                                {
                                    remove_restraint = Some(index);
                                }
                            });
                            ui.horizontal(|ui| {
                                for (axis, fixed) in
                                    restraint.fixed_translation.iter_mut().enumerate()
                                {
                                    ui.checkbox(fixed, ["Fix X", "Fix Y", "Fix Z"][axis]);
                                }
                            });
                        });
                    }
                    if let Some(index) = remove_restraint {
                        draft.value.restraints.remove(index);
                    }
                    ui.collapsing("Case details", |ui| {
                        ui.text_edit_singleline(&mut draft.value.name);
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut draft.value.mode,
                                AnalysisMode::Basic,
                                "Basic",
                            );
                            ui.selectable_value(
                                &mut draft.value.mode,
                                AnalysisMode::Enhanced,
                                "Enhanced",
                            );
                        });
                        ui.weak(format!(
                            "Part {} · source-local axes",
                            draft.value.occurrence_id.get()
                        ));
                    });
                },
            );
        });
        self.mechanics_model(ui);
        ui.collapsing("Manage case", |ui| {
            let Some(draft) = self.mechanics.draft.as_mut() else {
                return;
            };
            let pending = draft.submitted.is_some();
            if ui
                .add_enabled(!pending, egui::Button::new("Discard / close case"))
                .clicked()
            {
                self.mechanics.discard();
                return;
            }
            let draft = self.mechanics.draft.as_mut().unwrap();
            if ui
                .add_enabled(
                    ready
                        && !project.read_only
                        && !stale
                        && !draft.dirty()
                        && !pending
                        && draft.original.is_some(),
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
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fastener_reference_model_requires_explicit_selection_and_preserves_its_assumptions() {
        let mut ui = MechanicsUi::default();
        assert!(ui.settings_json.is_empty());
        assert!(!ui.settings_initialized);
        ui.select_reference_model().unwrap();
        let selected: spiling_contracts::analysis::mechanics::AnalysisRunSettings =
            serde_json::from_str(&ui.settings_json).unwrap();
        selected.validate().unwrap();
        assert_eq!(selected, crate::mechanics::reference_model().unwrap());
        assert_eq!(selected.recipe.infill_fraction, 0.3);
        assert!(!selected.laws.justification.is_empty());
        let mut incompatible = selected;
        incompatible.recipe.infill_fraction = 0.6;
        assert!(incompatible.validate().is_err());
    }
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
                fasteners: vec![],
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
    fn direct_load_edits_remain_drafts_and_magnitude_scales_the_complete_wrench() {
        let mut ui = MechanicsUi {
            draft: Some(draft()),
            ..Default::default()
        };
        let draft = ui.draft.as_mut().unwrap();
        draft.value.loads.push(AnalysisFaceLoad {
            footprint: spiling_contracts::analysis::load::ForceFootprint::WholeFace {},
            face_id: spiling_contracts::geometry::FaceId::from_step_entity(822).unwrap(),
            force_n: [0.0, -10.0, 0.0],
            moment_n_mm: [30.0, 0.0, -360.0],
        });
        draft.original = Some(draft.value.clone());
        draft.submitted = None;
        assert!(!ui.has_changes());
        let draft = ui.draft.as_mut().unwrap();
        draft.value.loads[0].force_n = draft.value.loads[0].force_n.map(|value| value * 2.0);
        draft.value.loads[0].moment_n_mm =
            draft.value.loads[0].moment_n_mm.map(|value| value * 2.0);
        assert_eq!(draft.value.loads[0].force_n, [0.0, -20.0, 0.0]);
        assert_eq!(draft.value.loads[0].moment_n_mm, [60.0, 0.0, -720.0]);
        assert_eq!(
            draft.original.as_ref().unwrap().loads[0].force_n,
            [0.0, -10.0, 0.0]
        );
        assert!(ui.has_changes());
        ui.discard();
        assert!(!ui.has_changes());
    }
}
