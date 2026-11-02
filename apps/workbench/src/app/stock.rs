// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::{
    appearance::AppearanceColor,
    optical::{OpticalPatch, OpticalTarget},
    project::ProjectCommand,
    stock::*,
};
pub(super) struct StockUi {
    pub matching: bool,
    target: String,
    scope: StockScope,
    on_hand: bool,
    width: f64,
    height: f64,
    z: f64,
    slot: f64,
    tolerance: f64,
    budget: u32,
    selection: Option<StockChoice>,
    reviewing: bool,
    seen: Option<SourceHash>,
    name: String,
}
impl Default for StockUi {
    fn default() -> Self {
        Self {
            matching: true,
            target: "#808080".into(),
            scope: StockScope::Both,
            on_hand: false,
            width: 10.0,
            height: 10.0,
            z: 0.0,
            slot: 0.2,
            tolerance: 2.0,
            budget: 65536,
            selection: None,
            reviewing: false,
            seen: None,
            name: "Target color recipe".into(),
        }
    }
}
impl StockUi {
    fn query(&self) -> Result<StockQuery, String> {
        let rgb = spiling_contracts::color_naming::parse_srgb8_hex(&self.target)?;
        let query = StockQuery {
            scope: self.scope,
            on_hand_only: self.on_hand,
            target: OpticalTarget {
                color: AppearanceColor::Srgb {
                    channels: rgb.map(|v| f64::from(v) / 255.0),
                },
                delta_e00_tolerance: self.tolerance,
                lightness_tolerance: None,
            },
            patch: OpticalPatch {
                origin_mm: [0.0, 0.0, self.z],
                normal_xy: [0.0, -1.0],
                width_mm: self.width,
                height_mm: self.height,
            },
            slot_height_mm: self.slot,
            max_evaluations: self.budget,
        };
        query.validate()?;
        Ok(query)
    }
}
impl StockUi {
    fn form(&mut self, ui: &mut egui::Ui) {
        ui::stock_target(ui, &mut self.target);
        ui.horizontal_wrapped(|ui| {
            ui.label("Search in");
            egui::ComboBox::from_id_salt("stock-scope")
                .selected_text(match self.scope {
                    StockScope::Project => "Project",
                    StockScope::MyLibrary => "My library",
                    StockScope::Both => "Project + My library",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.scope, StockScope::Project, "Project");
                    ui.selectable_value(&mut self.scope, StockScope::MyLibrary, "My library");
                    ui.selectable_value(&mut self.scope, StockScope::Both, "Project + My library");
                });
            ui.checkbox(&mut self.on_hand, "On hand only");
        });
        ui.collapsing("Study settings", |ui| {
            ui::number(ui, "Slot height (mm)", &mut self.slot, 0.01);
            ui::number(ui, "Patch width (mm)", &mut self.width, 1.0);
            ui::number(ui, "Patch height (mm)", &mut self.height, 1.0);
            ui::number(ui, "Patch Z (mm)", &mut self.z, 0.1);
            ui::number(ui, "Color goal (ΔE)", &mut self.tolerance, 0.1);
            ui.horizontal(|ui| {
                ui.label("Evaluation limit");
                ui.add(egui::DragValue::new(&mut self.budget).range(1..=65536));
            });
            ui.weak("Two runs of 1–4 equal-height slots, either material first.");
        });
        ui::help(
            ui,
            "stock-estimates",
            "Reference estimates",
            "Single colors use declared D65 color differences. Layered recipes use the existing Prusa v7 RGB reference model over this finite patch. HueForge TD is retained but not modelled here. Backing optics, physical calibration and machine readiness are unqualified. Missing process profiles do not block comparison; manufacturing independently admits actual use. Outside-goal results remain selectable.",
        );
    }
    fn details(&mut self, ui: &mut egui::Ui, report: &StockMatchReport, editable: bool) {
        let Some(choice) = self.selection else {
            return;
        };
        super::review::target(ui, report.query.target.color);
        let indices = match choice {
            StockChoice::Single(i) => {
                let row = &report.singles[i];
                ui.label(format!("Estimated difference: ΔE {:.2}", row.delta_e00));
                if !row.within_tolerance {
                    ui.label("Outside color goal");
                }
                vec![row.candidate]
            }
            StockChoice::Pair(i) => {
                let row = &report.pairs[i];
                ui.add_enabled_ui(editable, |ui| ui::text(ui, "Recipe name", &mut self.name));
                super::review::assessment(ui, "Layered estimate", &row.assessment);
                ui.label(format!(
                    "{} equal-height slots · {:.3} mm each",
                    row.cycle.slots.len(),
                    report.query.slot_height_mm
                ));
                ui.collapsing("Physical cycle", |ui| {
                    for slot in &row.cycle.slots {
                        let candidate = &report.candidates
                            [usize::from(row.candidates[usize::from(slot.material)])];
                        ui.label(format!("{} · {:.3} mm", candidate.name, slot.height_mm));
                    }
                    ui.label(format!(
                        "Patch {:.3} × {:.3} mm · Z {:.3} mm",
                        report.query.patch.width_mm,
                        report.query.patch.height_mm,
                        report.query.patch.origin_mm[2]
                    ));
                });
                row.candidates.to_vec()
            }
        };
        ui::group(ui, "Ingredients");
        for index in indices {
            let c = &report.candidates[usize::from(index)];
            ui.strong(&c.name);
            if let Some(appearance) = &c.appearance {
                ui.weak(match &appearance.source {
                    spiling_contracts::library::DefinitionSource::Operator => {
                        "Operator-declared color".into()
                    }
                    spiling_contracts::library::DefinitionSource::Imported { name, .. } => {
                        format!("Declared color · {name}")
                    }
                });
            }
            ui.weak(if c.profile_count == 0 {
                "Process profile missing"
            } else {
                "Process review required before use"
            });
            ui.collapsing(format!("Source details · {}", c.name), |ui| {
                ui.label(format!(
                    "{} · revision {}",
                    c.filament.id.0,
                    c.filament.revision.as_str()
                ));
                if report.query.scope == StockScope::Project && !report.query.on_hand_only {
                    ui.label("Physical stock not queried");
                } else if report.library.is_none() {
                    ui.label("Physical stock not recorded");
                } else {
                    ui.label(format!("{} physical spools on hand", c.on_hand_spools));
                }
                if let Some(spiling_contracts::library::DeclaredAppearance {
                    source: spiling_contracts::library::DefinitionSource::Imported { sha256, .. },
                    ..
                }) = &c.appearance
                {
                    ui.label(format!("Color source {}", sha256.as_str()));
                }
                ui.label(c.hueforge_td_mm.map_or("HueForge TD unknown".into(), |v| {
                    format!("HueForge TD {v} mm · retained declaration")
                }));
            });
        }
        ui.weak("Keeping a result does not assign tools or apply a manufacturing process.");
    }
}

