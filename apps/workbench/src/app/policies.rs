// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::ui;
use spiling_contracts::manufacturing::*;

pub(super) fn prepared(value: &ManufacturingIntent) -> Result<ManufacturingIntent, String> {
    let mut result = value.clone();
    let normalize = result.recipe.seam_policy.validate().is_err();
    if let SeamPolicy::PreferredBuildDirection { xy } = &mut result.recipe.seam_policy
        && normalize
    {
        let scale = xy[0].abs().max(xy[1].abs());
        if !xy.iter().all(|v| v.is_finite()) || !scale.is_finite() || scale == 0.0 {
            return Err(
                "Preferred seam direction requires a finite nonzero build XY vector".into(),
            );
        }
        let x = xy[0] / scale;
        let y = xy[1] / scale;
        let length = x.hypot(y);
        *xy = [x / length, y / length].map(|v| if v == 0.0 { 0.0 } else { v });
    }
    result.validate().map_err(|e| e.to_string())?;
    Ok(result)
}
pub(super) fn fields(ui: &mut egui::Ui, intent: &mut ManufacturingIntent) {
    ui.collapsing("Native accuracy and material",|ui|{
        ui::number(ui,"Native shape error (mm)",&mut intent.recipe.native_shape_error_mm,0.001);
        ui.add_enabled_ui(intent.material_profile.is_none(), |ui| { ui::number(ui,"Filament diameter (mm)",&mut intent.recipe.filament_diameter_mm,0.01); });
        ui::number(ui,"Minimum wall width (mm)",&mut intent.printer.deposition_limits.min_bead_width_mm,0.01);
        ui::number(ui,"Maximum wall width (mm)",&mut intent.printer.deposition_limits.max_bead_width_mm,0.01);
        ui.weak("Declared software limits; native admission and actual-program verification remain mandatory.");
    });
    ui.collapsing("Sparse band policy",|ui|{
        let mut combine=matches!(intent.recipe.sparse_infill,SparseInfillPolicy::CombineEligible{..});
        if ui.checkbox(&mut combine,"Combine eligible sparse interiors").changed(){intent.recipe.sparse_infill=if combine{SparseInfillPolicy::CombineEligible{max_height_mm:0.0}}else{SparseInfillPolicy::Fine};}
        if let SparseInfillPolicy::CombineEligible{max_height_mm}=&mut intent.recipe.sparse_infill {ui::number(ui,"Maximum physical height (mm)",max_height_mm,0.01);}
        ui.weak("Walls, skins and protected construction remain fine. Native vetoes can retain fine sparse intervals.");
    });
    ui.add_enabled_ui(intent.material_profile.is_none(), |ui| {
        ui.push_id("recipe-process", |ui| {
            process_fields(ui, &mut intent.recipe.process)
        });
    });
    ui.collapsing("Bed-rooted breakaway supports", |ui| {
        let mut enabled = matches!(intent.recipe.supports, SupportPolicy::Breakaway { .. });
        if ui
            .checkbox(&mut enabled, "Enable breakaway supports")
            .changed()
        {
            intent.recipe.supports = if enabled {
                SupportPolicy::Breakaway {
                    density: 0.0,
                    xy_clearance_mm: 0.0,
                    z_gap_mm: 0.0,
                    interface_thickness_mm: 0.0,
                }
            } else {
                SupportPolicy::Disabled
            };
        }
        if let SupportPolicy::Breakaway {
            density,
            xy_clearance_mm,
            z_gap_mm,
            interface_thickness_mm,
        } = &mut intent.recipe.supports
        {
            for (label, value) in [
                ("Support fraction", density),
                ("XY clearance (mm)", xy_clearance_mm),
                ("Z gap (mm)", z_gap_mm),
                ("Interface thickness (mm)", interface_thickness_mm),
            ] {
                ui::number(ui, label, value, 0.01);
            }
        }
    });
    ui.collapsing("Seam and build-axis policies",|ui|{
        let mut preferred=matches!(intent.recipe.seam_policy,SeamPolicy::PreferredBuildDirection{..});
        if ui.checkbox(&mut preferred,"Preferred build/machine XY seam direction").changed(){intent.recipe.seam_policy=if preferred{SeamPolicy::PreferredBuildDirection{xy:[0.0,0.0]}}else{SeamPolicy::Nearest};}
        if let SeamPolicy::PreferredBuildDirection{xy}=&mut intent.recipe.seam_policy{ui::number(ui,"Build X direction",&mut xy[0],0.1);ui::number(ui,"Build Y direction",&mut xy[1],0.1);ui.weak("Apply normalizes a finite nonzero direction. It selects existing perimeter vertices; bridge/support orientation remains native.");}
        for (label,value) in [("Interior hatch",&mut intent.recipe.rectilinear_axis),("Top skin hatch",&mut intent.recipe.top_skin_axis)]{
            egui::ComboBox::from_id_salt(label).selected_text(format!("{label}: {value:?}")).show_ui(ui,|ui|{for variant in [RectilinearAxisPolicy::X,RectilinearAxisPolicy::Y,RectilinearAxisPolicy::AlternatingXY,RectilinearAxisPolicy::AlternatingYX]{ui.selectable_value(value,variant,format!("{variant:?}"));}});
        }
    });
}

