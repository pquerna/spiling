// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Plate navigation and explicit job edits; membership and coordinates belong to Core.
use super::*;
use spiling_contracts::project::{PrintLayoutEdit, PrintLayoutId, ProjectCommand, ProjectId};

#[derive(Default)]
pub(super) struct PlatesUi {
    project: Option<ProjectId>,
    current: Option<PrintLayoutId>,
    views: Vec<PlateView>,
    sheet: Option<Sheet>,
    focus: Option<egui::Id>,
    pub overview: bool,
    overview_focus: Option<egui::Id>,
}
struct PlateView {
    layout: PrintLayoutId,
    camera: crate::viewport::CameraState,
    section: crate::analysis::SectionView,
    replay: crate::replay::View,
    mode: crate::layers::Mode,
    panel: usize,
    prepare_section: u8,
    record: Option<SourceHash>,
    occurrence: Option<OccurrenceId>,
}
struct Sheet {
    revision: spiling_contracts::project::ProjectRevision,
    source: PrintLayoutId,
    mode: Mode,
    name: String,
    target: Option<PrintLayoutId>,
    parts: Vec<OccurrenceId>,
    submitted: Option<(u64, u64, PrintLayoutEdit)>,
    error: Option<String>,
}
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Add,
    Rename,
    Duplicate,
    Remove,
    Move,
    Copy,
}
impl Mode {
    fn title(self) -> &'static str {
        match self {
            Self::Add => "Add plate",
            Self::Rename => "Rename plate",
            Self::Duplicate => "Duplicate plate",
            Self::Remove => "Remove plate",
            Self::Move => "Move parts",
            Self::Copy => "Copy parts",
        }
    }
}
impl Workbench {
    pub(super) fn plate_sheet_open(&self) -> bool {
        self.plates.sheet.is_some()
    }
    pub(super) fn reconcile_plate(&mut self, ctx: &egui::Context) {
        let Some(p) = self.state.project.clone() else {
            return;
        };
        if self.plates.project.as_ref() != Some(&p.project_id) {
            self.plates = PlatesUi {
                project: Some(p.project_id.clone()),
                ..Default::default()
            };
        }
        if let Some(sheet) = &mut self.plates.sheet
            && let Some((nonce, serial, edit)) = &sheet.submitted
        {
            let receipt = self.state.plate_edit.as_ref().filter(|r| {
                r.nonce > *nonce
                    && r.project == p.project_id
                    && r.base_revision == sheet.revision
                    && &r.edit == edit
                    && self
                        .state
                        .hello
                        .as_ref()
                        .is_some_and(|h| h.session_id == r.session)
                    && r.revision == p.revision
            });
            if receipt.is_some() {
                self.plates.sheet = None;
            } else if self.state.serial > *serial
                && self.queued_project.is_none()
                && !self.state.busy
                && self.awaiting_serial.is_none()
            {
                sheet.submitted = None;
                sheet.error = Some(
                    self.state
                        .problems
                        .back()
                        .cloned()
                        .unwrap_or_else(|| self.state.message.clone()),
                );
            }
        }
        if self.plates.sheet.is_none()
            && let Some(focus) = self.plates.focus.take()
        {
            ctx.memory_mut(|m| m.request_focus(focus));
        }
        if self.plates.current.as_ref() == Some(&p.layout_id) {
            return;
        }
        if let Some(old) = self.plates.current.take() {
            self.plates.views.retain(|view| {
                view.layout != old && p.layouts.iter().any(|l| l.layout_id == view.layout)
            });
            if p.layouts.iter().any(|l| l.layout_id == old) {
                self.plates.views.push(PlateView {
                    layout: old,
                    camera: self.viewport.camera(),
                    section: self.analysis.view.clone(),
                    replay: self.manufacturing.replay,
                    mode: self.manufacturing.layers.mode,
                    panel: if self.manufacturing.panel == 5 {
                        2
                    } else {
                        self.manufacturing.panel
                    },
                    prepare_section: self.manufacturing.prepare_section,
                    record: self.manufacturing.replay_record.clone(),
                    occurrence: self.occurrence,
                });
            }
        }
        self.plates.current = Some(p.layout_id.clone());
        self.selected = None;
        self.occurrence = None;
        self.pose_revision = None;
        self.pose = [0.; 6];
        self.pose_original = [0.; 6];
        self.surface.discard();
        self.process.discard();
        self.layout = Default::default();
        self.manufacturing = Default::default();
        self.material = Default::default();
        self.analysis = Default::default();
        self.analysis.project = Some(p.project_id.clone());
        if let Some(view) = self
            .plates
            .views
            .iter()
            .find(|view| view.layout == p.layout_id)
        {
            self.viewport.restore_camera(&view.camera);
            self.analysis.view = view.section.clone();
            self.manufacturing.replay = view.replay;
            self.manufacturing.layers.mode = view.mode;
            self.manufacturing.panel = view.panel;
            self.manufacturing.prepare_section = view.prepare_section;
            self.manufacturing.replay_record = view.record.clone();
            self.occurrence = view.occurrence.filter(|id| {
                self.state.layout_overview.as_ref().is_some_and(|o| {
                    o.layouts.iter().any(|l| {
                        l.layout_id == p.layout_id
                            && l.parts.iter().any(|part| part.occurrence_id == *id)
                    })
                })
            });
        } else {
            self.viewport.fit();
        }
    }

