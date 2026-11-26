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
pub use surfaces::{close, destructive, group, help, pane_header, primary, problem, verified};

mod routing;
pub use routing::{routed_material_fields, tool_reference};

mod modifiers;
pub use modifiers::{ModifierAction, modifier_delta, modifier_row, resolved_value};

mod navigation;
pub use navigation::{workspace_commands, workspace_tabs};

use egui::{RichText, Ui};
use spiling_contracts::manufacturing::{LayerHeightPolicy, LayerHeightReason, ManufacturingIntent};

pub fn replay_control_area(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::vertical()
        .id_salt("actual-motion-controls")
        .max_height(ui.available_height() * 0.5)
        .show(ui, contents);
}

mod theme;
pub(crate) use theme::rgb;
pub use theme::{Theme, Tokens};

/// Every command surface shares disabled reasons and target sizing.
pub fn action(ui: &mut Ui, label: &str, disabled: Option<&str>) -> bool {
    if ui.stack().contained_in(egui::UiKind::Menu) {
        ui.add_enabled(disabled.is_none(), egui::Button::new(label).frame(false))
            .on_disabled_hover_text(disabled.unwrap_or_default())
            .clicked()
    } else {
        surfaces::secondary(ui, label, disabled).clicked()
    }
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
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(theme, Theme::System, "System");
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

    fn luminance(color: egui::Color32) -> f64 {
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

    #[test]
    fn theme_pairs_keep_primary_labels_roles_and_focus_readable() {
        let ratio = |a, b| {
            let (a, b) = (luminance(a), luminance(b));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        for theme in [Theme::Graphite, Theme::Paper] {
            let t = theme.tokens();
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            assert_eq!(ctx.global_style().visuals.weak_text_color, Some(t.muted));
            for background in [t.panel, t.canvas, t.control, t.hover, t.pressed, t.selected] {
                for foreground in [t.text, t.muted, t.warning, t.error, t.success, t.accent] {
                    assert!(ratio(foreground, background) >= 4.5, "{theme:?} text");
                }
                for edge in [t.boundary, t.focus, t.accent] {
                    assert!(ratio(edge, background) >= 3.0, "{theme:?} edge");
                }
            }
            assert!(ratio(t.focus, t.focus_gap) >= 3.0);
            assert!(ratio(t.boundary, t.bed) >= 3.0);
            for fill in theme::ACTION {
                assert!(ratio(egui::Color32::WHITE, fill) >= 7.0);
            }
            for role in crate::replay::ROLES {
                for background in [t.panel, t.canvas, t.bed] {
                    assert!(
                        ratio(role.color(theme == Theme::Graphite), background) >= 3.0,
                        "{theme:?} {}",
                        role.label
                    );
                }
            }
        }
    }

    #[test]
    fn system_uses_both_installed_palettes_and_preserves_explicit_paper() {
        let ctx = egui::Context::default();
        Theme::System.apply(&ctx);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        let set_system = |theme| {
            ctx.run_ui(
                egui::RawInput {
                    system_theme: Some(theme),
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        };
        set_system(egui::Theme::Light);
        assert_eq!(
            ctx.global_style().visuals.panel_fill,
            Theme::Paper.tokens().panel
        );
        Theme::Paper.apply(&ctx);
        set_system(egui::Theme::Dark);
        assert_eq!(
            ctx.global_style().visuals.panel_fill,
            Theme::Paper.tokens().panel
        );
        Theme::System.apply(&ctx);
        assert_eq!(
            ctx.global_style().visuals.panel_fill,
            Theme::Graphite.tokens().panel
        );
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