/// Shared controls for explicit recipe policy or one captured profile override.
pub(super) fn process_fields(ui: &mut egui::Ui, p: &mut ProcessPolicy) {
    ui.collapsing("First layer, cooling and bridges", |ui| {
        for (label, value) in [
            ("First layer speed factor", &mut p.first_layer_speed_factor),
            ("First layer flow factor", &mut p.first_layer_flow_factor),
            ("Bridge speed factor", &mut p.bridge_speed_factor),
            ("Bridge flow factor", &mut p.bridge_flow_factor),
            ("Maximum bridge span (mm)", &mut p.max_bridge_span_mm),
        ] {
            ui::number(ui, label, value, 0.01);
        }
        for (label, value) in [
            ("First-layer fan PWM", &mut p.fan_first_layers),
            ("Later-layer fan PWM", &mut p.fan_after_first_layers),
            ("Bridge fan PWM", &mut p.bridge_fan_pwm),
        ] {
            ui.horizontal_wrapped(|ui| {
                ui.label(label);
                ui.add(egui::DragValue::new(value));
            });
        }
    });
    ui.collapsing("Retraction", |ui| {
        let mut enabled = matches!(p.retraction, RetractionPolicy::Enabled { .. });
        if ui
            .checkbox(&mut enabled, "External absolute-E retraction")
            .changed()
        {
            p.retraction = if enabled {
                RetractionPolicy::Enabled {
                    distance_mm: 0.0,
                    speed_mm_s: 0.0,
                    minimum_travel_mm: 0.0,
                    z_hop_mm: 0.0,
                }
            } else {
                RetractionPolicy::Disabled
            };
        }
        if let RetractionPolicy::Enabled {
            distance_mm,
            speed_mm_s,
            minimum_travel_mm,
            z_hop_mm,
        } = &mut p.retraction
        {
            for (label, value) in [
                ("Withdrawn filament (mm)", distance_mm),
                ("Feeder speed (mm/s)", speed_mm_s),
                ("Minimum travel (mm)", minimum_travel_mm),
                ("Z hop (mm)", z_hop_mm),
            ] {
                ui::number(ui, label, value, 0.01);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn applying_a_direction_preserves_every_other_declared_field() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/manufacturing/complete-sparse.intent.json");
        let original = decode_intent(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(prepared(&original).unwrap(), original);
        let mut draft = original.clone();
        draft.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [1e308, 1e308] };
        let result = prepared(&draft).unwrap();
        let SeamPolicy::PreferredBuildDirection { xy } = result.recipe.seam_policy else {
            panic!("direction lost");
        };
        assert!((xy[0] - 1.0 / 2f64.sqrt()).abs() < 1e-15);
        let mut restored = result.clone();
        restored.recipe.seam_policy = original.recipe.seam_policy;
        assert_eq!(restored, original);
        draft.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [0.0, 0.0] };
        assert!(prepared(&draft).is_err());
        let mut edited = result.clone();
        edited.recipe.flow_multiplier = 0.95;
        let mut expected = result;
        expected.recipe.flow_multiplier = 0.95;
        assert_eq!(prepared(&edited).unwrap(), expected);
    }
}
