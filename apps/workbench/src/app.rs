// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{
    commands::Action,
    host::{Command, Config, Host, Phase, Picker, Snapshot},
    ui::{self, Theme},
};
use egui::{Key, KeyboardShortcut, Modifiers, RichText};
use serde::{Deserialize, Serialize};

const LICENSE: &str = include_str!("../../../LICENSE.md");
const PREFS: &str = "spiling-native-preferences-v1";
#[derive(Serialize, Deserialize)]
struct Preferences {
    theme: Theme,
    accepted_osl3: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::Graphite,
            accepted_osl3: false,
        }
    }
}

pub struct Workbench {
    prefs: Preferences,
    host: Option<Host>,
    state: Snapshot,
    queued: Option<Action>,
    confirmation: Option<(u64, bool)>,
    awaiting_serial: Option<u64>,
    local_problem: Option<String>,
    show_license: bool,
    show_gallery: bool,
    start_requested: bool,
    close_requested: bool,
    gpu: String,
    title: String,
}
impl Workbench {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let prefs: Preferences = cc
            .storage
            .and_then(|s| eframe::get_value(s, PREFS))
            .unwrap_or_default();
        prefs.theme.apply(&cc.egui_ctx);
        let gpu = cc
            .wgpu_render_state
            .as_ref()
            .map(|s| {
                let info = s.adapter.get_info();
                format!(
                    "{} · {:?} · {:?}",
                    info.name, info.backend, info.device_type
                )
            })
            .unwrap_or_else(|| "wgpu adapter unavailable".into());
        eprintln!("Spiling native graphics: {gpu}");
        Self {
            start_requested: prefs.accepted_osl3,
            prefs,
            host: None,
            state: Snapshot::default(),
            queued: None,
            confirmation: None,
            awaiting_serial: None,
            local_problem: None,
            show_license: false,
            show_gallery: false,
            close_requested: false,
            gpu,
            title: String::new(),
        }
    }
    fn start(&mut self, ctx: &egui::Context) {
        let wake = ctx.clone();
        match Config::installed()
            .and_then(|config| Host::new(config, move || wake.request_repaint()))
        {
            Ok(host) => self.host = Some(host),
            Err(error) => {
                self.state.phase = Phase::Interrupted;
                self.state.busy = false;
                self.local_problem = Some(error);
            }
        }
    }
    fn disabled(&self, action: Action) -> Option<&'static str> {
        if self.awaiting_serial.is_some() && !matches!(action, Action::Gallery | Action::License) {
            Some("Waiting for native admission")
        } else {
            action.disabled(&self.state)
        }
    }
    fn button(&mut self, ui: &mut egui::Ui, action: Action) {
        if ui::action(ui, action.label(), self.disabled(action)) {
            self.queued = Some(action);
            ui.close_kind(egui::UiKind::Menu);
        }
    }
    fn dispatch(&mut self, action: Action, frame: &eframe::Frame, ctx: &egui::Context) {
        if self.disabled(action).is_some() {
            return;
        }
        match action {
            Action::License => {
                self.show_license = true;
                return;
            }
            Action::Gallery => {
                self.show_gallery = true;
                return;
            }
            _ => {}
        }
        let Some(host) = &mut self.host else {
            if action == Action::Close {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if action == Action::Restart {
                self.start(ctx);
            }
            return;
        };
        let result = match action {
            Action::Open => host.choose(frame, ctx, Picker::Open),
            Action::OpenReadOnly => host.choose(frame, ctx, Picker::OpenReadOnly),
            Action::Recover => host.choose(frame, ctx, Picker::Recover),
            Action::SaveAs => host.choose(frame, ctx, Picker::Save),
            Action::Save
                if self
                    .state
                    .project
                    .as_ref()
                    .is_some_and(|p| p.path_label.is_none()) =>
            {
                host.choose(frame, ctx, Picker::Save)
            }
            Action::Import => host.choose(frame, ctx, Picker::Import),
            _ => host.command(match action {
                Action::New => Command::New,
                Action::Save => Command::Save,
                Action::Undo => Command::Undo,
                Action::Redo => Command::Redo,
                Action::Restart => Command::Restart,
                Action::Close => Command::Close,
                Action::Cancel => Command::Cancel,
                Action::Resume => Command::Resume,
                _ => unreachable!(),
            }),
        };
        match result {
            Ok(()) => self.awaiting_serial = Some(self.state.serial),
            Err(error) => self.local_problem = Some(error),
        }
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Text/IME owns its own editing keys. Project undo never steals text undo.
        if ctx.text_edit_focused() || self.state.confirmation.is_some() {
            return;
        }
        for (key, shift, action) in [
            (Key::N, false, Action::New),
            (Key::O, false, Action::Open),
            (Key::S, true, Action::SaveAs),
            (Key::S, false, Action::Save),
            (Key::I, false, Action::Import),
            (Key::Z, true, Action::Redo),
            (Key::Z, false, Action::Undo),
        ] {
            let modifiers = Modifiers {
                shift,
                ..Modifiers::COMMAND
            };
            if self.disabled(action).is_none()
                && ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(modifiers, key)))
            {
                self.queued = Some(action);
                break;
            }
        }
    }
    fn project(&self, ui: &mut egui::Ui) {
        let tokens = self.prefs.theme.tokens();
        ui.heading("Project");
        if self.state.phase != Phase::Running {
            ui.colored_label(tokens.warning, "Last observed state · engine disconnected");
        }
        let Some(p) = &self.state.project else {
            ui.label("Waiting for checked project information");
            return;
        };
        ui::property(
            ui,
            "Location",
            p.path_label.as_deref().unwrap_or("Unsaved project"),
            tokens,
        );
        let status = if p.read_only {
            "Read-only"
        } else if p.save_uncertain {
            "Save outcome uncertain"
        } else if p.dirty {
            "Unsaved changes"
        } else {
            "Committed state"
        };
        ui::property(ui, "State", status, tokens);
        ui::property(ui, "Project revision", p.revision.0.to_string(), tokens);
        ui::property(
            ui,
            "Saved revision",
            p.saved_revision
                .map(|r| r.0.to_string())
                .unwrap_or_else(|| "Never saved".into()),
            tokens,
        );
        if p.recovered_previous {
            ui.colored_label(tokens.warning, "Recovered previous checkpoint");
        }
        if p.save_uncertain {
            ui.colored_label(tokens.warning,"A save may have reached disk. Inspect the saved project before relying on the outcome.");
        }
        if let Some(scene) = &self.state.scene {
            ui.separator();
            ui::property(
                ui,
                "Native definitions",
                scene.definition_count.to_string(),
                tokens,
            );
            ui::property(
                ui,
                "Native occurrences",
                scene.occurrence_count.to_string(),
                tokens,
            );
            ui::property(ui, "Scene revision", scene.revision.0.to_string(), tokens);
        }
        ui.collapsing("Native identity", |ui| {
            ui::property(ui, "Project ID", p.project_id.as_str(), tokens);
            if let Some(hello) = &self.state.hello {
                ui::property(ui, "Engine build", &hello.engine_build, tokens);
                ui::property(ui, "Kernel", &hello.kernel, tokens);
                ui::property(ui, "Session", hello.session_id.as_str(), tokens);
                ui::property(ui, "Child PID", hello.pid.to_string(), tokens);
            }
            ui::property(ui, "UI graphics adapter", &self.gpu, tokens);
        });
    }
    fn feedback(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            columns[0].heading("Jobs");
            egui::ScrollArea::vertical()
                .id_salt("jobs")
                .show(&mut columns[0], |ui| {
                    if self.state.jobs.is_empty() {
                        ui.weak("No native operations in this session");
                    }
                    for job in self.state.jobs.iter().rev() {
                        ui.label(format!("{} · {:?}", job.action, job.status));
                        ui.small(format!("{} · {} / {}", job.stage, job.completed, job.total));
                    }
                });
            columns[1].heading("Problems");
            egui::ScrollArea::vertical()
                .id_salt("problems")
                .show(&mut columns[1], |ui| {
                    if let Some(problem) = &self.local_problem {
                        ui.colored_label(self.prefs.theme.tokens().warning, problem);
                    }
                    for problem in self.state.problems.iter().rev() {
                        ui.label(problem);
                    }
                    if self.state.problems.is_empty() && self.local_problem.is_none() {
                        ui.weak("No reported problems");
                    }
                });
        });
    }
}
impl eframe::App for Workbench {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.start_requested {
            self.start_requested = false;
            self.start(ctx);
        }
        if let Some(host) = &self.host {
            self.state = host.snapshot();
            if self
                .awaiting_serial
                .is_some_and(|serial| self.state.serial > serial)
            {
                self.awaiting_serial = None;
            }
        }
        let title = self
            .state
            .project
            .as_ref()
            .map(|p| {
                format!(
                    "{}{}{} · Spiling",
                    if p.dirty { "● " } else { "" },
                    p.path_label.as_deref().unwrap_or("Unsaved project"),
                    if p.read_only { " [read-only]" } else { "" }
                )
            })
            .unwrap_or_else(|| "Spiling".into());
        if self.title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        if self.state.phase == Phase::Closed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.host.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.close_requested
                && self.awaiting_serial.is_none()
                && self.state.confirmation.is_none()
            {
                self.queued = Some(Action::Close);
                self.close_requested = true;
            }
        }
        if self.state.confirmation.is_some() || (!self.state.busy && self.awaiting_serial.is_none())
        {
            self.close_requested = false;
        }
        if let Some((id, discard)) = self.confirmation.take()
            && let Some(host) = &self.host
        {
            match host.confirm(id, discard) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if let Some(action) = self.queued.take() {
            self.dispatch(action, frame, ctx);
        }
        if self.prefs.accepted_osl3 {
            self.shortcuts(ctx);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let previous_theme = self.prefs.theme;
        if !self.prefs.accepted_osl3 {
            ui.heading("Welcome to Spiling");
            ui.label("Native BREP manufacturing workbench · development build");
            ui.label("Review the Open Software License 3.0 before using this application.");
            egui::ScrollArea::vertical()
                .max_height(ui.available_height() - 70.0)
                .show(ui, |ui| {
                    ui.label(LICENSE);
                });
            if ui.button("Accept OSL-3.0 and continue").clicked() {
                self.prefs.accepted_osl3 = true;
                self.start_requested = true;
                ctx.request_repaint();
            }
            return;
        }
        egui::Panel::top("commands").show(ui, |ui| {
            let wide = ui.available_width() >= 900.0;
            ui.horizontal(|ui| {
                ui.heading("Spiling");
                if wide {
                    ui.weak("Native workbench · development");
                }
                ui.menu_button("Project", |ui| {
                    for action in [
                        Action::New,
                        Action::Open,
                        Action::OpenReadOnly,
                        Action::Recover,
                        Action::Save,
                        Action::SaveAs,
                        Action::Import,
                        Action::Close,
                    ] {
                        self.button(ui, action);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    for action in [Action::Undo, Action::Redo] {
                        self.button(ui, action);
                    }
                });
                ui.menu_button("Engine", |ui| {
                    self.button(ui, Action::Restart);
                    self.button(ui, Action::Cancel);
                    self.button(ui, Action::Resume);
                });
                ui.menu_button("Help", |ui| {
                    self.button(ui, Action::License);
                    self.button(ui, Action::Gallery);
                });
                ui.separator();
                self.button(ui, Action::Open);
                self.button(ui, Action::Save);
                self.button(ui, Action::Import);
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{:?}", self.state.phase));
                ui.separator();
                ui.add(egui::Label::new(&self.state.message).truncate())
                    .on_hover_text(&self.state.message);
            });
        });
        egui::Panel::bottom("feedback")
            .resizable(true)
            .default_size(160.0)
            .min_size(72.0)
            .show(ui, |ui| self.feedback(ui));
        egui::Panel::right("project")
            .resizable(true)
            .default_size(280.0)
            .min_size(210.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.project(ui));
            });
        egui::Frame::central_panel(ui.style()).fill(self.prefs.theme.tokens().canvas).show(ui,|ui| {
            ui.set_min_size(ui.available_size());
            ui.add_space(24.0);
            ui::notice(ui,"3D display not implemented", "This development increment manages real native projects. Source geometry rendering and source-face picking arrive in the next increment.", self.prefs.theme.tokens());
            ui.add_space(20.0);
            ui.label("The project inspector shows information returned by the supervised engine.");
            ui.label("Manufacturing programs are not machine ready.");
            if self.state.phase == Phase::Interrupted { self.button(ui,Action::Restart); }
            ui.add_space(24.0); ui::theme_choice(ui,&mut self.prefs.theme);
        });
        if let Some(confirmation) = &self.state.confirmation {
            let id = confirmation.id;
            let modal = egui::Modal::new(egui::Id::unique("discard-project")).show(&ctx,|ui| {
                ui.set_max_width(420.0);
                ui.heading("Discard unsaved changes?");
                ui.label(format!("{} discards the current in-memory changes. Save first to preserve them.",confirmation.action));
                if self.state.project.as_ref().is_some_and(|p| p.save_uncertain) { ui.colored_label(self.prefs.theme.tokens().warning,"The last save may already have reached disk. Inspect it before relying on its outcome."); }
                ui.horizontal(|ui| {
                    if ui.button("Keep editing").clicked() { self.confirmation = Some((id,false)); }
                    if ui.button(RichText::new("Discard changes").color(self.prefs.theme.tokens().warning)).clicked() { self.confirmation = Some((id,true)); }
                });
            });
            if modal.should_close() {
                self.confirmation = Some((id, false));
            }
        }
        egui::Window::new("License · OSL-3.0")
            .open(&mut self.show_license)
            .vscroll(true)
            .show(&ctx, |ui| {
                ui.label(LICENSE);
            });
        egui::Window::new("Component gallery")
            .open(&mut self.show_gallery)
            .vscroll(true)
            .show(&ctx, |ui| ui::gallery(ui, &mut self.prefs.theme));
        if previous_theme != self.prefs.theme {
            self.prefs.theme.apply(&ctx);
        }
        // Side effects execute once in logic(), even if egui lays out the UI in multiple passes.
        if self.queued.is_some() || self.confirmation.is_some() {
            ctx.request_repaint();
        }
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFS, &self.prefs);
    }
}
