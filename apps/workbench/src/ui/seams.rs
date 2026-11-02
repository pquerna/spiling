// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::replay::{
    View,
    seams::{SeamStart, SectionClip},
};
use glam::DVec3;
use spiling_contracts::manufacturing::SeamPolicy;

pub fn seam_summary(policy: &SeamPolicy) -> String {
    match policy {
        SeamPolicy::Nearest => "Nearest".into(),
        SeamPolicy::PreferredBuildDirection { xy } => {
            if xy.iter().all(|value| value.is_finite()) && xy.iter().any(|value| *value != 0.0) {
                format!(
                    "Preferred direction · {:.1}° in build XY",
                    xy[1].atan2(xy[0]).to_degrees()
                )
            } else {
                "Preferred direction · enter a direction".into()
            }
        }
        SeamPolicy::Distributed => "Spread starts".into(),
    }
}

pub fn seam_policy_fields(ui: &mut Ui, policy: &mut SeamPolicy) {
    ui.label("Entire print job · closed constructed walls");
    let mut choice = match policy {
        SeamPolicy::Nearest => 0,
        SeamPolicy::PreferredBuildDirection { .. } => 1,
        SeamPolicy::Distributed => 2,
    };
    let original = choice;
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(&mut choice, 0, "Nearest");
        ui.radio_value(&mut choice, 1, "Preferred direction");
        ui.radio_value(&mut choice, 2, "Spread starts");
    });
    if choice != original {
        *policy = match choice {
            1 => SeamPolicy::PreferredBuildDirection { xy: [1.0, 0.0] },
            2 => SeamPolicy::Distributed,
            _ => SeamPolicy::Nearest,
        };
    }
    if let SeamPolicy::PreferredBuildDirection { xy } = policy {
        ui.horizontal_wrapped(|ui| {
            ui.label("Build XY presets");
            for (label, direction) in [
                ("+X", [1.0, 0.0]),
                ("−X", [-1.0, 0.0]),
                ("+Y", [0.0, 1.0]),
                ("−Y", [0.0, -1.0]),
            ] {
                if ui.button(label).clicked() {
                    *xy = direction;
                }
            }
        });
        number(ui, "Build X direction", &mut xy[0], 0.1);
        number(ui, "Build Y direction", &mut xy[1], 0.1);
        ui.weak("Apply settings normalizes a finite nonzero direction.");
    }
    ui.weak(match policy {
        SeamPolicy::Nearest => "Starts near the previous command position.",
        SeamPolicy::PreferredBuildDirection { .. } => {
            "Prefers existing vertices toward the chosen build XY direction."
        }
        SeamPolicy::Distributed => {
            "Spreads starts against up to eight recent closed wall starts on the same tool."
        }
    });
    help(
        ui,
        "seam-policy-help",
        "Wall starts",
        "The policy applies to the entire print job, not a selected face or part. It selects existing closed constructed-wall vertices. Spread starts maximizes the minimum command-XY distance from the same tool’s most recent eight closed wall starts in actual layer order. Empty history uses Nearest; ties are deterministic. It guarantees neither separation, seam identity nor sealing. Hatch, support and bridge orientation remain separate controls. Apply settings commits the complete intent draft.",
    );
}

pub fn seam_review(
    ui: &mut Ui,
    starts: &[SeamStart],
    selected: &mut Option<u32>,
    view: &mut View,
    clip: SectionClip,
    origin: DVec3,
    policy: &SeamPolicy,
) {
    ui.checkbox(&mut view.seams, "Seam starts");
    ui.label(format!("Captured policy · {}", seam_summary(policy)));
    let in_layers = starts.iter().filter(|start| start.in_layers(*view)).count();
    ui.label(format!("{in_layers} closed wall starts in selected layers"));
    if !view.seams {
        return;
    }
    let visible = || {
        starts
            .iter()
            .filter(|start| start.shown(*view, clip, origin))
    };
    let count = visible().count();
    ui.weak(format!(
        "{count} shown · role, tool, playback and section filters"
    ));
    let current = selected
        .and_then(|event| visible().position(|start| start.first_event == event))
        .unwrap_or(0);
    let mut index = current;
    navigation(ui, &mut index, count);
    let Some(start) = visible().nth(index) else {
        *selected = None;
        ui.weak("No closed wall starts in this view. Perimeter or Wall ownership alone does not establish closure.");
        return;
    };
    *selected = Some(start.first_event);
    ui.label(format!(
        "Start {} of {} · layer {} · tool {} · {}",
        index + 1,
        count,
        u32::from(start.layer) + 1,
        start.tool,
        crate::replay::ROLES[start.role as usize]
    ));
    ui.collapsing("Start coordinates and commands", |ui| {
        ui.add(
            egui::Label::new(format!(
                "Actual XYZ {}, {}, {} mm",
                start.position[0], start.position[1], start.position[2]
            ))
            .wrap()
            .selectable(true),
        );
        ui.add(
            egui::Label::new(format!(
                "Physical Z {}…{} mm",
                start.physical_z[0], start.physical_z[1]
            ))
            .wrap()
            .selectable(true),
        );
        ui.add(
            egui::Label::new(format!(
                "Program line {} · event {} · closes at line {} / event {}",
                start.command_line, start.first_event, start.closing_line, start.closing_event
            ))
            .wrap()
            .selectable(true),
        );
    });
    help(
        ui,
        "seam-review-help",
        "Decoded wall starts",
        "Read-only markers come only from fresh validated actual-program replay: a path-entry travel, optionally followed by stationary unretraction at the same layer/tool/position, then a contiguous Wall-associated deposited run that closes at exactly the same command XYZ. Variable widths and Wall-owned Bridge strokes can belong to that run. Travel, pressure within a run, tool epochs and association changes break it. A marker appears when playback reaches its first deposit; closure is established from the complete decoded run. Event ordinals are zero-based, program lines are one-based. Dim context layers never contribute markers or counts. Replay does not identify source faces, occurrences, wall depth or adjacency. These are not physical seam or sealing measurements.",
    );
}

