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
    error: Option<String>,
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
        ui.weak("Recipes capture a target, exact filament snapshots, evidence and a physical layer cycle. New geometry requires a fresh assessment and explicit application.");
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
        for recipe in entries {
            ui.push_id(&recipe.reference.id.0, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} · {} filaments",
                        recipe.name, recipe.ingredient_count
                    ));
                    if ui
                        .add_enabled(ready, egui::Button::new("Inspect recipe"))
                        .clicked()
                    {
                        self.library.queued = Some(LibraryAction::Recipe(RecipeAction::Inspect {
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
        if let Some(selected) = &state.recipe_selection
            && selected.personal == personal
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
                    ui.label(format!(
                        "{}: {} · {}",
                        ingredient.palette_key,
                        f.definition.name,
                        &f.reference.revision.as_str()[..12]
                    ));
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
            ui.weak(format!(
                "Evidence: {} · {}",
                transfer.recipe.definition.palette.source_revision,
                transfer.recipe.definition.palette.license
            ));
            if personal {
                if ui
                    .add_enabled(
                        writable,
                        egui::Button::new("Copy complete recipe to Project"),
                    )
                    .clicked()
                    && let Some(project) = current
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
                        ready,
                        egui::Button::new("Use these ingredients for selection capture"),
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
                }
                ui.collapsing("Study context for this project", |ui| {
                    let context = self.library.recipes.context.as_mut().expect("context initialized");
                    ui.weak("Enter the upright patch in build coordinates. This does not bind a source face or assign tools.");
                    ui.horizontal(|ui| { ui.label("Origin XYZ (mm)"); for value in &mut context.patch.origin_mm { ui.add(egui::DragValue::new(value).speed(0.1)); } });
                    ui.horizontal(|ui| { ui.label("Unit outward normal XY"); for value in &mut context.patch.normal_xy { ui.add(egui::DragValue::new(value).speed(0.01)); } });
                    ui.horizontal(|ui| { ui.label("Width / height (mm)"); ui.add(egui::DragValue::new(&mut context.patch.width_mm).speed(0.1)); ui.add(egui::DragValue::new(&mut context.patch.height_mm).speed(0.1)); });
                    ui.horizontal(|ui| { ui.label("Cycle origin Z (mm)"); ui.add(egui::DragValue::new(&mut context.cycle_origin_z_mm).speed(0.1)); });
                    if let Err(error) = context.request(&transfer.recipe.definition) { ui.colored_label(ui.visuals().error_fg_color, error); }
                    if ui.add_enabled(writable && context.request(&transfer.recipe.definition).is_ok(), egui::Button::new("Load as new unassessed study")).clicked() && let Some(h) = &self.state.hello {
                        self.queued_project = Some(ProjectCommand::LoadRecipeStudy { session_id: h.session_id.clone(), base_revision: current.as_ref().expect("current").1, reference: reference.clone(), context: context.clone() });
                    }
                });
                ui.weak("Continue in Optical study: Recompare, accept a selection, then bind two loaded declarations and Apply. Three-filament recipes support reference studies only.");
            } else {
                ui.weak("Project changed. Refresh and inspect this recipe again.");
            }
        }
        self.recipe_composer(ui, state, writable, personal_write);
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
        let personal = self.library.personal;
        ui.separator();
        ui.collapsing("Create recipe from filament declarations", |ui| {
            let draft = &mut self.library.recipes;
            ui.weak("Add 2–3 variants from Filaments in either scope. Declared sRGB is an estimate; missing color evidence stays unknown.");
            ui.horizontal(|ui| { ui.label("Recipe name"); ui.text_edit_singleline(&mut draft.name); });
            ui.horizontal(|ui| { ui.label("Target hex"); ui.text_edit_singleline(&mut draft.target); });
            let mut remove = None;
            for (i, filament) in draft.ingredients.iter().enumerate() {
                ui.horizontal(|ui| { ui.label(format!("{} · {}", i + 1, filament.definition.name)); ui.add(egui::DragValue::new(&mut draft.heights[i]).speed(0.01).suffix(" mm")); if ui.small_button("Remove").clicked() { remove = Some(i); } });
            }
            if let Some(i) = remove { draft.ingredients.remove(i); draft.heights.remove(i); }
            let enabled = if personal { personal_write } else { writable };
            if ui.add_enabled(enabled && (2..=3).contains(&draft.ingredients.len()), egui::Button::new(if personal { "Create in My library" } else { "Create in Project" })).clicked() {
                let result = spiling_contracts::color_naming::parse_srgb8_hex(draft.target.trim()).and_then(|rgb| RecipeTransfer::from_declarations(RecipeId(uuid::Uuid::new_v4().to_string()), draft.name.trim().into(), draft.ingredients.clone(), AppearanceColor::Srgb { channels: rgb.map(|v| f64::from(v) / 255.0) }, draft.heights.clone()));
                match result {
                    Ok(transfer) => {
                        draft.error = None;
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
