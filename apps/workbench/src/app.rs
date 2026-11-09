// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::viewport::Viewport;
use crate::{
    commands::Action,
    host::{Command, Config, Host, Phase, Picker, Snapshot},
    ui::{self, Theme},
};
use egui::{Key, KeyboardShortcut, Modifiers, RichText};
use glam::DVec3;
use serde::{Deserialize, Serialize};
use spiling_contracts::geometry::*;

mod chrome;
use crate::commands::Workspace;
use chrome::{Panel, WorkArea};
mod alternative;
mod interfaces;
pub(crate) use alternative::gallery as alternative_gallery;

const LICENSE: &str = include_str!("../../../LICENSE.md");
const PREFS: &str = "spiling-native-preferences-v1";
#[derive(Serialize, Deserialize)]
struct Preferences {
    #[serde(default)]
    compute: spiling_contracts::compute::ComputeSettings,
    theme: Theme,
    accepted_osl3: bool,
    #[serde(default)]
    library_location: Option<NativePath>,
    #[serde(default)]
    analysis_pins: Vec<analysis::Pinned>,
    #[serde(default)]
    dismissed_policy_contexts: Vec<SourceHash>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            compute: Default::default(),
            theme: Theme::Graphite,
            accepted_osl3: false,
            library_location: None,
            analysis_pins: Vec::new(),
            dismissed_policy_contexts: Vec::new(),
        }
    }
}

