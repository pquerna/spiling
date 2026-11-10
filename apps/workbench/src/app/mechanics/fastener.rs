// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::analysis::fastener::*;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Tool {
    pub armed: bool,
    pub selected: Option<u32>,
    pending: Option<FaceRef>,
    requested: Vec<FaceRef>,
    pub preview: Option<AnalysisFastener>,
    pub error: Option<String>,
    gesture: Option<(AnalysisFastener, bool)>,
}
impl Tool {
    fn place_once(&mut self, case: &mut AnalysisCase) -> Result<u32, String> {
        if !self.armed {
            return Err("Arm Attach fastener before placement".into());
        }
        let id = place(
            case,
            self.preview
                .as_ref()
                .ok_or("Prepare a native bore before placement")?,
        )?;
        self.cancel();
        self.selected = Some(id);
        Ok(id)
    }
    pub fn reset_selection(&mut self) {
        let requested = std::mem::take(&mut self.requested);
        *self = Self {
            requested,
            ..Default::default()
        };
    }
    pub fn cancel(&mut self) {
        self.armed = false;
        self.pending = None;
        self.preview = None;
        self.error = None;
    }
}
fn default_fastener(prepared: &AnalysisFastenerPreparation) -> AnalysisFastener {
    let bore = 2.0 * prepared.hole.radius_mm;
    AnalysisFastener {
        id: 1,
        name: "Fastener 1".into(),
        hole: prepared.hole.clone(),
        attachment: FastenerAttachment::Clamp {
            end: HoleEnd::End,
            washer_diameter_mm: (bore * 1.8).min((bore + prepared.washer_limit_mm()) * 0.5),
        },
    }
}
fn place(case: &mut AnalysisCase, preview: &AnalysisFastener) -> Result<u32, String> {
    preview.validate().map_err(|error| error.to_string())?;
    if case.fasteners.len() >= MAX_ANALYSIS_FASTENERS {
        return Err("This case already has eight fasteners".into());
    }
    if case.fasteners.iter().any(|saved| {
        saved
            .hole
            .bore_faces
            .iter()
            .any(|face| preview.hole.bore_faces.contains(face))
    }) {
        return Err("This hole already has a connection; select it to edit".into());
    }
    let id = (1..=MAX_ANALYSIS_FASTENERS as u32 + 1)
        .find(|id| !case.fasteners.iter().any(|saved| saved.id == *id))
        .unwrap();
    let mut fastener = preview.clone();
    fastener.id = id;
    fastener.name = format!("Fastener {id}");
    case.fasteners.push(fastener);
    Ok(id)
}

