// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{
    appearance::AppearanceColor, manufacturing::*, optical::OpticalAssessment,
};

pub(super) fn details(ui: &mut egui::Ui, review: &crate::manufacture::Review) {
    let report = &review.capture.report;
    let summary = &review.capture.record.summary;
    if let Some(selection) = &review.bundle.plan.interior_selection
        && let (JobWorkflow::FixedDualMarlin { job }, InteriorCandidateOutcome::Qualified { work }) = (
            &review.bundle.provenance.intent.printer.workflow,
            &selection.candidates[selection.selected.index()].outcome,
        )
    {
        let tools = work
            .interior_segments_per_tool
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(i, _)| format!("Tool {i} · Ø{:.2} mm", job.tools[i].nozzle_diameter_mm))
            .collect::<Vec<_>>()
            .join(" · ");
        crate::ui::resolved_value(
            ui,
            "Interior fill",
            if tools.is_empty() {
                "No interior paths"
            } else {
                &tools
            },
            if job
                .assignments
                .iter()
                .filter_map(|a| a.interior.as_ref())
                .any(|b| b.policy_override.is_some_and(|o| o.enabled))
            {
                "Modifier"
            } else {
                "Automatic"
            },
        );
        if selection.reason != InteriorSelectionReason::ImprovedModeledWork {
            ui.weak(interior_reason(selection.reason));
        }
        ui.collapsing("Construction choices", |ui| {
            for candidate in &selection.candidates {
                ui.push_id(candidate.strategy.index(), |ui| {
                    let name = match candidate.strategy {
                        InteriorStrategy::Primary => "Primary tool",
                        InteriorStrategy::Synchronized => "Mixed widths",
                        InteriorStrategy::Grouped => "Grouped interior",
                    };
                    ui.label(if candidate.strategy == selection.selected {
                        format!("{name} · Selected")
                    } else {
                        name.into()
                    });
                    match &candidate.outcome {
                        InteriorCandidateOutcome::Qualified { work } => {
                            ui.weak(work.time.as_ref().map_or_else(
                                || "Timing unknown".into(),
                                |time| format!("{:.1} min modeled", time.modeled_seconds() / 60.0),
                            ));
                            ui.weak(format!(
                                "Model {:.1} mm³ · Waste {:.1} mm³",
                                work.model_volume_mm3, work.waste_volume_mm3
                            ));
                        }
                        InteriorCandidateOutcome::Refused { error } => {
                            ui.weak(&error.message);
                        }
                        InteriorCandidateOutcome::NotEvaluated { reason } => {
                            ui.weak(interior_reason(*reason));
                        }
                    }
                });
            }
        });
    }
    if let Some(time) = &report.job_time {
        ui.collapsing("Job estimate", |ui| {
            crate::ui::resolved_value(
                ui,
                "Modeled work",
                &format!("{:.1} min", time.modeled_seconds() / 60.0),
                "Declared model",
            );
            crate::ui::resolved_value(ui, "Startup", "Unknown", "Homing and heating");
            ui.collapsing("Timing details", |ui| {
                for (label, seconds) in [
                    ("Model deposition", time.model_deposition_seconds),
                    ("Auxiliary deposition", time.auxiliary_deposition_seconds),
                    ("Travel", time.travel_seconds),
                    ("Feeder", time.feeder_seconds),
                    ("Tool activation", time.activation_seconds),
                    ("Command overhead", time.command_overhead_seconds),
                ] {
                    crate::ui::resolved_value(ui, label, &format!("{seconds:.1} s"), "");
                }
                ui.label(format!("Model {} · {}", time.model.id, time.model.revision));
                ui.weak("Rest at each command · declared acceleration and activation delays");
            });
        });
    } else if report.routing.is_some() {
        ui.collapsing("Job estimate", |ui| {
            ui.weak("No declared timing model");
        });
    }
    ui.collapsing("Actual accounting",|ui|{
        ui.label(format!("Object: {} strokes · {:.4} mm³ · {:.4} mm filament",report.deposition_segments,report.deposited_volume_mm3,report.filament_length_mm));
        ui.label(format!("Support: {} strokes · {:.4} mm³ · {:.4} mm filament",report.support_deposition_segments,report.support_deposited_volume_mm3,report.support_filament_length_mm));
        if report.adhesion_deposition_segments>0 {ui.label(format!("Skirt/brim: {} strokes · {:.4} mm filament",report.adhesion_deposition_segments,report.adhesion_filament_length_mm));}
        ui.label(format!("{} retractions · {:.4} mm withdrawn, distinct from deposition",report.retraction_count,report.retracted_filament_mm));
        ui.label(format!("{} lifecycle motions · {:.4} mm prime / route waste filament",report.lifecycle_motion_count,report.priming_filament_length_mm));
        if let Some(routing)=&report.routing {ui.label(format!("{} decoded tool selections",routing.selection_count));for tool in &routing.tools {ui.label(format!("Tool {} · {}: {} model strokes, {:.4} mm³ / {:.4} mm filament; {} waste strokes, {:.4} mm³ / {:.4} mm filament; final feeder debt {:.8} mm",tool.tool,tool.material_id,tool.model_deposition_segments,tool.model_volume_mm3,tool.model_filament_length_mm,tool.waste_deposition_segments,tool.waste_volume_mm3,tool.waste_filament_length_mm,tool.outstanding_retraction_mm));}}
        ui.weak(format!("Maximum position deviation {:.9} mm; extrusion deviation {:.9} mm",report.max_position_error_mm,report.max_extrusion_error_mm));
    });
    ui.collapsing("Native geometry evidence",|ui|{
        let n=&summary.native_geometry;let c=&n.source_feature_coverage;
        if n.evidence == PreflightEvidence::Tolerant {
            ui.label(format!("Within tolerance · {} sampled layers · {} height events",n.certified_layer_count,n.event_count));
        } else {
            ui.label(format!("Certified: {} occurrence analyses · {} closed slabs · {} layers · {} events",n.height_analysis_count,n.slab_count,n.certified_layer_count,n.event_count));
        }
        ui.label(format!("Requested shape error {:.9} mm; achieved {:.9} mm",n.requested_shape_error_mm,n.achieved_shape_error_mm));
        if n.evidence == PreflightEvidence::Certified {
            ui.label(format!("Disjoint-source layers {}; invariant source-union layers {}; native interface-complex layers {}; original-occurrence critical slabs {}",c.disjoint_source_layer_count,c.invariant_source_union_layer_count,c.native_interface_complex_layer_count,c.owned_critical_slab_count));
        }
        ui.weak("Sampled geometry is not a native certificate. Actual strokes establish deposition anchors.");
    });
    ui.collapsing("Surface resolution advisories",|ui|{
        let r=&summary.surface_resolution;
        ui.label(format!("{} sampled witnesses · resolved {} · ambiguous {} · below nominal resolution {} · unavailable {}",r.evaluated_witnesses,r.resolved,r.ambiguous,r.below_nominal_resolution,r.unavailable));
        ui.label(if r.coverage_complete{"Sample coverage complete"}else{"Sample coverage incomplete"});
        ui.weak("Sampled nominal-bead advice is separate from independent emitted-program verification and physical image fidelity.");
    });
    if !review.capture.provenance.surface_color.is_empty() {
        ui.collapsing("Constructed surface exposure", |ui| {
            for face in &review.capture.provenance.surface_color {
                ui.label(format!(
                    "Construction {} · {} · {:.3} × {:.3} mm · {} replay-qualified intervals",
                    face.construction_index,
                    face.face_id.as_str(),
                    face.realized_patch.width_mm,
                    face.realized_patch.height_mm,
                    face.interval_count
                ));
                ui.label(format!(
                    "Nominal side exposure: tool 0 {:.2}% / tool 1 {:.2}%",
                    face.fractions_by_tool[0] * 100.0,
                    face.fractions_by_tool[1] * 100.0
                ));
                if let Some(reference) = &face.reference {
                    ui.weak("Captured reference estimate; backing optics are unmodelled.");
                    ui.label(if reference.patch_matches_request {
                        "Requested patch matches"
                    } else {
                        "Realized patch differs from requested study"
                    });
                    if !reference.pose_matches_application {
                        ui.label("Occurrence pose changed since application");
                    }
                    ui.push_id(
                        (
                            "surface-reference",
                            face.construction_index,
                            face.face_id.as_str(),
                        ),
                        |ui| {
                            assessment(ui, "Requested cycle", &reference.requested);
                            assessment(ui, "Realized exposure", &reference.realized);
                        },
                    );
                }
            }
        });
    }
}

