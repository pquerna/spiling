// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    manufacturing::*,
    project::{ProjectId, ProjectRevision},
};
use std::sync::Arc;

const BEAD_MODEL: &str = "Nominal width and physical height over decoded straight strokes. Stadium sections; bridges use elliptical envelopes without sag prediction. Flat ends preserve breaks; intersecting strokes overlap. Displayed surface and overlap are not deposited mass or measured physical shape. Numerical projection ≤ 0.001 mm; section chord error ≤ 0.001 mm. Large reviews retain centerlines when the bead view reaches display limits.";

pub(super) struct IntentDraft {
    project: ProjectId,
    revision: ProjectRevision,
    nonce: u64,
    original: Arc<ManufacturingIntent>,
    value: ManufacturingIntent,
}
struct BeadStatistics {
    display: std::sync::Weak<crate::replay::Replay>,
    nonce: u64,
    view: crate::replay::View,
    value: Option<crate::replay::beads::Statistics>,
}
#[derive(Default)]
pub(super) struct ManufacturingUi {
    draft: Option<IntentDraft>,
    pub panel: usize,
    pub replay: crate::replay::View,
    pub layers: crate::layers::Browser,
    pub review_nonce: Option<u64>,
    bead_statistics: Option<BeadStatistics>,
}
impl ManufacturingUi {
    pub(super) fn has_intent_draft(
        &self,
        project: &ProjectId,
        current: Option<&ManufacturingIntent>,
    ) -> bool {
        self.draft.as_ref().is_some_and(|d| {
            &d.project == project && d.value != *d.original && current != Some(&d.value)
        })
    }
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
            self.button(ui, Action::Library);
            self.material_ui(ui, ready, writable);
            ui.separator();
            ui.heading("Recipe and construction");
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
                ui.add_enabled_ui(writable && !conflict, |ui| {
                    ui.heading("Protect the part");
                    ui::number(ui, "Shape approximation limit (mm)", &mut d.value.recipe.native_shape_error_mm, 0.001);
                    ui.weak("Source material and holes remain protected. This bounds geometry approximation, not physical print accuracy.");
                    ui.separator();
                    ui.heading("Construction goals");
                    ui::layer_height_goal(ui, &mut d.value, self.prefs.theme.tokens());
                    for (label, value) in [
                        ("Bead width (mm)", &mut d.value.recipe.bead_width_mm),
                        ("Sparse fraction", &mut d.value.recipe.infill_fraction),
                        ("Top skin (mm)", &mut d.value.recipe.top_skin_thickness_mm),
                        (
                            "Bottom skin (mm)",
                            &mut d.value.recipe.bottom_skin_thickness_mm,
                        ),
                        ("Travel speed (mm/s)", &mut d.value.recipe.travel_speed_mm_s),
                    ] {
                        ui::number(ui, label, value, 0.01);
                    }
                    ui.add_enabled_ui(d.value.material_profile.is_none(), |ui| {
                        ui::number(ui, "Print speed (mm/s)", &mut d.value.recipe.print_speed_mm_s, 0.1);
                        ui::number(ui, "Flow multiplier", &mut d.value.recipe.flow_multiplier, 0.01);
                    });
                    if d.value.material_profile.is_some() {
                        ui.weak("Material-bound speed, flow and process are edited as explicit overrides above. Filament diameter belongs to the captured material.");
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
                        let value = valid.expect("validated complete intent");
                        d.value = value.clone();
                        self.queued_manufacturing = Some(ManufacturingCommand::SetIntent {
                            session_id: hello.session_id.clone(),
                            base_revision: d.revision,
                            intent: Arc::new(value),
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
                if m.loaded.is_some()
                    || self
                        .manufacturing
                        .has_intent_draft(&project.project_id, m.intent.as_deref())
                {
                    ui.colored_label(
                        self.prefs.theme.tokens().warning,
                        "Unapplied construction changes. This review describes committed settings.",
                    );
                }
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
                let goal = &report.layer_height;
                ui.separator();
                ui.heading("Construction goals");
                ui.label(match goal.policy {
                    LayerHeightPolicy::Preferred => "Layer height · preferred target",
                    LayerHeightPolicy::ProtectedMaximum => "Layer height · protected maximum",
                });
                ui.label(format!(
                    "Requested {:.3} mm; selected nominal {:.3} mm",
                    goal.requested_mm, goal.selected_mm
                ));
                ui.label(format!(
                    "Actual layer heights {:.6}–{:.6} mm",
                    goal.realized_min_mm, goal.realized_max_mm
                ));
                if goal.reason != LayerHeightReason::Requested {
                    ui.colored_label(
                        self.prefs.theme.tokens().warning,
                        match goal.reason {
                            LayerHeightReason::ProcessMinimum => {
                                "Goal adjusted · captured process minimum requires thicker layers."
                            }
                            LayerHeightReason::ProcessMaximum => {
                                "Goal adjusted · captured process maximum requires thinner layers."
                            }
                            LayerHeightReason::Requested => unreachable!(),
                        },
                    );
                }
                if let Some(index) = goal.first_differing_layer {
                    ui.label(format!(
                        "{} of {} layers differ from the target by more than command precision.",
                        goal.differing_layer_count, goal.layer_count
                    ));
                    if ui.button("Inspect first adjusted layer").clicked() {
                        self.manufacturing.replay.layer = Some(index as u16);
                        self.manufacturing.replay.layer_end = None;
                        self.analysis.open = true;
                        self.analysis.follow_layer = true;
                        self.analysis.view.enabled = true;
                        self.analysis.view.representation =
                            crate::analysis::Representation::Compare;
                        self.manufacturing.replay.events = review
                            .display
                            .as_ref()
                            .and_then(|display| {
                                display
                                    .batches
                                    .iter()
                                    .flat_map(|batch| &batch.events)
                                    .filter(|event| u32::from(event.1) == index)
                                    .map(|event| event.0)
                                    .max()
                                    .and_then(|event| event.checked_add(1))
                            })
                            .unwrap_or(u32::MAX);
                        self.manufacturing.replay.visible = true;
                        self.manufacturing.replay.role = None;
                        self.manufacturing.replay.tool = None;
                    }
                }
                ui.weak("Geometry, anchors and physical interval boundaries remain checked. This assessment does not predict strength or physical printability.");
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
                        "Layer schedule interval {:.6}…{:.6} mm",
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
            self.manufacturing.replay.layer_end = None;
            self.manufacturing.replay.role = None;
            self.manufacturing.replay.tool = None;
        }
        let compact = ui.available_height() < 500.0 || ui.available_width() < 600.0;
        let section_representation = self.analysis.view.enabled;
        let view = &mut self.manufacturing.replay;
        ui.horizontal_wrapped(|ui| {
            ui.label("Actual emitted motion");
            if section_representation {
                ui.weak("Representation controlled by Section Analysis");
            } else {
                ui.checkbox(&mut view.source, "Show source");
                ui.checkbox(&mut view.visible, "Motion");
            }
            ui.checkbox(&mut view.travels, "Travels");
            ui.checkbox(&mut view.pressure, "Pressure events");
            if crate::replay::declared_tool_colors(&review.capture.provenance.workflow).is_some() {
                ui.checkbox(&mut view.material, "Declared material tints");
            }
        });
        if view.material
            && let Some(colors) =
                crate::replay::declared_tool_colors(&review.capture.provenance.workflow)
        {
            ui.collapsing("Declared tool-color legend", |ui| {
                for (tool, rgba) in colors.iter().enumerate() {
                    if let JobWorkflow::FixedDualMarlin { job } =
                        &review.capture.provenance.workflow
                    {
                        let material = &job.tools[tool].material;
                        ui.label(format!(
                            "Tool T{tool} · {} · {}",
                            material.polymer_family, material.id
                        ));
                    }
                    ui::ColorPresentation::known(
                        [rgba[0], rgba[1], rgba[2]].map(|c| (c * 255.0).round() as u8),
                        ui::ColorMeaning::CatalogEstimate,
                    )
                    .show(ui, ("tool-color", tool), ui::ColorDensity::Standard);
                }
            });
        }
        if let Some(display) = &review.display {
            if display.beads.is_err() {
                view.beads = false;
            }
            ui.horizontal_wrapped(|ui| {
                if !section_representation {
                    ui.selectable_value(&mut view.beads, false, "Centerlines");
                    ui.add_enabled_ui(display.beads.is_ok(), |ui| {
                        ui.selectable_value(&mut view.beads, true, "Nominal beads");
                    });
                }
                egui::ComboBox::from_id_salt("replay-role")
                    .selected_text(
                        view.role
                            .map_or("All roles", |role| crate::replay::ROLES[role as usize]),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut view.role, None, "All roles");
                        for (role, name) in crate::replay::ROLES.iter().enumerate() {
                            ui.selectable_value(&mut view.role, Some(role as u8), *name);
                        }
                    });
                if descriptor.info.tool_epoch_count > 0 {
                    egui::ComboBox::from_id_salt("replay-tool")
                        .selected_text(
                            view.tool
                                .map_or_else(|| "All tools".into(), |tool| format!("Tool {tool}")),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut view.tool, None, "All tools");
                            for tool in 0..2 {
                                ui.selectable_value(
                                    &mut view.tool,
                                    Some(tool),
                                    format!("Tool {tool}"),
                                );
                            }
                        });
                }
            });
            match &display.beads {
                Ok(beads) if view.beads => {
                    let cache = &mut self.manufacturing.bead_statistics;
                    if cache.as_ref().is_none_or(|c| {
                        c.nonce != review.capture.nonce
                            || !c.display.ptr_eq(&Arc::downgrade(display))
                            || c.view != *view
                    }) {
                        *cache = Some(BeadStatistics {
                            display: Arc::downgrade(display),
                            nonce: review.capture.nonce,
                            view: *view,
                            value: beads.statistics(*view),
                        });
                    }
                    let label = if let Some(stats) = cache.as_ref().and_then(|c| c.value) {
                        let dimension = |range: [f32; 2]| {
                            if range[1] - range[0] < 0.000001 {
                                format!("{:.3}", range[0])
                            } else {
                                format!("{:.3}…{:.3}", range[0], range[1])
                            }
                        };
                        format!(
                            "Nominal W {} · H {} mm · {} strokes",
                            dimension(stats.width),
                            dimension(stats.height),
                            stats.count
                        )
                    } else {
                        "No nominal deposition in this selection".into()
                    };
                    ui.weak(label).on_hover_text(format!("Visible nominal widths and physical stroke heights, distinct from the layer schedule. Section chord bound {:.6} mm; numerical expansion ≤ 0.001 mm. No physical bead-shape or mass measurement.",beads.chord_error_mm));
                }
                Err(reason) => {
                    ui.weak(format!("Centerlines · {reason}"));
                }
                _ => {}
            }
            if !compact {
                ui.collapsing("Bead display model", |ui| {
                    ui.label(BEAD_MODEL);
                });
            }
        }
        self.manufacturing.layers.controls(ui, view, &review);
        if compact {
            ui.collapsing("Event and model details", |ui| {
                ui.label(BEAD_MODEL);
                event_controls(ui, view, &review, descriptor);
            });
        } else {
            event_controls(ui, view, &review, descriptor);
        }
    }
}

