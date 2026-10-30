// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{Command, Phase, Workbench, ui};
use spiling_contracts::geometry::*;

impl Workbench {
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
        self.occurrence = Some(reference.occurrence_id);
        self.selected = Some(reference.clone());
        self.queued_geometry = Some(GeometryCommand::InspectFace { reference });
    }
    pub(super) fn tree_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Model");
        ui.horizontal(|ui| {
            ui.label("Section Z (mm)");
            ui.add(egui::DragValue::new(&mut self.section_z).speed(0.1));
        });
        if ui
            .add_enabled(
                self.display_current() && !self.state.busy && !self.state.section_loading,
                egui::Button::new("Native section"),
            )
            .clicked()
            && let Some(scene) = &self.state.scene
        {
            self.queued_geometry = Some(GeometryCommand::StartSection {
                session_id: scene.session_id.clone(),
                base_revision: scene.revision,
                plane: PlaneMm {
                    origin_mm: [0.0, 0.0, self.section_z],
                    normal: [0.0, 0.0, 1.0],
                },
            });
        }
        if self.state.section_loading {
            ui.weak("Retrieving section…");
            if ui.button("Cancel section transfer").clicked() {
                self.queued_host = Some(Command::CancelSection);
            }
        }
        if let Some(section) = &self.state.section {
            ui.weak(format!(
                "{} native section loops",
                section.summary.total_loop_count
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
                                self.occurrence = Some(occurrence.occurrence_id);
                                self.pose_revision = None;
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
        ui.heading("Selection");
        if self.pose_revision != Some(scene.summary.revision) {
            self.pose[..3].copy_from_slice(&occurrence.pose.translation_mm);
            let (x, y, z) = glam::DQuat::from_array(occurrence.pose.rotation_xyzw)
                .to_euler(glam::EulerRot::XYZ);
            self.pose[3..].copy_from_slice(&[x.to_degrees(), y.to_degrees(), z.to_degrees()]);
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
                        ui.horizontal(|ui| {
                            ui.label(*label);
                            ui.add(egui::DragValue::new(&mut self.pose[i]).speed(0.1));
                        });
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
                    if ui
                        .add_enabled(valid, egui::Button::new("Apply pose"))
                        .clicked()
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
                        .add_enabled(valid, egui::Button::new("Add copy at edited pose"))
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
