// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{
    appearance::AppearanceColor, manufacturing::*, optical::OpticalAssessment,
};

pub(super) fn details(ui: &mut egui::Ui, review: &crate::manufacture::Review) {
    let report = &review.capture.report;
    let summary = &review.capture.record.summary;
    let unsupported = || {
        review
            .bundle
            .plan
            .layers
            .iter()
            .filter(|layer| layer.region_coverage.unsupported_overhang_mm > 0.0)
    };
    let count = unsupported().count();
    if count > 0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("Unsupported deposition · {count} layers"),
        );
        ui.weak("Supports are disabled. Verification checks commands, not printability; inspect these spans before printing. Each row records the first unsupported span found on that layer, not its worst span.");
        ui.collapsing("Unsupported layers", |ui| {
            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            egui::ScrollArea::vertical()
                .id_salt("unsupported-layers")
                .max_height(180.0)
                .show_rows(ui, row_height, count, |ui, rows| {
                    for layer in unsupported().skip(rows.start).take(rows.len()) {
                        ui.label(format!(
                            "Layer {} · Z {:.3} mm · recorded span {:.3} mm",
                            layer.index, layer.z_mm, layer.region_coverage.unsupported_overhang_mm
                        ));
                    }
                });
        });
    }
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
            crate::ui::resolved_value(ui, "Modeled work", &format!("{:.1} min", time.modeled_seconds() / 60.0), "Declared model");
            crate::ui::resolved_value(ui, "Startup", "Unknown", "Homing and heating");
            ui.collapsing("Timing details", |ui| {
                for (label, seconds) in [
                    ("Model deposition", time.model_deposition_seconds),
                    ("Auxiliary deposition", time.auxiliary_deposition_seconds),
                    ("Travel", time.travel_seconds), ("Feeder", time.feeder_seconds),
                    ("Tool activation", time.activation_seconds),
                    ("Command overhead", time.command_overhead_seconds),
                ] { crate::ui::resolved_value(ui, label, &format!("{seconds:.1} s"), ""); }
                ui.label(format!("Model {} · {}", time.model.id, time.model.revision));
                ui.weak("Rest at each command · declared acceleration and activation delays · physical timing unqualified");
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
        ui.label(format!("{} retractions · {:.4} mm withdrawn, distinct from deposition",report.retraction_count,report.retracted_filament_mm));
        ui.label(format!("{} lifecycle motions · {:.4} mm prime / route waste filament",report.lifecycle_motion_count,report.priming_filament_length_mm));
        if let Some(routing)=&report.routing {ui.label(format!("{} decoded tool selections",routing.selection_count));for tool in &routing.tools {ui.label(format!("Tool {} · {}: {} model strokes, {:.4} mm³ / {:.4} mm filament; {} waste strokes, {:.4} mm³ / {:.4} mm filament; final feeder debt {:.8} mm",tool.tool,tool.material_id,tool.model_deposition_segments,tool.model_volume_mm3,tool.model_filament_length_mm,tool.waste_deposition_segments,tool.waste_volume_mm3,tool.waste_filament_length_mm,tool.outstanding_retraction_mm));}}
        ui.weak(format!("Maximum position deviation {:.9} mm; extrusion deviation {:.9} mm",report.max_position_error_mm,report.max_extrusion_error_mm));
    });
    ui.collapsing("Native geometry evidence",|ui|{
        let n=&summary.native_geometry;let c=&n.source_feature_coverage;
        ui.label(format!("{:?}: {} occurrence analyses · {} closed slabs · {} certified layers · {} events",n.evidence,n.height_analysis_count,n.slab_count,n.certified_layer_count,n.event_count));
        ui.label(format!("Requested shape error {:.9} mm; achieved {:.9} mm",n.requested_shape_error_mm,n.achieved_shape_error_mm));
        ui.label(format!("Disjoint-source layers {}; invariant source-union layers {}; native interface-complex layers {}; original-occurrence critical slabs {}",c.disjoint_source_layer_count,c.invariant_source_union_layer_count,c.native_interface_complex_layer_count,c.owned_critical_slab_count));
        ui.weak("Per-occurrence certificates do not prove a global union feature bound. Skin coverage uses scheduled material distance; support clearance uses continuous target outer occupancy. Actual strokes alone establish deposition anchors. No swept-tool or physical collision certificate.");
    });
    ui.collapsing("Surface resolution advisories",|ui|{
        let r=&summary.surface_resolution;
        ui.label(format!("{} sampled witnesses · resolved {} · ambiguous {} · below nominal resolution {} · unavailable {}",r.evaluated_witnesses,r.resolved,r.ambiguous,r.below_nominal_resolution,r.unavailable));
        ui.label(if r.coverage_complete{"Sample coverage complete"}else{"Sample coverage incomplete"});
        ui.weak("Sampled nominal-bead advice is separate from independent emitted-program verification and physical image fidelity.");
    });
    if !review.capture.provenance.surface_color.is_empty() {
        ui.collapsing("Constructed surface exposure",|ui|{
        for face in &review.capture.provenance.surface_color{
            ui.label(format!("Construction {} · {} · {:.3} × {:.3} mm · {} replay-qualified intervals",face.construction_index,face.face_id.as_str(),face.realized_patch.width_mm,face.realized_patch.height_mm,face.interval_count));
            ui.label(format!("Nominal side exposure: tool 0 {:.2}% / tool 1 {:.2}%",face.fractions_by_tool[0]*100.0,face.fractions_by_tool[1]*100.0));
            if let Some(reference)=&face.reference{
                ui.weak("Captured reference estimate; backing optics are unmodelled. No calibrated or physical color qualification.");
                ui.label(if reference.patch_matches_request{"Requested patch matches"}else{"Realized patch differs from requested study"});
                if !reference.pose_matches_application{ui.label("Occurrence pose changed since application");}
                ui.push_id(("surface-reference",face.construction_index,face.face_id.as_str()), |ui| {
                    assessment(ui,"Requested cycle",&reference.requested);
                    assessment(ui,"Realized exposure",&reference.realized);
                });
            }
        }
    });
    }
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
                "Unsupported deposition · first recorded span {:.3} mm",
                r.unsupported_overhang_mm
            ),
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
