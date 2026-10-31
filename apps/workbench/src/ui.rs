// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use egui::{Color32, RichText, Ui};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Graphite,
    Paper,
}
#[derive(Clone, Copy)]
pub struct Tokens {
    pub panel: Color32,
    pub canvas: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub warning: Color32,
}
impl Theme {
    pub fn tokens(self) -> Tokens {
        match self {
            Self::Graphite => Tokens {
                panel: Color32::from_rgb(27, 33, 42),
                canvas: Color32::from_rgb(18, 22, 28),
                text: Color32::from_rgb(232, 238, 246),
                muted: Color32::from_rgb(168, 183, 202),
                accent: Color32::from_rgb(121, 184, 255),
                warning: Color32::from_rgb(244, 196, 107),
            },
            Self::Paper => Tokens {
                panel: Color32::from_rgb(246, 248, 251),
                canvas: Color32::from_rgb(232, 237, 243),
                text: Color32::from_rgb(23, 33, 46),
                muted: Color32::from_rgb(75, 93, 115),
                accent: Color32::from_rgb(23, 95, 181),
                warning: Color32::from_rgb(128, 80, 0),
            },
        }
    }
    pub fn apply(self, ctx: &egui::Context) {
        let tokens = self.tokens();
        let mut visuals = match self {
            Self::Graphite => egui::Visuals::dark(),
            Self::Paper => egui::Visuals::light(),
        };
        visuals.panel_fill = tokens.panel;
        visuals.window_fill = tokens.panel;
        visuals.extreme_bg_color = tokens.canvas;
        visuals.override_text_color = Some(tokens.text);
        visuals.selection.stroke.color = tokens.accent;
        visuals.hyperlink_color = tokens.accent;
        visuals.warn_fg_color = tokens.warning;
        ctx.set_visuals(visuals);
        ctx.global_style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(10.0, 6.0);
            style.spacing.interact_size.y = 28.0;
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
            style
                .text_styles
                .insert(egui::TextStyle::Heading, egui::FontId::proportional(17.0));
            style
                .text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        });
    }
}

/// Every command surface shares disabled reasons and target sizing.
pub fn action(ui: &mut Ui, label: &str, disabled: Option<&str>) -> bool {
    ui.add_enabled(disabled.is_none(), egui::Button::new(label))
        .on_disabled_hover_text(disabled.unwrap_or_default())
        .clicked()
}
pub fn property(ui: &mut Ui, label: &str, value: impl AsRef<str>, tokens: Tokens) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).small().color(tokens.muted));
        ui.add(egui::Label::new(value.as_ref()).wrap());
    });
}
pub fn notice(ui: &mut Ui, heading: &str, detail: &str, tokens: Tokens) {
    ui.heading(heading);
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(detail).color(tokens.muted)).wrap());
}
pub fn theme_choice(ui: &mut Ui, theme: &mut Theme) {
    ui.horizontal(|ui| {
        ui.selectable_value(theme, Theme::Graphite, "Graphite");
        ui.selectable_value(theme, Theme::Paper, "Paper");
    });
}

pub fn gallery(ui: &mut Ui, theme: &mut Theme) {
    theme_choice(ui, theme);
    ui.separator();
    let tokens = theme.tokens();
    ui.heading("Spiling component gallery");
    ui.label("Production project-window components · illustrative data only");
    ui.horizontal(|ui| {
        let _ = action(ui, "Open project…", None);
        let _ = action(ui, "Save", Some("This example is read-only"));
    });
    ui.separator();
    property(ui, "Project state", "Read-only committed snapshot", tokens);
    property(
        ui,
        "Long source label",
        "Housing — 接口 — finite source-bound manufacturing requirements",
        tokens,
    );
    notice(
        ui,
        "Fresh verification required",
        "Project edits invalidate the captured review. Verify the current emitted program before exporting.",
        tokens,
    );
    let id = ui.make_persistent_id("gallery-dimension");
    let mut value = ui.data_mut(|d| d.get_temp::<f64>(id).unwrap_or(0.25));
    number(ui, "Layer height (mm)", &mut value, 0.01);
    ui.data_mut(|d| d.insert_temp(id, value));
    ui.add_enabled_ui(false, |ui| {
        number(ui, "Inherited (mm)", &mut 0.4, 0.01);
    });
    ui.label("Inherited from definition · occurrence override · mixed values");
    ui.colored_label(
        tokens.warning,
        "Draft conflict — reload current values before Apply",
    );
    ui.colored_label(
        tokens.warning,
        "Save outcome uncertain — inspect the project before another action",
    );
}

/// A local draft field; project mutation occurs only on its form's explicit Apply.
pub fn number(ui: &mut Ui, label: &str, value: &mut f64, speed: f64) -> egui::Response {
    // Draft warnings can insert siblings while the user is typing. DragValue's
    // auto ID must start from this stable field scope, not its current position.
    // Labels must be unique within the caller's form scope.
    let scope = ui.make_persistent_id(("number", label));
    ui.scope_builder(egui::UiBuilder::new().scope_id(scope), |ui| {
        ui.horizontal_wrapped(|ui| {
            let label = ui.label(label);
            ui.add(egui::DragValue::new(value).speed(speed))
                .labelled_by(label.id)
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_typing_survives_a_draft_warning_inserted_above_the_field() {
        let ctx = egui::Context::default();
        let mut fraction = 0.25;
        let mut skin = 0.8;
        let mut frame = |events| {
            let mut field = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480.0, 320.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        if fraction != 0.25 {
                            ui.weak("Apply or reload the full intent draft before applying a material profile.");
                        }
                        field = Some(number(ui, "Sparse fraction", &mut fraction, 0.01).id);
                        number(ui, "Top skin (mm)", &mut skin, 0.01);
                    });
                },
            );
            output.drop_without_applying_deltas();
            field.unwrap()
        };
        let field = frame(Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(field));
        frame(Vec::new());
        frame(vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            },
            egui::Event::Text("0".into()),
        ]);
        frame(vec![egui::Event::Text(".".into())]);
        frame(vec![egui::Event::Text("3".into())]);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field));
        assert_eq!(fraction, 0.3);
        assert_eq!(
            skin, 0.8,
            "typing must not spill into the neighboring field"
        );
    }
}
