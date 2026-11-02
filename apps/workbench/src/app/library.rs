// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use crate::host::{LibraryAction, library::State};
use spiling_contracts::{
    appearance::AppearanceColor,
    library::*,
    project::{ProjectId, ProjectRevision},
};

mod comparison;
mod declaration;
mod hardware;
mod recipes;

struct ProjectDefinitionDraft {
    capture: (ProjectId, ProjectRevision),
    origin: Option<LibraryOrigin>,
}
#[derive(Default)]
pub(super) struct LibraryUi {
    pub open: bool,
    detail: u8,
    discovery: ui::CatalogSearch,
    declaration: declaration::DeclarationDraft,
    catalog_origin: Option<spiling_contracts::catalog::CatalogRecord>,
    comparison_destination: Option<bool>,
    recipes: recipes::RecipeUi,
    hardware: hardware::HardwareUi,
    personal: bool,
    query: String,
    search_changed: Option<std::time::Instant>,
    on_hand: bool,
    include_archived: bool,
    name: String,
    color_name: String,
    hex: String,
    spool_name: String,
    spool_edit: Option<SpoolRecord>,
    spool_state: Option<StockState>,
    grams: String,
    lot: String,
    location: String,
    draft: Option<FilamentRevision>,
    draft_project: Option<ProjectDefinitionDraft>,
    pending: Option<LibraryWrite>,
    pub queued: Option<LibraryAction>,
    pub locator: Option<NativePath>,
    initialized: bool,
    receipt_seen: Option<String>,
    project_receipt_seen: Option<(ProjectId, ProjectRevision)>,
    project_submission: Option<FilamentRef>,
}
impl LibraryUi {
    pub(super) fn pending_save(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn has_drafts(&self, state: &State) -> bool {
        let definition = self.draft.as_ref().map_or(
            !self.name.is_empty() || !self.hex.is_empty() || !self.color_name.is_empty(),
            |d| {
                self.name != d.definition.name
                    || self.color_name != d.definition.color_name.clone().unwrap_or_default()
                    || self.hex != hex(d.definition.appearance.as_ref()).unwrap_or_default()
            },
        );
        definition
            || self.declaration.differs(
                &self
                    .draft
                    .as_ref()
                    .map(|d| d.definition.declaration.clone())
                    .unwrap_or_default(),
            )
            || self.spool_edit.is_some()
            || !self.spool_name.is_empty()
            || !self.grams.is_empty()
            || !self.lot.is_empty()
            || !self.location.is_empty()
            || self.recipes.has_changes()
            || self.hardware.has_changes(state)
    }
    pub(super) fn discard_drafts(&mut self) {
        self.declaration = Default::default();
        self.catalog_origin = None;
        self.discovery.open = false;
        self.draft = None;
        self.draft_project = None;
        self.project_submission = None;
        self.name.clear();
        self.color_name.clear();
        self.hex.clear();
        self.spool_edit = None;
        self.spool_state = None;
        self.spool_name.clear();
        self.grams.clear();
        self.lot.clear();
        self.location.clear();
        self.recipes.clear_composer();
        self.recipes.discard_context();
        self.hardware = hardware::HardwareUi::default();
        self.detail = 0;
    }

    fn mutation(&mut self, status: Option<&LibraryStatus>, mutation: LibraryMutation) {
        let identity = status.and_then(|s| s.identity.as_ref());
        let request = LibraryWrite {
            library_id: identity.map(|i| i.id.clone()),
            base_revision: identity.map_or(0, |i| i.revision),
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation,
        };
        self.pending = Some(request.clone());
        self.queued = Some(LibraryAction::Write(request));
    }
    fn refresh(&mut self) {
        if self.hardware.active {
            self.queued = Some(LibraryAction::Hardware(
                crate::host::hardware::HardwareAction::Refresh,
            ));
            return;
        }
        if self.recipes.active {
            self.queued = Some(LibraryAction::Recipe(
                crate::host::recipes::RecipeAction::Refresh {
                    query: LibraryRecipeQuery {
                        query: self.query.clone(),
                        ..Default::default()
                    },
                },
            ));
            return;
        }
        self.queued = Some(LibraryAction::Refresh(LibraryFilamentQuery {
            query: self.query.clone(),
            on_hand_only: self.on_hand,
            include_archived: self.include_archived,
            ..Default::default()
        }));
    }
    fn edit(&mut self, selected: &FilamentRevision) {
        self.draft_project = None;
        self.declaration = declaration::DeclarationDraft::from(&selected.definition.declaration);
        self.catalog_origin = selected.definition.catalog_origin.clone();
        self.draft = Some(selected.clone());
        self.name = selected.definition.name.clone();
        self.color_name = selected.definition.color_name.clone().unwrap_or_default();
        self.hex = hex(selected.definition.appearance.as_ref()).unwrap_or_default();
    }
    fn definition_mutation(&self) -> Result<LibraryMutation, String> {
        if self.name.trim().is_empty() {
            return Err("Enter a filament name".into());
        }
        let mut definition = self.draft.as_ref().map_or_else(
            || FilamentDefinition {
                declaration: Default::default(),
                catalog_origin: None,
                name: String::new(),
                material: None,
                color_name: None,
                appearance: None,
                profiles: vec![],
                source: DefinitionSource::Operator,
            },
            |d| d.definition.clone(),
        );
        definition.declaration = self.declaration.value()?;
        definition.catalog_origin = self.catalog_origin.clone();
        if let Some(origin) = &self.catalog_origin {
            definition.source = DefinitionSource::Imported {
                name: origin.origin.provider.clone(),
                sha256: origin.origin.content.clone(),
            };
        }
        definition.name = self.name.trim().into();
        definition.color_name = optional(&self.color_name);
        // Editing a label must preserve exact source coordinates and provenance.
        let unchanged_color = self.draft.as_ref().is_some_and(|d| {
            hex(d.definition.appearance.as_ref()).unwrap_or_default() == self.hex.trim()
        });
        if !unchanged_color {
            definition.appearance = if self.hex.trim().is_empty() {
                None
            } else {
                let rgb = spiling_contracts::color_naming::parse_srgb8_hex(self.hex.trim())?;
                Some(DeclaredAppearance {
                    color: AppearanceColor::Srgb {
                        channels: rgb.map(|v| f64::from(v) / 255.0),
                    },
                    source: self.catalog_origin.as_ref().filter(|r|matches!(r.item,spiling_contracts::catalog::CatalogItem::Filament{srgb,..} if srgb==rgb)).map_or(DefinitionSource::Operator,|r|DefinitionSource::Imported{name:r.origin.provider.clone(),sha256:r.origin.content.clone()}),
                })
            };
        }
        definition.content_hash()?;
        Ok(LibraryMutation::PutFilament {
            id: self.draft.as_ref().map_or_else(
                || FilamentId(uuid::Uuid::new_v4().to_string()),
                |d| d.reference.id.clone(),
            ),
            expected: self.draft.as_ref().map(|d| d.reference.revision.clone()),
            definition,
        })
    }
    fn spool_mutation(&self, selected: &FilamentRef) -> Result<LibraryMutation, String> {
        let quantity = if self.grams.trim().is_empty() {
            None
        } else {
            Some(StockQuantity {
                remaining_g: self.grams.parse().map_err(|_| "Enter grams as a number")?,
                source: QuantitySource::Entered,
            })
        };
        let quantity = quantity.map(|mut q| {
            if let Some(previous) = self.spool_edit.as_ref().and_then(|s| s.quantity.as_ref())
                && previous.remaining_g == q.remaining_g
            {
                q.source = previous.source;
            }
            q
        });
        let spool = SpoolRecord {
            id: self.spool_edit.as_ref().map_or_else(
                || SpoolId(uuid::Uuid::new_v4().to_string()),
                |s| s.id.clone(),
            ),
            filament: self
                .spool_edit
                .as_ref()
                .map_or_else(|| selected.clone(), |s| s.filament.clone()),
            name: optional(&self.spool_name).unwrap_or_else(|| "Spool".into()),
            state: self.spool_state.unwrap_or(StockState::OnHand),
            quantity,
            lot: optional(&self.lot),
            location: optional(&self.location),
        };
        spool.validate()?;
        Ok(LibraryMutation::PutSpool { spool })
    }
}
impl Workbench {
    pub(super) fn open_profile_catalog(&mut self, ctx: &egui::Context) {
        if !self.library.open {
            self.surface_focus[1] = ctx.memory(|m| m.focused());
        }
        self.library.hardware.clear_secret();
        self.library.open = true;
        self.library.detail = 6;
    }
    pub(super) fn finish_profile_selection(&mut self, ctx: &egui::Context) {
        self.close_library(ctx);
        self.show_prepare(1);
    }
    pub(super) fn choose_recipe(&mut self) {
        self.library.hardware.clear_secret();
        self.library.detail = 0;
        self.library.open = true;
        self.library.personal = false;
        self.library.recipes.active = true;
        self.library.hardware.active = false;
        self.library.refresh();
    }
    pub(super) fn library_sync(&mut self) {
        let state = self.state.library.clone();
        if let (Some(pending), Some(receipt)) = (&self.library.pending, &state.receipt)
            && pending.request_id == receipt.request_id
            && receipt.publication == LibraryPublication::Confirmed
            && self.library.receipt_seen.as_ref() != Some(&receipt.request_id)
        {
            self.library.receipt_seen = Some(receipt.request_id.clone());
            match &pending.mutation {
                LibraryMutation::PutFilament { .. } if self.library.draft_project.is_none() => {
                    self.library.draft = None;
                    self.library.declaration = Default::default();
                    self.library.catalog_origin = None;
                    self.library.detail = 0;
                    self.library.name.clear();
                    self.library.color_name.clear();
                    self.library.hex.clear();
                }
                LibraryMutation::PutRecipe { transfer, .. }
                    if self.library.recipes.submission.as_ref().is_some_and(
                        |(personal, _, reference)| {
                            *personal && *reference == transfer.recipe.reference()
                        },
                    ) =>
                {
                    self.library.recipes.clear_composer();
                    self.library.detail = 0;
                }
                LibraryMutation::ReplaceHardware { .. } => {
                    self.library.hardware.confirmed_write();
                }
                LibraryMutation::PutSpool { .. } => {
                    self.library.spool_edit = None;
                    self.library.spool_state = None;
                    self.library.spool_name.clear();
                    self.library.grams.clear();
                    self.library.lot.clear();
                    self.library.location.clear();
                }
                _ => {}
            }
            self.library.pending = None;
            self.library.refresh();
        }
        if let Some(receipt) = &state.project_receipt {
            let identity = (receipt.project.clone(), receipt.after);
            let current = self
                .state
                .project
                .as_ref()
                .map(|p| (p.project_id.clone(), p.revision));
            if current.as_ref() == Some(&identity)
                && self.library.project_receipt_seen.as_ref() != Some(&identity)
            {
                self.library.project_receipt_seen = Some(identity);
                if self
                    .library
                    .draft_project
                    .as_ref()
                    .is_some_and(|d| d.capture == (receipt.project.clone(), receipt.before))
                    && self.library.project_submission.as_ref() == Some(&receipt.reference)
                {
                    self.library.draft = None;
                    self.library.draft_project = None;
                    self.library.project_submission = None;
                    self.library.name.clear();
                    self.library.color_name.clear();
                    self.library.hex.clear();
                    self.library.declaration = Default::default();
                    self.library.catalog_origin = None;
                }
                self.library.personal = false;
                self.library.refresh();
            }
        }
        if self
            .library
            .recipes
            .submission
            .as_ref()
            .is_some_and(|(personal, project, reference)| {
                !personal
                    && state
                        .recipe_project
                        .as_ref()
                        .is_some_and(|(id, _)| Some(id) == project.as_ref())
                    && state
                        .project_recipes
                        .iter()
                        .any(|r| &r.reference == reference)
            })
        {
            self.library.recipes.clear_composer();
            self.library.detail = 0;
        }
        if !self.library.open {
            return;
        }
        if self.library.recipes.active
            && self.state.phase == Phase::Running
            && !self.state.busy
            && self.awaiting_serial.is_none()
        {
            let current = self
                .state
                .project
                .as_ref()
                .map(|p| (p.project_id.clone(), p.revision));
            if state.recipe_project != current {
                self.library.refresh();
            }
        }
        if !self.library.initialized && self.state.phase == Phase::Running && !self.state.busy {
            self.library.initialized = true;
            self.library.personal = true;
            self.library.refresh();
        }
    }
    pub(super) fn suspend_library(&mut self) {
        self.library.hardware.clear_secret();
    }
    pub(super) fn close_library(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.surface_focus[1].take() {
            ctx.memory_mut(|m| m.request_focus(id));
        }
        self.library.open = false;
        self.library.hardware.clear_secret();
    }
    pub(super) fn library_page(&mut self, ui: &mut egui::Ui) {
        let state = self.state.library.clone();
        ui.horizontal(|ui| {
            ui.heading("Library");
            if ui::close(
                ui,
                if self.library.detail == 6 {
                    "Back to Material"
                } else {
                    "Back to workbench"
                },
            ) {
                self.close_library(ui.ctx());
            }
            ui.weak("Project copies and My library are independent");
        });
        if self.library.detail == 6 {
            ui::pane_header(
                ui,
                "Material profiles",
                "Project setup · compare before staging a draft",
            );
            let ready = self.state.phase == Phase::Running
                && !self.state.busy
                && !self.state.picker_open
                && self.awaiting_serial.is_none();
            if self.state.manufacturing.loaded_catalog.is_none() {
                self.primary_button(ui, Action::LoadMaterialCatalog);
            } else {
                self.button(ui, Action::LoadMaterialCatalog);
            }
            egui::ScrollArea::vertical()
                .id_salt("profile-catalog-page")
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(900.0));
                    if let (Some(project), Some(hello)) =
                        (self.state.project.clone(), self.state.hello.clone())
                    {
                        self.catalog_ui(ui, &project, &hello.session_id, ready);
                    }
                });
            return;
        }
        self.library_content(ui, &state);
    }
    fn library_content(&mut self, ui: &mut egui::Ui, state: &State) {
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.state.confirmation.is_none()
            && self.awaiting_serial.is_none();
        let writable = ready
            && self.library.pending.is_none()
            && self.state.project.as_ref().is_some_and(|p| !p.read_only);
        let personal_write = ready && state.status.is_some() && self.library.pending.is_none();
        if self.library.discovery.open {
            let (query, choice) = self.library.discovery.show(
                ui,
                state.catalog.as_ref(),
                state.catalog_error.as_deref(),
                ready,
            );
            if let Some(query) = query {
                self.library.queued = Some(LibraryAction::SearchCatalog(query));
            }
            if let Some(choice) = choice {
                match choice {
                    ui::CatalogChoice::Manual => {}
                    ui::CatalogChoice::Review(row) => {
                        if let spiling_contracts::catalog::CatalogItem::Filament {
                            name,
                            declaration,
                            srgb,
                        } = &row.item
                        {
                            self.library.name =
                                format!("{} {}", declaration.brand.as_deref().unwrap_or(""), name)
                                    .trim()
                                    .into();
                            self.library.color_name = name.clone();
                            self.library.hex = spiling_contracts::color_naming::srgb8_hex(*srgb);
                            self.library.declaration =
                                declaration::DeclarationDraft::from(declaration);
                            self.library.catalog_origin = Some(row);
                        }
                    }
                }
                self.library.detail = 1;
            }
            return;
        }
        ui.horizontal(|ui| {
            let before = (
                self.library.personal,
                self.library.recipes.active,
                self.library.hardware.active,
            );
            ui.selectable_value(&mut self.library.personal, true, "My library");
            ui.selectable_value(&mut self.library.personal, false, "Project");
            ui.separator();
            if ui
                .selectable_label(
                    !self.library.hardware.active && !self.library.recipes.active,
                    "Filaments",
                )
                .clicked()
            {
                self.library.recipes.active = false;
                self.library.hardware.active = false;
            }
            if ui
                .selectable_label(
                    !self.library.hardware.active && self.library.recipes.active,
                    "Recipes",
                )
                .clicked()
            {
                self.library.recipes.active = true;
                self.library.hardware.active = false;
            }
            if ui
                .selectable_label(self.library.hardware.active, "Hardware")
                .clicked()
            {
                self.library.hardware.active = true;
            }
            if before
                != (
                    self.library.personal,
                    self.library.recipes.active,
                    self.library.hardware.active,
                )
            {
                self.library.hardware.clear_secret();
                self.library.detail = 0;
                self.library.refresh();
            }
            if ui
                .add_enabled(ready, egui::Button::new("Refresh"))
                .clicked()
            {
                self.library.refresh();
            }
        });
        ui.separator();
        if self.library.hardware.active {
            egui::ScrollArea::vertical()
                .id_salt("hardware-page")
                .show(ui, |ui| {
                    self.hardware_content(ui, state, ready, writable);
                    self.library_pending(ui, state, ready);
                });
            return;
        }
        if self.library.recipes.active {
            egui::ScrollArea::vertical()
                .id_salt("recipes-page")
                .show(ui, |ui| {
                    self.recipes_content(ui, state, ready, writable, personal_write);
                    self.library_pending(ui, state, ready);
                });
            return;
        }
        if self.library.personal {
            self.library_header(ui, state, ready);
        }
        self.library_pending(ui, state, ready);
        let definition_draft = self.library.draft.is_some()
            || !self.library.name.is_empty()
            || !self.library.hex.is_empty()
            || !self.library.color_name.is_empty();
        ui.horizontal_wrapped(|ui| {
            let no_selection = state
                .selected
                .as_ref()
                .is_none_or(|(personal, _)| *personal != self.library.personal);
            let label = if definition_draft {
                "Resume filament draft"
            } else {
                "Add filament"
            };
            if self.library.personal {
                let open = if self.library.detail == 0 && no_selection {
                    ui::primary(ui, label, None)
                } else {
                    ui.button(label).clicked()
                };
                if open {
                    if !definition_draft {
                        self.library
                            .discovery
                            .open(spiling_contracts::catalog::CatalogKind::Filament);
                    } else {
                        self.library.detail = 1;
                    }
                }
            } else if self.library.detail == 0
                && no_selection
                && ui::primary(ui, "Choose from My library", None)
            {
                self.library.personal = true;
                self.library.refresh();
            }
            if self.library.detail != 0 && ui::close(ui, "Back to filaments") {
                self.library.detail = 0;
            }
            if definition_draft {
                ui.label("Filament draft retained");
            }
        });
        if self.library.detail == 1 {
            ui::pane_header(
                ui,
                "Filament",
                if self.library.draft_project.is_some() {
                    "Project copy"
                } else {
                    "My library definition"
                },
            );
            let editor_writable = if self.library.draft_project.is_some() {
                writable
            } else {
                personal_write
            };
            egui::Panel::bottom("filament-editor-actions").show(ui, |ui| {
                self.library_definition_actions(ui, state, editor_writable);
            });
            egui::ScrollArea::vertical()
                .id_salt("filament-editor")
                .show(ui, |ui| self.library_definition_editor(ui, editor_writable));
            return;
        }
        if self.library.detail == 2 {
            if let Some((personal, selected)) = &state.selected
                && *personal
            {
                ui::pane_header(ui, "Physical spools", &selected.definition.name);
                egui::ScrollArea::vertical()
                    .id_salt("spool-editor")
                    .show(ui, |ui| {
                        self.library_spools(ui, state, selected, personal_write)
                    });
            }
            return;
        }
        let width = (ui.available_width() * 0.32).clamp(200.0, 340.0);
        egui::Panel::left("filament-list")
            .exact_size(width)
            .show(ui, |ui| {
                ui::group(ui, "Filaments");
                egui::ScrollArea::vertical()
                    .id_salt("filament-list-body")
                    .show(ui, |ui| self.library_rows(ui, state, ready));
            });
        egui::ScrollArea::vertical()
            .id_salt("filament-detail")
            .show(ui, |ui| {
                if let Some((personal, selected)) = &state.selected
                    && *personal == self.library.personal
                {
                    let current = self
                        .state
                        .project
                        .as_ref()
                        .map(|p| (p.project_id.clone(), p.revision));
                    if *personal || (current.is_some() && state.selected_project == current) {
                        self.library_selection(
                            ui,
                            state,
                            selected,
                            *personal,
                            writable,
                            personal_write,
                        );
                    } else {
                        ui.label("Project changed. Refresh and inspect this filament again.");
                    }
                } else {
                    ui::pane_header(
                        ui,
                        "Choose a filament",
                        "Select a row to inspect its declaration",
                    );
                }
                if self.library.personal {
                    self.library_capture_job(ui, state, personal_write);
                } else {
                    self.library_comparison(ui, state, writable, personal_write);
                }
            });
    }
    fn library_header(&mut self, ui: &mut egui::Ui, state: &State, ready: bool) {
        if let Some(status) = &state.status {
            ui.weak(format!(
                "{} variants · {} spools",
                status.filament_count, status.spool_count
            ));
            ui.collapsing("Data & library location", |ui| {
                let path = status
                    .location
                    .to_os_string()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|e| e.to_string());
                ui.label(path);
                ui.weak("Projects retain their own material copies.");
            });
        } else {
            ui.weak("My library is unavailable. Project materials remain independent.");
        }
        let draft = self.library.has_drafts(state);
        if draft {
            ui.label("Save or discard library drafts before changing this location.");
        }
        if ui
            .add_enabled(
                ready && self.library.pending.is_none() && !draft,
                egui::Button::new("Choose library…"),
            )
            .clicked()
        {
            self.queued_picker = Some(Picker::Library);
        }
        ui.horizontal(|ui| {
            ui.label("Search");
            if ui.text_edit_singleline(&mut self.library.query).changed() {
                self.library.search_changed = Some(std::time::Instant::now());
            }
            if !self.library.recipes.active {
                if ui.checkbox(&mut self.library.on_hand, "On hand").changed() {
                    self.library.search_changed = Some(std::time::Instant::now());
                }
                if ui
                    .checkbox(&mut self.library.include_archived, "Include archived")
                    .changed()
                {
                    self.library.search_changed = Some(std::time::Instant::now());
                }
            }
        });
        if let Some(changed) = self.library.search_changed {
            if ready && changed.elapsed() >= std::time::Duration::from_millis(75) {
                self.library.search_changed = None;
                self.library.refresh();
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(75));
            }
        }
    }
    fn library_rows(&mut self, ui: &mut egui::Ui, state: &State, ready: bool) {
        let spool_draft = self.library.spool_edit.is_some()
            || !self.library.spool_name.is_empty()
            || !self.library.grams.is_empty()
            || !self.library.lot.is_empty()
            || !self.library.location.is_empty();
        if spool_draft {
            ui.label(
                "Spool draft retained. Resume spools or discard before choosing another filament.",
            );
            if ui.button("Resume spools").clicked() {
                self.library.detail = 2;
            }
        }

        if !self.library.personal {
            let current = self
                .state
                .project
                .as_ref()
                .map(|p| (p.project_id.clone(), p.revision));
            if current.is_none() || state.project_capture != current {
                ui.weak("Refresh to browse the current project's materials.");
                return;
            }
        }
        if self.library.personal
            && state.personal_query.as_ref().is_some_and(|q| {
                q.query != self.library.query
                    || q.on_hand_only != self.library.on_hand
                    || q.include_archived != self.library.include_archived
            })
        {
            ui.weak("Searching…");
            return;
        }
        let rows = if self.library.personal {
            state
                .personal
                .as_ref()
                .map_or(&[][..], |p| p.entries.as_slice())
        } else {
            state.project.as_slice()
        };
        for row in rows {
            ui.push_id(&row.entry.current.id.0, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let selected = state.selected.as_ref().is_some_and(|(scope, r)| {
                        *scope == self.library.personal && r.reference == row.entry.current
                    });
                    if ui
                        .add_enabled(
                            ready && !spool_draft,
                            egui::Button::selectable(selected, &row.name),
                        )
                        .clicked()
                    {
                        self.library.queued = Some(LibraryAction::Inspect {
                            personal: self.library.personal,
                            reference: row.entry.current.clone(),
                        });
                    }
                    color(ui, row.appearance.as_ref(), row.color_name.as_deref());
                    ui.label(
                        row.material.as_ref().map_or(
                            row.declaration
                                .material_label
                                .as_deref()
                                .unwrap_or("Material not specified"),
                            |m| &m.polymer_family,
                        ),
                    );
                    if self.library.personal {
                        ui.weak(format!("{} on hand", row.on_hand_spools));
                    }
                });
            });
        }
        if self.library.personal
            && let Some(page) = &state.personal
        {
            ui.horizontal(|ui| {
                ui.weak(format!("{} matching variants", page.total_matches));
                if let Some(offset) = page.next_offset
                    && ui
                        .add_enabled(ready, egui::Button::new("Next page"))
                        .clicked()
                {
                    self.library.queued = Some(LibraryAction::Refresh(LibraryFilamentQuery {
                        expected: page.identity.clone(),
                        query: self.library.query.clone(),
                        on_hand_only: self.library.on_hand,
                        include_archived: self.library.include_archived,
                        offset,
                        ..Default::default()
                    }));
                }
            });
        }
    }
    fn library_selection(
        &mut self,
        ui: &mut egui::Ui,
        state: &State,
        selected: &FilamentRevision,
        personal: bool,
        writable: bool,
        personal_write: bool,
    ) {
        ui.heading(&selected.definition.name);
        ui.weak(if personal {
            "My library · reusable definition"
        } else {
            "Project · standalone copy"
        });
        color(
            ui,
            selected.definition.appearance.as_ref(),
            selected.definition.color_name.as_deref(),
        );
        if let Some(material) = &selected.definition.material {
            ui.label(format!(
                "{} · {} · {} mm",
                material.manufacturer, material.polymer_family, material.filament_diameter_mm
            ));
        }
        let declaration = &selected.definition.declaration;
        if declaration.brand.is_some() || declaration.material_label.is_some() {
            ui.label(format!(
                "{} · {}",
                declaration.brand.as_deref().unwrap_or("Brand unknown"),
                declaration
                    .material_label
                    .as_deref()
                    .unwrap_or("Material unknown")
            ));
        }
        if let Some(v) = declaration.diameter_mm {
            ui.label(format!("{v} mm filament"));
        }
        ui.collapsing("Declared properties", |ui| {
            ui.label(
                declaration
                    .hueforge_td_mm
                    .map_or("HueForge TD unknown".into(), |v| {
                        format!("HueForge TD: {v} mm")
                    }),
            );
            if let Some(origin) = &selected.definition.catalog_origin {
                ui.label(format!("{} · {}", origin.origin.provider, origin.origin.id));
                ui.label(&origin.origin.url);
                ui.label(format!("Captured revision {}", origin.origin.revision));
            }
            ui.weak("Catalog declarations are not physical calibration or a process profile.");
        });
        self.library_selection_actions(ui, state, selected, personal, writable, personal_write);
        if personal {
            if ui.button("Manage spools").clicked() {
                self.library.detail = 2;
            }
        } else {
            if selected.definition.profiles.is_empty() {
                ui.weak("No process profile captured. Add a compatible profile before applying to a job.");
            }
            for profile in &selected.definition.profiles {
                let label = format!("Review {} for this job", profile.id);
                let inspectable = self.state.phase == Phase::Running
                    && !self.state.busy
                    && self.awaiting_serial.is_none();
                if ui
                    .add_enabled(inspectable, egui::Button::new(label))
                    .clicked()
                {
                    self.library.queued = Some(LibraryAction::ReviewProfile(profile.clone()));
                    self.show_prepare(1);
                    self.close_library(ui.ctx());
                }
            }
        }
        ui.collapsing("Source and revision", |ui| {
            ui.label(format!(
                "{} captured process profiles",
                selected.definition.profiles.len()
            ));
            match &selected.definition.source {
                DefinitionSource::Operator => {
                    ui.label("Entered by the operator");
                }
                DefinitionSource::Imported { name, .. } => {
                    ui.label(format!("Imported from {name}"));
                }
            }
            ui.monospace(selected.reference.revision.as_str());
        });
    }
    fn library_selection_actions(
        &mut self,
        ui: &mut egui::Ui,
        state: &State,
        selected: &FilamentRevision,
        personal: bool,
        writable: bool,
        personal_write: bool,
    ) {
        self.recipe_ingredient(
            ui,
            selected,
            self.state.phase == Phase::Running && !self.state.busy,
        );
        ui.horizontal_wrapped(|ui| {
            if !personal {
                if ui
                    .add_enabled(
                        writable && self.library.draft.is_none() && self.library.name.is_empty(),
                        egui::Button::new("Edit project copy"),
                    )
                    .clicked()
                {
                    let origin = state
                        .project
                        .iter()
                        .find(|r| r.entry.current == selected.reference)
                        .and_then(|r| r.entry.origin.clone());
                    self.library.edit(selected);
                    self.library.detail = 1;
                    self.library.draft_project = state
                        .selected_project
                        .clone()
                        .map(|capture| ProjectDefinitionDraft { capture, origin });
                }
                let ready = self.state.phase == Phase::Running
                    && !self.state.busy
                    && self.awaiting_serial.is_none();
                let origin = state
                    .project
                    .iter()
                    .find(|r| r.entry.current == selected.reference)
                    .and_then(|r| r.entry.origin.as_ref());
                if origin.is_some()
                    && ui
                        .add_enabled(ready, egui::Button::new("Compare with My library"))
                        .clicked()
                    && let Some((project_id, base_revision)) = &state.selected_project
                {
                    self.library.queued = Some(LibraryAction::CompareOrigin {
                        project_id: project_id.clone(),
                        base_revision: *base_revision,
                        reference: selected.reference.clone(),
                    });
                }
            }
            if personal
                && ui::primary(
                    ui,
                    "Copy to project",
                    (!writable).then_some("Open a writable project to copy this filament"),
                )
            {
                self.library_copy_to_project(state, selected);
            }
            if personal
                && ui
                    .add_enabled(
                        personal_write
                            && self.library.draft.is_none()
                            && self.library.name.is_empty(),
                        egui::Button::new("Edit definition"),
                    )
                    .clicked()
            {
                self.library.edit(selected);
                self.library.detail = 1;
            }
            if !personal
                && ui
                    .add_enabled(
                        personal_write,
                        egui::Button::new("Save a new copy to My library"),
                    )
                    .clicked()
            {
                self.library.mutation(
                    state.status.as_ref(),
                    LibraryMutation::PutFilament {
                        id: FilamentId(uuid::Uuid::new_v4().to_string()),
                        expected: None,
                        definition: selected.definition.clone(),
                    },
                );
            }
            let archived = state
                .personal
                .as_ref()
                .and_then(|p| {
                    p.entries
                        .iter()
                        .find(|e| e.entry.current == selected.reference)
                })
                .is_some_and(|e| e.entry.archived);
            if personal
                && ui
                    .add_enabled(
                        personal_write,
                        egui::Button::new(if archived {
                            "Restore variant"
                        } else {
                            "Archive variant"
                        }),
                    )
                    .clicked()
            {
                self.library.mutation(
                    state.status.as_ref(),
                    LibraryMutation::ArchiveFilament {
                        filament: selected.reference.clone(),
                        archived: !archived,
                    },
                );
            }
        });
    }
    fn library_copy_to_project(&mut self, state: &State, selected: &FilamentRevision) {
        let (Some(id), Some(project)) =
            (state.selected_library.as_ref(), self.state.project.as_ref())
        else {
            return;
        };
        if state.project_capture.as_ref() != Some(&(project.project_id.clone(), project.revision)) {
            self.local_problem = Some(
                "Refresh materials to review the current project copy before replacing it".into(),
            );
            return;
        }
        let expected = state
            .project
            .iter()
            .find(|r| r.entry.current.id == selected.reference.id)
            .map(|r| r.entry.current.revision.clone());
        if expected.is_some() {
            self.local_problem = Some("This filament is already in Project. Inspect its Project copy and use Compare with My library to review a replacement.".into());
            return;
        }
        self.library.queued = Some(LibraryAction::PutProjectFilament {
            project_id: project.project_id.clone(),
            revision: selected.clone(),
            origin: Some(LibraryOrigin {
                library_id: id.clone(),
                filament: selected.reference.clone(),
            }),
            expected,
            base_revision: project.revision,
        });
    }
    fn library_spools(
        &mut self,
        ui: &mut egui::Ui,
        state: &State,
        selected: &FilamentRevision,
        writable: bool,
    ) {
        for spool in &state.spools {
            ui.push_id(&spool.id.0, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&spool.name);
                    ui.label(match spool.state {
                        StockState::OnHand => "On hand",
                        StockState::Ordered => "Ordered",
                        StockState::Empty => "Empty",
                        StockState::Archived => "Archived",
                    });
                    ui.label(
                        spool
                            .quantity
                            .as_ref()
                            .map_or("Quantity unknown".into(), |q| {
                                format!("{} g", q.remaining_g)
                            }),
                    );
                    if ui
                        .add_enabled(writable, egui::Button::new("Edit spool"))
                        .clicked()
                    {
                        self.library.spool_edit = Some(spool.clone());
                        self.library.spool_state = Some(spool.state);
                        self.library.spool_name = spool.name.clone();
                        self.library.grams = spool
                            .quantity
                            .as_ref()
                            .map_or(String::new(), |q| q.remaining_g.to_string());
                        self.library.lot = spool.lot.clone().unwrap_or_default();
                        self.library.location = spool.location.clone().unwrap_or_default();
                    }
                    if ui
                        .add_enabled(writable, egui::Button::new("Mark empty"))
                        .clicked()
                    {
                        let mut spool = spool.clone();
                        spool.state = StockState::Empty;
                        self.library
                            .mutation(state.status.as_ref(), LibraryMutation::PutSpool { spool });
                    }
                })
            });
        }
        ui.label(if self.library.spool_edit.is_some() {
            "Edit physical spool"
        } else {
            "Add another spool of this variant"
        });
        let state_value = self.library.spool_state.get_or_insert(StockState::OnHand);
        ui.add_enabled_ui(writable, |ui| {
            egui::ComboBox::from_label("Stock state")
                .selected_text(stock_label(*state_value))
                .show_ui(ui, |ui| {
                    for state in [
                        StockState::OnHand,
                        StockState::Ordered,
                        StockState::Empty,
                        StockState::Archived,
                    ] {
                        ui.selectable_value(state_value, state, stock_label(state));
                    }
                });
        });
        ui.add_enabled_ui(writable, |ui| {
            ui::text(ui, "Name", &mut self.library.spool_name);
            ui::text(ui, "Remaining (g, optional)", &mut self.library.grams);
            ui::text(ui, "Lot", &mut self.library.lot);
            ui::text(ui, "Location", &mut self.library.location);
        });
        if ui::primary(
            ui,
            if self.library.spool_edit.is_some() {
                "Save spool"
            } else {
                "Add spool"
            },
            (!writable).then_some("Wait for the current library operation"),
        ) {
            match self.library.spool_mutation(&selected.reference) {
                Ok(mutation) => self.library.mutation(state.status.as_ref(), mutation),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if ui.button("Discard spool draft").clicked() {
            self.library.spool_edit = None;
            self.library.spool_state = None;
            self.library.spool_name.clear();
            self.library.grams.clear();
            self.library.lot.clear();
            self.library.location.clear();
        }
        ui.weak("Slicing and exporting do not consume stock.");
    }
    fn library_capture_job(&mut self, ui: &mut egui::Ui, state: &State, writable: bool) {
        let Some(profile) = self
            .state
            .manufacturing
            .intent
            .as_ref()
            .and_then(|i| i.material_profile.as_ref())
            .map(|p| p.selection.profile.clone())
        else {
            return;
        };
        if ui
            .add_enabled(
                writable,
                egui::Button::new("Save current job material to My library"),
            )
            .clicked()
        {
            let definition = FilamentDefinition {
                declaration: Default::default(),
                catalog_origin: None,
                name: format!(
                    "{} {}",
                    profile.material.manufacturer, profile.material.product
                ),
                material: Some(profile.material.clone()),
                color_name: None,
                appearance: None,
                profiles: vec![profile],
                source: DefinitionSource::Operator,
            };
            self.library.mutation(
                state.status.as_ref(),
                LibraryMutation::PutFilament {
                    id: FilamentId(uuid::Uuid::new_v4().to_string()),
                    expected: None,
                    definition,
                },
            );
        }
    }
    fn library_definition_editor(&mut self, ui: &mut egui::Ui, writable: bool) {
        ui.set_max_width(ui.available_width().min(650.0));
        ui.add_enabled_ui(writable, |ui| {
            ui::text(ui, "Name", &mut self.library.name);
            ui::text(ui, "Brand", &mut self.library.declaration.brand);
            ui::text(ui, "Material label", &mut self.library.declaration.material);
            ui::text(ui, "Diameter (mm)", &mut self.library.declaration.diameter);
            ui::text(ui, "Supplier color name", &mut self.library.color_name);
            ui::text(ui, "Hex (optional)", &mut self.library.hex);
        });
        ui.collapsing("Optical declaration", |ui| {
            ui::text(ui, "HueForge TD (mm)", &mut self.library.declaration.td);
            ui.weak("Declared transmission distance; physical color remains uncalibrated.");
        });
        if let Some(origin) = &self.library.catalog_origin {
            ui.collapsing("Catalog source", |ui| {
                ui.label(format!("{} · {}", origin.origin.provider, origin.origin.id));
                ui.label(&origin.origin.url);
                ui.label(format!("Captured revision {}", origin.origin.revision));
            });
        }
        ui.weak("Unknown fields and missing process profiles can be completed later.");
    }
    fn library_definition_actions(&mut self, ui: &mut egui::Ui, state: &State, writable: bool) {
        let current = self
            .state
            .project
            .as_ref()
            .map(|p| (p.project_id.clone(), p.revision));
        let draft_current = self
            .library
            .draft_project
            .as_ref()
            .is_none_or(|d| current.as_ref() == Some(&d.capture));
        if !draft_current {
            ui.weak("Project revision changed. Draft values are retained; discard and inspect the current copy to start a fresh edit.");
        }
        let label = if self.library.draft_project.is_some() {
            "Save project copy"
        } else if self.library.draft.is_none() {
            "Add filament"
        } else {
            "Save filament"
        };
        let validation = if let Err(error) = self.library.declaration.value() {
            Some(error)
        } else if self.library.name.trim().is_empty() {
            Some("Enter a filament name".to_string())
        } else if !self.library.hex.trim().is_empty() {
            spiling_contracts::color_naming::parse_srgb8_hex(self.library.hex.trim()).err()
        } else {
            None
        };
        let reason = if !writable {
            Some("Wait for the current operation or choose a writable scope")
        } else if !draft_current {
            Some("Discard this stale draft and inspect the current Project copy")
        } else {
            validation.as_deref()
        };
        if ui::primary(ui, label, reason) {
            match self.library.definition_mutation() {
                Ok(LibraryMutation::PutFilament {
                    id,
                    expected,
                    definition,
                }) => {
                    if let Some(draft) = &self.library.draft_project {
                        let revision = FilamentRevision {
                            reference: FilamentRef {
                                id,
                                revision: definition.content_hash().expect("validated definition"),
                            },
                            definition,
                        };
                        self.library.project_submission = Some(revision.reference.clone());
                        self.library.queued = Some(LibraryAction::PutProjectFilament {
                            project_id: draft.capture.0.clone(),
                            base_revision: draft.capture.1,
                            revision,
                            expected,
                            origin: draft.origin.clone(),
                        });
                    } else {
                        self.library.mutation(
                            state.status.as_ref(),
                            LibraryMutation::PutFilament {
                                id,
                                expected,
                                definition,
                            },
                        );
                    }
                }
                Ok(_) => unreachable!("definition mutation"),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if ui.button("Discard definition draft").clicked() {
            self.library.detail = 0;
            self.library.declaration = Default::default();
            self.library.catalog_origin = None;
            self.library.draft = None;
            self.library.draft_project = None;
            self.library.project_submission = None;
            self.library.name.clear();
            self.library.color_name.clear();
            self.library.hex.clear();
        }
    }
    fn library_pending(&mut self, ui: &mut egui::Ui, state: &State, ready: bool) {
        let Some(pending) = self.library.pending.clone() else {
            return;
        };
        ui.separator();
        ui.label("Draft retained until its original save is confirmed.");
        if ui
            .add_enabled(ready, egui::Button::new("Resolve original save"))
            .clicked()
        {
            self.library.queued = Some(LibraryAction::Write(pending.clone()));
        }
        let rejected = state.rejected_request.as_deref() == Some(&pending.request_id);
        if rejected {
            ui.weak("Refresh and review before preparing a replacement request.");
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new("Keep editor values; discard rejected request"),
                )
                .clicked()
            {
                self.library.pending = None;
            }
        }
    }
}
fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.into())
}
fn hex(appearance: Option<&DeclaredAppearance>) -> Option<String> {
    appearance.and_then(|a| match a.color {
        AppearanceColor::Srgb { channels } => Some(spiling_contracts::color_naming::srgb8_hex(
            channels.map(|v| (v * 255.0).round() as u8),
        )),
        _ => None,
    })
}
fn color(ui: &mut egui::Ui, appearance: Option<&DeclaredAppearance>, name: Option<&str>) {
    let color = appearance.and_then(|a| match a.color {
        AppearanceColor::Srgb { channels } => Some(channels.map(|v| (v * 255.0).round() as u8)),
        _ => None,
    });
    let presentation = color.map_or(
        ui::ColorPresentation::unspecified(ui::ColorMeaning::DeclaredAppearance),
        |rgb| ui::ColorPresentation::known(rgb, ui::ColorMeaning::DeclaredAppearance),
    );
    presentation.declared_name(name.unwrap_or("")).show(
        ui,
        "library-color",
        ui::ColorDensity::Compact,
    );
}

