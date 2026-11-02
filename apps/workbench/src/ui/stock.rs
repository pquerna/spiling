// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Production/gallery presentation of engine-owned saved-stock decisions.
use spiling_contracts::{appearance::AppearanceColor, optical::OpticalAssessment, stock::*};
pub fn stock_target(ui: &mut egui::Ui, hex: &mut String) {
    ui.horizontal_wrapped(|ui| {
        let label = ui.label("Target color");
        ui.add(
            egui::TextEdit::singleline(hex)
                .char_limit(7)
                .desired_width(100.0),
        )
        .labelled_by(label.id);
        if let Ok(rgb) = spiling_contracts::color_naming::parse_srgb8_hex(hex) {
            let mut picked = rgb;
            if ui.color_edit_button_srgb(&mut picked).changed() {
                *hex = spiling_contracts::color_naming::srgb8_hex(picked);
            }
            super::ColorPresentation::known(rgb, super::ColorMeaning::Target).show(
                ui,
                "stock-target-color",
                super::ColorDensity::Compact,
            );
        }
    });
}
fn candidate(ui: &mut egui::Ui, row: &StockCandidate) {
    let rgb = row.appearance.as_ref().and_then(|a| match a.color {
        AppearanceColor::Srgb { channels } => Some(channels.map(|v| (v * 255.0).round() as u8)),
        _ => None,
    });
    let color = rgb.map_or(
        super::ColorPresentation::unspecified(super::ColorMeaning::DeclaredAppearance),
        |rgb| super::ColorPresentation::known(rgb, super::ColorMeaning::DeclaredAppearance),
    );
    color.show(
        ui,
        (&row.filament.id.0, row.filament.revision.as_str()),
        super::ColorDensity::Compact,
    );
    ui.weak(if row.in_project && row.in_library {
        "Project + My library"
    } else if row.in_project {
        "Project"
    } else {
        "My library"
    });
    if row.on_hand_spools > 0 {
        ui.weak(format!("{} on hand", row.on_hand_spools));
    }
}
pub fn stock_results(
    ui: &mut egui::Ui,
    report: &StockMatchReport,
    selected: &mut Option<StockChoice>,
) -> bool {
    let mut review = false;
    super::group(ui, "Closest filaments");
    if report.singles.is_empty() {
        ui.weak("No individual color estimates available.");
    }
    for (i, result) in report.singles.iter().enumerate() {
        let row = &report.candidates[usize::from(result.candidate)];
        ui.push_id(("stock-single", i), |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(
                        *selected == Some(StockChoice::Single(i)),
                        format!("{} · ΔE {:.2}", row.name, result.delta_e00),
                    )
                    .clicked()
                {
                    *selected = Some(StockChoice::Single(i));
                    review = true;
                }
                if !result.within_tolerance {
                    ui.weak("Outside color goal");
                }
                candidate(ui, row);
            });
        });
    }
    super::group(ui, "Layered recipes");
    if report.pairs.is_empty() {
        ui.weak("No assessed two-material recipes found.");
    }
    for (i, result) in report.pairs.iter().enumerate() {
        let a = &report.candidates[usize::from(result.candidates[0])];
        let b = &report.candidates[usize::from(result.candidates[1])];
        if let OpticalAssessment::Estimate {
            delta_e00,
            display_srgb8,
            within_tolerance,
            ..
        } = result.assessment
        {
            ui.push_id(("stock-pair", i), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .selectable_label(
                            *selected == Some(StockChoice::Pair(i)),
                            format!("{} + {} · ΔE {:.2}", a.name, b.name, delta_e00),
                        )
                        .clicked()
                    {
                        *selected = Some(StockChoice::Pair(i));
                        review = true;
                    }
                    if !within_tolerance {
                        ui.weak("Outside color goal");
                    }
                    super::ColorPresentation::known(
                        display_srgb8,
                        super::ColorMeaning::ReferenceEstimate,
                    )
                    .show(ui, "stock-predicted", super::ColorDensity::Compact);
                });
            });
        }
    }
    if report.unassessed_singles > 0 || report.unassessed_pairs > 0 {
        ui.collapsing(format!("Unavailable estimates · {} filaments, {} pairs",report.unassessed_singles,report.unassessed_pairs),|ui| {
            ui.weak("Missing or unsupported declarations remain unassessed. They are not zero, white or a successful match.");
            for row in report.candidates.iter().filter(|c|c.appearance.is_none()) {ui.label(&row.name);}
        });
    }
    review
}
