// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    process::*,
    project::{ProjectCommand, ProjectId, ProjectRevision},
    surface::SurfaceScope,
};

#[derive(Default)]
pub(super) struct ProcessUi {
    identity: Option<(ProjectId, OccurrenceId, bool)>,
    revision: Option<ProjectRevision>,
    definition: bool,
    boxed: bool,
    fraction: f64,
    min: [f64; 3],
    max: [f64; 3],
    permission: bool,
    plate: bool,
    thickness: f64,
    target: Option<ProcessOperationId>,
    erase: bool,
    dirty: bool,
    submitted: Option<PendingEdit>,
}
struct PendingEdit {
    serial: u64,
    after_nonce: u64,
    session: SessionId,
    project: ProjectId,
    revision: ProjectRevision,
    edit: ProcessEdit,
}
impl PendingEdit {
    fn matches(&self, receipt: &crate::host::ProcessEditReceipt) -> bool {
        receipt.nonce > self.after_nonce
            && receipt.session == self.session
            && receipt.project == self.project
            && receipt.base_revision == self.revision
            && receipt.edit == self.edit
    }
}
impl ProcessUi {
    pub(super) fn discard(&mut self) {
        self.dirty = false;
        self.target = None;
        self.submitted = None;
        self.identity = None;
    }
    pub(super) fn set_scope(&mut self, definition: bool, boxed: bool) {
        self.discard();
        self.definition = definition;
        self.boxed = boxed;
    }
    pub(super) fn has_changes(&self) -> bool {
        self.dirty
    }
    fn edit(&self, scope: SurfaceScope) -> ProcessEdit {
        let selector = if self.boxed {
            ProcessSelector::SourceBoxMm {
                min: self.min,
                max: self.max,
            }
        } else {
            ProcessSelector::WholeOwnedVolume
        };
        let action = if self.erase {
            ProcessAction::Erase
        } else {
            ProcessAction::Replace {
                value: ProcessValue::SparseFraction {
                    fraction: self.fraction,
                },
            }
        };
        let interface = if self.boxed && self.permission && !self.erase {
            Some(if self.plate {
                ProcessInterfacePermission::HorizontalDiaphragm {
                    thickness_mm: self.thickness,
                }
            } else {
                ProcessInterfacePermission::SeparatorWall
            })
        } else {
            None
        };
        match self.target {
            Some(operation_id) => ProcessEdit::UpdateOperation {
                operation_id,
                selector,
                action,
                interface,
            },
            None => ProcessEdit::AddLayer {
                scope,
                selector,
                action,
                interface,
            },
        }
    }
    fn change(&mut self, op: &ProcessOperation) {
        self.target = Some(op.operation_id);
        self.boxed = matches!(op.selector, ProcessSelector::SourceBoxMm { .. });
        if let ProcessSelector::SourceBoxMm { min, max } = op.selector {
            self.min = min;
            self.max = max;
        }
        self.erase = matches!(op.action, ProcessAction::Erase);
        if let ProcessAction::Replace {
            value: ProcessValue::SparseFraction { fraction },
        } = op.action
        {
            self.fraction = fraction;
        }
        self.permission = op.interface.is_some();
        self.plate = matches!(
            op.interface,
            Some(ProcessInterfacePermission::HorizontalDiaphragm { .. })
        );
        if let Some(ProcessInterfacePermission::HorizontalDiaphragm { thickness_mm }) = op.interface
        {
            self.thickness = thickness_mm;
        }
        self.dirty = true;
    }
}
impl Workbench {
    pub(super) fn reconcile_process_receipt(&mut self) {
        let Some(pending) = &self.process.submitted else {
            return;
        };
        let matches = self.state.process_edit.as_ref().is_some_and(|r| {
            pending.matches(r)
                && self
                    .state
                    .hello
                    .as_ref()
                    .is_some_and(|h| h.session_id == r.session)
                && self
                    .state
                    .project
                    .as_ref()
                    .is_some_and(|p| p.project_id == r.project && p.revision == r.revision)
        });
        if matches {
            self.process.discard();
        } else if self.state.serial > pending.serial && !self.state.busy {
            // A rejection, disconnect or unrelated revision never confirms application.
            self.process.submitted = None;
        }
    }
    pub(super) fn process_actions(&mut self, ui: &mut egui::Ui) {
        let (Some(project), Some(hello), Some(scene)) = (
            self.state.project.clone(),
            self.state.hello.clone(),
            self.state.display.clone(),
        ) else {
            return;
        };
        let Some(o) = scene
            .occurrences
            .iter()
            .find(|o| Some(o.occurrence_id) == self.occurrence)
        else {
            return;
        };
        let display_current = self.display_current();
        let p = &mut self.process;
        let scope = process_scope(p.definition, o.occurrence_id, o.definition_id.clone());
        let edit = p.edit(scope);
        let validation = edit.validate().err().map(|e| e.to_string());
        let reason = if p.submitted.is_some() {
            Some("Awaiting modifier receipt")
        } else if project.read_only {
            Some("Project is read-only")
        } else if self.state.phase != Phase::Running
            || self.state.busy
            || self.state.picker_open
            || self.awaiting_serial.is_some()
        {
            Some("Wait for the current operation")
        } else if p.revision != Some(project.revision)
            || p.identity.as_ref()
                != Some(&(project.project_id.clone(), o.occurrence_id, p.definition))
            || !display_current
        {
            Some("Project changed · discard this draft to reload")
        } else {
            validation.as_deref()
        };
        if ui::primary(
            ui,
            if p.target.is_some() {
                "Apply changes"
            } else {
                "Add modifier"
            },
            reason,
        ) {
            p.submitted = Some(PendingEdit {
                serial: self.state.serial,
                after_nonce: self.state.process_edit.as_ref().map_or(0, |r| r.nonce),
                session: hello.session_id.clone(),
                project: project.project_id.clone(),
                revision: project.revision,
                edit: edit.clone(),
            });
            self.queued_project = Some(ProjectCommand::EditProcess {
                session_id: hello.session_id.clone(),
                base_revision: project.revision,
                edit,
            });
        }
        if ui
            .add_enabled(p.submitted.is_none(), egui::Button::new("Discard draft"))
            .clicked()
        {
            self.process.discard();
        }
    }
    pub(super) fn process_ui(&mut self, ui: &mut egui::Ui, writable: bool) {
        let (Some(project), Some(hello), Some(scene)) = (
            self.state.project.clone(),
            self.state.hello.clone(),
            self.state.display.clone(),
        ) else {
            return;
        };
        let Some(o) = scene
            .occurrences
            .iter()
            .find(|o| Some(o.occurrence_id) == self.occurrence)
        else {
            ui.weak("Select a part to inspect its modifiers");
            return;
        };
        let status = if self.manufacturing_current() {
            None
        } else if self.state.manufacturing.artifact.is_some() {
            Some("Needs verification")
        } else {
            Some("Needs slicing")
        };
        let p = &mut self.process;
        ui::group(ui, format!("Part {}", o.occurrence_id.get()));
        let before = p.definition;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut p.definition, false, "This part");
            ui.selectable_value(&mut p.definition, true, "Shared definition");
        });
        if before != p.definition && p.dirty {
            self.navigation = Some(super::navigation::Navigation::InfillScope(
                p.definition,
                p.boxed,
            ));
            p.definition = before;
        }
        let identity = (project.project_id.clone(), o.occurrence_id, p.definition);
        let resolved = project
            .resolved_process
            .iter()
            .find(|r| r.occurrence_id == o.occurrence_id && r.definition_id == o.definition_id);
        if !p.dirty
            && (p.identity.as_ref() != Some(&identity) || p.revision != Some(project.revision))
        {
            p.identity = Some(identity);
            p.revision = Some(project.revision);
            p.fraction = resolved.map_or(0.0, |r| {
                if p.definition {
                    r.inherited.fraction
                } else {
                    r.value.fraction
                }
            });
        }
        let scope = process_scope(p.definition, o.occurrence_id, o.definition_id.clone());
        // Reuse the engine's owning provenance; never infer a winner from value equality.
        let base_override = resolved
            .and_then(|r| {
                if p.definition {
                    r.inherited.provenance
                } else {
                    r.value.provenance
                }
            })
            .and_then(|owner| {
                project
                    .process_document
                    .layers
                    .iter()
                    .find(|l| l.layer_id == owner.layer_id && l.scope == scope)
                    .and_then(|l| {
                        l.operations
                            .iter()
                            .find(|op| op.operation_id == owner.operation_id)
                    })
            });
        let can_edit = writable && !p.dirty && p.submitted.is_none();
        ui::group(ui, "Resolved base");
        if let Some(r) = resolved {
            let value = if p.definition { r.inherited } else { r.value };
            let source = value
                .provenance
                .and_then(|provenance| {
                    project
                        .process_document
                        .layers
                        .iter()
                        .find(|l| l.layer_id == provenance.layer_id)
                })
                .map_or("Plate defaults", |l| {
                    if matches!(l.scope, SurfaceScope::Definition { .. }) {
                        "Shared definition"
                    } else {
                        "This part"
                    }
                });
            ui::resolved_value(
                ui,
                "Infill density",
                &format!("{:.1}%", value.fraction * 100.0),
                source,
            );
            ui.collapsing("Defaults and inheritance", |ui| {
                ui::resolved_value(
                    ui,
                    "Plate density",
                    &format!("{:.1}%", r.global_fraction * 100.0),
                    "Captured recipe",
                );
                if !p.definition {
                    ui::resolved_value(
                        ui,
                        "Inherited density",
                        &format!("{:.1}%", r.inherited.fraction * 100.0),
                        "Shared definition",
                    );
                }
                ui.label("Whole-part base · source boxes may vary density");
            });
        } else {
            ui.weak("No resolved process values");
        }
        if !p.dirty {
            ui::group(ui, "Modifiers");
            if let Some(status) = status {
                ui.weak(status);
            }
            let layers: Vec<_> = project
                .process_document
                .layers
                .iter()
                .filter(|l| l.scope == scope && !l.operations.is_empty())
                .collect();
            if layers.is_empty() {
                ui.weak("Using inherited density");
            }
            let queue = &mut self.queued_project;
            for layer in layers {
                ui.push_id(layer.layer_id, |ui| {
                    let mut enabled = layer.enabled;
                    let label = if layer.operations.len() == 1 {
                        "Enabled"
                    } else {
                        "Enable group"
                    };
                    if ui
                        .add_enabled(can_edit, egui::Checkbox::new(&mut enabled, label))
                        .changed()
                    {
                        *queue = Some(ProjectCommand::EditProcess {
                            session_id: hello.session_id.clone(),
                            base_revision: project.revision,
                            edit: ProcessEdit::SetLayerEnabled {
                                layer_id: layer.layer_id,
                                enabled,
                            },
                        });
                    }
                    if !layer.enabled {
                        ui.weak("Disabled");
                    }
                    for operation in &layer.operations {
                        ui.push_id(operation.operation_id, |ui| {
                            let value = match operation.action {
                                ProcessAction::Replace {
                                    value: ProcessValue::SparseFraction { fraction },
                                } => format!("{:.1}%", fraction * 100.0),
                                ProcessAction::Erase => "Inherit".into(),
                            };
                            let region = match operation.selector {
                                ProcessSelector::WholeOwnedVolume => "Whole part",
                                ProcessSelector::SourceBoxMm { .. } => "Source box",
                            };
                            match ui::modifier_row(ui, "Infill density", &value, region, can_edit) {
                                Some(ui::ModifierAction::Change) => p.change(operation),
                                Some(ui::ModifierAction::Remove) => {
                                    let edit = if layer.operations.len() == 1 {
                                        ProcessEdit::RemoveLayer {
                                            layer_id: layer.layer_id,
                                        }
                                    } else {
                                        ProcessEdit::RemoveOperation {
                                            operation_id: operation.operation_id,
                                        }
                                    };
                                    *queue = Some(ProjectCommand::EditProcess {
                                        session_id: hello.session_id.clone(),
                                        base_revision: project.revision,
                                        edit,
                                    });
                                }
                                None => {}
                            }
                            ui.collapsing("Details", |ui| {
                                if let ProcessSelector::SourceBoxMm { min, max } =
                                    operation.selector
                                {
                                    for axis in 0..3 {
                                        ui::resolved_value(
                                            ui,
                                            ["X", "Y", "Z"][axis],
                                            &format!("{:.3}–{:.3} mm", min[axis], max[axis]),
                                            "Source frame",
                                        );
                                    }
                                }
                                match operation.interface {
                                    None => {}
                                    Some(ProcessInterfacePermission::SeparatorWall) => {
                                        ui.label("Permanent separator wall");
                                    }
                                    Some(ProcessInterfacePermission::HorizontalDiaphragm {
                                        thickness_mm,
                                    }) => {
                                        ui.label(format!("Permanent plate · {thickness_mm:.3} mm"));
                                    }
                                }
                                ui.weak(format!(
                                    "Layer {} · instruction {}",
                                    layer.layer_id.get(),
                                    operation.operation_id.get()
                                ));
                            });
                        });
                    }
                    ui.separator();
                });
            }
            if !p.dirty {
                ui.add_enabled_ui(writable, |ui| {
                    ui.menu_button("Add modifier…", |ui| {
                        for (boxed, name) in [
                            (false, "Infill density · whole part"),
                            (true, "Infill density · source box"),
                        ] {
                            if ui.button(name).clicked() {
                                if !boxed && let Some(operation) = base_override {
                                    p.change(operation);
                                    ui.close();
                                    continue;
                                }
                                p.target = None;
                                p.erase = false;
                                p.boxed = boxed;
                                p.permission = false;
                                p.min = [0.0; 3];
                                p.max = [0.0; 3];
                                p.dirty = true;
                                ui.close();
                            }
                        }
                    });
                });
            }
        }
        if p.dirty {
            ui.separator();
            ui::group(
                ui,
                if p.target.is_some() {
                    "Change modifier"
                } else {
                    "New modifier"
                },
            );
            ui.weak("Unapplied");
            let conflict = p.revision != Some(project.revision);
            if conflict {
                ui.colored_label(
                    self.prefs.theme.tokens().warning,
                    "Project changed · draft retained",
                );
            }
            ui.add_enabled_ui(writable && !conflict && p.submitted.is_none(), |ui| {
                // A change retains its exact selector/action kind; no guessed winning target.
                ui.label(if p.boxed { "Source box" } else { "Whole part" });
                if !p.erase { ui::percent(ui, "Infill density (%)", &mut p.fraction); }
                else { ui.weak("Restore inheritance in this region"); }
                if p.boxed {
                    for (label, values) in [("Minimum", &mut p.min), ("Maximum", &mut p.max)] {
                        for (axis, value) in values.iter_mut().enumerate() {
                            ui::number(ui, &format!("{label} {} (mm)", ["X", "Y", "Z"][axis]), value, 0.1);
                        }
                    }
                    ui::help(ui, "box-frame", "Source box", "Definition-local millimetres. Minimum inclusive; maximum exclusive. The engine checks regional construction when slicing.");
                    if !p.erase {
                        ui.checkbox(&mut p.permission, "Permanent interface");
                        if p.permission {
                            ui.horizontal_wrapped(|ui| {
                                ui.selectable_value(&mut p.plate, false, "Separator wall");
                                ui.selectable_value(&mut p.plate, true, "Source-Z plate");
                            });
                            if p.plate { ui::number(ui, "Plate thickness (mm)", &mut p.thickness, 0.01); }
                            ui.weak("Permanent deposited material");
                        }
                        ui::help(ui, "process-interface", "Permanent interfaces", "An explicit construction permission, not removable support or a strength qualification. Without permission the engine can refuse a nonempty density join. Separator width follows the captured bead width; a source-Z plate has its own physical thickness.");
                    }
                }
            });
        }
    }
}
fn process_scope(
    definition: bool,
    occurrence_id: OccurrenceId,
    definition_id: DefinitionId,
) -> SurfaceScope {
    if definition {
        SurfaceScope::Definition { definition_id }
    } else {
        SurfaceScope::Occurrence {
            occurrence_id,
            expected_definition_id: definition_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> SurfaceScope {
        SurfaceScope::Definition {
            definition_id: DefinitionId::from_source_sha256(&[8; 32]),
        }
    }
    #[test]
    fn only_the_matching_observed_transaction_can_clear_a_draft() {
        let pending = PendingEdit {
            serial: 1,
            after_nonce: 7,
            session: SessionId::new(),
            project: ProjectId::new(),
            revision: ProjectRevision(3),
            edit: ProcessEdit::AddLayer {
                scope: scope(),
                selector: ProcessSelector::WholeOwnedVolume,
                action: ProcessAction::Replace {
                    value: ProcessValue::SparseFraction { fraction: 0.25 },
                },
                interface: None,
            },
        };
        let receipt = crate::host::ProcessEditReceipt {
            nonce: 8,
            session: pending.session.clone(),
            project: pending.project.clone(),
            base_revision: pending.revision,
            revision: ProjectRevision(4),
            edit: pending.edit.clone(),
        };
        assert!(pending.matches(&receipt));
        let mut wrong = receipt.clone();
        wrong.nonce = 7;
        assert!(!pending.matches(&wrong));
        let mut wrong = receipt.clone();
        wrong.session = SessionId::new();
        assert!(!pending.matches(&wrong));
        let mut wrong = receipt.clone();
        wrong.project = ProjectId::new();
        assert!(!pending.matches(&wrong));
        let mut wrong = receipt.clone();
        wrong.base_revision = ProjectRevision(2);
        assert!(!pending.matches(&wrong));
        let mut wrong = receipt;
        wrong.edit = ProcessEdit::Reset { scope: scope() };
        assert!(!pending.matches(&wrong));
    }

    #[test]
    fn equal_to_default_is_still_an_explicit_pin() {
        let draft = ProcessUi {
            fraction: 0.25,
            dirty: true,
            ..Default::default()
        };
        assert!(draft.has_changes());
        assert!(matches!(
            draft.edit(scope()),
            ProcessEdit::AddLayer {
                selector: ProcessSelector::WholeOwnedVolume,
                action: ProcessAction::Replace {
                    value: ProcessValue::SparseFraction { fraction: 0.25 }
                },
                ..
            }
        ));
    }
    #[test]
    fn change_keeps_exact_identity_region_and_interface_instead_of_guessing_a_winner() {
        let operation = ProcessOperation {
            operation_id: ProcessOperationId::new(27).unwrap(),
            selector: ProcessSelector::SourceBoxMm {
                min: [1.0, 2.0, 3.0],
                max: [4.0, 5.0, 6.0],
            },
            action: ProcessAction::Replace {
                value: ProcessValue::SparseFraction { fraction: 0.4 },
            },
            interface: Some(ProcessInterfacePermission::HorizontalDiaphragm { thickness_mm: 0.8 }),
        };
        let mut draft = ProcessUi::default();
        draft.change(&operation);
        draft.fraction = 0.6;
        assert_eq!(
            draft.edit(scope()),
            ProcessEdit::UpdateOperation {
                operation_id: operation.operation_id,
                selector: operation.selector,
                action: ProcessAction::Replace {
                    value: ProcessValue::SparseFraction { fraction: 0.6 }
                },
                interface: operation.interface,
            }
        );
    }
    #[test]
    fn changing_an_erase_never_turns_it_into_a_density_override() {
        let operation = ProcessOperation {
            operation_id: ProcessOperationId::new(3).unwrap(),
            selector: ProcessSelector::WholeOwnedVolume,
            action: ProcessAction::Erase,
            interface: None,
        };
        let mut draft = ProcessUi::default();
        draft.change(&operation);
        assert_eq!(
            draft.edit(scope()),
            ProcessEdit::UpdateOperation {
                operation_id: operation.operation_id,
                selector: operation.selector,
                action: operation.action,
                interface: None,
            }
        );
    }
}
