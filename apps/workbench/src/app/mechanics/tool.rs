use super::*;
use crate::viewport::force::{Canvas, Contact, Event};
use spiling_contracts::analysis::load::{AnalysisFacePreparation, ForceFootprint};
use std::sync::Arc;

pub(super) struct Tool {
    pub armed: bool,
    pub magnitude: f64,
    pub diameter: f64,
    pub whole_face: bool,
    pub direction: usize,
    pub reversed: bool,
    pub selected: Option<FaceId>,
    pub(super) pending: Option<(FaceRef, [f64; 3], bool)>,
    requested: Option<FaceRef>,
    gesture_clean: Option<bool>,
    pub error: Option<String>,
}
impl Default for Tool {
    fn default() -> Self {
        Self {
            armed: false,
            magnitude: 10.0,
            diameter: 8.0,
            whole_face: false,
            direction: 0,
            reversed: false,
            selected: None,
            pending: None,
            requested: None,
            gesture_clean: None,
            error: None,
        }
    }
}

impl Workbench {
    pub(in crate::app) fn select_force_face(&mut self, reference: &FaceRef) {
        if self.work_area.workspace != Workspace::Analyze {
            return;
        }
        let Some(draft) = &self.mechanics.draft else {
            return;
        };
        if reference.definition_id != draft.value.definition_id
            || reference.occurrence_id != draft.value.occurrence_id
        {
            return;
        }
        if !draft
            .value
            .loads
            .iter()
            .any(|load| load.face_id == reference.face_id)
        {
            self.mechanics.tool.selected = None;
            return;
        }
        self.mechanics.tool.selected = Some(reference.face_id.clone());
        if self.force_preparation(&reference.face_id).is_none()
            && self.queued_project.is_none()
            && !self.state.busy
        {
            self.queued_project = Some(ProjectCommand::PrepareAnalysisFace {
                session_id: draft.session.clone(),
                base_revision: draft.base,
                reference: reference.clone(),
            });
        }
    }
    pub(in crate::app) fn cancel_force_tool(&mut self) -> bool {
        if let Some(event) = self.viewport.cancel_force() {
            self.force_event(event);
            return true;
        }
        if self.mechanics.tool.armed {
            self.force_event(Event::Cancel);
            return true;
        }
        self.mechanics.tool.selected.take().is_some()
    }
    fn force_preparation(&self, face: &FaceId) -> Option<Arc<AnalysisFacePreparation>> {
        let draft = self.mechanics.draft.as_ref()?;
        let project = self.state.project.as_ref()?;
        let scene = self.state.scene.as_ref()?;
        self.state
            .analysis_faces
            .iter()
            .find(|prepared| {
                prepared.project_id == project.project_id
                    && prepared.source_hash == draft.value.source_hash
                    && prepared.reference.session_id == draft.session
                    && prepared.reference.scene_revision == scene.revision
                    && prepared.reference.occurrence_id == draft.value.occurrence_id
                    && prepared.reference.definition_id == draft.value.definition_id
                    && &prepared.reference.face_id == face
            })
            .cloned()
    }

    fn force_preview(&self, reference: &FaceRef, seed: [f64; 3]) -> Result<Contact, String> {
        let prepared = self
            .force_preparation(&reference.face_id)
            .ok_or("Preparing native contact…")?;
        let center = prepared.project(seed).map_err(|error| error.to_string())?;
        if !self.mechanics.tool.magnitude.is_finite() || self.mechanics.tool.magnitude <= 0.0 {
            return Err("Enter a positive force in N".into());
        }
        let footprint = if self.mechanics.tool.whole_face {
            ForceFootprint::WholeFace {}
        } else {
            ForceFootprint::Disk {
                center_mm: center,
                diameter_mm: self.mechanics.tool.diameter,
            }
        };
        let direction = match self.mechanics.tool.direction {
            0 => -DVec3::from_array(prepared.outward_normal),
            1 => DVec3::from_array(prepared.outward_normal),
            2..=4 => DVec3::AXES[self.mechanics.tool.direction - 2],
            5..=7 => {
                glam::DQuat::from_array(prepared.pose.rotation_xyzw).conjugate()
                    * DVec3::AXES[self.mechanics.tool.direction - 5]
            }
            _ => return Err("Choose a force direction".into()),
        };
        let force = (direction
            * self.mechanics.tool.magnitude
            * if self.mechanics.tool.reversed {
                -1.0
            } else {
                1.0
            })
        .to_array();
        let load = prepared
            .load_on(footprint, force, [0.0; 3])
            .map_err(|error| error.to_string())?;
        Ok(Contact {
            preparation: prepared,
            load,
        })
    }