impl Workbench {
    pub(super) fn stock_page(&mut self, ui: &mut egui::Ui) {
        ui::pane_header(
            ui,
            if self.stock.reviewing {
                if matches!(self.stock.selection, Some(StockChoice::Pair(_))) {
                    "Review recipe"
                } else {
                    "Review filament"
                }
            } else {
                "Find useful materials"
            },
            if self.stock.reviewing {
                "Exact declarations · Project capture"
            } else {
                "Saved stock · reference color estimates"
            },
        );
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.awaiting_serial.is_none()
            && !self.state.stock.loading;
        let project = self.state.project.clone();
        let capture = self.state.stock.capture.clone();
        if capture
            .as_ref()
            .is_some_and(|c| self.stock.seen.as_ref() != Some(&c.record.hash))
        {
            self.stock.seen = capture.as_ref().map(|c| c.record.hash.clone());
            self.stock.selection = None;
            self.stock.reviewing = false;
        }
        // The action strip is allocated before the scrolling form/results body.
        // Effects are admitted later by App::logic, never during layout.
        let mut search = false;
        let mut keep = false;
        let mut open = false;
        let query_before = self.stock.query();
        let matching = capture.as_ref().filter(|c| {
            query_before.as_ref() == Ok(&c.report.query)
                && project.as_ref().is_some_and(|p| p.project_id == c.project)
        });
        let fresh = matching.is_some_and(|c| {
            project
                .as_ref()
                .is_some_and(|p| p.project_id == c.project && p.revision == c.revision)
        });
        egui::Panel::bottom("stock-actions")
            .exact_size(80.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if let Some(kept) = self
                        .state
                        .stock
                        .kept
                        .as_ref()
                        .filter(|_| matching.is_some())
                    {
                        match kept {
                            crate::host::stock::Kept::Recipe { .. } => {
                                if ui::primary(
                                    ui,
                                    "Open study",
                                    (!ready).then_some("Wait for the current operation"),
                                ) {
                                    open = true;
                                }
                            }
                            crate::host::stock::Kept::Filament { .. } => {
                                if ui::primary(
                                    ui,
                                    "Review material",
                                    (!ready).then_some("Wait for the current operation"),
                                ) {
                                    open = true;
                                }
                            }
                        }
                        ui.label("Kept in Project");
                        search = ui
                            .add_enabled(ready, egui::Button::new("Search again"))
                            .clicked();
                    } else if matching.is_some() && self.stock.selection.is_some() {
                        let reason = if !ready {
                            Some("Wait for the current operation")
                        } else if !fresh {
                            Some("Project changed; search again")
                        } else if project.as_ref().is_none_or(|p| p.read_only) {
                            Some("Open a writable project to keep this result")
                        } else {
                            None
                        };
                        keep = ui::primary(
                            ui,
                            if matches!(self.stock.selection, Some(StockChoice::Pair(_))) {
                                "Keep recipe"
                            } else {
                                "Keep filament"
                            },
                            reason,
                        );
                        search = ui
                            .add_enabled(ready, egui::Button::new("Search again"))
                            .clicked();
                    } else {
                        search = ui::primary(
                            ui,
                            "Find matches",
                            if !ready {
                                Some("Wait for the current operation")
                            } else {
                                query_before.as_ref().err().map(String::as_str)
                            },
                        );
                        if matching.is_some() {
                            ui.weak("Select a filament or recipe to review");
                        }
                    }
                    if self.state.active_operation && ui.button("Cancel operation").clicked() {
                        self.queued_host = Some(Command::Cancel);
                    }
                });
            });
        egui::ScrollArea::vertical()
            .id_salt((
                "stock-body",
                self.stock.reviewing,
                self.stock.selection.filter(|_| self.stock.reviewing),
                &self.stock.seen,
            ))
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(950.0));
                if !self.stock.reviewing {
                    self.stock.form(ui);
                }
                let query = self.stock.query();
                let report = capture.as_ref().filter(|c| {
                    query.as_ref() == Ok(&c.report.query)
                        && project.as_ref().is_some_and(|p| p.project_id == c.project)
                });
                if let Some(capture) = report {
                    let report = &capture.report;
                    ui.separator();
                    ui.label(match report.status {
                        StockSearchStatus::Complete => format!(
                            "{} filament revisions · {} pairs checked",
                            report.candidates.len(),
                            report.visited_pairs
                        ),
                        StockSearchStatus::WorkLimit => format!(
                            "Best found · {} of {} pairs checked · work limit",
                            report.visited_pairs, report.total_pairs
                        ),
                        StockSearchStatus::Cancelled => format!(
                            "Cancelled · incomplete · {} pairs checked",
                            report.visited_pairs
                        ),
                    });
                    if !fresh && self.state.stock.kept.is_none() {
                        ui.weak("Project changed. Search again before keeping a result.");
                    }
                    if report.candidates.is_empty() {
                        ui.label(if report.query.on_hand_only {
                            "No on-hand filaments recorded in this scope."
                        } else {
                            "No saved filaments in this scope."
                        });
                        if ui.button("Add filaments").clicked() {
                            self.choose_filament();
                        }
                    }
                    if self.stock.reviewing {
                        if ui::close(ui, "Back to results") {
                            self.stock.reviewing = false;
                        }
                        self.stock
                            .details(ui, report, self.state.stock.kept.is_none());
                    } else {
                        if ui::stock_results(ui, report, &mut self.stock.selection) {
                            self.stock.reviewing = true;
                            ui.ctx().request_repaint();
                        }
                    }
                } else if self.state.stock.loading {
                    ui.spinner();
                    ui.label("Loading matching results…");
                } else {
                    ui.weak("Choose a target and search your saved materials.");
                }
            });
        if search
            && let (Ok(query), Some(p), Some(h)) = (
                self.stock.query(),
                project.as_ref(),
                self.state.hello.as_ref(),
            )
        {
            self.stock.selection = None;
            self.stock.reviewing = false;
            self.queued_project = Some(ProjectCommand::MatchStock {
                session_id: h.session_id.clone(),
                base_revision: p.revision,
                query,
            });
        }
        if keep && let (Some(choice), Some(capture)) = (self.stock.selection, matching) {
            self.library.queued = Some(crate::host::LibraryAction::KeepStock(
                crate::host::stock::Keep {
                    input_hash: capture.report.input_hash.clone(),
                    choice,
                    name: self.stock.name.trim().into(),
                },
            ));
        }
        if open && let Some(kept) = self.state.stock.kept.clone() {
            match kept {
                crate::host::stock::Kept::Filament { .. } => self.choose_filament(),
                crate::host::stock::Kept::Recipe {
                    project: kept_project,
                    reference,
                    context,
                } => {
                    if let (Some(p), Some(h)) = (project, self.state.hello.as_ref())
                        && p.project_id == kept_project
                    {
                        self.queued_project = Some(ProjectCommand::LoadRecipeStudy {
                            session_id: h.session_id.clone(),
                            base_revision: p.revision,
                            reference,
                            context,
                        });
                        self.stock.matching = false;
                    }
                }
            }
        }
    }
}
