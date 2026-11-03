// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use egui::Ui;
use spiling_contracts::geometry::*;

pub fn layout_fields(ui: &mut Ui, request: &mut LayoutRequest, parts: &[(OccurrenceId, String)]) {
    let previous = request.objective;
    super::group(ui, "Goal");
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(
            &mut request.objective,
            LayoutObjective::Copies,
            "Make copies",
        );
        ui.selectable_value(
            &mut request.objective,
            LayoutObjective::Arrange,
            "Arrange parts",
        );
        ui.selectable_value(&mut request.objective, LayoutObjective::Fill, "Fill plate");
    });
    if request.objective != previous {
        request.quantity = if request.objective == LayoutObjective::Fill {
            MAX_OCCURRENCES
                .saturating_sub(parts.len().saturating_sub(1) as u32)
                .max(1)
        } else {
            2
        };
    }
    if request.objective != LayoutObjective::Arrange {
        ui.label("Part");
        egui::ComboBox::from_id_salt("layout-template")
            .wrap_mode(egui::TextWrapMode::Truncate)
            .width(ui.available_width().min(240.0))
            .selected_text(
                parts
                    .iter()
                    .find(|(id, _)| Some(*id) == request.template)
                    .map_or("Choose a part", |(_, name)| name.as_str()),
            )
            .show_ui(ui, |ui| {
                for (id, name) in parts {
                    ui.selectable_value(&mut request.template, Some(*id), name);
                }
            });
        ui.horizontal_wrapped(|ui| {
            ui.label(if request.objective == LayoutObjective::Fill {
                "Quantity limit"
            } else {
                "Quantity"
            });
            ui.add(egui::DragValue::new(&mut request.quantity).range(1..=MAX_OCCURRENCES));
        });
        if request.objective == LayoutObjective::Copies {
            ui.weak("Includes the selected original");
        } else {
            ui.weak("Best count found, up to this limit");
        }
    } else {
        request.template = None;
    }
    super::group(ui, "Layout");
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut request.style, LayoutStyle::Balanced, "Balanced");
        ui.selectable_value(&mut request.style, LayoutStyle::Dense, "Dense");
    });
    super::number(ui, "Spacing (mm)", &mut request.spacing_mm, 0.5);
    egui::CollapsingHeader::new("Pinned parts").show(ui, |ui| {
        for (id, name) in parts {
            let mut pinned = request.pinned.contains(id);
            if ui.checkbox(&mut pinned, name).changed() {
                if pinned {
                    request.pinned.push(*id);
                } else {
                    request.pinned.retain(|p| p != id);
                }
            }
        }
    });
    super::help(
        ui,
        "layout-policy",
        "Arrangement",
        "The engine preserves each part's orientation and height. Search reserves native geometry, finishes, declared obstacles and construction space, then compiles and independently replays the candidate. Search is bounded and does not prove a maximum count or physical collision clearance. Close leaves Current unchanged.",
    );
}
