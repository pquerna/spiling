// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

fn frame(
    context: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    mut body: impl FnMut(&mut egui::Ui),
) {
    let mut output = context.run_ui(
        egui::RawInput {
            time: Some(time),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ui| body(ui),
    );
    output.textures_delta.clear();
}
fn key(key: egui::Key, pressed: bool, repeat: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat,
        modifiers: Default::default(),
    }
}
fn button(
    position: egui::Pos2,
    button: egui::PointerButton,
    pressed: bool,
    modifiers: egui::Modifiers,
) -> egui::Event {
    egui::Event::PointerButton {
        pos: position,
        button,
        pressed,
        modifiers,
    }
}
fn rect() -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(50.0, 50.0), egui::vec2(600.0, 500.0))
}

#[test]
fn exact_named_snaps_and_cube_directions_have_identical_finite_cameras() {
    for view in StandardView::ALL {
        let mut keyboard = Viewport::default();
        keyboard.navigation_settings.animate = false;
        keyboard.navigate(ViewAction::Snap(view), 0.0, false);
        let direction = view.direction();
        let axes = direction.to_array().map(|value| {
            if value.abs() < 1e-10 {
                0
            } else if value > 0.0 {
                1
            } else {
                -1
            }
        });
        let mut cube = Viewport::default();
        cube.navigate(ViewAction::Direction(axes), 0.0, false);
        assert_eq!(keyboard.camera(), cube.camera());
        assert!((keyboard.rotation * DVec3::Z - direction).length() < 1e-12);
        assert!(keyboard.orthographic);
        for aspect in [0.2, 1.0, 4.0] {
            assert!(keyboard.matrix(aspect, 10.0).is_finite());
        }
    }
}
#[test]
fn orbit_crosses_both_poles_and_roll_and_projection_do_not_move_target() {
    let mut viewport = Viewport {
        pan: DVec3::new(1e6, -3.0, 2.0),
        ..Default::default()
    };
    viewport.navigate(ViewAction::Snap(StandardView::Top), 0.0, false);
    let target = viewport.pan;
    for _ in 0..3600 {
        viewport.orbit(egui::vec2(0.0, 0.01));
        assert!(viewport.camera().valid());
    }
    assert_eq!(viewport.pan, target);
    viewport.navigate(ViewAction::RollRight, 0.0, false);
    assert_eq!(viewport.pan, target);
    let distance = viewport.distance;
    viewport.navigate(ViewAction::Projection, 0.0, false);
    assert_eq!(viewport.distance, distance);
    assert_eq!(viewport.pan, target);
}
#[test]
fn normal_previous_and_reduced_motion_use_the_same_camera_path() {
    let mut viewport = Viewport::default();
    let initial = viewport.camera();
    assert!(viewport.view_disabled(ViewAction::Normal).is_some());
    viewport.normal = Some(DVec3::X);
    viewport.navigate(ViewAction::Normal, 1.0, false);
    assert!((viewport.rotation * DVec3::Z - DVec3::X).length() < 1e-12);
    viewport.navigate(ViewAction::Previous, 2.0, false);
    assert_eq!(viewport.camera(), initial);
    viewport.navigation_settings.animate = false;
    viewport.navigate(ViewAction::Snap(StandardView::Bottom), 3.0, true);
    assert!(viewport.motion.is_none());
    assert!((viewport.rotation * DVec3::Z + DVec3::Z).length() < 1e-12);
}
#[test]
fn keys_snap_once_and_preserve_text_slider_and_modifier_ownership() {
    let context = egui::Context::default();
    frame(
        &context,
        0.0,
        vec![key(egui::Key::Num1, true, false)],
        |ui| assert_eq!(keys(ui).0, Some(ViewAction::Snap(StandardView::Top))),
    );
    frame(
        &context,
        0.1,
        vec![key(egui::Key::Num1, true, true)],
        |ui| assert!(keys(ui).0.is_none()),
    );
    frame(
        &context,
        0.2,
        vec![key(egui::Key::Num1, false, false)],
        |_| {},
    );
    let mut input = Input::default();
    frame(
        &context,
        0.3,
        vec![key(egui::Key::ArrowUp, true, false)],
        |ui| {
            assert!(!input.keyboard(ui, false).used);
            assert!(ui.input(|input| input.key_pressed(egui::Key::ArrowUp)));
        },
    );
    frame(&context, 0.4, vec![], |ui| {
        assert!(!input.keyboard(ui, true).used)
    });
    frame(
        &context,
        0.5,
        vec![key(egui::Key::ArrowUp, false, false)],
        |ui| {
            input.keyboard(ui, true);
        },
    );
    frame(
        &context,
        0.6,
        vec![key(egui::Key::ArrowUp, true, false)],
        |ui| assert!(input.keyboard(ui, true).used),
    );
}
#[test]
fn hold_is_continuous_not_os_repeat_and_cancel_requires_release() {
    let context = egui::Context::default();
    let mut input = Input::default();
    frame(
        &context,
        0.0,
        vec![key(egui::Key::ArrowRight, true, false)],
        |ui| {
            assert!(
                (f64::from(input.keyboard(ui, true).orbit.x) - 15_f64.to_radians()).abs() < 1e-6
            )
        },
    );
    frame(
        &context,
        0.1,
        vec![key(egui::Key::ArrowRight, true, true)],
        |ui| assert_eq!(input.keyboard(ui, true).orbit, egui::Vec2::ZERO),
    );
    frame(&context, 0.4, vec![], |ui| {
        assert!(input.keyboard(ui, true).orbit.x > 0.0)
    });
    assert!(input.cancel_navigation());
    frame(&context, 0.5, vec![], |ui| {
        assert!(!input.keyboard(ui, true).used)
    });
    frame(
        &context,
        0.6,
        vec![key(egui::Key::ArrowRight, false, false)],
        |ui| assert!(!input.keyboard(ui, true).used),
    );
    frame(
        &context,
        0.7,
        vec![key(egui::Key::ArrowRight, true, false)],
        |ui| assert!(input.keyboard(ui, true).used),
    );
}
#[test]
fn tool_owns_primary_but_middle_navigation_never_becomes_placement() {
    let context = egui::Context::default();
    let mut input = Input::default();
    let start = egui::pos2(100.0, 100.0);
    frame(
        &context,
        0.0,
        vec![button(
            start,
            egui::PointerButton::Primary,
            true,
            Default::default(),
        )],
        |ui| {
            input.begin(ui, rect(), None);
            assert!(!input.reserves_pointer());
            assert!(input.pointer(ui, true).used);
            assert!(input.owns_tool());
        },
    );
    frame(
        &context,
        0.1,
        vec![button(
            start,
            egui::PointerButton::Primary,
            false,
            Default::default(),
        )],
        |ui| {
            assert!(input.pointer(ui, true).used);
        },
    );
    frame(
        &context,
        0.2,
        vec![button(
            start,
            egui::PointerButton::Middle,
            true,
            Default::default(),
        )],
        |ui| {
            input.begin(ui, rect(), None);
            assert!(input.reserves_pointer());
            assert!(input.pointer(ui, true).used);
        },
    );
    frame(
        &context,
        0.3,
        vec![egui::Event::PointerMoved(start + egui::vec2(70.0, 0.0))],
        |ui| {
            let motion = input.pointer(ui, true);
            assert!(motion.orbit.x > 0.0);
            assert_eq!(motion.pan, egui::Vec2::ZERO);
        },
    );
    frame(
        &context,
        0.4,
        vec![button(
            egui::pos2(900.0, 700.0),
            egui::PointerButton::Middle,
            false,
            Default::default(),
        )],
        |ui| {
            assert!(input.pointer(ui, false).used);
        },
    );
    frame(&context, 0.5, vec![], |ui| {
        assert!(!input.pointer(ui, false).used);
    });
}
#[test]
fn pointer_down_owner_survives_modifiers_and_cancel_suppresses_release() {
    let context = egui::Context::default();
    let mut input = Input::default();
    let start = egui::pos2(100.0, 100.0);
    frame(
        &context,
        0.0,
        vec![button(
            start,
            egui::PointerButton::Secondary,
            true,
            Default::default(),
        )],
        |ui| {
            input.begin(ui, rect(), None);
            input.pointer(ui, false);
        },
    );
    frame(
        &context,
        0.1,
        vec![egui::Event::PointerMoved(start + egui::vec2(70.0, 0.0))],
        |ui| {
            let motion = input.pointer(ui, false);
            assert_eq!(motion.orbit, egui::Vec2::ZERO);
            assert!(motion.pan.x > 0.0);
        },
    );
    assert!(input.cancel_navigation());
    frame(
        &context,
        0.2,
        vec![button(
            start,
            egui::PointerButton::Secondary,
            false,
            Default::default(),
        )],
        |ui| {
            input.begin(ui, rect(), None);
            assert!(input.pointer(ui, false).used);
        },
    );
}
#[test]
fn outside_press_cannot_start_a_camera_drag_and_click_does_not_move_camera() {
    let context = egui::Context::default();
    let mut input = Input::default();
    frame(
        &context,
        0.0,
        vec![button(
            egui::pos2(10.0, 10.0),
            egui::PointerButton::Primary,
            true,
            Default::default(),
        )],
        |ui| {
            input.begin(ui, rect(), None);
            assert!(!input.pointer(ui, false).used);
        },
    );
    frame(
        &context,
        0.1,
        vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0))],
        |ui| {
            input.begin(ui, rect(), None);
            assert!(!input.pointer(ui, false).used);
        },
    );
    frame(
        &context,
        0.2,
        vec![button(
            egui::pos2(200.0, 200.0),
            egui::PointerButton::Primary,
            false,
            Default::default(),
        )],
        |_| {},
    );
    frame(
        &context,
        0.3,
        vec![button(
            egui::pos2(200.0, 200.0),
            egui::PointerButton::Primary,
            true,
            Default::default(),
        )],
        |ui| {
            input.begin(ui, rect(), None);
            let motion = input.pointer(ui, false);
            assert!(!motion.used);
            assert_eq!(motion.orbit, egui::Vec2::ZERO);
        },
    );
}
#[test]
fn mouse_trackpad_and_scroll_ownership_are_explicit() {
    let context = egui::Context::default();
    frame(&context, 0.0, vec![], |ui| {
        ui.input_mut(|input| input.smooth_scroll_delta = egui::vec2(10.0, 30.0));
        assert!(!scroll(ui, Settings::default(), false).used);
        assert_eq!(ui.input(|input| input.smooth_scroll_delta.y), 30.0);
        assert!(scroll(ui, Settings::default(), true).zoom < 0.0);
        assert_eq!(
            ui.input(|input| input.smooth_scroll_delta),
            egui::Vec2::ZERO
        );
        ui.input_mut(|input| input.smooth_scroll_delta = egui::vec2(10.0, 30.0));
        let motion = scroll(
            ui,
            Settings {
                device: InputDevice::Trackpad,
                ..Default::default()
            },
            true,
        );
        assert_eq!(motion.pan, egui::vec2(-10.0, -30.0));
        assert_eq!(motion.zoom, 0.0);
    });
}
#[test]
fn camera_preferences_refuse_invalid_orientation_and_roundtrip_navigation_settings() {
    let camera = Viewport::default().camera();
    assert_eq!(
        serde_json::from_str::<CameraState>(&serde_json::to_string(&camera).unwrap()).unwrap(),
        camera
    );
    let mut broken = camera.clone();
    broken.rotation = [0.0; 4];
    assert!(!broken.valid());
    broken.rotation = [f64::NAN; 4];
    assert!(!broken.valid());
    let settings = Settings {
        device: InputDevice::Trackpad,
        reverse_zoom: true,
        animate: false,
    };
    assert_eq!(
        serde_json::from_str::<Settings>(&serde_json::to_string(&settings).unwrap()).unwrap(),
        settings
    );
}

