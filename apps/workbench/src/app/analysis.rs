// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::analysis::{Axis, Representation, SectionView};
use glam::{DQuat, DVec3};
use spiling_contracts::project::ProjectId;
use std::time::{Duration, Instant};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Pinned {
    project: ProjectId,
    layout_id: spiling_contracts::project::PrintLayoutId,
    name: String,
    view: SectionView,
    occurrence: Option<OccurrenceId>,
    camera: crate::viewport::CameraState,
}
impl Pinned {
    pub(super) fn valid(&self) -> bool {
        self.view.valid() && self.camera.valid() && self.name.len() <= 256
    }
}
#[derive(Default)]
pub(super) struct AnalysisUi {
    pub open: bool,
    observed: Option<SectionView>,
    pub follow_layer: bool,
    contour_problem: Option<String>,
    pub view: SectionView,
    name: String,
    pub(super) project: Option<ProjectId>,
    changed: Option<Instant>,
    last_query: Option<(SessionId, SceneRevision, PlaneMm)>,
    pub contours: Option<crate::analysis::ContourPreview>,
    contour_key: Option<(std::sync::Weak<crate::scene::Section>, Option<OccurrenceId>)>,
}
impl Workbench {
    pub(super) fn close_analysis(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.surface_focus[0].take() {
            ctx.memory_mut(|m| m.request_focus(id));
        }
        self.analysis.open = false;
        self.analysis.view.enabled = false;
        self.analysis.follow_layer = false;
    }
    pub(super) fn analysis_pose(&self) -> Option<&RigidPoseMm> {
        self.state
            .display
            .as_ref()?
            .occurrences
            .iter()
            .find(|o| Some(o.occurrence_id) == self.occurrence)
            .map(|o| &o.pose)
    }
    pub(super) fn analysis_plane(&self) -> PlaneMm {
        self.analysis.view.plane(self.analysis_pose())
    }
    pub(super) fn analysis_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let project = self.state.project.as_ref().map(|p| p.project_id.clone());
        if self.analysis.project != project {
            self.analysis = AnalysisUi {
                project,
                open: self.analysis.open,
                view: SectionView {
                    enabled: self.analysis.open,
                    ..Default::default()
                },
                ..Default::default()
            };
        }
        if self.analysis.follow_layer
            && let Some(review) = self
                .state
                .manufacturing
                .review
                .as_ref()
                .filter(|_| self.manufacturing_current())
            && let Some(layer) = self
                .manufacturing
                .replay
                .layer_end
                .or(self.manufacturing.replay.layer)
                .and_then(|i| review.bundle.plan.layers.get(i as usize))
        {
            self.analysis.view.axis = Axis::Z;
            self.analysis.view.local_frame = false;
            self.analysis.view.offset_mm = layer.section_z_mm;
        }
        let before = self.analysis.view.clone();
        let pose = self.analysis_pose().cloned();
        let bounds = self
            .state
            .display
            .as_ref()
            .and_then(|s| s.summary.bounds_mm);
        let ready = self.display_current()
            && !self.state.busy
            && !self.state.section_loading
            && self.awaiting_serial.is_none()
            && !self.state.picker_open
            && self.state.confirmation.is_none();
        ui::pane_header(
            ui,
            "Section analysis",
            "Transient inspection · no project edits",
        );
        if ui::close(ui, "Close analysis") {
            self.close_analysis(ui.ctx());
            ctx.request_repaint();
            return;
        }
        let inspect = ui::primary(
            ui,
            "Inspect contour",
            if ready {
                None
            } else {
                Some("Waiting for the current source or operation")
            },
        );
        ui.separator();
        let retry = egui::ScrollArea::vertical()
            .id_salt("analysis-body")
            .show(ui, |ui| {
                self.section_controls(ui, pose.as_ref(), bounds, ready)
            })
            .inner
            || inspect;
        if retry {
            self.analysis.view.enabled = true;
        }
        if self.analysis.view.axis != before.axis
            || self.analysis.view.local_frame != before.local_frame
            || self.analysis.view.offset_mm != before.offset_mm
        {
            self.analysis.follow_layer = false;
        }
        if self.analysis.observed.as_ref() != Some(&self.analysis.view) {
            self.analysis.changed = Some(Instant::now());
            self.analysis.observed = Some(self.analysis.view.clone());
        }
        let plane = self.analysis_plane();
        if let Some(scene) = &self.state.scene {
            let key = (scene.session_id.clone(), scene.revision, plane);
            if retry {
                self.analysis.last_query = None;
            }
            let settled = self
                .analysis
                .changed
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(250));
            if self.analysis.view.enabled
                && self.analysis.view.valid()
                && (!self.analysis.view.local_frame || pose.is_some())
                && (!self.analysis.view.selected_only || pose.is_some())
                && ready
                && settled
                && self.analysis.last_query.as_ref() != Some(&key)
            {
                self.analysis.last_query = Some(key);
                self.queued_geometry = Some(GeometryCommand::StartSection {
                    session_id: scene.session_id.clone(),
                    base_revision: scene.revision,
                    plane,
                });
            } else if !settled {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
    }
    fn section_controls(
        &mut self,
        ui: &mut egui::Ui,
        pose: Option<&RigidPoseMm>,
        bounds: Option<AabbMm>,
        ready: bool,
    ) -> bool {
        ui::group(ui, "Plane");
        ui.checkbox(&mut self.analysis.view.enabled, "Enable section");
        ui.checkbox(&mut self.analysis.follow_layer, "Follow layer (build Z)");
        ui.horizontal(|ui| {
            ui.label("Axis");
            for (axis, label) in [
                (Axis::X, "X"),
                (Axis::Y, "Y"),
                (Axis::Z, "Z"),
                (Axis::Free, "Free"),
            ] {
                ui.selectable_value(&mut self.analysis.view.axis, axis, label);
            }
        });
        if self.analysis.view.axis == Axis::Free {
            ui.horizontal(|ui| {
                ui.label("Azimuth / elevation");
                for a in &mut self.analysis.view.angles_deg {
                    ui.add(
                        egui::DragValue::new(a)
                            .speed(0.5)
                            .range(-180.0..=180.0)
                            .suffix("°"),
                    );
                }
            });
        }
        ui.add_enabled_ui(pose.is_some(), |ui| {
            ui.checkbox(&mut self.analysis.view.local_frame, "Part coordinates");
            ui.checkbox(&mut self.analysis.view.selected_only, "Selected part only");
        });
        if (self.analysis.view.local_frame || self.analysis.view.selected_only) && pose.is_none() {
            ui.colored_label(
                self.prefs.theme.tokens().warning,
                "Select a current part to restore this plane or source scope. The cut is disabled until then.",
            );
        }
        if let Some(bounds) = bounds {
            let range = self.analysis.view.range(bounds, pose);
            ui.add(
                egui::Slider::new(&mut self.analysis.view.offset_mm, range[0]..=range[1])
                    .clamping(egui::SliderClamping::Never)
                    .suffix(" mm")
                    .text("Position"),
            );
            ui.horizontal(|ui| {
                if ui.button("Center").clicked() {
                    self.analysis.view.offset_mm = (range[0] + range[1]) * 0.5;
                }
                ui.checkbox(&mut self.analysis.view.flipped, "Flip side");
            });
        }
        if let Some(inspection) = self
            .state
            .inspection
            .as_ref()
            .filter(|i| Some(&i.reference) == self.selected.as_ref())
            && let Some(AnalyticSurfaceDescription::Plane { origin_mm, normal }) =
                &inspection.face.geometry.analytic
            && ui.button("Use selected face").clicked()
        {
            let q = DQuat::from_array(inspection.pose.rotation_xyzw);
            let n = q * DVec3::from_array(*normal);
            let o = q * DVec3::from_array(*origin_mm)
                + DVec3::from_array(inspection.pose.translation_mm);
            self.analysis.view.local_frame = false;
            self.analysis.view.axis = Axis::Free;
            self.analysis.view.angles_deg = [
                n.y.atan2(n.x).to_degrees(),
                n.z.clamp(-1.0, 1.0).asin().to_degrees(),
            ];
            self.analysis.view.offset_mm = n.dot(o);
            self.analysis.view.enabled = true;
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.analysis.view.section_only, "Section only");
            if ui.button("Look normal").clicked() {
                self.viewport.normal_to(
                    DVec3::from_array(self.analysis_plane().normal)
                        * if self.analysis.view.flipped {
                            -1.0
                        } else {
                            1.0
                        },
                );
            }
        });
        ui.separator();
        ui.label("Display");
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.analysis.view.representation,
                Representation::Source,
                "Source",
            );
            ui.add_enabled_ui(self.manufacturing_current(), |ui| {
                ui.selectable_value(
                    &mut self.analysis.view.representation,
                    Representation::Actual,
                    "Program",
                );
                ui.selectable_value(
                    &mut self.analysis.view.representation,
                    Representation::Compare,
                    "Compare",
                );
            });
        });
        ui.weak("Approximate section · blue source / orange program");
        ui::help(
            ui,
            "section-help",
            "Section display",
            "Caps are display estimates, not native faces or physical extrusion. Native source contours and their sampling allowances are inspected separately. Program beads retain job, layer, role and command scope.",
        );
        if self.analysis.view.representation != Representation::Source {
            if !self.manufacturing_current() {
                ui.colored_label(
                    self.prefs.theme.tokens().warning,
                    "Verify the current program to compare deposition.",
                );
            }
            if let Some(problem) = self
                .state
                .manufacturing
                .review
                .as_ref()
                .and_then(|r| r.display.as_ref())
                .and_then(|d| d.beads.as_ref().err())
            {
                ui::problem(
                    ui,
                    "Bead surfaces unavailable · showing centerlines",
                    problem,
                );
            }
        }
        ui.separator();
        ui::group(ui, "Native contour");
        if self.state.section_loading {
            ui.label("Retrieving checked contour…");
            if ui.button("Cancel contour transfer").clicked() {
                self.queued_host = Some(Command::CancelSection);
            }
        }
        let retry = ui
            .add_enabled(ready, egui::Button::new("Refresh contour"))
            .clicked();
        let plane = self.analysis_plane();
        if let Some(section) = self.state.section.as_ref().filter(|s| {
            self.state.scene.as_ref().is_some_and(|scene| {
                s.summary.revision == scene.revision && s.summary.session_id == scene.session_id
            }) && self.analysis.view.valid()
                && (!self.analysis.view.local_frame || pose.is_some())
                && (!self.analysis.view.selected_only || pose.is_some())
                && same_plane(s.summary.plane, plane)
        }) {
            let scope = self.occurrence.filter(|_| self.analysis.view.selected_only);
            let new = self.analysis.contour_key.as_ref().is_none_or(|(key, s)| {
                !key.ptr_eq(&std::sync::Arc::downgrade(section)) || *s != scope
            });
            if new {
                self.analysis.contour_key = Some((std::sync::Arc::downgrade(section), scope));
                self.analysis.contours = None;
                match crate::analysis::ContourPreview::load(section, scope) {
                    Ok(p) => {
                        self.analysis.contours = Some(p);
                        self.analysis.contour_problem = None;
                    }
                    Err(e) => self.analysis.contour_problem = Some(e),
                }
            }
            if let Some(problem) = &self.analysis.contour_problem {
                ui.weak(problem);
            }
            ui.label(format!(
                "{} native loops · sampling allowance {:.4} mm",
                section.summary.total_loop_count, section.summary.sampling_tolerance_mm
            ));
            if let Some(preview) = &mut self.analysis.contours {
                preview.show(ui);
            }
        } else {
            ui.weak("No current native contour. Approximate clipping remains available.");
        }
        if let Some(review) = self
            .state
            .manufacturing
            .review
            .as_ref()
            .filter(|_| self.manufacturing_current())
        {
            let index = self
                .manufacturing
                .replay
                .layer_end
                .or(self.manufacturing.replay.layer)
                .map(usize::from);
            ui.collapsing("Construction at selected layer", |ui| {
                if let Some(layer) = index.and_then(|i| review.bundle.plan.layers.get(i)) {
                    let c = &layer.region_coverage;
                    ui.label(format!("Layer {} · Z {:.4}…{:.4} mm",layer.index + 1, layer.lower_z_mm, layer.z_mm));
                    ui.label(format!("Source section {:.3} mm² · constructed section {:.3} mm² · added surface {:.3} mm²",c.source_section_area_mm2, c.section_area_mm2, c.surface_added_area_mm2));
                    super::review::layer(ui, layer);
                    ui.weak("Captured plan coverage at this layer's section height; not an arbitrary-plane constructed-solid contour.");
                } else {
                    ui.label("Select a layer to inspect captured construction coverage.");
                }
            });
        }
        ui.separator();
        ui::group(ui, "Saved views");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.analysis.name)
                    .hint_text("View name")
                    .char_limit(64)
                    .desired_width(180.0),
            );
            if ui
                .add_enabled(
                    self.analysis.project.is_some() && self.prefs.analysis_pins.len() < 32,
                    egui::Button::new("Pin view"),
                )
                .clicked()
            {
                self.prefs.analysis_pins.push(Pinned {
                    project: self.analysis.project.clone().expect("checked project"),
                    layout_id: self.state.project.as_ref().unwrap().layout_id.clone(),
                    name: if self.analysis.name.trim().is_empty() {
                        format!("Section {}", self.prefs.analysis_pins.len() + 1)
                    } else {
                        self.analysis.name.trim().to_owned()
                    },
                    view: self.analysis.view.clone(),
                    occurrence: self.occurrence,
                    camera: self.viewport.camera(),
                });
            }
        });
        let mut remove = None;
        for (i, pin) in self
            .prefs
            .analysis_pins
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                Some(&p.project) == self.analysis.project.as_ref()
                    && self
                        .state
                        .project
                        .as_ref()
                        .is_some_and(|project| project.layout_id == p.layout_id)
            })
        {
            ui.horizontal(|ui| {
                if ui.button(&pin.name).clicked() && pin.view.valid() && pin.camera.valid() {
                    self.analysis.view = pin.view.clone();
                    self.analysis.follow_layer = false;
                    let occurrence = pin.occurrence.filter(|id| {
                        self.state
                            .display
                            .as_ref()
                            .is_some_and(|s| s.occurrences.iter().any(|o| o.occurrence_id == *id))
                    });
                    if self.occurrence != occurrence {
                        self.pose_revision = None;
                        self.selected = None;
                    }
                    self.occurrence = occurrence;
                    self.viewport.restore_camera(&pin.camera);
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.prefs.analysis_pins.remove(i);
        }

        retry
    }
}
fn same_plane(a: PlaneMm, b: PlaneMm) -> bool {
    a.normal
        .iter()
        .zip(b.normal)
        .all(|(a, b)| (a - b).abs() < 1e-10)
        && DVec3::from_array(a.normal)
            .dot(DVec3::from_array(a.origin_mm) - DVec3::from_array(b.origin_mm))
            .abs()
            < 1e-9
}
