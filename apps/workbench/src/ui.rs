// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod catalog_review;
mod catalog_search;
mod color;
pub use catalog_review::{CatalogReviewAction, catalog_review};
pub use catalog_search::{CatalogChoice, CatalogSearch};
mod gallery;
mod layout;
mod plates;
pub use layout::{additional_layout_previews, layout_fields};
pub use plates::{PlateEvent, PlatePartDrag, plate_navigator, plate_overview};
mod seams;
pub use gallery::gallery;
pub use seams::{seam_policy_fields, seam_review, seam_summary};
mod stock;
mod surfaces;
pub use color::{ColorDensity, ColorInspector, ColorMeaning, ColorPresentation};
pub use stock::{stock_results, stock_target};
pub use surfaces::{close, group, help, pane_header, primary, problem};

mod navigation;
pub use navigation::{workspace_commands, workspace_tabs};

use egui::{Color32, RichText, Ui};
use serde::{Deserialize, Serialize};
use spiling_contracts::manufacturing::{LayerHeightPolicy, LayerHeightReason, ManufacturingIntent};

pub fn replay_control_area(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::vertical()
        .id_salt("actual-motion-controls")
        .max_height(ui.available_height() * 0.5)
        .show(ui, contents);
}

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
    pub error: Color32,
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
                error: Color32::from_rgb(255, 138, 128),
            },
            Self::Paper => Tokens {
                panel: Color32::from_rgb(246, 248, 251),
                canvas: Color32::from_rgb(232, 237, 243),
                text: Color32::from_rgb(23, 33, 46),
                muted: Color32::from_rgb(75, 93, 115),
                accent: Color32::from_rgb(23, 95, 181),
                warning: Color32::from_rgb(128, 80, 0),
                error: Color32::from_rgb(179, 38, 30),
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
        visuals.weak_text_color = Some(tokens.muted);
        visuals.selection.stroke.color = tokens.accent;
        visuals.hyperlink_color = tokens.accent;
        visuals.warn_fg_color = tokens.warning;
        visuals.error_fg_color = tokens.error;
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
    pane_header(ui, heading, "");
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(detail).color(tokens.muted)).wrap());
}
pub fn theme_choice(ui: &mut Ui, theme: &mut Theme) {
    ui.horizontal(|ui| {
        ui.selectable_value(theme, Theme::Graphite, "Graphite");
        ui.selectable_value(theme, Theme::Paper, "Paper");
    });
}

/// Shared production/gallery draft control. No engine work or project mutation.
pub fn layer_height_goal(ui: &mut Ui, intent: &mut ManufacturingIntent, tokens: Tokens) {
    ui.label("Layer height goal");
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(
            &mut intent.recipe.layer_height_policy,
            LayerHeightPolicy::Preferred,
            "Preferred target",
        );
        ui.selectable_value(
            &mut intent.recipe.layer_height_policy,
            LayerHeightPolicy::ProtectedMaximum,
            "Protected maximum",
        );
    });
    number(
        ui,
        "Requested height (mm)",
        &mut intent.recipe.layer_height_mm,
        0.01,
    );
    match intent.select_layer_height() {
        Ok(selected) => {
            let limits = intent.effective_deposition_limits();
            ui.weak(format!(
                "Captured range {:.3}–{:.3} mm",
                limits.min_bead_height_mm,
                limits.max_bead_height_mm.min(intent.recipe.bead_width_mm)
            ));
            ui.label(format!("Draft selection: {:.3} mm", selected.height_mm));
            if selected.reason != LayerHeightReason::Requested {
                ui.colored_label(
                    tokens.warning,
                    match selected.reason {
                        LayerHeightReason::ProcessMinimum => {
                            "A thicker layer is selected to meet the captured process minimum."
                        }
                        LayerHeightReason::ProcessMaximum => {
                            "A thinner layer is selected to meet the captured process maximum."
                        }
                        LayerHeightReason::Requested => unreachable!(),
                    },
                );
            }
            help(
                ui,
                "height-help",
                "Layer height",
                "This is a draft goal. Geometry and protected features can refine the schedule. Compile and verify the program to inspect actual layer heights.",
            );
        }
        Err(error) => {
            ui.colored_label(tokens.warning, error.to_string());
        }
    }
}