#[derive(Default)]
pub(super) struct SupportInspection {
    has_support: bool,
    selection: Option<(Option<u32>, Option<u32>)>,
    bands: [Option<(u32, u32)>; 2],
}

impl SupportInspection {
    pub(super) fn from_layers(layers: &[PrintLayer]) -> Self {
        Self {
            has_support: layers.iter().any(|layer| {
                layer.paths.iter().any(|path| {
                    matches!(
                        path.kind,
                        DepositionKind::Support | DepositionKind::SupportInterface
                    )
                })
            }),
            ..Default::default()
        }
    }
}

pub(super) fn support_inspection(
    ui: &mut egui::Ui,
    layers: &[PrintLayer],
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
    inspection: &mut SupportInspection,
) {
    if !inspection.has_support {
        return;
    }
    ui.push_id("support-inspection", |ui| {
        ui.add_enabled_ui(display.is_some(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for (role, label, hint) in [
                    (None, "All paths", "Show all deposition roles"),
                    (Some(9), "Scaffold", "Show ordinary support paths"),
                    (
                        Some(10),
                        "Interfaces",
                        "Show upper and lower support interface paths",
                    ),
                ] {
                    if ui
                        .selectable_label(view.role == role, label)
                        .on_hover_text(hint)
                        .clicked()
                    {
                        view.role = role;
                        view.layer = None;
                        view.layer_end = None;
                        view.events = display.expect("enabled actual preview").event_count;
                        view.visible = true;
                        view.source = false;
                        view.tool = None;
                    }
                }
            });
            if view.role == Some(10) {
                if inspection.selection != Some((view.layer, view.layer_end)) {
                    inspection.bands = [
                        interface_band(layers, view, false),
                        interface_band(layers, view, true),
                    ];
                    inspection.selection = Some((view.layer, view.layer_end));
                }
                ui.horizontal_wrapped(|ui| {
                    for (band, label) in inspection
                        .bands
                        .into_iter()
                        .zip(["Previous band", "Next band"])
                    {
                        let band = band.filter(|&(start, end)| {
                            display.is_some_and(|display| {
                                display.layer_windows.get(start as usize).is_some()
                                    && display.layer_windows.get(end as usize).is_some()
                            })
                        });
                        if ui
                            .add_enabled(band.is_some(), egui::Button::new(label))
                            .clicked()
                        {
                            let (start, end) = band.expect("enabled interface band");
                            view.layer = Some(start);
                            view.layer_end = (start != end).then_some(end);
                            view.events = display.expect("enabled actual preview").layer_windows
                                [end as usize]
                                .end;
                        }
                    }
                });
            }
        });
    });
}