fn stock_label(state: StockState) -> &'static str {
    match state {
        StockState::OnHand => "On hand",
        StockState::Ordered => "Ordered",
        StockState::Empty => "Empty",
        StockState::Archived => "Archived",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renaming_a_filament_preserves_exact_color_coordinates_and_source() {
        let definition = FilamentDefinition {
            declaration: Default::default(),
            catalog_origin: None,
            name: "Before".into(),
            material: None,
            color_name: None,
            appearance: Some(DeclaredAppearance {
                color: AppearanceColor::Srgb {
                    channels: [0.5, 0.123456789, 0.75],
                },
                source: DefinitionSource::Imported {
                    name: "Source color".into(),
                    sha256: SourceHash::from_bytes(b"source"),
                },
            }),
            profiles: vec![],
            source: DefinitionSource::Operator,
        };
        let revision = FilamentRevision {
            reference: FilamentRef {
                id: FilamentId("30891e2d-a394-4811-b3a3-bba9d8894104".into()),
                revision: definition.content_hash().unwrap(),
            },
            definition,
        };
        let mut ui = LibraryUi::default();
        ui.edit(&revision);
        ui.name = "After".into();
        let LibraryMutation::PutFilament { definition, .. } = ui.definition_mutation().unwrap()
        else {
            panic!("definition edit")
        };
        assert_eq!(definition.name, "After");
        assert_eq!(definition.appearance, revision.definition.appearance);
    }
}