pub struct Workbench {
    startup_open: Option<crate::launch::Open>,
    opening_frames: crate::performance::Frames,
    prefs: Preferences,
    host: Option<Host>,
    state: Snapshot,
    queued: Option<Action>,
    confirmation: Option<(u64, bool)>,
    awaiting_serial: Option<u64>,
    local_problem: Option<String>,
    show_license: bool,
    show_gallery: bool,
    show_commands: bool,
    show_feedback: bool,
    show_compute: bool,
    queued_compute: Option<spiling_contracts::compute::ComputeSettings>,
    command_selection: Option<usize>,
    command_return_focus: Option<egui::Id>,
    surface_focus: [Option<egui::Id>; 5],
    command_query: String,
    command_focus: bool,
    compact_tree: bool,
    start_requested: bool,
    close_requested: bool,
    gpu: String,
    title: String,
    viewport: Viewport,
    interfaces: interfaces::InterfacesUi,
    selected: Option<FaceRef>,
    occurrence: Option<OccurrenceId>,
    pose: [f64; 6],
    pose_original: [f64; 6],
    navigation: Option<navigation::Navigation>,
    navigation_focus: Option<egui::Id>,
    pose_revision: Option<SceneRevision>,
    analysis: analysis::AnalysisUi,
    mechanics: mechanics::MechanicsUi,
    layout: layout::LayoutUi,
    plates: plates::PlatesUi,
    queued_geometry: Option<GeometryCommand>,
    queued_host: Option<Command>,
    queued_picker: Option<Picker>,
    queued_manufacturing: Option<spiling_contracts::manufacturing::ManufacturingCommand>,
    queued_project: Option<spiling_contracts::project::ProjectCommand>,
    work_area: WorkArea,
    manufacturing: manufacturing::ManufacturingUi,
    material: material::MaterialUi,
    process: process::ProcessUi,
    surface: surface::SurfaceUi,
    study: study::StudyUi,
    stock: stock::StockUi,
    colors: ui::ColorInspector,
    library: library::LibraryUi,
    queued_image: Option<crate::assets::ImageRequest>,
}
impl Workbench {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_open(cc, None)
    }
    pub fn with_open(cc: &eframe::CreationContext<'_>, open: Option<crate::launch::Open>) -> Self {
        let _opening = crate::performance::span("app_initialize");
        let mut prefs: Preferences = cc
            .storage
            .and_then(|s| eframe::get_value(s, PREFS))
            .unwrap_or_default();
        prefs.analysis_pins.retain(|p| p.valid());
        prefs.analysis_pins.truncate(32);
        prefs.dismissed_policy_contexts.truncate(32);
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
            startup_open: open,
            opening_frames: Default::default(),
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
            show_commands: false,
            show_feedback: false,
            show_compute: false,
            queued_compute: None,
            command_selection: None,
            command_return_focus: None,
            surface_focus: [None; 5],
            command_query: String::new(),
            command_focus: false,
            compact_tree: false,
            close_requested: false,
            gpu,
            title: String::new(),
            viewport: Viewport::new(cc),
            interfaces: Default::default(),
            selected: None,
            occurrence: None,
            pose: [0.0; 6],
            pose_original: [0.0; 6],
            navigation: None,
            navigation_focus: None,
            pose_revision: None,
            analysis: analysis::AnalysisUi::default(),
            mechanics: Default::default(),
            layout: layout::LayoutUi::default(),
            plates: Default::default(),
            queued_geometry: None,
            queued_host: None,
            queued_picker: None,
            queued_manufacturing: None,
            queued_project: None,
            work_area: WorkArea::default(),
            manufacturing: manufacturing::ManufacturingUi::default(),
            material: material::MaterialUi::default(),
            process: process::ProcessUi::default(),
            surface: surface::SurfaceUi::default(),
            study: study::StudyUi::default(),
            stock: stock::StockUi::default(),
            colors: ui::ColorInspector::default(),
            library: library::LibraryUi::default(),
            queued_image: None,
        }
    }
    fn start(&mut self, ctx: &egui::Context) {
        let wake = ctx.clone();
        match Config::installed().and_then(|config| {
            Host::with_compute(config, self.prefs.compute, move || wake.request_repaint())
        }) {
            Ok(host) => self.host = Some(host),
            Err(error) => {
                self.state.phase = Phase::Interrupted;
                self.state.busy = false;
                self.local_problem = Some(error);
            }
        }
    }
    fn disabled(&self, action: Action) -> Option<&'static str> {
        if matches!(action, Action::FocusSelection | Action::RevealSelection)
            && (!self.display_current()
                || self.occurrence.is_none()
                || self.layout.open
                || !self.source_display_visible())
        {
            return Some("Select a part or face in the current source display first");
        }
        if action == Action::Compile && (self.project_drafts() || self.subject_drafts()) {
            return Some("Apply or discard unapplied changes before slicing");
        }
        if action == Action::Compare && !self.manufacturing_current() {
            return Some("Freshly verify the current plate before comparing settings");
        }
        if let Some(project) = &self.state.project {
            if action == Action::LoadMaterial
                && self.material.has_changes(
                    &project.project_id,
                    self.state.manufacturing.intent.as_deref(),
                )
            {
                return Some("Apply or discard the Material draft before loading another profile");
            }
            if action == Action::LoadIntent
                && (self.manufacturing.has_intent_draft(
                    &project.project_id,
                    self.state.manufacturing.intent.as_deref(),
                ) || self
                    .manufacturing
                    .pending_loaded(self.state.manufacturing.loaded.as_deref()))
            {
                return Some("Apply or discard the Settings draft before loading another intent");
            }
        }
        if self.awaiting_serial.is_some()
            && !action.navigation()
            && !matches!(action, Action::Gallery | Action::License | Action::Analysis)
        {
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
    fn primary_button(&mut self, ui: &mut egui::Ui, action: Action) {
        if ui::primary(ui, action.label(), self.disabled(action)) {
            self.queued = Some(action);
        }
    }
    fn dispatch(&mut self, action: Action, frame: &eframe::Frame, ctx: &egui::Context) {
        if self.disabled(action).is_some() {
            return;
        }
        if self.task_action(action, ctx) {
            return;
        }
        if self.guard_action(action) {
            return;
        }
        if action != Action::Gallery && self.show_gallery {
            self.show_gallery = false;
            if let Some(id) = self.surface_focus[3].take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
        if action != Action::License && self.show_license {
            self.show_license = false;
            if let Some(id) = self.surface_focus[2].take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
        match action {
            Action::RevealSelection => {
                self.reveal_selection();
                ctx.request_repaint();
                return;
            }
            Action::FocusSelection => {
                self.focus_selection();
                ctx.request_repaint();
                return;
            }
            Action::Arrange => {
                self.open_layout(ctx);
                return;
            }
            Action::Analysis => {
                self.close_layout(ctx);
                self.close_library(ctx);
                if !self.analysis.open {
                    self.surface_focus[0] = ctx.memory(|m| m.focused());
                }
                self.analysis.open = true;
                self.analysis.view.enabled = true;
                return;
            }
            Action::Library => {
                if !self.library.open {
                    self.surface_focus[1] = ctx.memory(|m| m.focused());
                }
                self.library.open = true;
                return;
            }
            Action::License => {
                self.suspend_library();
                if !self.show_license {
                    self.surface_focus[2] = ctx.memory(|m| m.focused());
                }
                self.show_license = true;
                return;
            }
            Action::Gallery => {
                self.suspend_library();
                if !self.show_gallery {
                    self.surface_focus[3] = ctx.memory(|m| m.focused());
                }
                self.show_gallery = true;
                return;
            }
            _ => {}
        }
        match action {
            Action::LoadIntent => self.show_prepare(2),
            Action::LoadMaterial => self.show_prepare(1),
            Action::LoadMaterialCatalog => {
                self.show_prepare(1);
                self.open_profile_catalog(ctx);
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
            Action::LoadIntent => host.choose(frame, ctx, Picker::Intent),
            Action::LoadMaterial => host.choose(frame, ctx, Picker::Material),
            Action::LoadMaterialCatalog => host.choose(frame, ctx, Picker::MaterialCatalog),
            Action::LoadAnalysisModel => host.choose(frame, ctx, Picker::AnalysisModel),
            Action::Export => host.choose(frame, ctx, Picker::Export),
            Action::Preflight | Action::Compile | Action::Verify => {
                use spiling_contracts::manufacturing::{ManufacturingCommand, VerificationOutput};
                let session_id = self
                    .state
                    .hello
                    .as_ref()
                    .expect("running engine")
                    .session_id
                    .clone();
                let base_revision = self
                    .state
                    .project
                    .as_ref()
                    .expect("checked project")
                    .revision;
                let layout_id = self
                    .state
                    .project
                    .as_ref()
                    .expect("checked project")
                    .layout_id
                    .clone();
                host.manufacturing(match action {
                    Action::Preflight => ManufacturingCommand::Preflight {
                        layout_id,
                        session_id,
                        base_revision,
                    },
                    Action::Compile => ManufacturingCommand::Compile {
                        layout_id,
                        session_id,
                        base_revision,
                    },
                    _ => ManufacturingCommand::Inspect {
                        layout_id,
                        session_id,
                        base_revision,
                        output: VerificationOutput::ReportAndMotion,
                    },
                })
            }
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
        if self.plate_sheet_open() {
            return;
        }
        if self.navigation.is_some() {
            return;
        }
        // Route Escape once, from the topmost surface. Popups and modal decisions
        // retain their own Escape; an edit gets its first opportunity to finish.
        if !self.show_commands
            && self.navigation.is_none()
            && self.state.confirmation.is_none()
            && !egui::Popup::is_any_open(ctx)
            && !ctx.text_edit_focused()
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            if self.mechanics.evidence_open {
                self.mechanics.evidence_open = false;
            } else if self.show_gallery {
                self.show_gallery = false;
                if let Some(id) = self.surface_focus[3].take() {
                    ctx.memory_mut(|m| m.request_focus(id));
                }
            } else if self.show_license {
                self.show_license = false;
                if let Some(id) = self.surface_focus[2].take() {
                    ctx.memory_mut(|m| m.request_focus(id));
                }
            } else if self.library.open {
                self.close_library(ctx);
            } else if self.layout.open {
                self.close_layout(ctx);
            } else if self.plates.overview {
                self.close_plate_overview(ctx);
            } else if self.viewport.close_reveal(
                ctx.input(|input| input.time),
                ctx.global_style().animation_time > 0.0,
            ) || self.viewport.stop_focus()
            {
                ctx.request_repaint();
            } else if self.analysis.open {
                self.close_analysis(ctx);
            } else if self.show_feedback {
                self.show_feedback = false;
            } else if self.work_area.panel == Panel::Study {
                self.close_study(ctx);
            } else if self.work_area.panel != self.work_area.home_panel() {
                self.work_area.close_task();
            } else if self.selected.is_some() || self.occurrence.is_some() {
                self.clear_selection();
            }
            return;
        }
        // Text/IME owns its own editing keys. Project undo never steals text undo.
        if ctx.text_edit_focused() && !self.show_commands {
            return;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::K)))
            && self.state.confirmation.is_none()
            && !self.state.picker_open
        {
            self.show_commands = !self.show_commands;
            self.command_focus = self.show_commands;
            self.command_query.clear();
            ctx.request_repaint();
        }
        if ctx.text_edit_focused() || self.show_commands || self.state.confirmation.is_some() {
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
        ui::pane_header(ui, "Project", "Saved state and identity");
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
        } else if p.path_label.is_some() {
            "Saved"
        } else {
            "Not saved"
        };
        ui::property(ui, "State", status, tokens);
        ui.collapsing("Saved revisions", |ui| {
            ui::property(ui, "Project revision", p.revision.0.to_string(), tokens);
            ui::property(
                ui,
                "Saved revision",
                p.saved_revision
                    .map(|r| r.0.to_string())
                    .unwrap_or_else(|| "Never saved".into()),
                tokens,
            );
        });
        if p.recovered_previous {
            ui.colored_label(tokens.warning, "Recovered previous checkpoint");
        }
        if p.save_uncertain {
            ui.colored_label(tokens.warning,"A save may have reached disk. Inspect the saved project before relying on the outcome.");
        }
        ui.collapsing("Scene details", |ui| {
            if let Some(scene) = &self.state.scene {
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
        });
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
        ui.horizontal(|ui| {
            ui::pane_header(ui, "Activity", "Closing details keeps operations running");
            if ui::close(ui, "Close details") {
                self.show_feedback = false;
            }
            self.button(ui, Action::Cancel);
            self.button(ui, Action::Resume);
        });
        ui.add(
            egui::Label::new(&self.state.message)
                .wrap()
                .selectable(true),
        );
        ui.columns(2, |columns| {
            ui::group(&mut columns[0], "Jobs");
            if let Some(d) = &self.state.diagnostic {
                columns[0].label(format!(
                    "Unitless diagnostic · {} · {}/{}",
                    d.state, d.completed_units, d.total_units
                ));
                if let Some(error) = &d.error_message {
                    columns[0].label(error);
                }
            }
            if self.state.diagnostic_triangle {
                let (rect, _) =
                    columns[0].allocate_exact_size(egui::vec2(80.0, 40.0), egui::Sense::hover());
                columns[0].painter().add(egui::Shape::convex_polygon(
                    vec![rect.left_bottom(), rect.right_bottom(), rect.center_top()],
                    self.prefs.theme.tokens().accent,
                    egui::Stroke::NONE,
                ));
                columns[0].weak("Checked SPLT · unitless diagnostic, separate from project BREP");
            }
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
            ui::group(&mut columns[1], "Problems");
            egui::ScrollArea::vertical()
                .id_salt("problems")
                .show(&mut columns[1], |ui| {
                    if let Some(problem) = &self.local_problem {
                        ui::problem(ui, "Workbench action unavailable", problem);
                    }
                    for problem in self.state.problems.iter().rev() {
                        ui.push_id(problem, |ui| {
                            ui::problem(ui, "Native operation reported a problem", problem)
                        });
                    }
                    if self.state.problems.is_empty() && self.local_problem.is_none() {
                        ui.weak("No reported problems");
                    }
                });
        });
    }
}
mod analysis;
mod compute;
mod functional_material;
mod interior_policy;
mod layout;
mod library;
mod manufacturing;
mod material;
mod mechanics;
mod navigation;
mod plates;
mod policies;
mod process;
mod review;
mod stock;
mod study;
mod surface;
mod workspace;

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
        self.reconcile_plate(ctx);
        self.reconcile_alternative_receipt();
        self.reconcile_process_receipt();
        self.reconcile_mechanics();
        if self.state.phase == Phase::Running
            && !self.state.busy
            && self.awaiting_serial.is_none()
            && let Some(open) = self.startup_open.take()
            && let Some(host) = &self.host
        {
            match host.open_from_launch(open) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if self.state.phase == Phase::Running
            && !self.state.busy
            && self.library.locator.is_none()
            && let Some(location) = self.prefs.library_location.clone()
        {
            self.library.locator = Some(location.clone());
            self.library.queued = Some(crate::host::LibraryAction::SetLocation(location));
        }
        if let Some(status) = &self.state.library.status {
            self.prefs.library_location = Some(status.location.clone());
            self.library.locator = Some(status.location.clone());
        }
        if let Some(action) = self.library.queued.take()
            && let Some(host) = &self.host
        {
            match host.library(action) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(error) => self.local_problem = Some(error),
            }
        }
        self.image_texture(ctx);
        if let Some(request) = self.queued_image.take()
            && let Some(host) = &mut self.host
        {
            match host.choose_image(frame, ctx, request) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if self.selected.as_ref().is_some_and(|r| {
            self.state
                .scene
                .as_ref()
                .is_none_or(|s| s.session_id != r.session_id || s.revision != r.scene_revision)
        }) {
            self.selected = None;
        }
        if let Some(settings) = self.queued_compute.take()
            && let Some(host) = &self.host
            && let Err(error) = host.compute_settings(settings)
        {
            self.local_problem = Some(error);
            self.queued_compute = Some(settings);
        }
        if self.queued_compute.is_none()
            && let Some(command) = self.queued_host.take()
            && let Some(host) = &self.host
            && let Err(error) = host.command(command)
        {
            self.local_problem = Some(error);
        }
        if let Some(command) = self.queued_geometry.take()
            && let Some(host) = &self.host
        {
            match host.geometry(command) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if let Some(picker) = self.queued_picker.take()
            && let Some(host) = &mut self.host
        {
            match host.choose(frame, ctx, picker) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if let Some(command) = self.queued_manufacturing.take()
            && let Some(host) = &self.host
        {
            match host.manufacturing(command) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if let Some(command) = self.queued_project.take()
            && let Some(host) = &self.host
        {
            match host.project(command) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(error) => self.local_problem = Some(error),
            }
        }
        let title = self
            .state
            .project
            .as_ref()
            .map(|p| {
                format!(
                    "{}{}{} · {} · Spiling",
                    if p.dirty { "● " } else { "" },
                    p.path_label.as_deref().unwrap_or("Unsaved project"),
                    if p.read_only { " [read-only]" } else { "" },
                    p.layouts
                        .iter()
                        .find(|l| l.layout_id == p.layout_id)
                        .map_or("Plate", |l| l.name.as_str())
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
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.opening_frames
            .observe(&ctx, self.state.display.as_ref());
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
            if ui::primary(ui, "Accept and continue", None) {
                self.prefs.accepted_osl3 = true;
                self.start_requested = true;
                ctx.request_repaint();
            }
            return;
        }
        self.library_sync();
        let compact = ui.available_width() < 900.0;
        self.chrome_ui(ui, compact);
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{:?}", self.state.phase));
                if self
                    .state
                    .project
                    .as_ref()
                    .is_some_and(|p| p.save_uncertain)
                {
                    ui.colored_label(self.prefs.theme.tokens().warning, "Save outcome uncertain");
                }
                let problems =
                    self.state.problems.len() + usize::from(self.local_problem.is_some());
                ui.toggle_value(
                    &mut self.show_feedback,
                    format!(
                        "Activity · {} jobs · {problems} problems",
                        self.state.jobs.len()
                    ),
                );
                ui.add(egui::Label::new(&self.state.message).truncate())
                    .on_hover_text(&self.state.message);
            });
        });
        if self.show_feedback {
            egui::Panel::bottom("feedback")
                .resizable(true)
                .default_size(200.0)
                .min_size(100.0)
                .max_size((ui.available_height() * 0.5).max(100.0))
                .show(ui, |ui| self.feedback(ui));
        }
        if self.show_gallery {
            egui::CentralPanel::default_margins().show(ui, |ui| {
                if ui::close(ui, "Back to workbench") {
                    self.show_gallery = false;
                    if let Some(id) = self.surface_focus[3].take() {
                        ctx.memory_mut(|m| m.request_focus(id));
                    }
                }
                ui::gallery(ui, &mut self.prefs.theme);
            });
        } else if self.show_license {
            egui::CentralPanel::default_margins().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("License · OSL-3.0");
                    if ui::close(ui, "Back to workbench") {
                        self.show_license = false;
                        if let Some(id) = self.surface_focus[2].take() {
                            ctx.memory_mut(|m| m.request_focus(id));
                        }
                    }
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(egui::Label::new(LICENSE).wrap().selectable(true));
                });
            });
        } else if self.library.open {
            egui::CentralPanel::default_margins().show(ui, |ui| self.library_page(ui));
        } else if self.work_area.panel == Panel::Study {
            egui::CentralPanel::default_margins().show(ui, |ui| self.study_page(ui));
        } else {
            if self.layout.open
                || self.analysis.open
                || self.work_area.panel == Panel::Mechanics
                || !compact
                || !self.compact_tree
            {
                egui::Panel::right("project")
                    .resizable(true)
                    .default_size(if compact { 250.0 } else { 340.0 })
                    .min_size(210.0)
                    // A pasted numeric draft or long source label must retain a canvas.
                    .max_size((ui.available_width() * 0.5).max(210.0))
                    .show(ui, |ui| {
                        if self.layout.open {
                            self.layout_ui(ui);
                            return;
                        }
                        if self.analysis.open {
                            self.analysis_ui(ui);
                            return;
                        }
                        if self.work_area.panel == Panel::Surface {
                            self.finish_pane(ui);
                            return;
                        }
                        if self.work_area.panel == Panel::Mechanics {
                            self.mechanics_actions(ui);
                            egui::ScrollArea::vertical()
                                .id_salt("mechanics-inspector")
                                .show(ui, |ui| self.mechanics_pane(ui));
                            return;
                        }
                        if matches!(
                            self.work_area.panel,
                            Panel::Prepare | Panel::Review | Panel::Alternative | Panel::Option
                        ) {
                            self.manufacturing_pane(ui);
                            return;
                        }
                        self.properties_pane(ui);
                    });
            }
            if !compact || (self.compact_tree && !self.analysis.open) {
                egui::Panel::left("model-tree")
                    .resizable(true)
                    .default_size(210.0)
                    .min_size(140.0)
                    .show(ui, |ui| self.tree_ui(ui));
            }
            egui::CentralPanel::default_margins()
                .frame(
                    egui::Frame::central_panel(ui.style()).fill(self.prefs.theme.tokens().canvas),
                )
                .show(ui, |ui| {
                    if self.plates.overview {
                        self.plate_overview_ui(ui);
                        return;
                    }
                    if self.state.display_loading {
                        ui.label("Updating source display · previous revision retained");
                        if ui.button("Pause transfer").clicked() {
                            self.queued_host = Some(Command::PauseDisplay);
                        }
                    }
                    if self.state.display_paused {
                        ui.colored_label(
                            self.prefs.theme.tokens().warning,
                            "Source display paused · previous revision retained",
                        );
                        if ui.button("Resume transfer").clicked() {
                            self.queued_host = Some(Command::ResumeDisplay);
                        }
                    }
                    if !self.layout.open && self.work_area.workspace == Workspace::Review {
                        ui::replay_control_area(ui, |ui| self.replay_controls(ui));
                    }
                    let current = self.display_current();
                    if !current && self.state.display.is_some() {
                        ui.colored_label(
                            self.prefs.theme.tokens().warning,
                            "Displayed source is stale · picking and edits disabled",
                        );
                    }
                    if self
                        .state
                        .display
                        .as_ref()
                        .is_none_or(|s| s.occurrences.is_empty())
                    {
                        ui::pane_header(
                            ui,
                            if self.state.project.is_some() {
                                "This plate is empty"
                            } else {
                                "No source geometry"
                            },
                            if self.state.project.is_some() {
                                "Import a part, or move or copy parts from another plate"
                            } else {
                                "Import a STEP part or open a saved project"
                            },
                        );
                        self.primary_button(ui, Action::Import);
                    }
                    let layout_preview = self.layout_preview();
                    let review = if self.layout.open
                        || (self.work_area.workspace != Workspace::Review && !self.analysis.open)
                    {
                        None
                    } else {
                        self.selected_manufacturing_review()
                    };
                    let mut view = if review.is_some() {
                        self.manufacturing.replay
                    } else {
                        crate::replay::View {
                            source: true,
                            visible: false,
                            ..Default::default()
                        }
                    };
                    let plane = self.analysis_plane();
                    view.source = self.source_display_visible();
                    let mut analysis = self.analysis.view.clone();
                    analysis.enabled &= analysis.valid()
                        && (!analysis.local_frame || self.analysis_pose().is_some())
                        && (!analysis.selected_only || self.occurrence.is_some());
                    if analysis.enabled {
                        match self.analysis.view.representation {
                            crate::analysis::Representation::Source => {
                                view.visible = false;
                            }
                            crate::analysis::Representation::Actual => {
                                view.visible = review.is_some();
                            }
                            crate::analysis::Representation::Compare => {
                                view.visible = review.is_some();
                            }
                        }
                    }
                    let mut picked = None;
                    let mechanics = self.mechanics_view();
                    let inspection = self
                        .interface_overlay(self.state.display.clone().as_ref(), review.as_ref());
                    ui.horizontal_top(|ui| {
                        let rail = if review.is_some() { 84.0 } else { 0.0 };
                        let size = egui::vec2(
                            (ui.available_width() - rail).max(1.0),
                            ui.available_height(),
                        );
                        ui.allocate_ui_with_layout(
                            size,
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                picked = self.viewport.show(
                                    ui,
                                    crate::viewport::Input {
                                        mechanics: mechanics.clone(),
                                        inspection: inspection.clone(),
                                        project_id: self
                                            .state
                                            .project
                                            .as_ref()
                                            .map(|p| p.project_id.clone()),
                                        plate: crate::viewport::plate::Plate::current(
                                            self.state
                                                .project
                                                .as_ref()
                                                .and_then(|p| p.hardware.as_ref()),
                                            self.state.manufacturing.intent.as_ref(),
                                        ),
                                        layout_preview: layout_preview.is_some(),
                                        scene: if let Some(preview) = &layout_preview {
                                            Some(preview.clone())
                                        } else if review.is_some() && !current {
                                            None
                                        } else {
                                            self.state.display.clone()
                                        },
                                        section: self.state.section.clone().filter(|s| {
                                            self.analysis.view.enabled
                                                && (DVec3::from_array(s.summary.plane.normal)
                                                    - DVec3::from_array(plane.normal))
                                                .length()
                                                    < 1e-10
                                                && DVec3::from_array(plane.normal)
                                                    .dot(
                                                        DVec3::from_array(
                                                            s.summary.plane.origin_mm,
                                                        ) - DVec3::from_array(plane.origin_mm),
                                                    )
                                                    .abs()
                                                    < 1e-9
                                        }),
                                        review: review.clone(),
                                        view,
                                        selected: if layout_preview.is_some() {
                                            None
                                        } else {
                                            self.selected.as_ref()
                                        },
                                        interactive: current && layout_preview.is_none(),
                                        analysis: &mut analysis,
                                        plane,
                                        scope: self.occurrence,
                                    },
                                    frame,
                                );
                            },
                        );
                        if let Some(review) = &review {
                            self.manufacturing.layers.rail(
                                ui,
                                &mut self.manufacturing.replay,
                                review,
                            );
                        }
                    });
                    if analysis.enabled && self.analysis.view.offset_mm != analysis.offset_mm {
                        self.analysis.follow_layer = false;
                        self.analysis.view.offset_mm = analysis.offset_mm;
                    }
                    if let Some(reference) = picked {
                        self.select_face(reference);
                    }
                });
        }
        if self.show_commands {
            if self.command_focus {
                self.command_return_focus = ctx.memory(|m| m.focused());
            }
            let modal = egui::Modal::new(egui::Id::unique("command-palette")).show(&ctx, |ui| {
                ui.set_width(360.0);
                ui.horizontal(|ui| {
                    ui.heading("Commands");
                    if ui::close(ui, "Close commands") {
                        self.show_commands = false;
                    }
                });
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.command_query)
                        .hint_text("Search commands")
                        .char_limit(128),
                );
                let query = self.command_query.to_lowercase();
                let actions: Vec<_> = Action::ALL
                    .iter()
                    .copied()
                    .filter(|a| a.label().to_lowercase().contains(&query))
                    .collect();
                if self.command_focus || response.changed() {
                    self.command_selection = actions.iter().position(|a| {
                        self.disabled(*a).is_none() && !matches!(a, Action::Close | Action::Restart)
                    });
                }
                if self.command_focus {
                    response.request_focus();
                    self.command_focus = false;
                }
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown))
                    && !actions.is_empty()
                {
                    self.command_selection = Some(
                        self.command_selection
                            .map_or(0, |n| (n + 1).min(actions.len() - 1)),
                    );
                }
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp))
                    && !actions.is_empty()
                {
                    self.command_selection =
                        Some(self.command_selection.unwrap_or(1).saturating_sub(1));
                }
                let enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for (index, action) in actions.iter().copied().enumerate() {
                            let selected = self.command_selection == Some(index);
                            let disabled = self.disabled(action);
                            let response = ui.add_enabled(
                                disabled.is_none(),
                                egui::Button::selectable(selected, action.label()),
                            );
                            if selected {
                                if let Some(reason) = disabled {
                                    ui.label(reason);
                                }
                                if ui.input(|i| {
                                    i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::ArrowUp)
                                }) {
                                    response.scroll_to_me(None);
                                }
                            }
                            if response.clicked() || (enter && selected && disabled.is_none()) {
                                self.queued = Some(action);
                                self.show_commands = false;
                            }
                        }
                        if actions.is_empty() {
                            ui.label("No matching commands");
                        }
                    });
            });
            if modal.should_close() {
                self.show_commands = false;
            }
            if !self.show_commands
                && let Some(id) = self.command_return_focus.take()
            {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
        self.navigation_dialog(&ctx);
        self.compute_dialog(&ctx);
        self.plate_dialog(&ctx);
        if let Some(confirmation) = &self.state.confirmation {
            let id = confirmation.id;
            let modal = egui::Modal::new(egui::Id::unique("discard-project")).show(&ctx,|ui| {
                ui.set_max_width(420.0);
                ui.heading("Discard unsaved changes?");
                ui.label(format!("{} discards the current in-memory changes. Save first to preserve them.",confirmation.action));
                if self.state.project.as_ref().is_some_and(|p| p.save_uncertain) { ui.colored_label(self.prefs.theme.tokens().warning,"The last save may already have reached disk. Inspect it before relying on its outcome."); }
                ui.horizontal(|ui| {
                    if ui::primary(ui, "Keep editing", None) { self.confirmation = Some((id,false)); }
                    if ui.button(RichText::new("Discard changes").color(self.prefs.theme.tokens().warning)).clicked() { self.confirmation = Some((id,true)); }
                });
            });
            if modal.should_close() {
                self.confirmation = Some((id, false));
            }
        }
        if previous_theme != self.prefs.theme {
            self.prefs.theme.apply(&ctx);
        }

        // Side effects execute once in logic(), even if egui lays out the UI in multiple passes.
        if self.queued.is_some()
            || self.queued_compute.is_some()
            || self.confirmation.is_some()
            || self.queued_geometry.is_some()
            || self.queued_host.is_some()
            || self.queued_picker.is_some()
            || self.queued_manufacturing.is_some()
            || self.queued_project.is_some()
            || self.library.queued.is_some()
            || self.queued_image.is_some()
        {
            ctx.request_repaint();
        }
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFS, &self.prefs);
    }
}

#[cfg(test)]
mod compute_preferences_tests {
    use super::*;
    use spiling_contracts::compute::{ComputeBackend, ComputeSettings};

    #[test]
    fn machine_compute_preferences_roundtrip_and_default_to_inheritance() {
        let mut prefs = Preferences::default();
        assert_eq!(prefs.compute, ComputeSettings::default());
        prefs.compute = ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: Some(ComputeBackend::Vulkan),
        };
        let bytes = serde_json::to_vec(&prefs).unwrap();
        let decoded: Preferences = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.compute, prefs.compute);
        let mut legacy_preferences = serde_json::to_value(&prefs).unwrap();
        legacy_preferences
            .as_object_mut()
            .unwrap()
            .remove("compute");
        let decoded: Preferences = serde_json::from_value(legacy_preferences).unwrap();
        assert_eq!(decoded.compute, ComputeSettings::default());
    }
}