    pub(in crate::app) fn force_canvas(&self) -> Option<Canvas> {
        if self.work_area.workspace != Workspace::Analyze
            || !self.display_current()
            || self.state.busy
            || self.state.picker_open
            || self.state.confirmation.is_some()
            || self.awaiting_serial.is_some()
        {
            return None;
        }
        let draft = self.mechanics.draft.as_ref()?;
        let project = self.state.project.as_ref()?;
        if project.read_only || draft.base != project.revision || draft.submitted.is_some() {
            return None;
        }
        let active = if self.mechanics.tool.armed {
            self.mechanics
                .tool
                .pending
                .as_ref()
                .and_then(|(reference, seed, _)| self.force_preview(reference, *seed).ok())
        } else {
            self.mechanics.tool.selected.as_ref().and_then(|face| {
                Some(Contact {
                    preparation: self.force_preparation(face)?,
                    load: draft
                        .value
                        .loads
                        .iter()
                        .find(|load| &load.face_id == face)?
                        .clone(),
                })
            })
        };
        Some(Canvas {
            armed: self.mechanics.tool.armed,
            active,
            event: None,
        })
    }

    pub(super) fn publish_force_gesture(&mut self, clean: bool) {
        let Some(draft) = self.mechanics.draft.as_mut() else {
            return;
        };
        if clean && draft.dirty() && draft.value.validate().is_ok() && self.queued_project.is_none()
        {
            draft.submitted = Some(self.state.serial);
            self.queued_project = Some(ProjectCommand::EditAnalysis {
                session_id: draft.session.clone(),
                base_revision: draft.base,
                edit: AnalysisEdit::Put {
                    case: draft.value.clone(),
                },
            });
        }
    }

    pub(in crate::app) fn force_event(&mut self, event: Event) {
        match event {
            Event::Leave => {
                if !self
                    .mechanics
                    .tool
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.2)
                {
                    self.mechanics.tool.pending = None;
                }
            }
            Event::Cancel => {
                self.mechanics.tool.armed = false;
                self.mechanics.tool.pending = None;
                self.mechanics.tool.gesture_clean = None;
                self.mechanics.tool.error = None;
            }
            Event::Revert(load) => {
                if let Some(draft) = &mut self.mechanics.draft
                    && let Some(target) = draft
                        .value
                        .loads
                        .iter_mut()
                        .find(|target| target.face_id == load.face_id)
                {
                    *target = load;
                }
                self.mechanics.tool.gesture_clean = None;
            }
            Event::Point {
                reference,
                seed_mm,
                place,
            } => {
                let Some(draft) = &self.mechanics.draft else {
                    return;
                };
                if reference.definition_id != draft.value.definition_id
                    || reference.occurrence_id != draft.value.occurrence_id
                    || reference.session_id != draft.session
                {
                    return;
                }
                self.mechanics.tool.pending = Some((reference.clone(), seed_mm, place));
                self.mechanics.tool.error = None;
                if self.force_preparation(&reference.face_id).is_none()
                    && self.mechanics.tool.requested.as_ref() != Some(&reference)
                    && self.queued_project.is_none()
                {
                    self.mechanics.tool.requested = Some(reference.clone());
                    self.queued_project = Some(ProjectCommand::PrepareAnalysisFace {
                        session_id: draft.session.clone(),
                        base_revision: draft.base,
                        reference,
                    });
                }
                self.finish_force_placement();
            }
            Event::Change { load, finished } => {
                let Some(draft) = self.mechanics.draft.as_mut() else {
                    return;
                };
                let clean = *self
                    .mechanics
                    .tool
                    .gesture_clean
                    .get_or_insert_with(|| !draft.dirty());
                if let Some(target) = draft
                    .value
                    .loads
                    .iter_mut()
                    .find(|target| target.face_id == load.face_id)
                {
                    *target = load;
                }
                if finished {
                    self.mechanics.tool.gesture_clean = None;
                    self.publish_force_gesture(clean);
                }
            }
        }
    }

    pub(super) fn finish_force_placement(&mut self) {
        if self.state.busy
            || self.state.project.as_ref().is_none_or(|project| {
                project.read_only
                    || self.mechanics.draft.as_ref().is_none_or(|draft| {
                        draft.project != project.project_id
                            || draft.base != project.revision
                            || draft.submitted.is_some()
                    })
            })
        {
            return;
        }
        let Some((reference, seed, true)) = self.mechanics.tool.pending.clone() else {
            return;
        };
        if self.force_preparation(&reference.face_id).is_none() {
            return;
        }
        match self.force_preview(&reference, seed) {
            Ok(contact) => {
                let Some(draft) = &mut self.mechanics.draft else {
                    return;
                };
                if draft
                    .value
                    .restraints
                    .iter()
                    .any(|restraint| restraint.face_id == reference.face_id)
                    || draft
                        .value
                        .loads
                        .iter()
                        .any(|load| load.face_id == reference.face_id)
                    || draft.value.loads.len() == MAX_ANALYSIS_PATCHES
                {
                    self.mechanics.tool.error = Some("Choose an unloaded, unsupported face".into());
                    self.mechanics.tool.pending = None;
                    return;
                }
                let clean = !draft.dirty();
                draft.value.loads.push(contact.load);
                self.selected = Some(reference.clone());
                self.mechanics.tool.selected = Some(reference.face_id);
                self.mechanics.tool.armed = false;
                self.mechanics.tool.pending = None;
                self.publish_force_gesture(clean);
            }
            Err(error) => {
                self.mechanics.tool.error = Some(error);
                self.mechanics.tool.pending = None;
            }
        }
    }
}
