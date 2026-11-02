// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use crate::host::recipes::RecipeAction;
use spiling_contracts::{library::RecipeStudyContext, project::ProjectCommand};
#[derive(Default)]
pub(super) struct RecipeUi {
    pub active: bool,
    pub ingredients: Vec<FilamentRevision>,
    name: String,
    target: String,
    heights: Vec<f64>,
    context_for: Option<RecipeRef>,
    context: Option<RecipeStudyContext>,
    context_original: Option<RecipeStudyContext>,
    destination: Option<bool>,
    pub(super) submission: Option<(bool, Option<ProjectId>, RecipeRef)>,
    error: Option<String>,
}
impl RecipeUi {
    pub(super) fn has_changes(&self) -> bool {
        !self.ingredients.is_empty()
            || !self.name.is_empty()
            || !self.target.is_empty()
            || self.context != self.context_original
    }
    pub(super) fn discard_context(&mut self) {
        self.context = self.context_original.clone();
    }

    pub(super) fn clear_composer(&mut self) {
        self.ingredients.clear();
        self.name.clear();
        self.target.clear();
        self.heights.clear();
        self.destination = None;
        self.submission = None;
        self.error = None;
    }
}
impl Workbench {
    pub(super) fn recipe_ingredient(
        &mut self,
        ui: &mut egui::Ui,
        filament: &FilamentRevision,
        ready: bool,
    ) {
        if ui
            .add_enabled(
                ready
                    && self.library.recipes.ingredients.len() < 3
                    && !self
                        .library
                        .recipes
                        .ingredients
                        .iter()
                        .any(|f| f.reference.id == filament.reference.id),
                egui::Button::new("Add to recipe ingredients"),
            )
            .clicked()
        {
            self.library.recipes.ingredients.push(filament.clone());
            self.library.recipes.heights.push(0.2);
        }
    }
    pub(super) fn recipes_content(
        &mut self,
        ui: &mut egui::Ui,
        state: &State,
        ready: bool,
        writable: bool,
        personal_write: bool,
    ) {
        let personal = self.library.personal;
        ui.set_max_width(ui.available_width().min(800.0));
        ui::help(
            ui,
            "recipe-definition",
            "Recipes",
            "A recipe captures a target, filament snapshots, evidence and a physical layer cycle. Geometry requires a fresh study and explicit construction binding.",
        );
        if self.library.detail == 3 {
            if ui::close(ui, "Back to recipes") {
                self.library.detail = 0;
            }
            self.recipe_composer(ui, state, writable, personal_write);
            return;
        }
        let empty = if personal {
            state
                .personal_recipes
                .as_ref()
                .is_none_or(|p| p.entries.is_empty())
        } else {
            state.project_recipes.is_empty()
        };
        let label = if self.library.recipes.destination.is_some() {
            "Resume recipe draft"
        } else {
            "Create recipe"
        };
        let compose = if empty && self.library.detail == 0 {
            ui::primary(ui, label, None)
        } else {
            ui.button(label).clicked()
        };
        if compose {
            self.library.recipes.destination.get_or_insert(personal);
            self.library.detail = 3;
        }
        let context_dirty = self.library.recipes.context != self.library.recipes.context_original;
        if context_dirty {
            ui.label(
                "Study context draft retained. Finish or discard before choosing another recipe.",
            );
            if ui.button("Discard study context").clicked() {
                self.library.recipes.context = self.library.recipes.context_original.clone();
            }
        }
        let entries = if personal {
            state
                .personal_recipes
                .as_ref()
                .map(|p| p.entries.as_slice())
                .unwrap_or_default()
        } else {
            &state.project_recipes
        };
        if entries.is_empty() {
            ui.label("No recipes in this scope. Add ingredients from Filaments, or capture a retained study selection below.");
        }
        if self.library.detail != 5 {
            for recipe in entries {
                ui.push_id(&recipe.reference.id.0, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{} · {} filaments",
                            recipe.name, recipe.ingredient_count
                        ));
                        if ui
                            .add_enabled(
                                ready && !context_dirty,
                                egui::Button::new("Inspect recipe"),
                            )
                            .clicked()
                        {
                            self.library.detail = 5;
                            self.library.queued =
                                Some(LibraryAction::Recipe(RecipeAction::Inspect {
                                    personal,
                                    reference: recipe.reference.clone(),
                                }));
                        }
                    });
                });
            }
            if personal && let Some(page) = &state.personal_recipes {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(ready, egui::Button::new("First page"))
                        .clicked()
                    {
                        self.library.refresh();
                    }
                    if let Some(offset) = page.next_offset
                        && ui
                            .add_enabled(ready, egui::Button::new("Next recipes"))
                            .clicked()
                    {
                        self.library.queued = Some(LibraryAction::Recipe(RecipeAction::Refresh {
                            query: LibraryRecipeQuery {
                                expected: page.identity.clone(),
                                query: self.library.query.clone(),
                                offset,
                            },
                        }));
                    }
                });
            }
        }
        if self.library.detail == 5 && ui::close(ui, "Back to recipes") {
            self.library.detail = 0;
        }
        if let Some(selected) = &state.recipe_selection
            && selected.personal == personal
            && self.library.detail == 5
        {
            let current = self
                .state
                .project
                .as_ref()
                .map(|p| (p.project_id.clone(), p.revision));
            let fresh = personal || selected.project == current;
            let transfer = &selected.transfer;
            let reference = transfer.recipe.reference();
            ui.separator();
            ui.heading(&transfer.recipe.definition.name);
            if let Some(identity) = &selected.library {
                ui.weak(format!(
                    "My library · captured revision {}",
                    identity.revision
                ));
            } else {
                ui.weak("Project · complete standalone recipe");
            }
            super::super::review::target(ui, transfer.recipe.definition.request.target.color);
            for ingredient in &transfer.recipe.definition.ingredients {
                if let Some(f) = transfer
                    .filaments
                    .iter()
                    .find(|f| f.reference == ingredient.filament)
                {
                    ui.label(&f.definition.name);
                }
            }
            ui.label(format!(
                "Cycle: {}",
                transfer
                    .recipe
                    .definition
                    .request
                    .recipe
                    .slots
                    .iter()
                    .map(|s| format!("{}: {:.3} mm", s.material + 1, s.height_mm))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            ui.collapsing("Recipe provenance", |ui| {
                ui.add(
                    egui::Label::new(format!("{:#?}", transfer.recipe.definition))
                        .wrap()
                        .selectable(true),
                );
            });
            if personal {
                if ui::primary(
                    ui,
                    "Copy to project",
                    (!writable)
                        .then_some("Open a writable project and wait for pending operations"),
                ) && let Some(project) = current
                {
                    self.library.queued =
                        Some(LibraryAction::Recipe(RecipeAction::CopyToProject {
                            project,
                            transfer: transfer.clone(),
                        }));
                }
            } else if fresh {
                if ui
                    .add_enabled(
                        ready
                            && self.library.recipes.ingredients.is_empty()
                            && self.library.recipes.name.is_empty(),
                        egui::Button::new("Use ingredients"),
                    )
                    .clicked()
                {
                    self.library.recipes.ingredients = transfer
                        .recipe
                        .definition
                        .ingredients
                        .iter()
                        .filter_map(|i| {
                            transfer
                                .filaments
                                .iter()
                                .find(|f| f.reference == i.filament)
                                .cloned()
                        })
                        .collect();
                    self.library.recipes.heights =
                        vec![0.2; self.library.recipes.ingredients.len()];
                    self.library.recipes.name = transfer.recipe.definition.name.clone();
                }
                if ui
                    .add_enabled(
                        personal_write,
                        egui::Button::new("Save a copy to My library"),
                    )
                    .clicked()
                {
                    let mut copy = transfer.clone();
                    copy.recipe.id = RecipeId(uuid::Uuid::new_v4().to_string());
                    self.library.mutation(
                        state.status.as_ref(),
                        LibraryMutation::PutRecipe {
                            transfer: copy,
                            expected: None,
                        },
                    );
                }
                if self.library.recipes.context_for.as_ref() != Some(&reference) {
                    self.library.recipes.context_for = Some(reference.clone());
                    self.library.recipes.context = Some(RecipeStudyContext {
                        patch: transfer.recipe.definition.request.patch,
                        cycle_origin_z_mm: transfer.recipe.definition.request.recipe.origin_z_mm,
                    });
                    self.library.recipes.context_original = self.library.recipes.context.clone();
                }
                ui::group(ui, "Study context");
                ui.vertical(|ui| {
                    let context = self.library.recipes.context.as_mut().expect("context initialized");
                    ui.weak("Enter the upright patch in build coordinates. This does not bind a source face or assign tools.");
                    for (axis, value) in context.patch.origin_mm.iter_mut().enumerate() { ui::number(ui, ["Origin X (mm)", "Origin Y (mm)", "Origin Z (mm)"][axis], value, 0.1); }
                    for (axis, value) in context.patch.normal_xy.iter_mut().enumerate() { ui::number(ui, ["Normal X", "Normal Y"][axis], value, 0.01); }
                    ui::number(ui, "Width (mm)", &mut context.patch.width_mm, 0.1); ui::number(ui, "Height (mm)", &mut context.patch.height_mm, 0.1);
                    ui::number(ui, "Cycle origin Z (mm)", &mut context.cycle_origin_z_mm, 0.1);
                    if let Err(error) = context.request(&transfer.recipe.definition) { ui.colored_label(ui.visuals().error_fg_color, error); }
                    let validation = context.request(&transfer.recipe.definition).err();
                    if ui::primary(ui, "Load study", if !writable { Some("Open a writable project and wait for pending operations") } else { validation.as_deref() }) && let Some(h) = &self.state.hello {
                        self.library.hardware.clear_secret(); self.library.open = false; self.manufacturing.panel = 4;
                        self.queued_project = Some(ProjectCommand::LoadRecipeStudy { session_id: h.session_id.clone(), base_revision: current.as_ref().expect("current").1, reference: reference.clone(), context: context.clone() });
                    }
                });
                ui.label(
                    "Loads an unassessed study. Three-filament recipes remain reference-only.",
                );
            } else {
                ui.weak("Project changed. Refresh and inspect this recipe again.");
            }
        }
        if !personal {
            self.recipe_capture(ui, writable);
        }
    }
    fn recipe_composer(
        &mut self,
        ui: &mut egui::Ui,
        state: &State,
        writable: bool,
        personal_write: bool,
    ) {
        let personal = self
            .library
            .recipes
            .destination
            .unwrap_or(self.library.personal);
        ui::pane_header(
            ui,
            "Create recipe",
            if personal { "My library" } else { "Project" },
        );
        if ui.button("Discard recipe draft").clicked() {
            self.library.recipes.clear_composer();
            self.library.detail = 0;
            return;
        }
        let enabled = (if personal { personal_write } else { writable })
            && self.library.recipes.submission.is_none();
        ui.add_enabled_ui(enabled, |ui| {
            let draft = &mut self.library.recipes;
            ui.weak("Add 2–3 variants from Filaments in either scope. Declared sRGB is an estimate; missing color evidence stays unknown.");
            ui::text(ui, "Recipe name", &mut draft.name);
            ui::text(ui, "Target hex", &mut draft.target);
            let mut remove = None;
            for (i, filament) in draft.ingredients.iter().enumerate() {
                ui.horizontal(|ui| { ui.label(format!("{} · {}", i + 1, filament.definition.name)); ui.add(egui::DragValue::new(&mut draft.heights[i]).speed(0.01).suffix(" mm")); if ui.small_button("Remove").clicked() { remove = Some(i); } });
            }
            if let Some(i) = remove { draft.ingredients.remove(i); draft.heights.remove(i); }
            let enabled = if personal { personal_write } else { writable };
            if ui::primary(ui, "Create recipe", if !enabled { Some("Wait for pending operations or choose a writable scope") } else if !(2..=3).contains(&draft.ingredients.len()) { Some("Add two or three ingredients from Filaments") } else { None }) {
                let result = spiling_contracts::color_naming::parse_srgb8_hex(draft.target.trim()).and_then(|rgb| RecipeTransfer::from_declarations(RecipeId(uuid::Uuid::new_v4().to_string()), draft.name.trim().into(), draft.ingredients.clone(), AppearanceColor::Srgb { channels: rgb.map(|v| f64::from(v) / 255.0) }, draft.heights.clone()));
                match result {
                    Ok(transfer) => {
                        draft.error = None;
                        draft.submission = Some((personal, self.state.project.as_ref().map(|p| p.project_id.clone()), transfer.recipe.reference()));
                        if personal { self.library.mutation(state.status.as_ref(), LibraryMutation::PutRecipe { transfer, expected: None }); }
                        else if let Some(p) = &self.state.project { self.library.queued = Some(LibraryAction::Recipe(RecipeAction::CopyToProject { project: (p.project_id.clone(), p.revision), transfer })); }
                    }
                    Err(error) => draft.error = Some(error),
                }
            }
            if let Some(error) = &self.library.recipes.error { ui.colored_label(ui.visuals().error_fg_color, error); }
        });
    }
    fn recipe_capture(&mut self, ui: &mut egui::Ui, writable: bool) {
        ui.collapsing("Capture accepted study selection", |ui| {
                let Some(p) = &self.state.project else { return; };
                let Some(study) = &p.appearance_study.optical else { ui.label("Load a recipe study and assess it first."); return; };
                let retained = study.selection.is_some() && study.report.as_ref().is_some_and(|r| r.complete);
                ui.weak("Map each evidence row to its exact Project filament snapshot. Copy ingredients into Project first. The retained selection supplies the chosen physical cycle; no tool assignments are saved.");
                ui.text_edit_singleline(&mut self.library.recipes.name);
                for (i, key) in study.request.materials.iter().enumerate() { ui.label(format!("{}: {}", key, self.library.recipes.ingredients.get(i).map_or("choose ingredient in Filaments", |f| f.definition.name.as_str()))); }
                if ui.add_enabled(writable && retained && self.library.recipes.ingredients.len() == study.request.materials.len() && !self.library.recipes.name.trim().is_empty(), egui::Button::new("Capture selection as project recipe")).clicked() && let Some(h) = &self.state.hello {
                    self.queued_project = Some(ProjectCommand::CaptureRecipe { session_id: h.session_id.clone(), base_revision: p.revision, id: RecipeId(uuid::Uuid::new_v4().to_string()), name: self.library.recipes.name.trim().into(), ingredients: study.request.materials.iter().zip(&self.library.recipes.ingredients).map(|(key, f)| RecipeIngredient { filament: f.reference.clone(), palette_key: key.clone() }).collect(), expected: None });
                }
            });
    }
}