fn navigation(ui: &mut Ui, index: &mut usize, count: usize) -> [egui::Response; 3] {
    ui.push_id("seam-navigation", |ui| {
        ui.horizontal_wrapped(|ui| {
            let first = ui.add_enabled(count > 0 && *index > 0, egui::Button::new("First start"));
            let previous =
                ui.add_enabled(count > 0 && *index > 0, egui::Button::new("Previous start"));
            let next = ui.add_enabled(*index + 1 < count, egui::Button::new("Next start"));
            if first.clicked() {
                *index = 0;
            }
            if previous.clicked() {
                *index -= 1;
            }
            if next.clicked() {
                *index += 1;
            }
            [first, previous, next]
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_previous_and_next_accept_keyboard_activation() {
        let ctx = egui::Context::default();
        let mut index = 0;
        let mut ids = [egui::Id::NULL; 3];
        let mut frame = |events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    ids = navigation(ui, &mut index, 4).map(|response| response.id);
                },
            )
            .drop_without_applying_deltas();
            (index, ids)
        };
        let key = |key, pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let (_, ids) = frame(vec![]);
        ctx.memory_mut(|memory| memory.request_focus(ids[2]));
        assert_eq!(frame(vec![key(egui::Key::Enter, true)]).0, 1);
        frame(vec![key(egui::Key::Enter, false)]);
        assert_eq!(frame(vec![key(egui::Key::Enter, true)]).0, 2);
        frame(vec![key(egui::Key::Enter, false)]);
        ctx.memory_mut(|memory| memory.request_focus(ids[1]));
        assert_eq!(frame(vec![key(egui::Key::Enter, true)]).0, 1);
        frame(vec![key(egui::Key::Enter, false)]);
        ctx.memory_mut(|memory| memory.request_focus(ids[0]));
        assert_eq!(frame(vec![key(egui::Key::Enter, true)]).0, 0);
    }

    #[test]
    fn review_selection_is_read_only_and_refilters_without_context() {
        let ctx = egui::Context::default();
        let starts = [0, 1].map(|tool| SeamStart {
            position: [f64::from(tool), 0.0, 0.2],
            physical_z: [0.0, 0.2],
            layer: tool as u16,
            tool,
            role: 1,
            entry_event: tool * 4,
            first_event: tool * 4 + 1,
            closing_event: tool * 4 + 3,
            command_line: tool * 5 + 10,
            closing_line: tool * 5 + 12,
        });
        let original = starts.clone();
        let mut selected = Some(5);
        let mut view = View {
            seams: true,
            layer: Some(0),
            ..Default::default()
        };
        let mut frame = |view: &mut View| {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                seam_review(
                    ui,
                    &starts,
                    &mut selected,
                    view,
                    SectionClip::default(),
                    DVec3::ZERO,
                    &SeamPolicy::Distributed,
                )
            })
            .drop_without_applying_deltas();
            selected
        };
        assert_eq!(frame(&mut view), Some(1));
        view.tool = Some(1);
        assert_eq!(frame(&mut view), None);
        view.layer = None;
        assert_eq!(frame(&mut view), Some(5));
        assert_eq!(starts, original);
    }

    #[test]
    fn expanded_review_controls_preserve_half_the_canvas_in_short_scaled_windows() {
        for theme in [Theme::Graphite, Theme::Paper] {
            for scale in [1.0, 1.5, 2.0] {
                let ctx = egui::Context::default();
                theme.apply(&ctx);
                ctx.set_zoom_factor(scale);
                for _ in 0..2 {
                    ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(320.0 * scale, 360.0 * scale),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let height = ui.available_height();
                            let right = ui.max_rect().right();
                            replay_control_area(ui, |ui| {
                                seam_review(
                                    ui,
                                    &[],
                                    &mut None,
                                    &mut View {
                                        seams: true,
                                        ..Default::default()
                                    },
                                    SectionClip::default(),
                                    DVec3::ZERO,
                                    &SeamPolicy::Distributed,
                                );
                                for _ in 0..30 {
                                    ui.label("Expanded command playback and inspection");
                                }
                            });
                            assert!(
                                ui.available_height()
                                    >= height * 0.5 - ui.spacing().item_spacing.y - 1.0,
                                "remaining {} / original {height}",
                                ui.available_height()
                            );
                            assert!(ui.min_rect().right() <= right + 1.0);
                        },
                    )
                    .drop_without_applying_deltas();
                }
            }
        }
    }
}