impl Workbench {
    fn fastener_editable(&self) -> bool {
        self.work_area.workspace == Workspace::Analyze
            && self.display_current()
            && !self.state.busy
            && !self.state.picker_open
            && self.state.confirmation.is_none()
            && self.awaiting_serial.is_none()
            && self.state.project.as_ref().is_some_and(|project| {
                !project.read_only
                    && self.mechanics.draft.as_ref().is_some_and(|draft| {
                        draft.project == project.project_id
                            && draft.base == project.revision
                            && draft.submitted.is_none()
                            && draft.value.layout_id == project.layout_id
                            && self
                                .state
                                .hello
                                .as_ref()
                                .is_some_and(|hello| hello.session_id == draft.session)
                    })
            })
    }
    fn fastener_preparation(
        &self,
        reference: &FaceRef,
    ) -> Option<Arc<AnalysisFastenerPreparation>> {
        let draft = self.mechanics.draft.as_ref()?;
        let scene = self.state.scene.as_ref()?;
        if reference.session_id != scene.session_id
            || reference.scene_revision != scene.revision
            || reference.definition_id != draft.value.definition_id
            || reference.occurrence_id != draft.value.occurrence_id
        {
            return None;
        }
        let pose = &self
            .state
            .display
            .as_ref()?
            .occurrences
            .iter()
            .find(|item| item.occurrence_id == draft.value.occurrence_id)?
            .pose;
        self.state
            .analysis_fasteners
            .iter()
            .find(|prepared| {
                prepared.project_id == draft.project
                    && prepared.source_hash == draft.value.source_hash
                    && prepared.reference.session_id == draft.session
                    && prepared.reference.scene_revision == scene.revision
                    && prepared.reference.occurrence_id == draft.value.occurrence_id
                    && prepared.reference.definition_id == draft.value.definition_id
                    && prepared.pose == *pose
                    && prepared.hole.bore_faces.contains(&reference.face_id)
            })
            .cloned()
    }
    pub(in crate::app) fn cancel_fastener_tool(&mut self, ctx: &egui::Context) -> bool {
        if let Some((original, _)) = self.mechanics.fastener.gesture.take() {
            ctx.stop_dragging();
            if let Some(draft) = &mut self.mechanics.draft
                && let Some(target) = draft
                    .value
                    .fasteners
                    .iter_mut()
                    .find(|item| item.id == original.id)
            {
                *target = original;
            }
            true
        } else if self.mechanics.fastener.armed {
            self.mechanics.fastener.cancel();
            true
        } else {
            self.mechanics.fastener.selected.take().is_some()
        }
    }
    pub(in crate::app) fn select_fastener_face(&mut self, reference: &FaceRef) {
        let Some(draft) = &self.mechanics.draft else {
            return;
        };
        if self.work_area.workspace != Workspace::Analyze
            || reference.definition_id != draft.value.definition_id
            || reference.occurrence_id != draft.value.occurrence_id
        {
            return;
        }
        self.mechanics.fastener.selected = draft
            .value
            .fasteners
            .iter()
            .find(|item| item.hole.bore_faces.contains(&reference.face_id))
            .map(|item| item.id);
    }
    fn queue_fastener_preparation(&mut self, reference: FaceRef) {
        self.mechanics.fastener.requested.retain(|old| {
            old.session_id == reference.session_id && old.scene_revision == reference.scene_revision
        });
        if self.fastener_preparation(&reference).is_some()
            || self.mechanics.fastener.requested.contains(&reference)
            || self.queued_project.is_some()
        {
            return;
        }
        if self.mechanics.fastener.requested.len() >= 256 {
            self.mechanics.fastener.error =
                Some("Native preparation limit reached for this scene".into());
            return;
        }
        let Some(draft) = &self.mechanics.draft else {
            return;
        };
        self.mechanics.fastener.requested.push(reference.clone());
        self.queued_project = Some(ProjectCommand::PrepareAnalysisFastener {
            session_id: draft.session.clone(),
            base_revision: draft.base,
            reference,
        });
    }
    pub(super) fn reconcile_fastener(&mut self) {
        if self
            .mechanics
            .fastener
            .pending
            .as_ref()
            .is_some_and(|reference| {
                self.state.scene.as_ref().is_none_or(|scene| {
                    scene.session_id != reference.session_id
                        || scene.revision != reference.scene_revision
                })
            })
        {
            self.mechanics.fastener.cancel();
        }
        if self.fastener_editable()
            && !self.mechanics.fastener.armed
            && let (Some(draft), Some(scene)) = (&self.mechanics.draft, &self.state.scene)
            && let Some(face) = draft
                .value
                .fasteners
                .iter()
                .find(|item| Some(item.id) == self.mechanics.fastener.selected)
                .and_then(|item| item.hole.bore_faces.first())
        {
            let reference = FaceRef {
                session_id: draft.session.clone(),
                scene_revision: scene.revision,
                occurrence_id: draft.value.occurrence_id,
                definition_id: draft.value.definition_id.clone(),
                face_id: face.clone(),
            };
            self.queue_fastener_preparation(reference);
        }
        let Some(reference) = self.mechanics.fastener.pending.clone() else {
            return;
        };
        if !self.fastener_editable() {
            return;
        }
        if let Some(prepared) = self.fastener_preparation(&reference) {
            if self
                .mechanics
                .fastener
                .preview
                .as_ref()
                .is_none_or(|preview| preview.hole != prepared.hole)
            {
                self.mechanics.fastener.preview = Some(default_fastener(&prepared));
            }
            self.mechanics.fastener.error = None;
        }
    }
    pub(in crate::app) fn fastener_event(&mut self, event: crate::viewport::fastener::Event) {
        use crate::viewport::fastener::Event;
        if !self.fastener_editable() || !self.mechanics.fastener.armed {
            return;
        }
        match event {
            Event::Leave => {
                self.mechanics.fastener.pending = None;
                self.mechanics.fastener.preview = None;
            }
            Event::Point {
                reference,
                place: clicked,
            } => {
                let draft = self.mechanics.draft.as_ref().unwrap();
                let scene = self.state.scene.as_ref().unwrap();
                if reference.session_id != draft.session
                    || reference.scene_revision != scene.revision
                    || reference.definition_id != draft.value.definition_id
                    || reference.occurrence_id != draft.value.occurrence_id
                {
                    return;
                }
                let visible = self
                    .mechanics
                    .fastener
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.hole.bore_faces.contains(&reference.face_id));
                if self.mechanics.fastener.pending.as_ref() != Some(&reference) {
                    let same_hole =
                        self.mechanics
                            .fastener
                            .preview
                            .as_ref()
                            .is_some_and(|preview| {
                                preview.hole.bore_faces.contains(&reference.face_id)
                            });
                    if !same_hole {
                        self.mechanics.fastener.preview = None;
                    }
                    self.mechanics.fastener.pending = Some(reference.clone());
                }
                self.reconcile_fastener();
                if self.fastener_preparation(&reference).is_none() {
                    self.queue_fastener_preparation(reference);
                    self.mechanics.fastener.error = Some("Choose a native through-hole; inspect operation details if preparation refuses".into());
                    return;
                }
                if clicked && !visible {
                    self.mechanics.fastener.error =
                        Some("Review the preview, then click the bore to attach".into());
                    return;
                }
                if clicked && let Some(preview) = self.mechanics.fastener.preview.clone() {
                    let prepared = self.fastener_preparation(&reference).unwrap();
                    if let FastenerAttachment::Clamp {
                        washer_diameter_mm, ..
                    } = preview.attachment
                        && washer_diameter_mm > prepared.washer_limit_mm()
                    {
                        self.mechanics.fastener.error =
                            Some("Washer exceeds native face clearance".into());
                        return;
                    }
                    let draft = self.mechanics.draft.as_mut().unwrap();
                    let clean = !draft.dirty();
                    match self.mechanics.fastener.place_once(&mut draft.value) {
                        Ok(_) => {
                            self.mechanics.tool.selected = None;
                            self.publish_force_gesture(clean);
                        }
                        Err(error) => self.mechanics.fastener.error = Some(error),
                    }
                }
            }
        }
    }
    pub(in crate::app) fn fastener_canvas(&self) -> Option<crate::viewport::fastener::Canvas> {
        if self.work_area.workspace != Workspace::Analyze || !self.display_current() {
            return None;
        }
        let draft = self.mechanics.draft.as_ref()?;
        let scene = self.state.scene.as_ref()?;
        let occurrence = self
            .state
            .display
            .as_ref()?
            .occurrences
            .iter()
            .find(|item| {
                item.occurrence_id == draft.value.occurrence_id
                    && item.definition_id == draft.value.definition_id
            })?;
        if scene.session_id != draft.session {
            return None;
        }
        let armed = self.mechanics.fastener.armed && self.fastener_editable();
        Some(crate::viewport::fastener::Canvas {
            armed,
            selected: self.mechanics.fastener.selected,
            preview: armed
                .then(|| self.mechanics.fastener.preview.clone())
                .flatten(),
            saved: draft.value.fasteners.clone(),
            pose: occurrence.pose,
            reference: draft
                .value
                .fasteners
                .first()
                .and_then(|item| item.hole.bore_faces.first())
                .cloned()
                .or_else(|| {
                    self.mechanics
                        .fastener
                        .pending
                        .as_ref()
                        .map(|reference| reference.face_id.clone())
                })
                .map(|face_id| FaceRef {
                    session_id: scene.session_id.clone(),
                    scene_revision: scene.revision,
                    occurrence_id: draft.value.occurrence_id,
                    definition_id: draft.value.definition_id.clone(),
                    face_id,
                }),
            event: None,
        })
    }
    pub(super) fn fastener_inspector(&mut self, ui: &mut egui::Ui, editable: bool) {
        let editable = editable && self.fastener_editable();
        let clean = self
            .mechanics
            .draft
            .as_ref()
            .is_some_and(|draft| !draft.dirty());
        let mut commit = false;
        let mut gesture_clean = clean;
        ui.add_enabled_ui(editable, |ui| {
            if ui
                .toggle_value(&mut self.mechanics.fastener.armed, "Attach fastener")
                .changed()
            {
                self.mechanics.fastener.pending = None;
                self.mechanics.fastener.preview = None;
                self.mechanics.fastener.error = None;
                if self.mechanics.fastener.armed {
                    self.mechanics.tool.armed = false;
                    self.mechanics.tool.pending = None;
                    self.mechanics.tool.selected = None;
                }
            }
            if self.mechanics.fastener.armed {
                ui.weak("Click a prepared bore to place one connection.");
                if ui.button("Cancel attachment").clicked() {
                    self.mechanics.fastener.cancel();
                }
            }
        });
        let pending_limit = self
            .mechanics
            .fastener
            .pending
            .as_ref()
            .and_then(|reference| self.fastener_preparation(reference))
            .or_else(|| {
                let draft = self.mechanics.draft.as_ref()?;
                let selected = draft
                    .value
                    .fasteners
                    .iter()
                    .find(|item| Some(item.id) == self.mechanics.fastener.selected)?;
                self.state
                    .analysis_fasteners
                    .iter()
                    .find(|prepared| {
                        prepared.hole == selected.hole
                            && prepared.project_id == draft.project
                            && prepared.source_hash == draft.value.source_hash
                            && self.state.scene.as_ref().is_some_and(|scene| {
                                scene.session_id == prepared.reference.session_id
                                    && scene.revision == prepared.reference.scene_revision
                            })
                    })
                    .cloned()
            })
            .map(|prepared| prepared.washer_limit_mm());
        let Some(draft) = self.mechanics.draft.as_mut() else {
            return;
        };
        for fastener in &draft.value.fasteners {
            if ui
                .selectable_label(
                    self.mechanics.fastener.selected == Some(fastener.id),
                    &fastener.name,
                )
                .clicked()
            {
                self.mechanics.fastener.selected = Some(fastener.id);
                self.mechanics.tool.selected = None;
                self.mechanics.tool.armed = false;
                self.mechanics.tool.pending = None;
            }
        }
        let mut remove = None;
        let previewing = self.mechanics.fastener.armed;
        let target = if previewing {
            self.mechanics.fastener.preview.as_mut()
        } else {
            draft
                .value
                .fasteners
                .iter_mut()
                .find(|item| Some(item.id) == self.mechanics.fastener.selected)
        };
        if let Some(target) = target {
            ui.horizontal_wrapped(|ui| {
                ui.weak("Ideal connection");
                if ui.button("Evidence…").clicked() {
                    self.mechanics.evidence_open = true;
                }
            });
            ui.add_enabled_ui(editable, |ui| {
                let original = target.clone();
                let edit = controls(ui, target, pending_limit);
                if !previewing {
                    if edit.started {
                        self.mechanics.fastener.gesture = Some((original, clean));
                    }
                    if edit.finished {
                        gesture_clean = self
                            .mechanics
                            .fastener
                            .gesture
                            .take()
                            .map_or(clean, |(_, clean)| clean);
                    }
                    self.mechanics.apply_requested |= edit.enter;
                }
                commit |= edit.commit || edit.finished;
                if !previewing && ui.button("Remove fastener").clicked() {
                    remove = Some(target.id);
                    commit = true;
                }
            });
        }
        if let Some(id) = remove {
            draft.value.fasteners.retain(|item| item.id != id);
            self.mechanics.fastener.selected = None;
        }
        if let Some(error) = &self.mechanics.fastener.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if !previewing && commit {
            self.publish_force_gesture(gesture_clean);
        }
        if self.mechanics.evidence_open
            && self.state.mechanics.as_ref().is_none_or(|capture| {
                self.mechanics.draft.as_ref().is_none_or(|draft| {
                    draft.project != capture.project
                        || draft.value.id != capture.summary.case.case.id
                })
            })
        {
            let mut open = true;
            egui::Window::new("Connection evidence")
                .open(&mut open)
                .default_width(440.0)
                .show(ui.ctx(), results::connection_evidence);
            self.mechanics.evidence_open = open;
        }
    }
}

