// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Transient workspace/task navigation. No project or operation authority.
use super::*;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Panel {
    #[default]
    Parts,
    Prepare,
    Review,
    Mechanics,
    Surface,
    Study,
    Alternative,
    Option,
}
#[derive(Default, Debug, Clone)]
pub(super) struct WorkArea {
    pub workspace: Workspace,
    pub panel: Panel,
    prepare: Panel,
    review: Option<Panel>,
    study_return: Option<(Workspace, Panel)>,
}
impl WorkArea {
    pub fn home_panel(&self) -> Panel {
        match self.workspace {
            Workspace::Prepare => Panel::Parts,
            Workspace::Review => Panel::Review,
            Workspace::Analyze => Panel::Mechanics,
        }
    }
    pub fn close_task(&mut self) {
        self.panel = self.home_panel();
    }
    pub fn switch(&mut self, workspace: Workspace) {
        if self.panel == Panel::Study {
            self.close_study();
        }
        if self.workspace == workspace {
            return;
        }
        match self.workspace {
            Workspace::Prepare => self.prepare = self.panel,
            Workspace::Review => self.review = Some(self.panel),
            Workspace::Analyze => {}
        }
        self.workspace = workspace;
        self.panel = match workspace {
            Workspace::Prepare => self.prepare,
            Workspace::Review => self.review.unwrap_or(Panel::Review),
            Workspace::Analyze => Panel::Mechanics,
        };
    }
    pub fn open(&mut self, panel: Panel) {
        if panel == Panel::Study {
            if self.panel != Panel::Study {
                self.study_return = Some((self.workspace, self.panel));
            }
        } else {
            self.switch(match panel {
                Panel::Review | Panel::Alternative | Panel::Option => Workspace::Review,
                Panel::Mechanics => Workspace::Analyze,
                _ => Workspace::Prepare,
            });
        }
        self.panel = panel;
    }
    pub fn close_study(&mut self) {
        if let Some((workspace, panel)) = self.study_return.take() {
            self.workspace = workspace;
            self.panel = panel;
        } else {
            self.close_task();
        }
    }
    /// Trials are captured separately and cannot be restored as a fresh plate task.
    pub fn end_trial(&mut self) {
        if self.panel == Panel::Alternative {
            self.panel = Panel::Review;
        }
        if self.review == Some(Panel::Alternative) {
            self.review = Some(Panel::Review);
        }
    }
    pub fn retained(&self) -> Self {
        let mut value = self.clone();
        if matches!(value.panel, Panel::Alternative | Panel::Option) {
            value.panel = Panel::Review;
        }
        if matches!(value.review, Some(Panel::Alternative | Panel::Option)) {
            value.review = Some(Panel::Review);
        }
        if let Some((_, panel)) = &mut value.study_return
            && matches!(panel, Panel::Alternative | Panel::Option)
        {
            *panel = Panel::Review;
        }
        value
    }
}
impl Workbench {
    pub(super) fn show_panel(&mut self, panel: Panel) {
        self.work_area.open(panel);
    }
    pub(super) fn close_study(&mut self, ctx: &egui::Context) {
        self.work_area.close_study();
        if let Some(id) = self.surface_focus[4].take() {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }
    fn select_workspace(&mut self, workspace: Workspace, ctx: &egui::Context) {
        self.surface_focus[4] = None;
        self.close_library(ctx);
        self.close_layout(ctx);
        self.close_plate_overview(ctx);
        self.show_gallery = false;
        self.show_license = false;
        self.work_area.switch(workspace);
    }
    pub(super) fn task_action(&mut self, action: Action, ctx: &egui::Context) -> bool {
        match action {
            Action::PrepareWorkspace => self.select_workspace(Workspace::Prepare, ctx),
            Action::ReviewWorkspace => self.select_workspace(Workspace::Review, ctx),
            Action::AnalyzeWorkspace => self.select_workspace(Workspace::Analyze, ctx),
            Action::Properties
            | Action::Printer
            | Action::Material
            | Action::Settings
            | Action::Placement
            | Action::Infill
            | Action::Surface
            | Action::Study
            | Action::ReviewLayers => {
                self.show_gallery = false;
                self.show_license = false;
                if self.work_area.panel == Panel::Study && action != Action::Study {
                    self.work_area.close_study();
                    self.surface_focus[4] = None;
                }
                self.close_library(ctx);
                self.close_layout(ctx);
                self.close_analysis(ctx);
                self.close_plate_overview(ctx);
                match action {
                    Action::Properties => self.work_area.panel = Panel::Parts,
                    Action::Printer => self.show_prepare(0),
                    Action::Material => self.show_prepare(1),
                    Action::Settings => self.show_prepare(2),
                    Action::Placement => self.show_prepare(3),
                    Action::Infill => self.show_prepare(4),
                    Action::Surface => self.show_panel(Panel::Surface),
                    Action::Study => {
                        if self.work_area.panel != Panel::Study {
                            self.surface_focus[4] = ctx.memory(|m| m.focused());
                        }
                        self.show_panel(Panel::Study);
                    }
                    Action::ReviewLayers => self.show_panel(Panel::Review),
                    _ => unreachable!(),
                }
            }
            Action::Compare => {
                self.select_workspace(Workspace::Review, ctx);
                self.close_analysis(ctx);
                self.begin_alternative(None);
            }
            _ => return false,
        }
        true
    }
    fn active_task(&self) -> Option<Action> {
        if self.layout.open {
            return Some(Action::Arrange);
        }
        if self.analysis.open {
            return Some(Action::Analysis);
        }
        Some(match self.work_area.panel {
            Panel::Parts => Action::Properties,
            Panel::Prepare => match self.manufacturing.prepare_section {
                0 => Action::Printer,
                1 => Action::Material,
                2 => Action::Settings,
                3 => Action::Placement,
                _ => Action::Infill,
            },
            Panel::Surface => Action::Surface,
            Panel::Study => Action::Study,
            Panel::Review => Action::ReviewLayers,
            Panel::Mechanics => Action::AnalyzeWorkspace,
            Panel::Alternative | Panel::Option => Action::Compare,
        })
    }
    pub(super) fn chrome_ui(&mut self, ui: &mut egui::Ui, compact: bool) {
        egui::Panel::top("commands").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("Project", |ui| {
                    for action in [
                        Action::New,
                        Action::Open,
                        Action::OpenReadOnly,
                        Action::Recover,
                        Action::Save,
                        Action::SaveAs,
                        Action::Import,
                    ] {
                        self.button(ui, action);
                    }
                    ui.menu_button("Model import options", |ui| {
                        egui::ComboBox::from_id_salt("import_stl_units")
                            .selected_text(match self.import_options.stl_unit {
                                None | Some(SourceUnit::Millimetre) => "STL · mm",
                                Some(SourceUnit::Inch) => "STL · inch",
                                Some(SourceUnit::Metre) => "STL · m",
                                _ => "STL units",
                            })
                            .show_ui(ui, |ui| {
                                for (unit, label) in [
                                    (None, "Millimetres (default)"),
                                    (Some(SourceUnit::Inch), "Inches"),
                                    (Some(SourceUnit::Metre), "Metres"),
                                ] {
                                    ui.selectable_value(
                                        &mut self.import_options.stl_unit,
                                        unit,
                                        label,
                                    );
                                }
                            });
                        let mut selected = self.import_options.source_plate.is_some();
                        if ui.checkbox(&mut selected, "Select 3MF plate").changed() {
                            self.import_options.source_plate = selected.then_some(1);
                        }
                        if let Some(plate) = &mut self.import_options.source_plate {
                            ui.horizontal(|ui| {
                                ui.label("Source plate");
                                ui.add(egui::DragValue::new(plate).range(1..=u32::MAX));
                            });
                        }
                        self.button(ui, Action::Import);
                    });
                    ui.separator();
                    self.button(ui, Action::Library);
                    ui.menu_button("Import profiles", |ui| {
                        for action in [
                            Action::LoadIntent,
                            Action::LoadMaterial,
                            Action::LoadMaterialCatalog,
                        ] {
                            self.button(ui, action);
                        }
                    });
                    ui.separator();
                    self.button(ui, Action::Close);
                });
                ui.menu_button("Edit", |ui| {
                    for action in [Action::Undo, Action::Redo] {
                        self.button(ui, action);
                    }
                    ui.separator();
                    if ui.button("Compute settings…").clicked() {
                        self.show_compute = true;
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    self.viewport.navigation_menu(ui);
                    ui.separator();
                    for action in [
                        Action::Properties,
                        Action::FocusSelection,
                        Action::RevealSelection,
                        Action::Analysis,
                        Action::Study,
                    ] {
                        self.button(ui, action);
                    }
                    if ui.button("Activity").clicked() {
                        self.show_feedback = true;
                        ui.close();
                    }
                    if compact {
                        ui.checkbox(&mut self.compact_tree, "Models");
                    }
                    ui.separator();
                    ui::theme_choice(ui, &mut self.prefs.theme);
                });
                ui.menu_button("Help", |ui| {
                    self.button(ui, Action::License);
                    self.button(ui, Action::Gallery);
                    ui.menu_button("Diagnostics", |ui| {
                        for action in [Action::Restart, Action::Cancel, Action::Resume] {
                            self.button(ui, action);
                        }
                        if ui
                            .add_enabled(
                                self.state.phase == Phase::Running && !self.state.busy,
                                egui::Button::new("Run unitless triangle diagnostic"),
                            )
                            .clicked()
                        {
                            self.queued_host = Some(Command::Diagnostic);
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Find command…")
                        .on_hover_text("Ctrl/Cmd+K")
                        .clicked()
                    {
                        self.show_commands = true;
                        self.command_focus = true;
                        self.command_query.clear();
                    }
                    if ui.available_width() > 90.0 {
                        let label = self
                            .state
                            .project
                            .as_ref()
                            .and_then(|p| p.path_label.as_deref())
                            .and_then(|s| s.rsplit(['/', '\\']).next())
                            .unwrap_or("Untitled");
                        let dirty = self.state.project.as_ref().is_some_and(|p| p.dirty);
                        ui.add(
                            egui::Label::new(format!(
                                "{label}{}",
                                if dirty { " · Unsaved" } else { "" }
                            ))
                            .truncate(),
                        );
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if compact {
                    ui.toggle_value(&mut self.compact_tree, "Models");
                    ui.separator();
                }
                if let Some(action) = ui::workspace_tabs(ui, self.work_area.workspace) {
                    self.queued = Some(action);
                }
            });
            if let Some(action) =
                ui::workspace_commands(ui, self.work_area.workspace, self.active_task(), |a| {
                    self.disabled(a)
                })
            {
                self.queued = Some(action);
            }
        });
    }
    pub(super) fn properties_pane(&mut self, ui: &mut egui::Ui) {
        if self.work_area.workspace == Workspace::Analyze && ui::close(ui, "Back to analysis case")
        {
            self.show_panel(Panel::Mechanics);
        }
        if self.work_area.workspace == Workspace::Review && ui::close(ui, "Back to layers") {
            self.show_panel(Panel::Review);
        }
        if self.work_area.workspace == Workspace::Prepare
            && self
                .state
                .scene
                .as_ref()
                .is_some_and(|s| s.occurrence_count > 0)
        {
            if self.project_drafts() || self.subject_drafts() {
                ui.weak("Unapplied changes are retained in their editor");
            } else if self.state.manufacturing.intent.is_some() {
                self.primary_button(
                    ui,
                    if self.disabled(Action::Compile).is_none() {
                        Action::Compile
                    } else {
                        Action::Preflight
                    },
                );
            } else if ui::primary(ui, "Choose printer", self.disabled(Action::Printer)) {
                self.queued = Some(Action::Printer);
            }
            ui.separator();
        }
        egui::ScrollArea::vertical()
            .id_salt("properties-body")
            .show(ui, |ui| {
                self.selection_ui(ui);
                self.project(ui);
            });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn analyze_navigation_retains_other_tasks_and_section_inspection_is_not_a_case() {
        let mut area = WorkArea::default();
        area.open(Panel::Surface);
        area.switch(Workspace::Analyze);
        assert_eq!(area.panel, Panel::Mechanics);
        area.panel = Panel::Parts;
        area.close_task();
        assert_eq!(area.panel, Panel::Mechanics);
        area.switch(Workspace::Prepare);
        assert_eq!(area.panel, Panel::Surface);
        area.switch(Workspace::Analyze);
        area.open(Panel::Study);
        area.close_study();
        assert_eq!(
            (area.workspace, area.panel),
            (Workspace::Analyze, Panel::Mechanics)
        );
    }
    #[test]
    fn completed_adoption_does_not_steal_preparation_and_plate_return_has_no_orphan_trial() {
        let mut area = WorkArea::default();
        area.open(Panel::Alternative);
        area.switch(Workspace::Prepare);
        area.open(Panel::Surface);
        area.end_trial();
        assert_eq!(
            (area.workspace, area.panel),
            (Workspace::Prepare, Panel::Surface)
        );
        area.switch(Workspace::Review);
        assert_eq!(area.panel, Panel::Review);
        area.open(Panel::Option);
        area.open(Panel::Study);
        let mut restored = area.retained();
        restored.close_study();
        assert_eq!(restored.panel, Panel::Review);
    }
    #[test]
    fn phase_navigation_preserves_tasks_and_study_returns_to_its_invoker() {
        let mut area = WorkArea::default();
        area.open(Panel::Surface);
        area.switch(Workspace::Review);
        assert_eq!(area.panel, Panel::Review);
        area.panel = Panel::Parts;
        area.open(Panel::Study);
        area.close_study();
        assert_eq!(
            (area.workspace, area.panel),
            (Workspace::Review, Panel::Parts)
        );
        area.switch(Workspace::Prepare);
        assert_eq!(area.panel, Panel::Surface);
        area.open(Panel::Study);
        area.switch(Workspace::Review);
        assert_eq!(area.panel, Panel::Parts);
        area.switch(Workspace::Prepare);
        assert_eq!(area.panel, Panel::Surface);
    }
}
