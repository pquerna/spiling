// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    manufacturing::*,
    project::{ProjectId, ProjectRevision},
};
use std::sync::Arc;

pub(super) struct IntentDraft {
    project: ProjectId,
    revision: ProjectRevision,
    nonce: u64,
    original: Arc<ManufacturingIntent>,
    value: ManufacturingIntent,
}
#[derive(Default)]
pub(super) struct ManufacturingUi {
    draft: Option<IntentDraft>,
    pub panel: usize,
    pub replay: crate::replay::View,
    pub review_nonce: Option<u64>,
}
impl Workbench {
    pub(super) fn manufacturing_current(&self) -> bool {
        self.state.phase == Phase::Running
            && self
                .state
                .hello
                .as_ref()
                .zip(self.state.project.as_ref())
                .is_some_and(|(h, p)| {
                    self.state
                        .manufacturing
                        .current_capture(p, &h.session_id)
                        .is_some()
                })
    }
    pub(super) fn manufacturing_ui(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.state.project.clone() else {
            return;
        };
        let Some(hello) = self.state.hello.clone() else {
            return;
        };
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.state.confirmation.is_none()
            && self.awaiting_serial.is_none();
        let writable = ready && !project.read_only;
        let m = self.state.manufacturing.clone();
        if self.manufacturing.panel == 1 {
            ui.heading("Prepare");
            ui.label("Declared printer and process");
            self.button(ui, Action::LoadIntent);
            let (source, nonce) = if let Some(loaded) = &m.loaded {
                (Some(loaded.intent.clone()), loaded.nonce)
            } else {
                (m.intent.clone(), 0)
            };
            if let Some(source) = source {
                let reset = self.manufacturing.draft.as_ref().is_none_or(|d| {
                    d.project != project.project_id
                        || d.nonce != nonce
                        || (d.revision != project.revision
                            && (d.value == *d.original
                                || super::policies::prepared(&d.value).is_ok_and(|v| v == *source)))
                });
                if reset {
                    self.manufacturing.draft = Some(IntentDraft {
                        project: project.project_id.clone(),
                        revision: project.revision,
                        nonce,
                        original: source.clone(),
                        value: (*source).clone(),
                    });
                }
                let d = self
                    .manufacturing
                    .draft
                    .as_mut()
                    .expect("initialized intent");
                let conflict = d.revision != project.revision;
                if conflict {
                    ui.colored_label(
                        self.prefs.theme.tokens().warning,
                        "Project changed while editing. Reload current values before applying.",
                    );
                }
                if let Some(loaded) = &m.loaded {
                    ui.weak(format!("Selected: {}", loaded.label));
                }
                ui.label(format!(
                    "{} · {}",
                    d.value.printer.name, d.value.recipe.material_id
                ));
                ui.weak(format!(
                    "Recipe {} / {}",
                    d.value.recipe.id, d.value.recipe.revision
                ));
                if let Some(profile) = &d.value.material_profile {
                    ui.collapsing("Captured material profile", |ui| {
                        let selection = &profile.selection;
                        ui.label(format!("{} · {} / {}", selection.profile.material.product, selection.profile.id, selection.profile.revision));
                        ui.label(format!("Source: {:?} · {} / {}", selection.profile.source.kind, selection.profile.source.source_id, selection.profile.source.source_revision));
                        ui.label(format!("Effective flow ceiling: {:.3} mm³/s", d.value.effective_max_volumetric_flow_mm3_s()));
                        ui.weak("Profile selection and explicit overrides are authored through the CLI. The full capture is retained; conflicting recipe edits are rejected by the engine. Declarations are not calibration.");
                    });
                }
                ui.add_enabled_ui(writable && !conflict, |ui| {
                    for (label, value) in [
                        ("Layer height (mm)", &mut d.value.recipe.layer_height_mm),
                        ("Bead width (mm)", &mut d.value.recipe.bead_width_mm),
                        ("Sparse fraction", &mut d.value.recipe.infill_fraction),
                        ("Top skin (mm)", &mut d.value.recipe.top_skin_thickness_mm),
                        (
                            "Bottom skin (mm)",
                            &mut d.value.recipe.bottom_skin_thickness_mm,
                        ),
                        ("Print speed (mm/s)", &mut d.value.recipe.print_speed_mm_s),
                        ("Travel speed (mm/s)", &mut d.value.recipe.travel_speed_mm_s),
                        ("Flow multiplier", &mut d.value.recipe.flow_multiplier),
                    ] {
                        ui::number(ui, label, value, 0.01);
                    }
                    ui.horizontal(|ui| {
                        ui.label("Perimeters");
                        ui.add(
                            egui::DragValue::new(&mut d.value.recipe.perimeter_count).range(1..=8),
                        );
                    });
                    super::policies::fields(ui, &mut d.value);
                    let valid = super::policies::prepared(&d.value);
                    if let Err(error) = &valid {
                        ui.colored_label(self.prefs.theme.tokens().warning, error.to_string());
                    }
                    if ui
                        .add_enabled(
                            valid.as_ref().is_ok_and(|v| m.intent.as_deref() != Some(v)),
                            egui::Button::new("Apply full intent"),
                        )
                        .clicked()
                    {
                        self.queued_manufacturing = Some(ManufacturingCommand::SetIntent {
                            session_id: hello.session_id.clone(),
                            base_revision: d.revision,
                            intent: Arc::new(valid.expect("validated complete intent")),
                        });
                    }
                });
                if ui.button("Reload declared intent").clicked() {
                    self.manufacturing.draft = None;
                }
                ui.collapsing("Complete declaration", |ui| {
                    ui.label(serde_json::to_string_pretty(&source).unwrap_or_default());
                });
            } else {
                ui.weak("Load an explicit inert intent. No printer or material is inferred.");
            }
            ui.separator();
            self.process_ui(ui, writable && self.display_current());
            ui.heading("Native placement");
            if let Some(scene) = self.state.display.clone()
                && let Some(o) = scene
                    .occurrences
                    .iter()
                    .find(|o| Some(o.occurrence_id) == self.occurrence)
            {
                if let Some(intent) = &m.intent
                    && ui
                        .add_enabled(
                            writable && self.display_current(),
                            egui::Button::new("Prepare native bed placement"),
                        )
                        .clicked()
                {
                    self.queued_geometry = Some(GeometryCommand::PrepareBedPose {
                        session_id: hello.session_id.clone(),
                        base_revision: scene.summary.revision,
                        project_revision: project.revision,
                        occurrence_id: o.occurrence_id,
                        expected_definition_id: o.definition_id.clone(),
                        expected_pose: o.pose,
                        envelope_mm: intent.printer.build_envelope,
                        center_xy: true,
                        contact_tolerance_mm: SECTION_PLANE_TOLERANCE_MM,
                    });
                }
            } else {
                ui.weak("Select an occurrence in the model tree.");
            }
            if let Some(proposal) = &self.state.bed_proposal {
                let current = proposal.session_id == hello.session_id
                    && proposal.project_revision == project.revision
                    && self
                        .state
                        .scene
                        .as_ref()
                        .is_some_and(|s| s.revision == proposal.scene_revision);
                ui.label(format!(
                    "Proposed translation: {:.4?} mm",
                    proposal.pose.translation_mm
                ));
                ui.label(format!(
                    "Original support Z: {:.6}…{:.6} mm",
                    proposal.minimum_lower_mm[2], proposal.minimum_upper_mm[2]
                ));
                ui.weak(format!(
                    "Maximum contact gap {:.6} mm; allowed {:.6} mm",
                    proposal.maximum_contact_gap_mm, proposal.contact_tolerance_mm
                ));
                ui.collapsing("Native support and envelope evidence", |ui| {
                    ui.label(format!("{proposal:#?}"));
                });
                if ui
                    .add_enabled(
                        writable && current,
                        egui::Button::new("Apply reviewed placement"),
                    )
                    .clicked()
                {
                    self.queued_geometry = Some(GeometryCommand::SetInstancePose {
                        session_id: proposal.session_id.clone(),
                        base_revision: proposal.scene_revision,
                        project_revision: proposal.project_revision,
                        occurrence_id: proposal.occurrence_id,
                        pose: proposal.pose,
                    });
                }
            }
            ui.separator();
            self.button(ui, Action::Preflight);
            if let Some(preflight) = &m.preflight {
                ui.label(format!(
                    "Preflight: {:?} · revision {}",
                    preflight.disposition, preflight.input_revision.0
                ));
                ui.collapsing("Eligibility evidence", |ui| {
                    for check in &preflight.checks {
                        ui.label(format!(
                            "{:?} · {}: {}",
                            check.evidence, check.code, check.message
                        ));
                    }
                });
            }
            self.button(ui, Action::Compile);
        } else if self.manufacturing.panel == 2 {
            ui.heading("Actual program review");
            ui.colored_label(self.prefs.theme.tokens().warning, "NOT MACHINE READY");
            ui.label("Independent replay is software evidence. Physical printability and machine execution remain unqualified.");
            self.button(ui, Action::Verify);
            if m.loading {
                ui.spinner();
                ui.label("Retrieving checked actual output…");
                if ui.button("Cancel review transfer").clicked() {
                    self.queued_host = Some(Command::CancelReview);
                }
            }
            if self.manufacturing_current()
                && let Some(review) = &m.review
            {
                let report = &review.capture.report;
                ui.label(format!(
                    "Fresh verification: {}",
                    if report.verified { "Passed" } else { "Failed" }
                ));
                ui.label(format!(
                    "{} deposition · {} travel segments",
                    report.deposition_segments, report.travel_segments
                ));
                ui.label(format!(
                    "{:.3} mm³ deposited · {:.3} mm filament",
                    report.deposited_volume_mm3, report.filament_length_mm
                ));
                super::review::details(ui, review);
                ui.collapsing("Coverage", |ui| {
                    for row in &report.coverage {
                        ui.label(row);
                    }
                });
                ui.collapsing("Limits", |ui| {
                    for row in &report.limitations {
                        ui.label(row);
                    }
                });
                ui.collapsing("Captured inspection identity", |ui| {
                    ui.label(format!("{:#?}", review.capture.provenance));
                });
                self.button(ui, Action::Export);
                if let Some(label) = &m.last_export {
                    ui.label(format!("Written to {label}: plan.json, program.gcode, verification.json, provenance.json"));
                }
                let plan = &review.bundle.plan;
                if let Some(index) = self.manufacturing.replay.layer
                    && let Some(layer) = plan.layers.get(index as usize)
                {
                    ui.separator();
                    ui.heading(format!("Layer {}", index + 1));
                    ui.label(format!(
                        "Physical interval {:.6}…{:.6} mm",
                        layer.lower_z_mm, layer.z_mm
                    ));
                    ui.label(format!(
                        "Section Z {:.6} mm · {} paths",
                        layer.section_z_mm,
                        layer.paths.len()
                    ));
                    if layer.paths.is_empty() {
                        ui.weak("Empty gap layer · no events");
                    }
                    ui.collapsing("Resolved regional coverage", |ui| {
                        super::review::layer(ui, layer);
                    });
                }
                ui.collapsing("Sparse band decisions", |ui| {
                    egui::ScrollArea::vertical().max_height(240.0).show_rows(
                        ui,
                        24.0,
                        plan.sparse_bands.len(),
                        |ui, range| {
                            for index in range {
                                let band = &plan.sparse_bands[index];
                                ui.label(format!(
                                    "Layers {}–{} · {:?} · {:?}",
                                    band.first_layer, band.last_layer, band.owner, band.disposition
                                ));
                            }
                        },
                    );
                });
            } else {
                ui.weak("Freshly verify the active compiled output to enable replay and export.");
            }
        }
    }
    pub(super) fn replay_controls(&mut self, ui: &mut egui::Ui) {
        if !self.manufacturing_current() {
            return;
        }
        let Some(review) = self.state.manufacturing.review.clone() else {
            return;
        };
        let Some(descriptor) = &review.capture.replay else {
            return;
        };
        if self.manufacturing.review_nonce != Some(review.capture.nonce) {
            self.manufacturing.review_nonce = Some(review.capture.nonce);
            self.manufacturing.replay.events = descriptor.info.event_count;
            self.manufacturing.replay.layer = None;
            self.viewport.fit();
        }
        let view = &mut self.manufacturing.replay;
        ui.horizontal_wrapped(|ui| {
            ui.label("Actual emitted motion");
            ui.checkbox(&mut view.source, "Show source");
            ui.checkbox(&mut view.travels, "Travels");
            ui.checkbox(&mut view.visible, "Motion");
            if crate::replay::declared_tool_colors(&review.capture.provenance.workflow).is_some() {
                ui.checkbox(&mut view.material, "Declared material tints");
            }
        });
        ui.horizontal_wrapped(|ui| {
            let mut all = view.layer.is_none();
            if ui.checkbox(&mut all, "All layers").changed() {
                view.layer = if all { None } else { Some(0) };
            }
            if let Some(layer) = &mut view.layer {
                ui.add(
                    egui::Slider::new(
                        layer,
                        0..=descriptor.info.layer_count.saturating_sub(1) as u16,
                    )
                    .text("Layer index"),
                );
            }
        });
        ui.add(
            egui::Slider::new(&mut view.events, 0..=descriptor.info.event_count)
                .text("Actual events"),
        );
        if let Some(index) = view.events.checked_sub(1)
            && let Some(bytes) = &review.motion
        {
            let start = REPLAY_HEADER_BYTES + index as usize * REPLAY_RECORD_BYTES;
            let r = &bytes[start..start + REPLAY_RECORD_BYTES];
            let number = |offset| {
                f64::from_le_bytes(
                    r[offset..offset + 8]
                        .try_into()
                        .expect("checked SPLR6 record"),
                )
            };
            let tool = u32::from_le_bytes(r[76..80].try_into().expect("checked active tool"));
            ui.add(
                egui::Label::new(format!(
                    "{} · tool {} · XYZ {:.6}, {:.6}, {:.6} mm · E {:.8} mm · F {:.1} mm/min",
                    crate::replay::ROLES[r[46] as usize],
                    tool,
                    number(0),
                    number(8),
                    number(16),
                    number(24),
                    number(32)
                ))
                .wrap(),
            );
            if r[47] & 2 != 0 {
                ui.weak(format!("Nominal association: width {:.6} mm · physical Z {:.6}…{:.6} mm · height {:.6} mm",number(48),number(56),number(64),number(64)-number(56)));
            }
        }
        ui.collapsing("Actual motion legend",|ui|{ui.label("Perimeter · solid · top/bottom · sparse · bridge · support/interface · prime · waste · travel · stationary retract/unretract. Tool epochs change state without a connecting segment. Dimensions and tool colors are nominal declarations.");});
    }
}
