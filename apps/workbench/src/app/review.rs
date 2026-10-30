// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{manufacturing::*, optical::OpticalAssessment};

pub(super) fn details(ui: &mut egui::Ui, review: &crate::manufacture::Review) {
    let report = &review.capture.report;
    let summary = &review.capture.record.summary;
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
                assessment(ui,"Requested",&reference.requested);assessment(ui,"Realized",&reference.realized);
            }
        }
    });
    }
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
            ui.horizontal_wrapped(|ui| {
                let color =
                    egui::Color32::from_rgb(display_srgb8[0], display_srgb8[1], display_srgb8[2]);
                ui.colored_label(color, "■");
                ui.label(format!(
                    "Reference sRGB {} / {} / {} · ΔE00 {:.3} · {}",
                    display_srgb8[0],
                    display_srgb8[1],
                    display_srgb8[2],
                    delta_e00,
                    if *within_tolerance {
                        "Within target"
                    } else {
                        "Outside target"
                    }
                ));
            });
            ui.label(format!(
                "Nominal material exposure {}",
                exposure_text(&exposure.fractions)
            ));
        }
    }
}
pub(super) fn layer(ui: &mut egui::Ui, layer: &PrintLayer) {
    let r = &layer.region_coverage;
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
            delta_e00,
            within_tolerance,
            ..
        } => format!(
            "ΔE00 {:.3} · {}",
            delta_e00,
            if *within_tolerance {
                "Within target"
            } else {
                "Outside target"
            }
        ),
    }
}
