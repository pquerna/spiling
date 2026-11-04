// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Shared workspace chrome. Controls return typed actions; callers own dispatch.
use crate::commands::{Action, Workspace};
use egui::Ui;

pub fn workspace_tabs(ui: &mut Ui, workspace: Workspace) -> Option<Action> {
    let mut chosen = None;
    for (value, action) in [
        (Workspace::Prepare, Action::PrepareWorkspace),
        (Workspace::Review, Action::ReviewWorkspace),
    ] {
        if ui
            .selectable_label(workspace == value, action.label())
            .clicked()
        {
            chosen = Some(action);
        }
    }
    chosen
}
fn groups(workspace: Workspace) -> &'static [(&'static str, &'static [Action])] {
    match workspace {
        Workspace::Prepare => &[
            (
                "Parts",
                &[
                    Action::Import,
                    Action::Properties,
                    Action::Placement,
                    Action::Arrange,
                ],
            ),
            (
                "Setup",
                &[Action::Printer, Action::Material, Action::Settings],
            ),
            (
                "Construction",
                &[Action::Surface, Action::Infill, Action::Study],
            ),
        ],
        Workspace::Review => &[
            ("Output", &[Action::ReviewLayers, Action::Compare]),
            ("Inspect", &[Action::Properties, Action::Analysis]),
        ],
    }
}
fn text_width(ui: &Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.into(),
            egui::TextStyle::Button.resolve(ui.style()),
            ui.visuals().text_color(),
        )
        .size()
        .x
}
fn menu_width(ui: &Ui, title: &str) -> f32 {
    text_width(ui, title) + ui.spacing().button_padding.x * 2.0 + 24.0
}
pub fn workspace_commands<'a>(
    ui: &mut Ui,
    workspace: Workspace,
    active: Option<Action>,
    disabled: impl Fn(Action) -> Option<&'a str>,
) -> Option<Action> {
    let groups = groups(workspace);
    let mut chosen = None;
    ui.horizontal(|ui| {
        for (i, (title, actions)) in groups.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            let reserve: f32 = groups[i + 1..]
                .iter()
                .map(|(t, _)| menu_width(ui, t) + 20.0)
                .sum();
            let full = text_width(ui, title)
                + actions
                    .iter()
                    .map(|a| {
                        text_width(ui, a.label())
                            + ui.spacing().button_padding.x * 2.0
                            + ui.spacing().item_spacing.x
                    })
                    .sum::<f32>()
                + 8.0;
            if full + reserve <= ui.available_width() {
                ui.weak(*title);
                for action in *actions {
                    let reason = disabled(*action);
                    let response = ui.add_enabled(
                        reason.is_none(),
                        egui::Button::selectable(active == Some(*action), action.label()),
                    );
                    let response = if let Some(reason) = reason {
                        response.on_disabled_hover_text(reason)
                    } else {
                        response
                    };
                    if response.clicked() {
                        chosen = Some(*action);
                    }
                }
            } else {
                ui.menu_button(*title, |ui| {
                    for action in *actions {
                        if super::action(ui, action.label(), disabled(*action)) {
                            chosen = Some(*action);
                            ui.close();
                        }
                    }
                });
            }
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_commands_remain_inside_narrow_and_wide_surfaces() {
        for theme in [super::super::Theme::Graphite, super::super::Theme::Paper] {
            for width in [320.0, 760.0, 1180.0] {
                for workspace in [Workspace::Prepare, Workspace::Review] {
                    let ctx = egui::Context::default();
                    theme.apply(&ctx);
                    for _ in 0..2 {
                        let output = ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 180.0),
                                )),
                                ..Default::default()
                            },
                            |ui| {
                                ui.horizontal(|ui| {
                                    workspace_tabs(ui, workspace);
                                });
                                workspace_commands(ui, workspace, None, |_| None);
                            },
                        );
                        for shape in &output.shapes {
                            if let egui::Shape::Text(text) = &shape.shape {
                                assert!(
                                    shape.clip_rect.contains_rect(text.visual_bounding_rect()),
                                    "{theme:?} {workspace:?} at {width}: {}",
                                    text.galley.text()
                                );
                            }
                        }
                        output.drop_without_applying_deltas();
                    }
                }
            }
        }
    }
}