fn interface_band(
    layers: &[PrintLayer],
    view: &crate::replay::View,
    forward: bool,
) -> Option<(u32, u32)> {
    let has_interface = |layer: &PrintLayer| {
        layer
            .paths
            .iter()
            .any(|path| path.kind == DepositionKind::SupportInterface)
    };
    let mut bands = layers.iter().enumerate().filter_map(|(index, layer)| {
        if !has_interface(layer) || index > 0 && has_interface(&layers[index - 1]) {
            return None;
        }
        let end = layers[index..]
            .iter()
            .take_while(|layer| has_interface(layer))
            .last()?;
        Some((layer.index, end.index))
    });
    if forward {
        bands.find(|&(start, _)| {
            view.layer
                .is_none_or(|layer| start > view.layer_end.unwrap_or(layer))
        })
    } else {
        bands.rfind(|&(_, end)| view.layer.is_none_or(|layer| end < layer))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ReviewAction {
    InspectLayer {
        layer: u32,
        compare_section: bool,
    },
    TryAlternative {
        name: String,
        intent: Box<ManufacturingIntent>,
    },
    OpenPosition,
    OpenSettings,
}

/// Suggest a narrower valid nominal bead width when thin features are omitted,
/// respecting the printer's own declared deposition envelope.
pub(super) fn finer_wall_candidate(intent: &ManufacturingIntent) -> Option<ManufacturingIntent> {
    let limits = intent.effective_deposition_limits();
    let min_width = limits.min_bead_width_mm;
    let current = intent.recipe.bead_width_mm;
    if !min_width.is_finite() || !current.is_finite() || current <= min_width + 0.01 {
        return None;
    }
    let target = ((current * 0.85).max(min_width) * 100.0).round() / 100.0;
    if target >= current - 0.005 {
        return None;
    }
    let mut candidate = intent.clone();
    candidate.recipe.bead_width_mm = target;
    let max_tol = target * 0.5;
    candidate.recipe.road_tolerances.surface_mm =
        candidate.recipe.road_tolerances.surface_mm.min(max_tol);
    candidate.recipe.road_tolerances.structure_mm =
        candidate.recipe.road_tolerances.structure_mm.min(max_tol);
    candidate.recipe.road_tolerances.sparse_mm =
        candidate.recipe.road_tolerances.sparse_mm.min(max_tol);
    candidate.recipe.road_tolerances.support_mm =
        candidate.recipe.road_tolerances.support_mm.min(max_tol);
    candidate.validate().ok()?;
    Some(candidate)
}

pub(super) fn findings(
    ui: &mut egui::Ui,
    review: &crate::manufacture::Review,
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
    can_try_alternative: bool,
) -> Option<ReviewAction> {
    findings_from_parts(
        ui,
        &review.bundle.plan.layers,
        &review.bundle.provenance.intent,
        &review.capture.report,
        &review.capture.record.summary.surface_resolution,
        display,
        view,
        can_try_alternative,
    )
}

#[allow(clippy::too_many_arguments)]
fn findings_from_parts(
    ui: &mut egui::Ui,
    layers: &[PrintLayer],
    intent: &ManufacturingIntent,
    report: &VerificationReport,
    surface_resolution: &SurfaceResolutionTotals,
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
    can_try_alternative: bool,
) -> Option<ReviewAction> {
    let mut action = None;
    let mut any_finding = false;

    if unsupported_overhang_finding(
        ui,
        layers,
        intent,
        display,
        view,
        can_try_alternative,
        &mut action,
    ) {
        any_finding = true;
    }
    if thin_wall_finding(
        ui,
        layers,
        intent,
        display,
        view,
        can_try_alternative,
        &mut action,
    ) {
        any_finding = true;
    }
    if layer_height_finding(
        ui,
        report,
        intent,
        display,
        can_try_alternative,
        &mut action,
    ) {
        any_finding = true;
    }
    if surface_resolution_finding(ui, surface_resolution, &mut action) {
        any_finding = true;
    }
    if !any_finding && report.verified {
        ui.weak("No construction advisories · all layers within policy");
    }
    action
}

fn unsupported_overhang_finding(
    ui: &mut egui::Ui,
    layers: &[PrintLayer],
    intent: &ManufacturingIntent,
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
    can_try_alternative: bool,
    action: &mut Option<ReviewAction>,
) -> bool {
    let unsupported = || {
        layers
            .iter()
            .filter(|layer| layer.region_coverage.unsupported_overhang_mm > 0.0)
    };
    let Some(featured) = unsupported().max_by(|left, right| {
        left.region_coverage
            .unsupported_overhang_mm
            .total_cmp(&right.region_coverage.unsupported_overhang_mm)
    }) else {
        return false;
    };
    let count = unsupported().count();
    let supports_off = matches!(intent.recipe.supports, SupportPolicy::Disabled);
    ui.add_space(4.0);
    ui.colored_label(
        ui.visuals().warn_fg_color,
        format!(
            "Unsupported overhang · {count} {}",
            if count == 1 { "layer" } else { "layers" }
        ),
    );
    ui.weak(format!(
        "Supports {} → worst span {:.2} mm at layer {} (Z {:.2} mm)",
        if supports_off { "Off" } else { "Automatic" },
        featured.region_coverage.unsupported_overhang_mm,
        u64::from(featured.index) + 1,
        featured.z_mm
    ));
    ui.horizontal_wrapped(|ui| {
        unsupported_layer_row(ui, featured, display, view);
        if supports_off {
            if let Ok(policy) = intent.automatic_support_policy()
                && ui
                    .add_enabled(
                        can_try_alternative,
                        egui::Button::new("Try Automatic supports"),
                    )
                    .on_hover_text(
                        "Compare Automatic supports in Try & Compare without changing Current",
                    )
                    .clicked()
            {
                let mut candidate = intent.clone();
                candidate.recipe.supports = policy;
                *action = Some(ReviewAction::TryAlternative {
                    name: "Automatic supports".into(),
                    intent: Box::new(candidate),
                });
            }
        } else {
            if ui
                .button("Re-orient part")
                .on_hover_text("Open Prepare › Position to try a better resting orientation")
                .clicked()
            {
                *action = Some(ReviewAction::OpenPosition);
            }
            if ui
                .add_enabled(can_try_alternative, egui::Button::new("Try alternative…"))
                .on_hover_text("Compare a support or recipe adjustment without changing Current")
                .clicked()
            {
                *action = Some(ReviewAction::TryAlternative {
                    name: "Overhang alternative".into(),
                    intent: Box::new(intent.clone()),
                });
            }
        }
    });
    if display.is_none() {
        ui.weak("Actual-program layer preview unavailable.");
    }
    unsupported_details(ui, layers, featured, count, display, view);
    true
}

fn thin_wall_finding(
    ui: &mut egui::Ui,
    layers: &[PrintLayer],
    intent: &ManufacturingIntent,
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
    can_try_alternative: bool,
    action: &mut Option<ReviewAction>,
) -> bool {
    let thin_layers = || {
        layers.iter().filter(|layer| {
            let r = &layer.region_coverage;
            r.thin_omitted_area_mm2 > 0.0
                || r.support_omitted_wall_mm > 0.0
                || r.thin_widened_mm > 0.0
        })
    };
    let severity = |layer: &PrintLayer| {
        let r = &layer.region_coverage;
        (
            r.thin_omitted_area_mm2 > 0.0 || r.support_omitted_wall_mm > 0.0,
            r.thin_omitted_area_mm2
                .max(r.support_omitted_wall_mm)
                .max(r.thin_widened_mm * 0.01),
        )
    };
    let Some(featured) = thin_layers().max_by(|a, b| {
        let (a_omit, a_val) = severity(a);
        let (b_omit, b_val) = severity(b);
        a_omit.cmp(&b_omit).then_with(|| a_val.total_cmp(&b_val))
    }) else {
        return false;
    };
    let count = thin_layers().count();
    let r = &featured.region_coverage;
    let omitted = r.thin_omitted_area_mm2 > 0.0 || r.support_omitted_wall_mm > 0.0;
    ui.add_space(4.0);
    ui.colored_label(
        ui.visuals().warn_fg_color,
        format!(
            "{} · {count} {}",
            if omitted {
                "Thin wall omitted"
            } else {
                "Thin wall widened"
            },
            if count == 1 { "layer" } else { "layers" }
        ),
    );
    let detail = if r.thin_omitted_area_mm2 > 0.0 {
        format!(
            "Bead {:.2} mm → {:.3} mm² omitted at layer {} (Z {:.2} mm)",
            intent.recipe.bead_width_mm,
            r.thin_omitted_area_mm2,
            u64::from(featured.index) + 1,
            featured.z_mm
        )
    } else if r.support_omitted_wall_mm > 0.0 {
        format!(
            "Bead {:.2} mm → {:.2} mm wall omitted beside support at layer {}",
            intent.recipe.bead_width_mm,
            r.support_omitted_wall_mm,
            u64::from(featured.index) + 1
        )
    } else {
        format!(
            "Bead {:.2} mm → {:.2} mm widened to minimum width at layer {}",
            intent.recipe.bead_width_mm,
            r.thin_widened_mm,
            u64::from(featured.index) + 1
        )
    };
    ui.weak(detail);
    ui.horizontal_wrapped(|ui| {
        show_layer_button(ui, "thin-wall-layer", featured, display, view);
        if let Some(candidate) = finer_wall_candidate(intent) {
            let label = format!("Try {:.2} mm wall", candidate.recipe.bead_width_mm);
            if ui
                .add_enabled(can_try_alternative, egui::Button::new(label))
                .on_hover_text(
                    "Compare a narrower valid bead width in Try & Compare without changing Current",
                )
                .clicked()
            {
                *action = Some(ReviewAction::TryAlternative {
                    name: format!("Finer wall ({:.2} mm)", candidate.recipe.bead_width_mm),
                    intent: Box::new(candidate),
                });
            }
        } else if ui.button("Open Settings").clicked() {
            *action = Some(ReviewAction::OpenSettings);
        }
    });
    true
}

fn layer_height_finding(
    ui: &mut egui::Ui,
    report: &VerificationReport,
    intent: &ManufacturingIntent,
    display: Option<&crate::replay::Replay>,
    can_try_alternative: bool,
    action: &mut Option<ReviewAction>,
) -> bool {
    let goal = &report.layer_height;
    if goal.reason == LayerHeightReason::Requested && goal.differing_layer_count == 0 {
        return false;
    }
    ui.add_space(4.0);
    ui.colored_label(
        ui.visuals().warn_fg_color,
        format!(
            "Layer height adapted · {}/{} layers",
            goal.differing_layer_count, goal.layer_count
        ),
    );
    let reason = match goal.reason {
        LayerHeightReason::ProcessMinimum => "process minimum required thicker layers",
        LayerHeightReason::ProcessMaximum => "process maximum required thinner layers",
        LayerHeightReason::Requested => "adapted to part heights and slopes",
    };
    ui.weak(format!(
        "Requested {:.3} mm → realized {:.3}–{:.3} mm ({reason})",
        goal.requested_mm, goal.realized_min_mm, goal.realized_max_mm
    ));
    ui.horizontal_wrapped(|ui| {
        if let Some(index) = goal.first_differing_layer {
            let has_layer = display.is_some_and(|d| d.layer_windows.get(index as usize).is_some());
            if ui
                .add_enabled(
                    has_layer,
                    egui::Button::new(format!("Inspect layer {}", u64::from(index) + 1)),
                )
                .clicked()
            {
                *action = Some(ReviewAction::InspectLayer {
                    layer: index,
                    compare_section: true,
                });
            }
        }
        if ui
            .add_enabled(can_try_alternative, egui::Button::new("Try alternative…"))
            .on_hover_text("Compare a different layer height goal without changing Current")
            .clicked()
        {
            *action = Some(ReviewAction::TryAlternative {
                name: "Layer height alternative".into(),
                intent: Box::new(intent.clone()),
            });
        }
    });
    true
}

fn surface_resolution_finding(
    ui: &mut egui::Ui,
    resolution: &SurfaceResolutionTotals,
    action: &mut Option<ReviewAction>,
) -> bool {
    if resolution.below_nominal_resolution == 0 && resolution.ambiguous == 0 {
        return false;
    }
    ui.add_space(4.0);
    ui.colored_label(
        ui.visuals().warn_fg_color,
        format!(
            "Surface detail · {} below nominal · {} ambiguous",
            resolution.below_nominal_resolution, resolution.ambiguous
        ),
    );
    ui.weak(format!(
        "{} sampled surface witnesses evaluated against nominal bead resolution",
        resolution.evaluated_witnesses
    ));
    if ui.button("Open Settings").clicked() {
        *action = Some(ReviewAction::OpenSettings);
    }
    true
}

#[cfg(test)]
pub(super) fn unsupported_layers(
    ui: &mut egui::Ui,
    layers: &[PrintLayer],
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
) {
    let unsupported = || {
        layers
            .iter()
            .filter(|layer| layer.region_coverage.unsupported_overhang_mm > 0.0)
    };
    let Some(featured) = unsupported().max_by(|left, right| {
        left.region_coverage
            .unsupported_overhang_mm
            .total_cmp(&right.region_coverage.unsupported_overhang_mm)
    }) else {
        return;
    };
    let count = unsupported().count();
    ui.colored_label(
        ui.visuals().warn_fg_color,
        format!(
            "Unsupported overhang · {count} {}",
            if count == 1 { "layer" } else { "layers" }
        ),
    );
    unsupported_layer_row(ui, featured, display, view);
    if display.is_none() {
        ui.weak("Actual-program layer preview unavailable.");
    }
    unsupported_details(ui, layers, featured, count, display, view);
}

fn unsupported_details(
    ui: &mut egui::Ui,
    layers: &[PrintLayer],
    featured: &PrintLayer,
    count: usize,
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
) {
    let unsupported = || {
        layers
            .iter()
            .filter(|layer| layer.region_coverage.unsupported_overhang_mm > 0.0)
    };
    ui.collapsing("Support details", |ui| {
        ui.label(
            "Each affected layer records its largest unsupported span, without an exact location.",
        );
        ui.label("Show layer reveals all actual paths on that layer, not an individual span.");
        ui.weak(format!(
            "Largest recorded span {:.3} mm · layer {}",
            featured.region_coverage.unsupported_overhang_mm,
            u64::from(featured.index) + 1
        ));
        if count > 1 {
            let row_height = ui.spacing().interact_size.y
                + 2.0 * ui.text_style_height(&egui::TextStyle::Body)
                + 2.0 * ui.spacing().item_spacing.y;
            egui::ScrollArea::vertical()
                .id_salt("unsupported-layers")
                .max_height(180.0)
                .show_rows(ui, row_height, count - 1, |ui, rows| {
                    for layer in unsupported()
                        .filter(|layer| layer.index != featured.index)
                        .skip(rows.start)
                        .take(rows.len())
                    {
                        unsupported_layer_row(ui, layer, display, view);
                        ui.weak(format!(
                            "Index {} · span {:.6} mm",
                            layer.index, layer.region_coverage.unsupported_overhang_mm
                        ));
                    }
                });
        }
    });
}

fn show_layer_button(
    ui: &mut egui::Ui,
    salt: &str,
    layer: &PrintLayer,
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
) {
    ui.push_id((salt, layer.index), |ui| {
        let window = display.and_then(|display| display.layer_windows.get(layer.index as usize));
        if ui
            .add_enabled(
                window.is_some(),
                egui::Button::new(format!("Show layer {}", u64::from(layer.index) + 1))
                    .selected(view.layer == Some(layer.index) && view.layer_end.is_none()),
            )
            .on_hover_text(format!(
                "Layer {} · Z {:.2} mm",
                u64::from(layer.index) + 1,
                layer.z_mm
            ))
            .clicked()
        {
            view.layer = Some(layer.index);
            view.layer_end = None;
            view.events = window.expect("enabled layer preview").end;
            view.role = None;
            view.tool = None;
            view.source = false;
            view.visible = true;
        }
    });
}

fn unsupported_layer_row(
    ui: &mut egui::Ui,
    layer: &PrintLayer,
    display: Option<&crate::replay::Replay>,
    view: &mut crate::replay::View,
) {
    ui.push_id(("unsupported-layer", layer.index), |ui| {
        let window = display.and_then(|display| display.layer_windows.get(layer.index as usize));
        if ui
            .add_enabled(
                window.is_some(),
                egui::Button::new(format!("Show layer {}", u64::from(layer.index) + 1))
                    .selected(view.layer == Some(layer.index) && view.layer_end.is_none()),
            )
            .on_hover_text(format!(
                "Recorded span {:.3} mm · Z {:.2} mm",
                layer.region_coverage.unsupported_overhang_mm, layer.z_mm
            ))
            .clicked()
        {
            view.layer = Some(layer.index);
            view.layer_end = None;
            view.events = window.expect("enabled layer preview").end;
            view.role = None;
            view.tool = None;
            view.source = false;
            view.visible = true;
        }
    });
}
/// Keep the exact source encoding visible; only an explicit sRGB target has an sRGB swatch.
pub(super) fn target(ui: &mut egui::Ui, color: AppearanceColor) {
    ui.collapsing("Target color", |ui| {
        if let Err(error) = color.validate() {
            ui.label(format!("Target cannot be displayed: {error}"));
            return;
        }
        match color {
            AppearanceColor::Srgb { channels } => {
                crate::ui::ColorPresentation::known(
                    channels.map(|v| (v * 255.0).round() as u8),
                    crate::ui::ColorMeaning::Target,
                )
                .show(ui, "target", crate::ui::ColorDensity::Standard);
                ui.label(format!("Encoded sRGB: R {} · G {} · B {}", channels[0], channels[1], channels[2]));
                ui.weak("The swatch and name use an 8-bit display sample; target coordinates remain unchanged.");
            }
            AppearanceColor::D65Lab { channels } => {
                ui.label(format!("Target · D65 Lab: L* {} · a* {} · b* {}", channels[0], channels[1], channels[2]));
            }
            AppearanceColor::XyzY1 { channels } => {
                ui.label(format!("Target · XYZ (Y = 1): X {} · Y {} · Z {}", channels[0], channels[1], channels[2]));
            }
            AppearanceColor::XyzY100 { channels } => {
                ui.label(format!("Target · XYZ (Y = 100): X {} · Y {} · Z {}", channels[0], channels[1], channels[2]));
            }
        }
    });
}

pub(super) fn assessment(ui: &mut egui::Ui, label: &str, value: &OpticalAssessment) {
    ui.label(label);
    match value {
        OpticalAssessment::FilteredPhase => {
            ui.weak("Filtered phase · no assessment");
        }
        OpticalAssessment::Unassessed { reason, exposure } => {
            ui.label(format!(
                "Unassessed: {reason:?} · nominal exposure {}",
                exposure_text(&exposure.fractions)
            ));
        }
        OpticalAssessment::Estimate {
            exposure,
            display_srgb8,
            delta_e00,
            within_tolerance,
            ..
        } => {
            crate::ui::ColorPresentation::known(
                *display_srgb8,
                crate::ui::ColorMeaning::ReferenceEstimate,
            )
            .show(
                ui,
                ("optical-color", label),
                crate::ui::ColorDensity::Standard,
            );
            ui.label(format!(
                "ΔE00 {:.3} · {}",
                delta_e00,
                if *within_tolerance {
                    "Within target"
                } else {
                    "Outside target"
                }
            ));
            ui.label(format!(
                "Nominal material exposure {}",
                exposure_text(&exposure.fractions)
            ));
        }
    }
}
pub(super) fn layer(ui: &mut egui::Ui, layer: &PrintLayer) {
    let r = &layer.region_coverage;
    if r.unsupported_overhang_mm > 0.0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Unsupported deposition · largest span {:.3} mm",
                r.unsupported_overhang_mm
            ),
        );
    }
    if r.thin_widened_mm > 0.0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Thin feature widened · {:.3} mm of bead at the narrowest width",
                r.thin_widened_mm
            ),
        );
    }
    if r.thin_omitted_area_mm2 > 0.0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Thin feature omitted · {:.4} mm² below a quarter nozzle",
                r.thin_omitted_area_mm2
            ),
        );
    }
    if r.support_omitted_wall_mm > 0.0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("Support wall omitted · {:.4} mm", r.support_omitted_wall_mm),
        );
    }
    ui.label(format!(
        "Source {:.4} + added {:.4} = realized {:.4} mm²",
        r.source_section_area_mm2, r.surface_added_area_mm2, r.section_area_mm2
    ));
    ui.label(format!("Solid interior {:.4}, sparse {:.4}, nominal intentional void {:.4}, protected holes {:.4} mm²",r.solid_interior_area_mm2,r.sparse_interior_area_mm2,r.intentional_void_area_mm2,r.protected_hole_area_mm2));
    ui.label(format!(
        "Separators {:.4} mm² · {} physical plates, {:.4} mm²",
        r.separator_area_mm2,
        r.process_diaphragms.len(),
        r.diaphragm_area_mm2
    ));
    for plate in &r.process_diaphragms {
        ui.label(format!("Occurrence {} · source Z {:.6} mm · requested thickness {:.6} mm · actual interval {:.6}…{:.6} mm · area {:.4} mm²",plate.occurrence_id.get(),plate.source_plane_mm,plate.requested_thickness_mm,plate.physical_z_mm[0],plate.physical_z_mm[1],plate.area_mm2));
    }
    for (index, region) in r.process_regions.iter().enumerate() {
        ui.label(format!(
            "Region {index}: {:.4} mm² at sparse fraction {:.4}",
            region.area_mm2, region.fraction
        ));
        for contribution in &region.contributions {
            ui.weak(format!(
                "Occurrence {} · provenance {:?}",
                contribution.occurrence_id.get(),
                contribution.provenance
            ));
        }
    }
}

