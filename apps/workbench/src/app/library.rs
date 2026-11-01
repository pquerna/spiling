// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use crate::host::{LibraryAction, library::State};
use spiling_contracts::{appearance::AppearanceColor, library::*};

#[derive(Default)]
pub(super) struct LibraryUi {
    pub open: bool,
    personal: bool,
    query: String,
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
    pending: Option<LibraryWrite>,
    pub queued: Option<LibraryAction>,
    pub locator: Option<NativePath>,
    initialized: bool,
    receipt_seen: Option<String>,
}
impl LibraryUi {
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
        self.queued = Some(LibraryAction::Refresh(LibraryFilamentQuery {
            query: self.query.clone(),
            on_hand_only: self.on_hand,
            include_archived: self.include_archived,
            ..Default::default()
        }));
    }
    fn edit(&mut self, selected: &FilamentRevision) {
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
                name: String::new(),
                material: None,
                color_name: None,
                appearance: None,
                profiles: vec![],
                source: DefinitionSource::Operator,
            },
            |d| d.definition.clone(),
        );
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
                    source: DefinitionSource::Operator,
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
    pub(super) fn library_window(&mut self, ctx: &egui::Context) {
        let state = self.state.library.clone();
        if let (Some(pending), Some(receipt)) = (&self.library.pending, &state.receipt)
            && pending.request_id == receipt.request_id
            && receipt.publication == LibraryPublication::Confirmed
            && self.library.receipt_seen.as_ref() != Some(&receipt.request_id)
        {
            self.library.receipt_seen = Some(receipt.request_id.clone());
            match &pending.mutation {
                LibraryMutation::PutFilament { .. } => {
                    self.library.draft = None;
                    self.library.name.clear();
                    self.library.color_name.clear();
                    self.library.hex.clear();
                }
                LibraryMutation::PutSpool { .. } => {
                    self.library.spool_edit = None;
                    self.library.spool_state = None;
                }
                _ => {}
            }
            self.library.pending = None;
            self.library.refresh();
        }
        if !self.library.open {
            return;
        }
        if !self.library.initialized && self.state.phase == Phase::Running && !self.state.busy {
            self.library.initialized = true;
            self.library.personal = true;
            self.library.refresh();
        }
        let mut open = true;
        egui::Window::new("Materials")
            .open(&mut open)
            .default_size([760.0, 640.0])
            .vscroll(true)
            .show(ctx, |ui| self.library_content(ui, &state));
        self.library.open = open;
    }
    fn library_content(&mut self, ui: &mut egui::Ui, state: &State) {
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && self.awaiting_serial.is_none();
        let writable = ready && self.state.project.as_ref().is_some_and(|p| !p.read_only);
        let personal_write = ready && state.status.is_some() && self.library.pending.is_none();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.library.personal, true, "My library");
            ui.selectable_value(&mut self.library.personal, false, "Project");
            if ui
                .add_enabled(ready, egui::Button::new("Refresh"))
                .clicked()
            {
                self.library.refresh();
            }
        });
        if self.library.personal {
            self.library_header(ui, state, ready);
        }
        self.library_rows(ui, state, ready);
        ui.separator();
        if let Some((personal, selected)) = &state.selected
            && *personal == self.library.personal
        {
            let current = self
                .state
                .project
                .as_ref()
                .map(|p| (p.project_id.clone(), p.revision));
            if *personal || (current.is_some() && state.selected_project == current) {
                self.library_selection(ui, state, selected, *personal, writable, personal_write);
            } else {
                ui.weak("Project material selection changed. Refresh and inspect it again.");
            }
        }
        if self.library.personal {
            self.library_capture_job(ui, state, personal_write);
            ui.separator();
            let title = if self.library.draft.is_some() {
                "Edit definition"
            } else {
                "Add filament"
            };
            ui.collapsing(title, |ui| {
                self.library_definition_editor(ui, state, personal_write)
            });
        }
        self.library_pending(ui, state, ready);
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
        if ui
            .add_enabled(
                ready && self.library.pending.is_none(),
                egui::Button::new("Choose another library .spil…"),
            )
            .clicked()
        {
            self.queued_picker = Some(Picker::Library);
        }
        ui.horizontal(|ui| {
            ui.label("Search");
            ui.text_edit_singleline(&mut self.library.query);
            ui.checkbox(&mut self.library.on_hand, "On hand");
            ui.checkbox(&mut self.library.include_archived, "Include archived");
            if ui.add_enabled(ready, egui::Button::new("Search")).clicked() {
                self.library.refresh();
            }
        });
    }
    fn library_rows(&mut self, ui: &mut egui::Ui, state: &State, ready: bool) {
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
                        .add_enabled(ready, egui::Button::selectable(selected, &row.name))
                        .clicked()
                    {
                        self.library.queued = Some(LibraryAction::Inspect {
                            personal: self.library.personal,
                            reference: row.entry.current.clone(),
                        });
                    }
                    color(ui, row.appearance.as_ref(), row.color_name.as_deref());
                    ui.label(
                        row.material
                            .as_ref()
                            .map_or("Material not specified", |m| &m.polymer_family),
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
        self.library_selection_actions(ui, state, selected, personal, writable, personal_write);
        if personal {
            ui.collapsing("Physical spools", |ui| {
                self.library_spools(ui, state, selected, personal_write)
            });
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
                    self.manufacturing.panel = 1;
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
        ui.horizontal_wrapped(|ui| {
            if personal
                && ui
                    .add_enabled(writable, egui::Button::new("Add to project"))
                    .clicked()
            {
                self.library_copy_to_project(state, selected);
            }
            if personal
                && ui
                    .add_enabled(
                        personal_write,
                        egui::Button::new("Edit My library definition"),
                    )
                    .clicked()
            {
                self.library.edit(selected);
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
        let (Some(id), Some(project)) = (
            state.status.as_ref().and_then(|s| s.identity.as_ref()),
            self.state.project.as_ref(),
        ) else {
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
        self.library.queued = Some(LibraryAction::UseInProject {
            revision: selected.clone(),
            origin: LibraryOrigin {
                library_id: id.id.clone(),
                filament: selected.reference.clone(),
            },
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
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.library.spool_name);
        });
        ui.horizontal(|ui| {
            ui.label("Remaining g (optional)");
            ui.text_edit_singleline(&mut self.library.grams);
        });
        ui.horizontal(|ui| {
            ui.label("Lot");
            ui.text_edit_singleline(&mut self.library.lot);
        });
        ui.horizontal(|ui| {
            ui.label("Location");
            ui.text_edit_singleline(&mut self.library.location);
        });
        if ui
            .add_enabled(
                writable,
                egui::Button::new(if self.library.spool_edit.is_some() {
                    "Save spool"
                } else {
                    "Add spool"
                }),
            )
            .clicked()
        {
            match self.library.spool_mutation(&selected.reference) {
                Ok(mutation) => self.library.mutation(state.status.as_ref(), mutation),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if ui.button("Start a new spool draft").clicked() {
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
    fn library_definition_editor(&mut self, ui: &mut egui::Ui, state: &State, writable: bool) {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.library.name);
        });
        ui.horizontal(|ui| {
            ui.label("Supplier color name");
            ui.text_edit_singleline(&mut self.library.color_name);
        });
        ui.horizontal(|ui| {
            ui.label("Hex (optional)");
            ui.text_edit_singleline(&mut self.library.hex);
        });
        ui.weak("New colors are operator declarations. Unspecified color and profile are allowed.");
        if ui
            .add_enabled(writable, egui::Button::new("Save definition"))
            .clicked()
        {
            match self.library.definition_mutation() {
                Ok(mutation) => self.library.mutation(state.status.as_ref(), mutation),
                Err(error) => self.local_problem = Some(error),
            }
        }
        if ui.button("Discard definition draft").clicked() {
            self.library.draft = None;
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