// Numeric typing remains a draft; a completed pointer gesture commits once.
#[derive(Default)]
struct Edit {
    commit: bool,
    started: bool,
    finished: bool,
    enter: bool,
}
impl Edit {
    fn number(&mut self, response: egui::Response, ui: &egui::Ui) {
        self.started |= response.drag_started();
        self.finished |= response.drag_stopped();
        self.enter |=
            response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    }
}
fn controls(ui: &mut egui::Ui, target: &mut AnalysisFastener, washer_limit: Option<f64>) -> Edit {
    let mut edit = Edit::default();
    let mut commit = false;
    let mut mode = match target.attachment {
        FastenerAttachment::Clamp { .. } => 0,
        FastenerAttachment::Pin { .. } => 1,
        FastenerAttachment::BondedEngagement { .. } => 2,
    };
    let previous = mode;
    egui::ComboBox::from_id_salt("fastener-attachment")
        .selected_text(["Clamp to support", "Pin / pivot", "Threaded attachment"][mode])
        .show_ui(ui, |ui| {
            for (index, label) in ["Clamp to support", "Pin / pivot", "Threaded attachment"]
                .iter()
                .enumerate()
            {
                ui.selectable_value(&mut mode, index, *label);
            }
        });
    if mode != previous {
        target.attachment = match mode {
            1 => FastenerAttachment::Pin {
                allow_axial_slide: false,
                allow_spin: true,
            },
            2 => FastenerAttachment::BondedEngagement {
                end: HoleEnd::End,
                engagement_mm: target.hole.depth_mm.min(3.0),
            },
            _ => FastenerAttachment::Clamp {
                end: HoleEnd::End,
                washer_diameter_mm: (target.hole.radius_mm * 3.6).min(
                    (target.hole.radius_mm * 2.0 + washer_limit.unwrap_or(f64::INFINITY)) * 0.5,
                ),
            },
        };
        commit = true;
    }
    match &mut target.attachment {
        FastenerAttachment::Clamp {
            end,
            washer_diameter_mm,
        } => {
            end_control(ui, end, &mut commit);
            let response = ui::number(ui, "Washer diameter (mm)", washer_diameter_mm, 0.1);
            edit.number(response, ui);
            if let Some(limit) = washer_limit {
                ui.weak(format!("Native clearance: {limit:.2} mm diameter"));
                if *washer_diameter_mm > limit {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        "Washer exceeds native face clearance",
                    );
                }
            }
            ui.weak("Translation and rotation held.");
        }
        FastenerAttachment::Pin {
            allow_axial_slide,
            allow_spin,
        } => {
            commit |= ui
                .checkbox(allow_axial_slide, "Allow axial slide")
                .changed();
            commit |= ui.checkbox(allow_spin, "Allow spin").changed();
            ui.weak("Transverse motion and tilt held.");
            if *allow_spin || *allow_axial_slide {
                ui.weak("Free motion may need another support.");
            }
        }
        FastenerAttachment::BondedEngagement { end, engagement_mm } => {
            end_control(ui, end, &mut commit);
            let response = ui::number(ui, "Engagement (mm)", engagement_mm, 0.1);
            edit.number(response, ui);
            ui.weak("Declared bonded engagement; translation and rotation held.");
        }
    }
    if let Err(error) = target.validate() {
        ui.colored_label(ui.visuals().error_fg_color, error.to_string());
    }
    edit.commit = commit;
    edit
}
fn end_control(ui: &mut egui::Ui, end: &mut HoleEnd, commit: &mut bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Seat");
        *commit |= ui
            .selectable_value(end, HoleEnd::Start, "Lower end")
            .changed();
        *commit |= ui
            .selectable_value(end, HoleEnd::End, "Upper end")
            .changed();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft() -> Draft {
        let prepared = crate::viewport::fastener::test_preparation();
        let case = AnalysisCase {
            id: AnalysisCaseId::new(),
            name: "Mount study".into(),
            layout_id: spiling_contracts::project::PrintLayoutId::new(),
            occurrence_id: prepared.reference.occurrence_id,
            definition_id: prepared.reference.definition_id,
            source_hash: prepared.source_hash,
            mode: AnalysisMode::Basic,
            loads: vec![],
            restraints: vec![],
            fasteners: vec![],
        };
        Draft {
            project: prepared.project_id,
            session: prepared.reference.session_id,
            base: prepared.project_revision,
            original: Some(case.clone()),
            value: case,
            submitted: None,
            removing: false,
        }
    }
    #[test]
    fn fastener_placement_and_edits_preserve_identity_and_unrelated_drafts() {
        let prepared = crate::viewport::fastener::test_preparation();
        prepared.validate().unwrap();
        let preview = default_fastener(&prepared);
        let FastenerAttachment::Clamp {
            washer_diameter_mm, ..
        } = preview.attachment
        else {
            panic!()
        };
        assert!(washer_diameter_mm > 2.0 * prepared.hole.radius_mm);
        assert!(washer_diameter_mm < prepared.washer_limit_mm());
        let mut draft = draft();
        draft.value.name = "Unapplied name".into();
        let original = draft.original.clone();
        let mut tool = Tool {
            armed: true,
            preview: Some(preview.clone()),
            ..Default::default()
        };
        let id = tool.place_once(&mut draft.value).unwrap();
        assert!(!tool.armed && tool.preview.is_none());
        assert_eq!(tool.selected, Some(id));
        assert!(tool.place_once(&mut draft.value).is_err());
        assert!(place(&mut draft.value, &preview).is_err());
        assert_eq!(draft.value.fasteners.len(), 1);
        draft.value.fasteners[0].attachment = FastenerAttachment::Pin {
            allow_spin: true,
            allow_axial_slide: false,
        };
        assert_eq!(draft.value.fasteners[0].id, id);
        assert_eq!(
            draft.value.fasteners[0].constrained_motions(),
            [true, true, true, true, true, false]
        );
        draft.value.fasteners[0].attachment = FastenerAttachment::BondedEngagement {
            end: HoleEnd::Start,
            engagement_mm: 2.0,
        };
        draft.value.fasteners[0].validate().unwrap();
        draft.value.fasteners.retain(|item| item.id != id);
        assert_eq!(draft.value.name, "Unapplied name");
        assert_eq!(draft.original, original);
        assert!(draft.dirty());
        assert!(draft.submitted.is_none());
    }
    #[test]
    fn fastener_cancel_discards_only_the_unplaced_preview() {
        let prepared = crate::viewport::fastener::test_preparation();
        let mut draft = draft();
        place(&mut draft.value, &default_fastener(&prepared)).unwrap();
        let saved = draft.value.clone();
        let mut tool = Tool {
            armed: true,
            preview: Some(default_fastener(&prepared)),
            pending: Some(prepared.reference.clone()),
            requested: vec![prepared.reference],
            ..Default::default()
        };
        tool.cancel();
        assert!(!tool.armed && tool.preview.is_none() && tool.pending.is_none());
        assert_eq!(tool.requested.len(), 1); // Escape cannot trigger another native request.
        assert_eq!(draft.value, saved);
    }
    #[test]
    fn fastener_controls_work_in_both_themes_at_ordinary_and_narrow_widths() {
        for theme in [ui::Theme::Graphite, ui::Theme::Paper] {
            for width in [240.0, 400.0] {
                let context = egui::Context::default();
                theme.apply(&context);
                let prepared = crate::viewport::fastener::test_preparation();
                let mut target = default_fastener(&prepared);
                target.attachment = FastenerAttachment::Pin {
                    allow_spin: true,
                    allow_axial_slide: false,
                };
                let frame = |events, target: &mut AnalysisFastener| {
                    context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 500.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            controls(ui, target, Some(prepared.washer_limit_mm()));
                        },
                    )
                };
                let output = frame(vec![], &mut target);
                let mut spin = None;
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape {
                        assert!(
                            shape.clip_rect.contains_rect(text.visual_bounding_rect()),
                            "{theme:?} {width}: {}",
                            text.galley.text()
                        );
                        if text.galley.text() == "Allow spin" {
                            spin = Some(text.visual_bounding_rect().center());
                        }
                    }
                }
                let point = spin.expect("visible independent spin control");
                output.drop_without_applying_deltas();
                frame(
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: Default::default(),
                        },
                    ],
                    &mut target,
                )
                .drop_without_applying_deltas();
                frame(
                    vec![egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    }],
                    &mut target,
                )
                .drop_without_applying_deltas();
                assert_eq!(
                    target.attachment,
                    FastenerAttachment::Pin {
                        allow_spin: false,
                        allow_axial_slide: false
                    }
                );
                assert_eq!(target.hole, prepared.hole);
            }
        }
    }
    #[test]
    fn fastener_dimension_drag_reports_one_release_and_keeps_preview_local() {
        let context = egui::Context::default();
        let prepared = crate::viewport::fastener::test_preparation();
        let mut target = default_fastener(&prepared);
        let original = target.clone();
        let frame = |events, target: &mut AnalysisFastener| {
            let mut edit = Edit::default();
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 500.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    edit = controls(ui, target, Some(prepared.washer_limit_mm()));
                },
            );
            (output, edit)
        };
        let (output, edit) = frame(vec![], &mut target);
        assert!(!edit.commit && !edit.started && !edit.finished);
        let point = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape
                    && text.galley.text().parse::<f64>().is_ok()
                {
                    Some(text.visual_bounding_rect().center())
                } else {
                    None
                }
            })
            .expect("dimension field");
        output.drop_without_applying_deltas();
        let (output, _) = frame(
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            &mut target,
        );
        output.drop_without_applying_deltas();
        let moved = point + egui::vec2(30.0, 0.0);
        let (output, edit) = frame(vec![egui::Event::PointerMoved(moved)], &mut target);
        assert!(edit.started && !edit.commit && !edit.finished);
        output.drop_without_applying_deltas();
        let (output, edit) = frame(
            vec![egui::Event::PointerButton {
                pos: moved,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            &mut target,
        );
        assert!(edit.finished && !edit.commit);
        output.drop_without_applying_deltas();
        assert_ne!(target.attachment, original.attachment);
        assert_eq!(target.id, original.id);
        assert_eq!(target.hole, original.hole);
        let (output, edit) = frame(vec![], &mut target);
        assert!(!edit.finished && !edit.commit);
        output.drop_without_applying_deltas();
    }
}