fn exposure_text(fractions: &[f64; 3]) -> String {
    fractions
        .iter()
        .map(|f| format!("{:.2}%", 100.0 * f))
        .collect::<Vec<_>>()
        .join(" / ")
}
pub(super) fn assessment_label(value: &OpticalAssessment) -> String {
    match value {
        OpticalAssessment::FilteredPhase => "Filtered phase".into(),
        OpticalAssessment::Unassessed { reason, .. } => format!("Unassessed: {reason:?}"),
        OpticalAssessment::Estimate {
            display_srgb8,
            delta_e00,
            within_tolerance,
            ..
        } => format!(
            "{} · ΔE00 {:.3} · {}",
            crate::ui::ColorPresentation::known(
                *display_srgb8,
                crate::ui::ColorMeaning::ReferenceEstimate,
            )
            .summary(),
            delta_e00,
            if *within_tolerance {
                "Within target"
            } else {
                "Outside target"
            }
        ),
    }
}

fn interior_reason(reason: InteriorSelectionReason) -> &'static str {
    match reason {
        InteriorSelectionReason::ExplicitPrimary => "Primary tool only",
        InteriorSelectionReason::NoTimingModel => "No timing model",
        InteriorSelectionReason::NoObjective => "No selection objective",
        InteriorSelectionReason::GroupingDisabled => "Grouping off",
        InteriorSelectionReason::NoQualifiedAlternative => "No qualified alternative",
        InteriorSelectionReason::IncomparableTiming => "Timing differs",
        InteriorSelectionReason::ObjectiveNotMet => "Objective not met",
        InteriorSelectionReason::ImprovedModeledWork => "Lower modeled work",
    }
}