#[test]
fn navigation_chord_cannot_place_after_its_first_button_is_released() {
    let context = egui::Context::default();
    let mut input = Input::default();
    let position = egui::pos2(100.0, 100.0);
    for (time, button, pressed) in [
        (0.0, egui::PointerButton::Middle, true),
        (0.1, egui::PointerButton::Primary, true),
        (0.2, egui::PointerButton::Middle, false),
        (0.3, egui::PointerButton::Primary, false),
    ] {
        frame(
            &context,
            time,
            vec![self::button(position, button, pressed, Default::default())],
            |ui| {
                input.begin(ui, rect(), None);
                assert!(input.pointer(ui, false).used);
            },
        );
    }
    frame(&context, 0.4, vec![], |ui| {
        assert!(!input.pointer(ui, false).used)
    });
}

#[test]
fn both_button_orders_suppress_actual_primary_click_dispatch() {
    for first in [egui::PointerButton::Primary, egui::PointerButton::Middle] {
        let second = if first == egui::PointerButton::Primary {
            egui::PointerButton::Middle
        } else {
            egui::PointerButton::Primary
        };
        let context = egui::Context::default();
        let mut input = Input::default();
        let mut placements = 0;
        let position = egui::pos2(100.0, 100.0);
        for (time, button, pressed) in [
            (0.0, first, true),
            (0.1, second, true),
            (0.2, first, false),
            (0.3, second, false),
        ] {
            frame(
                &context,
                time,
                vec![self::button(position, button, pressed, Default::default())],
                |ui| {
                    input.begin(ui, rect(), None);
                    let motion = input.pointer(ui, false);
                    if ui.input(|input| input.pointer.primary_clicked()) && !motion.used {
                        placements += 1;
                    }
                },
            );
        }
        assert_eq!(placements, 0);
    }
}

