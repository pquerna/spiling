// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use crate::host::library::DefinitionComparison;

impl Workbench {
    pub(super) fn library_comparison(
        &mut self,
        ui: &mut egui::Ui,
        state: &State,
        writable: bool,
        personal_write: bool,
    ) {
        let Some(comparison) = &state.comparison else {
            return;
        };
        let current = self
            .state
            .project
            .as_ref()
            .map(|p| (p.project_id.clone(), p.revision));
        if current.as_ref() != Some(&comparison.project)
            || !state
                .selected
                .as_ref()
                .is_some_and(|(personal, r)| !personal && r.reference == comparison.copy.reference)
        {
            return;
        }
        ui.separator();
        ui.heading(comparison.summary());
        ui.weak(format!(
            "Compared at My library revision {}",
            comparison.identity.revision
        ));
        if comparison.archived {
            ui.weak("The personal entry is archived. Replacing its definition keeps it archived.");
        }
        let changed = changed_fields(&comparison.copy.definition, &comparison.personal.definition);
        if changed.is_empty() {
            return;
        }
        ui.label(format!("Different: {}", changed.join(", ")));
        ui.columns(2, |columns| {
            for (column, (scope, definition)) in columns.iter_mut().zip([
                ("Project", &comparison.copy.definition),
                ("My library", &comparison.personal.definition),
            ]) {
                column.push_id(scope, |ui| {
                    ui.strong(scope);
                    ui.label(&definition.name);
                    color(
                        ui,
                        definition.appearance.as_ref(),
                        definition.color_name.as_deref(),
                    );
                    ui.weak(format!("{} process profiles", definition.profiles.len()));
                });
            }
        });
        ui.collapsing("Inspect exact definitions", |ui| {
            for (scope, definition) in [
                ("Project", &comparison.copy.definition),
                ("My library", &comparison.personal.definition),
            ] {
                ui.strong(scope);
                ui.monospace(
                    serde_json::to_string_pretty(definition).expect("validated definition"),
                );
            }
        });
        ui.weak("Choose a complete replacement. The current job profile changes only through Manufacturing Apply.");
        if ui
            .add_enabled(
                writable,
                egui::Button::new("Replace project copy with My library version"),
            )
            .clicked()
        {
            self.library.queued = Some(LibraryAction::PutProjectFilament {
                project_id: comparison.project.0.clone(),
                base_revision: comparison.project.1,
                revision: FilamentRevision {
                    reference: FilamentRef {
                        id: comparison.copy.reference.id.clone(),
                        revision: comparison.personal.reference.revision.clone(),
                    },
                    definition: comparison.personal.definition.clone(),
                },
                expected: Some(comparison.copy.reference.revision.clone()),
                origin: Some(LibraryOrigin {
                    library_id: comparison.identity.id.clone(),
                    filament: comparison.personal.reference.clone(),
                }),
            });
        }
        if ui
            .add_enabled(
                personal_write,
                egui::Button::new("Update My library with project version"),
            )
            .clicked()
        {
            let request = promotion_request(comparison);
            self.library.pending = Some(request.clone());
            self.library.queued = Some(LibraryAction::Write(request));
        }
    }
}

fn promotion_request(comparison: &DefinitionComparison) -> LibraryWrite {
    LibraryWrite {
        library_id: Some(comparison.identity.id.clone()),
        base_revision: comparison.identity.revision,
        request_id: uuid::Uuid::new_v4().to_string(),
        mutation: LibraryMutation::PutFilament {
            id: comparison.personal.reference.id.clone(),
            expected: Some(comparison.personal.reference.revision.clone()),
            definition: comparison.copy.definition.clone(),
        },
    }
}
fn changed_fields(a: &FilamentDefinition, b: &FilamentDefinition) -> Vec<&'static str> {
    [
        (a.name != b.name, "name"),
        (a.material != b.material, "material identity"),
        (a.color_name != b.color_name, "supplier color name"),
        (a.appearance != b.appearance, "declared appearance/source"),
        (a.profiles != b.profiles, "process profiles"),
        (a.source != b.source, "definition source"),
    ]
    .into_iter()
    .filter_map(|(changed, field)| changed.then_some(field))
    .collect()
}
