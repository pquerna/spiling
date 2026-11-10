// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardView {
    Isometric,
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
}
impl StandardView {
    pub const ALL: [Self; 7] = [
        Self::Isometric,
        Self::Top,
        Self::Bottom,
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Isometric => "Isometric",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Front => "Front",
            Self::Back => "Back",
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
    pub(super) fn direction(self) -> DVec3 {
        match self {
            Self::Isometric => DVec3::new(1.0, -1.0, 1.0).normalize(),
            Self::Top => DVec3::Z,
            Self::Bottom => -DVec3::Z,
            Self::Front => -DVec3::Y,
            Self::Back => DVec3::Y,
            Self::Left => -DVec3::X,
            Self::Right => DVec3::X,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewAction {
    Snap(StandardView),
    Direction([i8; 3]),
    Fit,
    FitParts,
    FitPlate,
    Projection,
    Normal,
    OppositeNormal,
    RollLeft,
    RollRight,
    Previous,
}
impl ViewAction {
    pub const ALL: [Self; 16] = [
        Self::Snap(StandardView::Isometric),
        Self::Snap(StandardView::Top),
        Self::Snap(StandardView::Bottom),
        Self::Snap(StandardView::Front),
        Self::Snap(StandardView::Back),
        Self::Snap(StandardView::Left),
        Self::Snap(StandardView::Right),
        Self::Fit,
        Self::FitParts,
        Self::FitPlate,
        Self::Projection,
        Self::Normal,
        Self::OppositeNormal,
        Self::RollLeft,
        Self::RollRight,
        Self::Previous,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Snap(view) => view.label(),
            Self::Direction(_) => "Diagonal view",
            Self::Fit => "Fit view",
            Self::FitParts => "Fit parts",
            Self::FitPlate => "Fit plate",
            Self::Projection => "Perspective / orthographic",
            Self::Normal => "Normal to section or selection",
            Self::OppositeNormal => "Opposite normal view",
            Self::RollLeft => "Roll view left 90°",
            Self::RollRight => "Roll view right 90°",
            Self::Previous => "Back to previous view",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputDevice {
    #[default]
    Mouse,
    Trackpad,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub device: InputDevice,
    pub reverse_zoom: bool,
    pub animate: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            device: InputDevice::Mouse,
            reverse_zoom: false,
            animate: true,
        }
    }
}

pub(super) fn orientation(direction: DVec3) -> DQuat {
    let direction = direction.normalize();
    let up = if direction.z.abs() > 1.0 - 1e-10 {
        DVec3::Y
    } else {
        DVec3::Z
    };
    let right = up.cross(direction).normalize();
    DQuat::from_mat3(&glam::DMat3::from_cols(
        right,
        direction.cross(right),
        direction,
    ))
    .normalize()
}

#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Pending,
    Tool,
    Orbit,
    Pan,
    Cube,
}
struct Drag {
    owner: Owner,
    button: egui::PointerButton,
    last: egui::Pos2,
    moved: bool,
    exclusive: bool,
}
#[derive(Default)]
pub(super) struct Input {
    drag: Option<Drag>,
    held: [Option<f64>; 6],
    suppressed: bool,
    keys_suppressed: bool,
}
#[derive(Default)]
pub(super) struct Motion {
    pub orbit: egui::Vec2,
    pub pan: egui::Vec2,
    pub zoom: f64,
    pub used: bool,
}
impl Input {
    pub fn cancel(&mut self) -> bool {
        self.suppressed |= self.drag.is_some();
        self.keys_suppressed |= self.held.iter().any(Option::is_some);
        let active = self.drag.take().is_some() || self.held.iter().any(Option::is_some);
        self.held = [None; 6];
        active
    }
    pub fn begin(&mut self, ui: &egui::Ui, rect: egui::Rect, cube: Option<egui::Rect>) {
        let enabled = ui.is_enabled()
            && !egui::Popup::is_any_open(ui.ctx())
            && ui
                .ctx()
                .memory(|memory| memory.is_above_modal_layer(ui.layer_id()));
        let origin = ui.input(|input| input.pointer.press_origin());
        let layer_allowed = origin.is_some_and(|origin| {
            ui.ctx().layer_id_at(origin).is_none_or(|layer| {
                layer == ui.layer_id()
                    || layer == cube::layer(ui)
                    || layer
                        == egui::LayerId::new(
                            ui.layer_id().order,
                            egui::Id::unique("force-intent-overlay"),
                        )
            })
        });
        ui.input(|input| {
            if !input.focused || !enabled {
                self.cancel();
                return;
            }
            if self.drag.is_some() || self.suppressed {
                return;
            }
            if !layer_allowed {
                return;
            }
            let Some(origin) = input
                .pointer
                .press_origin()
                .filter(|origin| rect.contains(*origin))
            else {
                return;
            };
            let Some(button) = [
                egui::PointerButton::Primary,
                egui::PointerButton::Middle,
                egui::PointerButton::Secondary,
            ]
            .into_iter()
            .find(|button| input.pointer.button_pressed(*button)) else {
                return;
            };
            let owner = if cube.is_some_and(|cube| cube.contains(origin)) {
                Owner::Cube
            } else if button == egui::PointerButton::Secondary
                || (button == egui::PointerButton::Middle && input.modifiers.shift)
                || (button == egui::PointerButton::Primary
                    && input.modifiers.alt
                    && input.modifiers.shift)
            {
                Owner::Pan
            } else if button == egui::PointerButton::Middle || input.modifiers.alt {
                Owner::Orbit
            } else {
                Owner::Pending
            };
            let exclusive = owner != Owner::Pending;
            self.drag = Some(Drag {
                owner,
                button,
                last: origin,
                moved: false,
                exclusive,
            });
        });
    }
    pub fn reserves_pointer(&self) -> bool {
        self.suppressed
            || self
                .drag
                .as_ref()
                .is_some_and(|drag| matches!(drag.owner, Owner::Orbit | Owner::Pan | Owner::Cube))
    }
    pub fn cancel_navigation(&mut self) -> bool {
        if self.reserves_pointer() || self.held.iter().any(Option::is_some) {
            self.cancel()
        } else {
            false
        }
    }
    pub fn owns_tool(&self) -> bool {
        self.drag
            .as_ref()
            .is_some_and(|drag| drag.owner == Owner::Tool)
    }
    pub fn suppressed(&self) -> bool {
        self.suppressed
    }
    pub fn pointer(&mut self, ui: &egui::Ui, blocked: bool) -> Motion {
        let mut motion = Motion::default();
        if self.suppressed {
            motion.used = true;
            if !ui.input(|input| input.pointer.any_down()) {
                self.suppressed = false;
            }
            return motion;
        }
        let Some(drag) = &mut self.drag else {
            return motion;
        };
        if drag.owner == Owner::Pending {
            drag.owner = if blocked { Owner::Tool } else { Owner::Orbit };
        }
        let released = ui.input(|input| {
            if [
                egui::PointerButton::Primary,
                egui::PointerButton::Middle,
                egui::PointerButton::Secondary,
            ]
            .into_iter()
            .any(|button| button != drag.button && input.pointer.button_pressed(button))
            {
                drag.exclusive = true;
            }
            if let Some(position) = input.pointer.interact_pos() {
                let delta = position - drag.last;
                drag.moved |= input.pointer.is_decidedly_dragging();
                if drag.moved {
                    match drag.owner {
                        Owner::Orbit | Owner::Cube => motion.orbit = delta * 0.007,
                        Owner::Pan => motion.pan = delta,
                        _ => {}
                    }
                }
                drag.last = position;
            }
            motion.used = drag.moved || drag.owner == Owner::Tool || drag.exclusive;
            !input.pointer.button_down(drag.button)
        });
        if released {
            self.drag = None;
            self.suppressed = ui.input(|input| input.pointer.any_down());
        }
        motion
    }
    pub fn keyboard(&mut self, ui: &egui::Ui, focused: bool) -> Motion {
        let mut motion = Motion::default();
        if !focused
            || !ui.input(|input| input.focused)
            || ui.ctx().text_edit_focused()
            || egui::Popup::is_any_open(ui.ctx())
        {
            self.keys_suppressed |= self.held.iter().any(Option::is_some);
            self.held = [None; 6];
            return motion;
        }
        let keys = [
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
            egui::Key::Plus,
            egui::Key::Minus,
        ];
        if self.keys_suppressed {
            if !ui.input(|input| {
                keys.into_iter().any(|key| input.key_down(key)) || input.key_down(egui::Key::Equals)
            }) {
                self.keys_suppressed = false;
            }
            return motion;
        }
        ui.input_mut(|input| {
            if input.modifiers.alt || input.modifiers.ctrl || input.modifiers.command { self.held = [None; 6]; return; }
            for (index, key) in keys.into_iter().enumerate() {
                let down = input.key_down(key) || (key == egui::Key::Plus && input.key_down(egui::Key::Equals));
                if !down { self.held[index] = None; continue; }
                let initial = self.held[index].is_none();
                if initial && !input.events.iter().any(|event| matches!(event,egui::Event::Key{key:pressed_key,pressed:true,repeat:false,..} if *pressed_key==key || (key==egui::Key::Plus && *pressed_key==egui::Key::Equals))) {continue;}
                let start = *self.held[index].get_or_insert(input.time);
                let amount = if initial { 1.0 } else if input.time - start >= 0.25 {
                    f64::from(input.stable_dt.min(0.05))
                } else { 0.0 };
                let step = if initial { 15_f64.to_radians() } else { std::f64::consts::FRAC_PI_2 } * amount;
                let sign = if index % 2 == 0 { -1.0 } else { 1.0 };
                if index < 4 {
                    let axis = if index < 2 { egui::Vec2::X } else { egui::Vec2::Y };
                    if input.modifiers.shift { motion.pan += axis * (sign * if initial { 0.03 } else { 0.5 } * amount) as f32; }
                    else { motion.orbit += axis * (sign * step) as f32; }
                } else { motion.zoom += sign * if initial { 1.1_f64.ln() } else { 1.5 * amount }; }
                motion.used = true;
            }
            input.events.retain(|event| !matches!(event, egui::Event::Key { key, .. } if keys.contains(key) || *key == egui::Key::Equals));
        });
        if motion.used {
            ui.ctx().request_repaint();
        }
        motion
    }
}

pub(super) struct NormalTarget {
    scene: std::sync::Weak<Scene>,
    face: Option<FaceRef>,
    scope: Option<OccurrenceId>,
    normal: Option<DVec3>,
}

pub(super) fn scroll(ui: &egui::Ui, settings: Settings, allowed: bool) -> Motion {
    let mut motion = Motion::default();
    if !allowed || egui::Popup::is_any_open(ui.ctx()) || !ui.is_enabled() {
        return motion;
    }
    ui.input_mut(|input| {
        let scroll = std::mem::take(&mut input.smooth_scroll_delta);
        let pinch = input.zoom_delta();
        if settings.device == InputDevice::Trackpad {
            motion.pan = -scroll;
        } else {
            motion.zoom -= f64::from(scroll.y) * 0.002;
        }
        if pinch.is_finite() && pinch > 0.0 {
            motion.zoom -= f64::from(pinch).ln();
        }
        if settings.reverse_zoom {
            motion.zoom = -motion.zoom;
        }
        motion.used = scroll != egui::Vec2::ZERO || pinch != 1.0;
    });
    motion
}

pub(super) fn keys(ui: &egui::Ui) -> (Option<ViewAction>, bool) {
    if ui.ctx().text_edit_focused()
        || egui::Popup::is_any_open(ui.ctx())
        || !ui.input(|input| input.focused)
    {
        return (None, false);
    }
    let mut action = None;
    let mut focus = false;
    ui.input_mut(|input| {
        input.events.retain(|event| {
            let egui::Event::Key {
                key,
                pressed: true,
                repeat,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            if modifiers.alt || modifiers.ctrl || modifiers.command {
                return true;
            }
            if modifiers.shift && *key != egui::Key::N {
                return true;
            }
            let selected = match key {
                egui::Key::Num0 => Some(ViewAction::Snap(StandardView::Isometric)),
                egui::Key::Num1 => Some(ViewAction::Snap(StandardView::Top)),
                egui::Key::Num2 => Some(ViewAction::Snap(StandardView::Bottom)),
                egui::Key::Num3 => Some(ViewAction::Snap(StandardView::Front)),
                egui::Key::Num4 => Some(ViewAction::Snap(StandardView::Back)),
                egui::Key::Num5 => Some(ViewAction::Snap(StandardView::Left)),
                egui::Key::Num6 => Some(ViewAction::Snap(StandardView::Right)),
                egui::Key::F => Some(ViewAction::Fit),
                egui::Key::P => Some(ViewAction::Projection),
                egui::Key::N => Some(if modifiers.shift {
                    ViewAction::OppositeNormal
                } else {
                    ViewAction::Normal
                }),
                _ => None,
            };
            if selected.is_some() {
                if !*repeat {
                    action = selected;
                }
                return false;
            }
            if *key == egui::Key::G && !modifiers.shift {
                if !*repeat {
                    focus = true;
                }
                return false;
            }
            true
        });
    });
    (action, focus)
}

impl Viewport {
    fn navigation_button(&mut self, ui: &mut egui::Ui, action: ViewAction) {
        if crate::ui::action(ui, action.label(), self.view_disabled(action)) {
            self.queue_view(action);
            ui.close();
            ui.ctx().request_repaint();
        }
    }
    pub fn navigation_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Standard views", |ui| {
            for view in StandardView::ALL {
                self.navigation_button(ui, ViewAction::Snap(view));
            }
        });
        ui.menu_button("Frame view", |ui| {
            for action in [ViewAction::Fit, ViewAction::FitParts, ViewAction::FitPlate] {
                self.navigation_button(ui, action);
            }
        });
        if ui
            .selectable_label(self.orthographic, "Orthographic")
            .clicked()
        {
            self.queue_view(ViewAction::Projection);
            ui.close();
        }
        for action in [
            ViewAction::Normal,
            ViewAction::OppositeNormal,
            ViewAction::Previous,
        ] {
            self.navigation_button(ui, action);
        }
        ui.menu_button("Roll view", |ui| {
            self.navigation_button(ui, ViewAction::RollLeft);
            self.navigation_button(ui, ViewAction::RollRight);
        });
        ui.menu_button("Navigation settings", |ui| {
            ui.horizontal(|ui| {
                ui.label("Input");
                ui.selectable_value(
                    &mut self.navigation_settings.device,
                    InputDevice::Mouse,
                    "Mouse",
                );
                ui.selectable_value(
                    &mut self.navigation_settings.device,
                    InputDevice::Trackpad,
                    "Trackpad",
                );
            });
            ui.checkbox(&mut self.navigation_settings.reverse_zoom, "Reverse zoom");
            if ui
                .checkbox(&mut self.navigation_settings.animate, "Animate navigation")
                .changed()
                && !self.navigation_settings.animate
            {
                self.advance_focus(ui.input(|input| input.time) + focus::DURATION_SECONDS);
            }
        });
    }
    pub fn suspend_navigation(&mut self) {
        self.navigation.cancel();
        self.interrupt_focus();
        self.pending_view = None;
    }
    pub(super) fn selection_normal(
        &mut self,
        scene: Option<&Arc<Scene>>,
        face: Option<&FaceRef>,
        scope: Option<OccurrenceId>,
    ) -> Option<DVec3> {
        let Some(scene) = scene else {
            self.normal_cache = None;
            return None;
        };
        if self.normal_cache.as_ref().is_none_or(|cached| {
            !cached.scene.ptr_eq(&Arc::downgrade(scene))
                || cached.face.as_ref() != face
                || cached.scope != scope
        }) {
            self.normal_cache = Some(NormalTarget {
                scene: Arc::downgrade(scene),
                face: face.cloned(),
                scope,
                normal: face
                    .and_then(|_| scene.focus_target(face, scope).ok())
                    .and_then(|target| target.normal),
            });
        }
        self.normal_cache.as_ref().and_then(|cached| cached.normal)
    }
    pub fn cancel_navigation(&mut self) -> bool {
        self.navigation.cancel_navigation()
    }
    pub fn queue_view(&mut self, action: ViewAction) {
        self.pending_view = Some(action);
    }
    pub fn view_disabled(&self, action: ViewAction) -> Option<&'static str> {
        match action {
            ViewAction::Previous if self.previous_camera.is_none() || self.reveal.is_some() => {
                Some("No previous camera is available")
            }
            ViewAction::FitPlate if !self.had_plate => Some("No build plate is visible"),
            ViewAction::FitParts if !self.had_source => Some("No source parts are visible"),
            ViewAction::Normal | ViewAction::OppositeNormal if self.normal.is_none() => {
                Some("Select a face with a stable normal or open a section")
            }
            _ => None,
        }
    }
    pub(super) fn navigate(&mut self, action: ViewAction, now: f64, animated: bool) {
        if self.view_disabled(action).is_some() {
            return;
        }
        if action == ViewAction::Previous {
            if let Some(previous) = self.previous_camera.take() {
                self.interrupt_focus();
                self.animate_to(previous, now, animated);
            }
            return;
        }
        self.interrupt_focus();
        self.previous_camera = Some(self.camera());
        match action {
            ViewAction::Fit | ViewAction::FitParts | ViewAction::FitPlate => {
                self.fit();
                self.fit_scope = Some(action);
            }
            _ => {
                let mut camera = self.camera();
                match action {
                    ViewAction::Snap(view) => {
                        camera.rotation = orientation(view.direction()).to_array();
                        camera.orthographic = true;
                    }
                    ViewAction::Direction(axes) => {
                        let direction =
                            DVec3::new(f64::from(axes[0]), f64::from(axes[1]), f64::from(axes[2]));
                        if direction == DVec3::ZERO {
                            return;
                        }
                        camera.rotation = orientation(direction).to_array();
                        camera.orthographic = true;
                    }
                    ViewAction::Projection => camera.orthographic = !camera.orthographic,
                    ViewAction::RollLeft | ViewAction::RollRight => {
                        let angle = std::f64::consts::FRAC_PI_2
                            * if action == ViewAction::RollLeft {
                                1.0
                            } else {
                                -1.0
                            };
                        camera.rotation = (self.rotation * DQuat::from_rotation_z(angle))
                            .normalize()
                            .to_array();
                    }
                    ViewAction::Normal | ViewAction::OppositeNormal => {
                        let normal = self.normal.unwrap()
                            * if action == ViewAction::Normal {
                                1.0
                            } else {
                                -1.0
                            };
                        camera.rotation = orientation(normal).to_array();
                        camera.orthographic = true;
                    }
                    _ => {}
                }
                self.animate_to(camera, now, animated);
            }
        }
    }
    pub(super) fn orbit(&mut self, delta: egui::Vec2) {
        if delta == egui::Vec2::ZERO {
            return;
        }
        self.interrupt_focus();
        self.rotation = (DQuat::from_rotation_z(-f64::from(delta.x))
            * self.rotation
            * DQuat::from_rotation_x(-f64::from(delta.y)))
        .normalize();
    }
    pub(super) fn pan_pixels(&mut self, delta: egui::Vec2, height: f32) {
        if delta == egui::Vec2::ZERO {
            return;
        }
        self.interrupt_focus();
        let span = self.distance * 2.0 * 22.5_f64.to_radians().tan();
        self.pan += self.rotation * DVec3::new(-f64::from(delta.x), f64::from(delta.y), 0.0) * span
            / f64::from(height.max(1.0));
    }
}

#[cfg(test)]
mod tests;