#[test]
fn floating_window_and_modal_layers_cannot_start_camera_drags() {
    for modal in [false, true] {
        let context = egui::Context::default();
        let mut input = Input::default();
        for (time, events) in [
            (0.0, vec![]),
            (
                0.1,
                vec![button(
                    egui::pos2(140.0, 140.0),
                    egui::PointerButton::Primary,
                    true,
                    Default::default(),
                )],
            ),
        ] {
            frame(&context, time, events, |ui| {
                if modal {
                    egui::Modal::new(egui::Id::unique("blocking-plate-dialog")).show(
                        ui.ctx(),
                        |ui| {
                            ui.label("Plate");
                        },
                    );
                } else {
                    egui::Window::new("Compute")
                        .fixed_pos(egui::pos2(90.0, 90.0))
                        .fixed_size(egui::vec2(220.0, 180.0))
                        .show(ui.ctx(), |ui| {
                            ui.label("Compute");
                        });
                }
                input.begin(ui, rect(), None);
                assert!(!input.reserves_pointer());
                assert_eq!(input.pointer(ui, false).orbit, egui::Vec2::ZERO);
            });
        }
    }
}

#[test]
fn read_only_camera_animation_survives_reconciliation_without_source_picking() {
    let mut viewport = Viewport::default();
    viewport.reconcile_inspection(None, false, (None, None), 0.0, true);
    viewport.navigate(ViewAction::Snap(StandardView::Bottom), 0.0, true);
    viewport.advance_focus(0.05);
    viewport.reconcile_inspection(None, false, (None, None), 0.1, true);
    assert!(viewport.motion.is_some());
    viewport.advance_focus(1.0);
    assert!((viewport.rotation * DVec3::Z + DVec3::Z).length() < 1e-12);
}