/// Keep the contract fraction; use a percentage only as the local editor representation.
pub fn percent(ui: &mut Ui, label: &str, fraction: &mut f64) -> egui::Response {
    let mut value = *fraction * 100.0;
    let response = number(ui, label, &mut value, 1.0);
    if response.changed() {
        *fraction = value / 100.0;
    }
    response
}

/// Text editing uses the same stable owning-label scope as numeric fields.
pub fn text(ui: &mut Ui, label: &str, value: &mut String) -> egui::Response {
    let scope = ui.make_persistent_id(("text", label));
    ui.scope_builder(egui::UiBuilder::new().scope_id(scope), |ui| {
        if ui.available_width() < 340.0 {
            let label = ui.label(label);
            ui.add(egui::TextEdit::singleline(value).desired_width(360.0))
                .labelled_by(label.id)
        } else {
            ui.horizontal(|ui| {
                let width = (ui.available_width() * 0.32).clamp(100.0, 200.0);
                let label = ui
                    .allocate_ui_with_layout(
                        egui::vec2(width, 28.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(width);
                            ui.add(egui::Label::new(label).wrap())
                        },
                    )
                    .inner;
                ui.add(egui::TextEdit::singleline(value).desired_width(360.0))
                    .labelled_by(label.id)
            })
            .inner
        }
    })
    .inner
}

/// A local draft field; project mutation occurs only on its form's explicit Apply.
pub fn number<N: egui::emath::Numeric>(
    ui: &mut Ui,
    label: &str,
    value: &mut N,
    speed: f64,
) -> egui::Response {
    // Draft warnings can insert siblings while the user is typing. DragValue's
    // auto ID must start from this stable field scope, not its current position.
    // Labels must be unique within the caller's form scope.
    let scope = ui.make_persistent_id(("number", label));
    ui.scope_builder(egui::UiBuilder::new().scope_id(scope), |ui| {
        if ui.available_width() < 280.0 {
            let label = ui.label(label);
            ui.add(egui::DragValue::new(value).speed(speed))
                .labelled_by(label.id)
        } else {
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 98.0).clamp(80.0, 380.0);
                let label = ui
                    .allocate_ui_with_layout(
                        egui::vec2(width, 28.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(width);
                            ui.add(egui::Label::new(label).wrap())
                        },
                    )
                    .inner;
                ui.add_sized([80.0, 28.0], egui::DragValue::new(value).speed(speed))
                    .labelled_by(label.id)
            })
            .inner
        }
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_themes_keep_secondary_text_readable_on_panel_and_canvas() {
        fn luminance(color: Color32) -> f64 {
            let [r, g, b, _] = color.to_array();
            let linear = [r, g, b].map(|v| {
                let c = f64::from(v) / 255.0;
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            });
            0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
        }
        for theme in [Theme::Graphite, Theme::Paper] {
            let tokens = theme.tokens();
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            assert_eq!(
                ctx.global_style().visuals.weak_text_color,
                Some(tokens.muted)
            );
            for foreground in [tokens.text, tokens.muted, tokens.warning, tokens.error] {
                for background in [tokens.panel, tokens.canvas] {
                    let a = luminance(foreground);
                    let b = luminance(background);
                    let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                    assert!(ratio >= 4.5, "{theme:?} text contrast {ratio}");
                }
            }
        }
    }

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
                        field = Some(percent(ui, "Infill (%)", &mut fraction).id);
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
            egui::Event::Text("3".into()),
        ]);
        frame(vec![egui::Event::Text("0".into())]);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field));
        assert_eq!(fraction, 0.3);
        assert_eq!(
            skin, 0.8,
            "typing must not spill into the neighboring field"
        );
    }
}