#[cfg(test)]
mod findings_tests {
    use super::*;
    use crate::ui::Theme;

    fn sample_intent() -> ManufacturingIntent {
        serde_json::from_str(include_str!(
            "../../../../fixtures/manufacturing/complete-sparse.intent.json"
        ))
        .unwrap()
    }

    fn empty_coverage() -> PlanarRegionCoverage {
        PlanarRegionCoverage {
            section_area_mm2: 0.0,
            source_section_area_mm2: 0.0,
            surface_added_area_mm2: 0.0,
            wall_area_mm2: 0.0,
            bottom_skin_area_mm2: 0.0,
            top_skin_area_mm2: 0.0,
            solid_interior_area_mm2: 0.0,
            sparse_interior_area_mm2: 0.0,
            intentional_void_area_mm2: 0.0,
            protected_hole_area_mm2: 0.0,
            nominal_deposition_volume_mm3: 0.0,
            support_area_mm2: 0.0,
            support_nominal_deposition_volume_mm3: 0.0,
            unsupported_overhang_mm: 0.0,
            support_omitted_wall_mm: 0.0,
            thin_widened_mm: 0.0,
            thin_omitted_area_mm2: 0.0,
            process_regions: Vec::new(),
            process_interfaces: Vec::new(),
            separator_area_mm2: 0.0,
            process_diaphragms: Vec::new(),
            diaphragm_area_mm2: 0.0,
            skin_bridge_domains: Vec::new(),
            web_roof_domains: Vec::new(),
        }
    }