    pub(super) fn close_plate_overview(&mut self, ctx: &egui::Context) {
        self.plates.overview = false;
        if let Some(id) = self.plates.overview_focus.take() {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }
    pub(super) fn plate_overview_ui(&mut self, ui: &mut egui::Ui) {
        ui::pane_header(
            ui,
            "Plates",
            "Source geometry · three-quarter view · shared scale",
        );
        if ui::close(ui, "Back to plate") {
            self.close_plate_overview(ui.ctx());
            return;
        }
        ui::help(
            ui,
            "plate-overview-help",
            "Plate overview",
            "Previews show source geometry on the declared build area at a common scale and angle. They exclude additive finishes, supports and clearance. Focus a plate for geometry and manufacturing inspection. Drag a part from the model tree to another plate to review a transfer.",
        );
        let (Some(p), Some(overview)) = (
            self.state.project.clone(),
            self.state.layout_overview.clone(),
        ) else {
            ui.weak("Waiting for plate context");
            return;
        };
        if overview.project_id != p.project_id || overview.project_revision != p.revision {
            ui.weak("Plate context is stale");
            return;
        }
        let idle = self.state.phase == Phase::Running
            && !self.state.busy
            && self.awaiting_serial.is_none();
        let display = self
            .state
            .display
            .clone()
            .filter(|s| s.context.same_geometry(&overview));
        let tokens = self.prefs.theme.tokens();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(event) = ui::plate_overview(
                ui,
                &p.layouts,
                &overview,
                &p.layout_id,
                idle,
                tokens,
                |ui, rect, index, active| {
                    crate::viewport::overview::paint(
                        ui,
                        rect,
                        display.as_ref(),
                        index,
                        active,
                        tokens,
                    )
                },
            ) {
                self.plate_event(ui, event);
            }
        });
    }
    fn plate_event(&mut self, ui: &egui::Ui, event: ui::PlateEvent) {
        match event {
            ui::PlateEvent::Focus(id) => {
                self.close_plate_overview(ui.ctx());
                self.select_plate(id);
            }
            ui::PlateEvent::Drop { target, part } => {
                let valid = self.state.project.as_ref().is_some_and(|p| {
                    !p.read_only
                        && p.project_id == part.project
                        && p.revision == part.revision
                        && p.layout_id == part.source
                        && p.layouts.iter().any(|l| l.layout_id == target)
                }) && self
                    .state
                    .scene
                    .as_ref()
                    .is_some_and(|s| s.session_id == part.session && s.revision == part.scene)
                    && self.state.display.as_ref().is_some_and(|s| {
                        s.occurrences.iter().any(|o| o.occurrence_id == part.part)
                    });
                if !valid || self.project_drafts() || self.subject_drafts() || self.state.busy {
                    self.local_problem = Some(
                        "Apply or discard drafts, then drag a current part to another plate."
                            .into(),
                    );
                    return;
                }
                self.plate_sheet(ui, Mode::Move);
                if let Some(sheet) = &mut self.plates.sheet {
                    sheet.target = Some(target);
                    sheet.parts = vec![part.part];
                }
            }
        }
    }
    pub(super) fn select_plate(&mut self, id: PrintLayoutId) {
        if self.project_drafts() || self.subject_drafts() {
            self.navigation = Some(super::navigation::Navigation::Plate(id));
            return;
        }
        if let (Some(p), Some(h)) = (&self.state.project, &self.state.hello) {
            self.queued_project = Some(ProjectCommand::SelectPrintLayout {
                session_id: h.session_id.clone(),
                base_revision: p.revision,
                layout_id: id,
            });
        }
    }
    fn plate_sheet(&mut self, ui: &egui::Ui, mode: Mode) {
        let Some(p) = &self.state.project else { return };
        let name = p
            .layouts
            .iter()
            .find(|l| l.layout_id == p.layout_id)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        self.plates.focus = ui.ctx().memory(|m| m.focused());
        self.plates.sheet = Some(Sheet {
            revision: p.revision,
            source: p.layout_id.clone(),
            mode,
            name: match mode {
                Mode::Add => format!("Plate {}", p.layouts.len() + 1),
                Mode::Duplicate => format!("{name} copy"),
                _ => name,
            },
            target: p
                .layouts
                .iter()
                .find(|l| l.layout_id != p.layout_id)
                .map(|l| l.layout_id.clone()),
            parts: self.occurrence.into_iter().collect(),
            submitted: None,
            error: None,
        });
    }
    pub(super) fn plates_ui(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.state.project.clone() else {
            return;
        };
        ui::pane_header(ui, "Plates", "Independent print jobs");
        let idle = self.state.phase == Phase::Running
            && !self.state.busy
            && self.awaiting_serial.is_none();
        if let Some(event) = ui::plate_navigator(ui, &p.layouts, &p.layout_id, idle) {
            self.plate_event(ui, event);
        }
        let overview = ui.selectable_label(self.plates.overview, "Overview");
        if overview.clicked() {
            if self.plates.overview {
                self.close_plate_overview(ui.ctx());
            } else {
                self.plates.overview = true;
                self.plates.overview_focus = Some(overview.id);
            }
        }
        let reason = if !idle || p.read_only {
            Some("Open a writable project and wait for current work")
        } else if self.project_drafts() || self.subject_drafts() {
            Some("Apply or discard drafts before editing plates")
        } else {
            None
        };
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    reason.is_none() && p.layouts.len() < 16,
                    egui::Button::new("Add plate"),
                )
                .on_disabled_hover_text(reason.unwrap_or("A project supports up to 16 plates"))
                .clicked()
            {
                self.plate_sheet(ui, Mode::Add);
            }
            ui.add_enabled_ui(reason.is_none(), |ui| {
                ui.menu_button("Plate actions", |ui| {
                    for mode in [
                        Mode::Rename,
                        Mode::Duplicate,
                        Mode::Move,
                        Mode::Copy,
                        Mode::Remove,
                    ] {
                        let enabled = match mode {
                            Mode::Move | Mode::Copy => p.layouts.len() > 1,
                            Mode::Remove => p.layouts.len() > 1,
                            _ => true,
                        };
                        if ui
                            .add_enabled(enabled, egui::Button::new(mode.title()))
                            .clicked()
                        {
                            self.plate_sheet(ui, mode);
                            ui.close();
                        }
                    }
                    let i = p
                        .layouts
                        .iter()
                        .position(|l| l.layout_id == p.layout_id)
                        .expect("current plate");
                    for (label, to) in [
                        ("Move up", i.checked_sub(1)),
                        ("Move down", (i + 1 < p.layouts.len()).then_some(i + 1)),
                    ] {
                        if ui
                            .add_enabled(to.is_some(), egui::Button::new(label))
                            .clicked()
                        {
                            let mut ids = p
                                .layouts
                                .iter()
                                .map(|l| l.layout_id.clone())
                                .collect::<Vec<_>>();
                            ids.swap(i, to.unwrap());
                            self.queued_project = Some(ProjectCommand::EditPrintLayout {
                                session_id: self.state.hello.as_ref().unwrap().session_id.clone(),
                                base_revision: p.revision,
                                edit: PrintLayoutEdit::Reorder { layout_ids: ids },
                            });
                            ui.close();
                        }
                    }
                })
            });
        });
        ui.separator();
    }
    pub(super) fn plate_dialog(&mut self, ctx: &egui::Context) {
        let (Some(p), Some(hello)) = (self.state.project.clone(), self.state.hello.clone()) else {
            return;
        };
        let Some(sheet) = &mut self.plates.sheet else {
            return;
        };
        let mut accept = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::unique("plate-edit")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(sheet.mode.title());
            let source = p.layouts.iter().find(|l| l.layout_id == sheet.source);
            ui.weak(source.map_or("Source plate unavailable", |l| l.name.as_str()));
            ui.add_enabled_ui(sheet.submitted.is_none(), |ui| match sheet.mode {
                Mode::Add | Mode::Rename | Mode::Duplicate => {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut sheet.name)
                            .char_limit(80)
                            .hint_text("Plate name"),
                    );
                    if sheet.mode == Mode::Add {
                        ui.weak("Uses the selected plate's setup and settings");
                    }
                }
                Mode::Remove => {
                    ui.label(format!(
                        "Remove this plate and its {} parts? Undo restores them.",
                        source.map_or(0, |l| l.occurrence_count)
                    ));
                }
                Mode::Move | Mode::Copy => {
                    ui.label("Destination");
                    let target = p
                        .layouts
                        .iter()
                        .find(|l| Some(&l.layout_id) == sheet.target.as_ref());
                    egui::ComboBox::from_id_salt("plate-target")
                        .selected_text(target.map_or("Choose a plate", |l| l.name.as_str()))
                        .show_ui(ui, |ui| {
                            for layout in &p.layouts {
                                if layout.layout_id != sheet.source {
                                    ui.selectable_value(
                                        &mut sheet.target,
                                        Some(layout.layout_id.clone()),
                                        &layout.name,
                                    );
                                }
                            }
                        });
                    ui::group(ui, "Parts");
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            if let Some(scene) = &self.state.display {
                                for part in &scene.occurrences {
                                    let mut checked = sheet.parts.contains(&part.occurrence_id);
                                    let name =
                                        scene.definition(&part.definition_id).map_or("Part", |d| {
                                            d.record.provenance.source_name.as_str()
                                        });
                                    if ui
                                        .checkbox(
                                            &mut checked,
                                            format!("{} · {}", name, part.occurrence_id.get()),
                                        )
                                        .changed()
                                    {
                                        if checked {
                                            sheet.parts.push(part.occurrence_id);
                                        } else {
                                            sheet.parts.retain(|id| id != &part.occurrence_id);
                                        }
                                    }
                                }
                            }
                        });
                    ui.weak("Position is preserved. Arrange on the destination before preparing.");
                }
            });
            let reason = if sheet.submitted.is_some() {
                Some("Waiting for the engine receipt")
            } else if p.revision != sheet.revision || p.layout_id != sheet.source {
                Some("Project changed. Cancel and reopen this action.")
            } else if self.state.busy {
                Some("Wait for current work")
            } else if matches!(sheet.mode, Mode::Move | Mode::Copy)
                && (sheet.target.is_none() || sheet.parts.is_empty())
            {
                Some("Choose parts and a destination")
            } else if matches!(sheet.mode, Mode::Add | Mode::Rename | Mode::Duplicate)
                && spiling_contracts::project::validate_layout_name(&sheet.name).is_err()
            {
                Some("Enter a name of 1 to 80 bytes")
            } else {
                None
            };
            if let Some(error) = &sheet.error {
                ui::problem(ui, "Plate action was not applied", error);
            }
            accept = ui::primary(ui, sheet.mode.title(), reason);
            cancel = ui
                .add_enabled(sheet.submitted.is_none(), egui::Button::new("Cancel"))
                .clicked();
        });
        if accept {
            let s = self.plates.sheet.as_mut().unwrap();
            let edit = match s.mode {
                Mode::Add => PrintLayoutEdit::Create {
                    name: s.name.clone(),
                    copy_setup_from: Some(s.source.clone()),
                },
                Mode::Rename => PrintLayoutEdit::Rename {
                    layout_id: s.source.clone(),
                    name: s.name.clone(),
                },
                Mode::Duplicate => PrintLayoutEdit::Duplicate {
                    source: s.source.clone(),
                    name: s.name.clone(),
                },
                Mode::Remove => PrintLayoutEdit::Remove {
                    layout_id: s.source.clone(),
                },
                Mode::Move | Mode::Copy => PrintLayoutEdit::Transfer {
                    source: s.source.clone(),
                    target: s.target.clone().unwrap(),
                    occurrences: s.parts.clone(),
                    copy: s.mode == Mode::Copy,
                },
            };
            s.submitted = Some((
                self.state.plate_edit.as_ref().map_or(0, |r| r.nonce),
                self.state.serial,
                edit.clone(),
            ));
            s.error = None;
            self.queued_project = Some(ProjectCommand::EditPrintLayout {
                session_id: hello.session_id.clone(),
                base_revision: s.revision,
                edit,
            });
        } else if cancel
            || (modal.should_close()
                && self
                    .plates
                    .sheet
                    .as_ref()
                    .is_some_and(|s| s.submitted.is_none()))
        {
            self.plates.sheet = None;
        }
        if self.plates.sheet.is_none()
            && let Some(focus) = self.plates.focus.take()
        {
            ctx.memory_mut(|m| m.request_focus(focus));
        }
    }
}
