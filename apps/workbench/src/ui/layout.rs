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
    if request.objective != LayoutObjective::Copies {
        request.use_additional_plates = false;
    }
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
            ui.checkbox(&mut request.use_additional_plates, "Use additional plates")
                .on_hover_text("Create additional print jobs with the same setup when the requested quantity cannot fit. Review every proposed plate before Apply.");
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

/// Bounded source reservations for proposed jobs; this never grants picking authority.
pub fn additional_layout_previews(ui: &mut Ui, proposal: &LayoutProposal, bed: Option<AabbMm>) {
    if proposal.additional_plates.is_empty() {
        return;
    }
    super::group(ui, "Proposed plates");
    ui.label(format!(
        "Current plate · {} {}",
        proposal.placed_count,
        if proposal.placed_count == 1 {
            "part"
        } else {
            "parts"
        }
    ));
    let mut bounds = bed.unwrap_or(proposal.footprints_mm[0]);
    for b in proposal.footprints_mm.iter().chain(
        proposal
            .additional_plates
            .iter()
            .flat_map(|p| &p.footprints_mm),
    ) {
        for a in 0..2 {
            bounds.min[a] = bounds.min[a].min(b.min[a]);
            bounds.max[a] = bounds.max[a].max(b.max[a]);
        }
    }
    for (index, plate) in proposal.additional_plates.iter().enumerate() {
        ui.label(format!(
            "New plate {} · {} {}",
            index + 1,
            plate.placements.len(),
            if plate.placements.len() == 1 {
                "part"
            } else {
                "parts"
            }
        ));
        let (area, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 100.0),
            egui::Sense::hover(),
        );
        let scale = ((area.width() - 12.0) as f64 / (bounds.max[0] - bounds.min[0]).max(1.0))
            .min(88.0 / (bounds.max[1] - bounds.min[1]).max(1.0));
        let center = area.center();
        let point = |p: [f64; 3]| {
            egui::pos2(
                center.x + ((p[0] - (bounds.min[0] + bounds.max[0]) * 0.5) * scale) as f32,
                center.y - ((p[1] - (bounds.min[1] + bounds.max[1]) * 0.5) * scale) as f32,
            )
        };
        if let Some(bed) = bed {
            ui.painter().rect_stroke(
                egui::Rect::from_two_pos(point(bed.min), point(bed.max)),
                2.0,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }
        for b in &plate.footprints_mm {
            let rect = egui::Rect::from_two_pos(point(b.min), point(b.max));
            ui.painter().rect_filled(
                rect,
                2.0,
                ui.visuals().selection.bg_fill.gamma_multiply(0.45),
            );
            ui.painter().rect_stroke(
                rect,
                2.0,
                ui.visuals().widgets.active.fg_stroke,
                egui::StrokeKind::Inside,
            );
        }
    }
    ui.weak("Part footprints · shared scale");
}