    fn sample_layers() -> Vec<PrintLayer> {
        (0..4)
            .map(|index| PrintLayer {
                index,
                hatch_course: index,
                lower_z_mm: f64::from(index) * 0.2,
                z_mm: f64::from(index + 1) * 0.2,
                height_mm: 0.2,
                section_z_mm: f64::from(index) * 0.2 + 0.1,
                paths: Vec::new(),
                region_coverage: empty_coverage(),
                skin_command_fits: Vec::new(),
            })
            .collect()
    }

    fn sample_report() -> VerificationReport {
        VerificationReport {
            verified: true,
            deposition_segments: 4,
            travel_segments: 0,
            deposited_volume_mm3: 1.0,
            filament_length_mm: 0.4,
            support_deposition_segments: 0,
            support_deposited_volume_mm3: 0.0,
            support_filament_length_mm: 0.0,
            adhesion_deposition_segments: 0,
            adhesion_filament_length_mm: 0.0,
            retraction_count: 0,
            retracted_filament_mm: 0.0,
            lifecycle_motion_count: 0,
            priming_filament_length_mm: 0.0,
            max_position_error_mm: 0.0,
            max_extrusion_error_mm: 0.0,
            flow_limited_segments: 0,
            cooling_slowed_layers: 0,
            minimum_cooling_scale: 1.0,
            estimated_seconds: Some(12.0),
            job_time: None,
            routing: None,
            layer_height: LayerHeightAssessment {
                policy: LayerHeightPolicy::Preferred,
                requested_mm: 0.2,
                selected_mm: 0.2,
                realized_min_mm: 0.2,
                realized_max_mm: 0.2,
                layer_count: 4,
                differing_layer_count: 0,
                first_differing_layer: None,
                reason: LayerHeightReason::Requested,
            },
            coverage: Vec::new(),
            limitations: Vec::new(),
        }
    }

