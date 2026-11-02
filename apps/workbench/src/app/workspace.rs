// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{Phase, Workbench, ui};
use spiling_contracts::geometry::*;

impl Workbench {
    pub(super) fn clear_selection(&mut self) {
        if self.subject_drafts() {
            self.navigation = Some(super::navigation::Navigation::ClearSelection);
        } else {
            self.selected = None;
            self.occurrence = None;
            self.pose_revision = None;
        }
    }

    pub(super) fn display_current(&self) -> bool {
        self.state.phase == Phase::Running
            && self.state.display.as_ref().is_some_and(|d| {
                self.state
                    .scene
                    .as_ref()
                    .is_some_and(|s| d.same_revision(s))
            })
    }
    pub(super) fn select_face(&mut self, reference: FaceRef) {
        if self.selected.as_ref() != Some(&reference) && self.subject_drafts() {
            self.navigation = Some(super::navigation::Navigation::Face(reference));
            return;
        }
        if self.occurrence != Some(reference.occurrence_id) {
            self.pose_revision = None;
        }
        self.occurrence = Some(reference.occurrence_id);
        self.selected = Some(reference.clone());
        self.queued_geometry = Some(GeometryCommand::InspectFace { reference });
    }
    pub(super) fn tree_ui(&mut self, ui: &mut egui::Ui) {
        ui::pane_header(ui, "Model", "Source parts and faces");
        self.button(ui, crate::commands::Action::Analysis);
        if self.analysis.view.enabled {
            ui.weak(format!(
                "Section {:?} · {:.3} mm",
                self.analysis.view.axis, self.analysis.view.offset_mm
            ));
        }

        let Some(scene) = self.state.display.clone() else {
            ui.weak("Waiting for native scene");
            return;
        };
        let current = self.display_current() && !self.state.busy;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_enabled_ui(current, |ui| {
                for (index, occurrence) in scene.occurrences.iter().enumerate() {
                    let Some(definition) = scene.definition(&occurrence.definition_id) else {
                        continue;
                    };
                    let title = format!(
                        "{} · {}",
                        definition.record.provenance.source_name,
                        index + 1
                    );
                    egui::CollapsingHeader::new(title)
                        .id_salt(occurrence.occurrence_id)
                        .default_open(scene.occurrences.len() == 1)
                        .show(ui, |ui| {
                            if ui
                                .selectable_label(
                                    self.occurrence.as_ref() == Some(&occurrence.occurrence_id),
                                    "Occurrence properties",
                                )
                                .clicked()
                            {
                                self.choose_occurrence(occurrence.occurrence_id);
                            }
                            if let DefinitionDisplay::Unavailable { error } =
                                &definition.record.display
                            {
                                ui.colored_label(
                                    self.prefs.theme.tokens().warning,
                                    format!("Geometry only: {}", error.message),
                                );
                            }
                            egui::ScrollArea::vertical()
                                .id_salt(occurrence.occurrence_id)
                                .max_height(240.0)
                                .show_rows(ui, 24.0, definition.faces.len(), |ui, range| {
                                    for ordinal in range {
                                        let face = &definition.faces[ordinal];
                                        let selected = self.selected.as_ref().is_some_and(|r| {
                                            r.occurrence_id == occurrence.occurrence_id
                                                && r.face_id == face.face_id
                                        });
                                        if ui
                                            .selectable_label(
                                                selected,
                                                format!("Face {}", ordinal + 1),
                                            )
                                            .clicked()
                                            && let Some(reference) =
                                                scene.reference(occurrence, ordinal)
                                        {
                                            self.select_face(reference);
                                        }
                                    }
                                });
                        });
                }
            });
        });
    }
    pub(super) fn selection_ui(&mut self, ui: &mut egui::Ui) {
        let Some(scene) = self.state.display.clone() else {
            return;
        };
        let Some(occurrence) = scene
            .occurrences
            .iter()
            .find(|o| Some(&o.occurrence_id) == self.occurrence.as_ref())
        else {
            return;
        };
        let current = self.display_current() && !self.state.busy;
        ui::pane_header(ui, "Selection", "Source occurrence");
        if ui::close(ui, "Clear selection") {
            self.clear_selection();
            return;
        }
        if self.pose_revision != Some(scene.summary.revision) {
            self.pose[..3].copy_from_slice(&occurrence.pose.translation_mm);
            let (x, y, z) = glam::DQuat::from_array(occurrence.pose.rotation_xyzw)
                .to_euler(glam::EulerRot::XYZ);
            self.pose[3..].copy_from_slice(&[x.to_degrees(), y.to_degrees(), z.to_degrees()]);
            self.pose_original = self.pose;
            self.pose_revision = Some(scene.summary.revision);
        }
        ui.add_enabled_ui(
            current && self.state.project.as_ref().is_some_and(|p| !p.read_only),
            |ui| {
                ui.collapsing("Position and orientation", |ui| {
                    for (i, label) in [
                        "X (mm)",
                        "Y (mm)",
                        "Z (mm)",
                        "X rotation (°)",
                        "Y rotation (°)",
                        "Z rotation (°)",
                    ]
                    .iter()
                    .enumerate()
                    {
                        ui::number(ui, label, &mut self.pose[i], 0.1);
                    }
                    let rotation = glam::DQuat::from_euler(
                        glam::EulerRot::XYZ,
                        self.pose[3].to_radians(),
                        self.pose[4].to_radians(),
                        self.pose[5].to_radians(),
                    );
                    let pose = RigidPoseMm {
                        translation_mm: [self.pose[0], self.pose[1], self.pose[2]],
                        rotation_xyzw: rotation.to_array(),
                    };
                    let valid = pose.validate().is_ok();
                    if self.pose != self.pose_original
                        && ui::primary(
                            ui,
                            "Apply pose",
                            (!valid).then_some("Enter a finite position and rotation"),
                        )
                        && let Some(project) = &self.state.project
                    {
                        self.queued_geometry = Some(GeometryCommand::SetInstancePose {
                            session_id: scene.summary.session_id.clone(),
                            base_revision: scene.summary.revision,
                            project_revision: project.revision,
                            occurrence_id: occurrence.occurrence_id,
                            pose,
                        });
                    }
                    if ui
                        .add_enabled(valid, egui::Button::new("Add posed copy"))
                        .clicked()
                    {
                        self.queued_geometry = Some(GeometryCommand::AddInstance {
                            session_id: scene.summary.session_id.clone(),
                            base_revision: scene.summary.revision,
                            definition_id: occurrence.definition_id.clone(),
                            pose,
                        });
                    }
                    if ui.button("Remove occurrence").clicked() {
                        self.queued_geometry = Some(GeometryCommand::RemoveInstance {
                            session_id: scene.summary.session_id.clone(),
                            base_revision: scene.summary.revision,
                            occurrence_id: occurrence.occurrence_id,
                        });
                    }
                });
            },
        );
        if let Some(inspection) = &self.state.inspection
            && self.selected.as_ref() == Some(&inspection.reference)
        {
            let tokens = self.prefs.theme.tokens();
            ui::property(
                ui,
                "Native surface",
                format!("{:?}", inspection.face.geometry.family),
                tokens,
            );
            ui::property(
                ui,
                "Source entity",
                format!(
                    "{} #{}",
                    inspection.face.source_surface_entity_kind, inspection.face.surface_face_entity
                ),
                tokens,
            );
            ui::property(
                ui,
                "Trim domain",
                format!(
                    "{} loops · {} holes",
                    inspection.face.geometry.domain.trim_loop_count,
                    inspection.face.geometry.domain.inner_loop_count
                ),
                tokens,
            );
            ui.collapsing("Exact native description", |ui| {
                ui.label(format!("{:#?}", inspection.face.geometry.analytic));
            });
        }
        ui.separator();
    }
}
