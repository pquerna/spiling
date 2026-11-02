// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use egui::{Color32, RichText, Ui};

/// Presentation only: callers retain command enablement and receipt ownership.
pub fn primary(ui: &mut Ui, label: &str, disabled: Option<&str>) -> bool {
    let clicked = ui
        .add_enabled(
            disabled.is_none(),
            egui::Button::new(RichText::new(label).color(Color32::WHITE))
                .fill(Color32::from_rgb(23, 95, 181)),
        )
        .clicked();
    if let Some(reason) = disabled {
        ui.add(egui::Label::new(reason).wrap());
    }
    clicked
}

pub fn pane_header(ui: &mut Ui, title: &str, scope: &str) {
    ui.label(RichText::new(title).size(14.0).strong());
    if !scope.is_empty() {
        ui.add(egui::Label::new(RichText::new(scope).weak()).wrap());
    }
    ui.add_space(4.0);
}

pub fn group(ui: &mut Ui, title: impl Into<String>) {
    ui.add_space(4.0);
    ui.label(RichText::new(title.into()).strong());
}

/// A deliberate, focusable explanation. Essential state stays in the caller.
pub fn help(ui: &mut Ui, id: impl egui::AsIdSalt, title: &str, detail: &str) -> egui::Response {
    ui.push_id(id, |ui| {
        let response = ui.button("About…").on_hover_text(title);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("About {title}"))
        });
        let id = egui::Popup::default_response_id(&response);
        let was_open = egui::Popup::is_id_open(ui.ctx(), id);
        let keyboard = response.has_focus()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F1));
        let popup = egui::Popup::from_response(&response)
            .open_memory((response.clicked() || keyboard).then_some(egui::SetOpenCommand::Toggle))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(300.0)
            .show(|ui| {
                pane_header(ui, title, "");
                ui.add(egui::Label::new(detail).wrap().selectable(true));
                let close = ui.button("Close help");
                if !was_open {
                    close.request_focus();
                }
                if close.clicked() {
                    ui.close();
                }
            });
        if popup.as_ref().is_some_and(|p| p.response.should_close()) {
            // A help Escape belongs to this popup, never to the containing tool.
            ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
            response.request_focus();
        }
        response
    })
    .inner
}

/// Bounded readable summary; the original evidence remains selectable in details.
pub fn problem(ui: &mut Ui, summary: &str, detail: &str) {
    ui.label(RichText::new(summary).color(ui.visuals().error_fg_color));
    egui::CollapsingHeader::new("Details")
        .id_salt((summary, detail))
        .show(ui, |ui| {
            ui.add(egui::Label::new(detail).wrap().selectable(true));
        });
}

pub fn close(ui: &mut Ui, label: &str) -> bool {
    ui.button(label).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_help_owns_escape_and_restores_its_invoker() {
        let ctx = egui::Context::default();
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let frame = |events| {
            let mut response = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    response = Some(help(
                        ui,
                        "help-test",
                        "Layer height",
                        "A preferred goal can be adjusted within the captured process envelope.",
                    ));
                },
            );
            output.drop_without_applying_deltas();
            response.unwrap()
        };
        let response = frame(vec![]);
        response.request_focus();
        frame(vec![]);
        frame(vec![key(egui::Key::F1)]);
        assert!(egui::Popup::is_any_open(&ctx));
        frame(vec![key(egui::Key::Escape)]);
        assert!(!egui::Popup::is_any_open(&ctx));
        assert_eq!(ctx.memory(|m| m.focused()), Some(response.id));
        assert!(
            !ctx.input(|i| i.key_pressed(egui::Key::Escape)),
            "Escape must not reach a parent surface"
        );
    }
}
