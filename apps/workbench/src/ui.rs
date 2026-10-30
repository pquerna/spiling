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
        "No 3D display in this increment",
        "Native source rendering and source-face picking are the next workbench increment. Project and engine information remain inspectable.",
        tokens,
    );
    ui.colored_label(
        tokens.warning,
        "Save outcome uncertain — inspect the project before another action",
    );
}

/// A local draft field; project mutation occurs only on its form's explicit Apply.
pub fn number(ui: &mut Ui, label: &str, value: &mut f64, speed: f64) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(speed));
    });
}
