// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use egui::Ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierAction {
    Change,
    Remove,
}

/// Presentation only. Values are read-only; Change enters the owning draft.
pub fn modifier_row(
    ui: &mut Ui,
    property: &str,
    value: &str,
    scope: &str,
    writable: bool,
) -> Option<ModifierAction> {
    row(ui, property, value, scope, writable).0
}

/// Compact proposed values. The arrow is painted, independent of font coverage.
pub fn modifier_delta(ui: &mut Ui, property: &str, before: &str, after: &str) {
    ui.label(property);
    ui.horizontal_wrapped(|ui| {
        ui.label(before);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(14.0, 16.0), egui::Sense::hover());
        ui.painter().arrow(
            rect.left_center() + egui::vec2(1.0, 0.0),
            egui::vec2(12.0, 0.0),
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, "changes to"));
        ui.label(egui::RichText::new(after).strong());
    });
}

fn row(
    ui: &mut Ui,
    property: &str,
    value: &str,
    scope: &str,
    writable: bool,
) -> (Option<ModifierAction>, egui::Rect) {
    let mut action = None;
    let mut menu_rect = egui::Rect::NOTHING;
    ui.horizontal(|ui| {
        // Reserve the action before laying out variable text. A nested enabled UI
        // in a wrapping row otherwise squeezes the menu to one character per line.
        let body_width = (ui.available_width() - 32.0 - ui.spacing().item_spacing.x).max(0.0);
        ui.allocate_ui_with_layout(
            egui::vec2(body_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(property);
                    ui.label(egui::RichText::new(value).strong());
                });
                if !scope.is_empty() {
                    ui.weak(scope);
                }
            },
        );
        ui.add_enabled_ui(writable, |ui| {
            let menu = ui.menu_button("…", |ui| {
                if ui.button("Change").clicked() {
                    action = Some(ModifierAction::Change);
                    ui.close();
                }
                if ui
                    .button("Remove modifier")
                    .on_hover_text(
                        "Remove only this instruction; earlier or inherited instructions remain.",
                    )
                    .clicked()
                {
                    action = Some(ModifierAction::Remove);
                    ui.close();
                }
            });
            menu.response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Button,
                    writable,
                    format!("Actions for {property} · {value} · {scope}"),
                )
            });
            menu.response
                .clone()
                .on_hover_text(format!("Actions for {property}"));
            menu_rect = menu.response.rect;
        });
    });
    (action, menu_rect)
}

pub fn resolved_value(ui: &mut Ui, property: &str, value: &str, source: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(property);
        ui.label(egui::RichText::new(value).strong());
    });
    if !source.is_empty() {
        ui.weak(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn narrow_rows_reserve_a_single_line_action_in_both_themes() {
        for theme in [crate::ui::Theme::Graphite, crate::ui::Theme::Paper] {
            for width in [160.0, 210.0, 320.0] {
                let ctx = egui::Context::default();
                theme.apply(&ctx);
                for _ in 0..2 {
                    ctx.run_ui(egui::RawInput::default(), |ui| {
                        ui.set_width(width);
                        let left = ui.cursor().left();
                        let (_, menu) = row(ui, "Infill density", "40.0%", "Source box", true);
                        assert!(
                            menu.right() <= left + width + 1.0,
                            "{theme:?} {width}: {menu:?}"
                        );
                        assert!(
                            menu.height() <= 32.0,
                            "action must not wrap into letters: {menu:?}"
                        );
                    })
                    .drop_without_applying_deltas();
                }
            }
        }
    }
}