#[test]
fn section_look_normal_shares_snap_history_and_animation_policy() {
    let mut button = Viewport::default();
    let mut keyboard = Viewport {
        normal: Some(DVec3::Z),
        ..Default::default()
    };
    button.normal_to(DVec3::Z, 0.0, false);
    keyboard.navigate(ViewAction::Normal, 0.0, false);
    assert_eq!(button.camera(), keyboard.camera());
    assert_eq!(button.previous_camera, keyboard.previous_camera);
    button.normal_to(-DVec3::Z, 1.0, true);
    assert!(button.motion.is_some());
    button.advance_focus(2.0);
    assert!((button.rotation * DVec3::Z + DVec3::Z).length() < 1e-12);
}

#[test]
fn every_cube_edge_and_corner_has_a_named_keyboard_search_action() {
    for label in ["Top", "Bottom", "Front", "Back", "Left", "Right"] {
        assert_eq!(
            crate::commands::Action::ALL
                .iter()
                .find(|action| action
                    .label()
                    .to_lowercase()
                    .contains(&label.to_lowercase()))
                .unwrap()
                .label(),
            label
        );
    }
    let mut labels = std::collections::HashSet::new();
    for action in ViewAction::DIAGONALS {
        assert!(labels.insert(action.label()));
        assert_ne!(action.label(), "Diagonal view");
        assert!(crate::commands::Action::ALL.contains(&crate::commands::Action::View(action)));
        let ViewAction::Direction(axes) = action else {
            unreachable!()
        };
        let expected =
            DVec3::new(f64::from(axes[0]), f64::from(axes[1]), f64::from(axes[2])).normalize();
        let mut viewport = Viewport::default();
        viewport.navigate(action, 0.0, false);
        assert!((viewport.rotation * DVec3::Z - expected).length() < 1e-12);
        assert!(viewport.camera().valid());
    }
}