fn event_controls(
    ui: &mut egui::Ui,
    view: &mut crate::replay::View,
    review: &crate::manufacture::Review,
    _descriptor: &ReplayDisplayDescriptor,
) {
    let window = crate::layers::event_window(review, *view);
    let mut progress = view
        .events
        .saturating_sub(window.start)
        .min(window.end - window.start);
    if ui
        .add(
            egui::Slider::new(&mut progress, 0..=window.end - window.start)
                .text("Commands in selection"),
        )
        .changed()
    {
        view.events = window.start + progress;
    }
    if view.events > window.start
        && view.events <= window.end
        && let Some(index) = view.events.checked_sub(1)
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
            ui.weak(format!(
                "Nominal association: width {:.6} mm · physical Z {:.6}…{:.6} mm · height {:.6} mm",
                number(48),
                number(56),
                number(64),
                number(64) - number(56)
            ));
        }
    }
    ui.collapsing("Actual motion legend",|ui|{ui.label("Perimeter · solid · top/bottom · sparse · bridge · support/interface · prime · waste · travel · stationary retract/unretract. Tool epochs change state without a connecting segment. Dimensions and tool colors are nominal declarations.");});
}

#[cfg(test)]
mod goal_draft_tests {
    use super::*;

    #[test]
    fn acknowledged_intent_is_not_pending_before_prepare_refreshes_its_draft() {
        let original: ManufacturingIntent = serde_json::from_str(include_str!(
            "../../../../fixtures/manufacturing/preferred-layers.intent.json"
        ))
        .unwrap();
        let mut edited = original.clone();
        edited.recipe.layer_height_mm = 0.15;
        let project = ProjectId::default();
        let view = ManufacturingUi {
            draft: Some(IntentDraft {
                project: project.clone(),
                revision: ProjectRevision(3),
                nonce: 0,
                original: Arc::new(original.clone()),
                value: edited.clone(),
            }),
            ..Default::default()
        };
        assert!(
            view.has_intent_draft(&project, Some(&original)),
            "unacknowledged edits remain visible"
        );
        assert!(
            !view.has_intent_draft(&project, Some(&edited)),
            "a matching engine receipt clears pending status without rendering Prepare"
        );
        let mut different = edited.clone();
        different.recipe.bead_width_mm += 0.01;
        assert!(
            view.has_intent_draft(&project, Some(&different)),
            "another authoritative result cannot acknowledge this draft"
        );
    }
}