    fn sample_display() -> crate::replay::Replay {
        crate::replay::Replay {
            origin: glam::DVec3::ZERO,
            layer_windows: vec![0..4, 4..8, 8..12, 12..16],
            event_count: 16,
            batches: Vec::new(),
            seams: Vec::new(),
            owned_bytes: 0,
            beads: Err("Centerlines only".into()),
        }
    }

    fn click_button(
        context: &egui::Context,
        width: f32,
        label: &str,
        mut draw: impl FnMut(&mut egui::Ui) -> Option<ReviewAction>,
    ) -> Option<ReviewAction> {
        let mut run = |events| {
            let mut action = None;
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    action = draw(ui);
                },
            );
            (output, action)
        };
        let (output, _) = run(Vec::new());
        let point = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    assert!(
                        shape.clip_rect.contains_rect(text.visual_bounding_rect()),
                        "clipped text at width {width}: {}",
                        text.galley.text()
                    );
                    (text.galley.text() == label).then(|| text.visual_bounding_rect().center())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| panic!("expected button/label '{label}'"));
        output.drop_without_applying_deltas();
        let mut fired = None;
        for pressed in [true, false] {
            let (out, action) = run(vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            out.drop_without_applying_deltas();
            if action.is_some() {
                fired = action;
            }
        }
        fired
    }

    #[test]
    fn overhangs_with_supports_off_propose_automatic_supports_alternative() {
        let mut intent = sample_intent();
        intent.recipe.supports = SupportPolicy::Disabled;
        let expected_policy = intent
            .automatic_support_policy()
            .expect("single-tool fixture admits automatic supports");
        let mut layers = sample_layers();
        layers[2].region_coverage.unsupported_overhang_mm = 6.5;
        let report = sample_report();
        let display = sample_display();
        let surface = SurfaceResolutionTotals::default();

        for theme in [Theme::Graphite, Theme::Paper] {
            for width in [240.0, 400.0] {
                let context = egui::Context::default();
                theme.apply(&context);
                context.global_style_mut(|s| s.animation_time = 0.0);
                let mut view = crate::replay::View::default();
                let action = click_button(&context, width, "Try Automatic supports", |ui| {
                    findings_from_parts(
                        ui,
                        &layers,
                        &intent,
                        &report,
                        &surface,
                        Some(&display),
                        &mut view,
                        true,
                    )
                });
                match action {
                    Some(ReviewAction::TryAlternative { name, intent: next }) => {
                        assert_eq!(name, "Automatic supports");
                        assert_eq!(next.recipe.supports, expected_policy);
                    }
                    other => {
                        panic!("expected TryAlternative with automatic supports, got {other:?}")
                    }
                }
            }
        }
    }

    #[test]
    fn overhangs_with_supports_on_offer_reorient_in_prepare() {
        let mut intent = sample_intent();
        intent.recipe.supports = intent.automatic_support_policy().unwrap();
        let mut layers = sample_layers();
        layers[3].region_coverage.unsupported_overhang_mm = 4.2;
        let report = sample_report();
        let display = sample_display();
        let surface = SurfaceResolutionTotals::default();
        let context = egui::Context::default();
        Theme::Graphite.apply(&context);
        context.global_style_mut(|s| s.animation_time = 0.0);
        let mut view = crate::replay::View::default();
        let action = click_button(&context, 320.0, "Re-orient part", |ui| {
            findings_from_parts(
                ui,
                &layers,
                &intent,
                &report,
                &surface,
                Some(&display),
                &mut view,
                true,
            )
        });
        assert_eq!(action, Some(ReviewAction::OpenPosition));
    }

    #[test]
    fn thin_omitted_wall_and_layer_height_findings_surface_actions() {
        let intent = sample_intent();
        let candidate = finer_wall_candidate(&intent).expect("fixture bead width can narrow");
        assert!(candidate.recipe.bead_width_mm < intent.recipe.bead_width_mm);
        let mut layers = sample_layers();
        layers[1].region_coverage.thin_omitted_area_mm2 = 0.42;
        let mut report = sample_report();
        report.layer_height.differing_layer_count = 2;
        report.layer_height.first_differing_layer = Some(2);
        report.layer_height.realized_min_mm = 0.10;
        let display = sample_display();
        let surface = SurfaceResolutionTotals::default();
        let context = egui::Context::default();
        Theme::Paper.apply(&context);
        context.global_style_mut(|s| s.animation_time = 0.0);
        let mut view = crate::replay::View::default();

        let wall_label = format!("Try {:.2} mm wall", candidate.recipe.bead_width_mm);
        let wall_action = click_button(&context, 320.0, &wall_label, |ui| {
            findings_from_parts(
                ui,
                &layers,
                &intent,
                &report,
                &surface,
                Some(&display),
                &mut view,
                true,
            )
        });
        match wall_action {
            Some(ReviewAction::TryAlternative { intent: next, .. }) => {
                assert_eq!(next.recipe.bead_width_mm, candidate.recipe.bead_width_mm);
            }
            other => panic!("expected finer wall TryAlternative, got {other:?}"),
        }

        let layer_action = click_button(&context, 320.0, "Inspect layer 3", |ui| {
            findings_from_parts(
                ui,
                &layers,
                &intent,
                &report,
                &surface,
                Some(&display),
                &mut view,
                true,
            )
        });
        assert_eq!(
            layer_action,
            Some(ReviewAction::InspectLayer {
                layer: 2,
                compare_section: true,
            })
        );
    }

    #[test]
    fn clean_verified_job_reports_no_advisories() {
        let intent = sample_intent();
        let layers = sample_layers();
        let report = sample_report();
        let surface = SurfaceResolutionTotals::default();
        let context = egui::Context::default();
        let mut view = crate::replay::View::default();
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(
                findings_from_parts(
                    ui, &layers, &intent, &report, &surface, None, &mut view, true
                ),
                None
            );
        });
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("No construction advisories")),
            "expected clean advisory banner, got {texts:?}"
        );
    }
}
