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
    identity: Option<(ProjectId, OccurrenceId, bool, bool)>,
    revision: Option<ProjectRevision>,
    definition: bool,
    boxed: bool,
    fraction: f64,
    min: [f64; 3],
    max: [f64; 3],
    permission: bool,
    plate: bool,
    thickness: f64,
    dirty: bool,
    submitted: Option<u64>,
}
impl Workbench {
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
            return;
        };
        let p = &mut self.process;
        ui.heading("Source-bound sparse process");
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut p.definition, false, "Occurrence");
            ui.selectable_value(&mut p.definition, true, "Shared definition");
        });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut p.boxed, false, "Whole owned volume");
            ui.selectable_value(&mut p.boxed, true, "Source-local box");
        });
        let identity = (
            project.project_id.clone(),
            o.occurrence_id,
            p.definition,
            p.boxed,
        );
        if p.submitted
            .is_some_and(|serial| self.state.serial > serial && !self.state.busy)
        {
            if p.revision != Some(project.revision) {
                p.dirty = false;
            }
            p.submitted = None;
        }
        let scope = if p.definition {
            SurfaceScope::Definition {
                definition_id: o.definition_id.clone(),
            }
        } else {
            SurfaceScope::Occurrence {
                occurrence_id: o.occurrence_id,
                expected_definition_id: o.definition_id.clone(),
            }
        };
        if p.identity.as_ref() != Some(&identity)
            || (!p.dirty && p.revision != Some(project.revision))
        {
            p.identity = Some(identity);
            p.revision = Some(project.revision);
            p.dirty = false;
            let resolved = project
                .resolved_process
                .iter()
                .find(|r| r.occurrence_id == o.occurrence_id && r.definition_id == o.definition_id);
            p.fraction = resolved.map_or(0.0, |r| {
                if p.definition {
                    r.inherited.fraction
                } else {
                    r.value.fraction
                }
            });
            p.permission = false;
            p.plate = false;
            p.thickness = 0.0;
            if let Some(operation) = project
                .process_document
                .layers
                .iter()
                .filter(|l| l.scope == scope)
                .flat_map(|l| &l.operations)
                .rev()
                .find(|op| matches!(op.selector, ProcessSelector::SourceBoxMm { .. }))
            {
                if let ProcessSelector::SourceBoxMm { min, max } = operation.selector {
                    p.min = min;
                    p.max = max;
                }
                if let ProcessAction::Replace {
                    value: ProcessValue::SparseFraction { fraction },
                } = operation.action
                    && p.boxed
                {
                    p.fraction = fraction;
                }
                p.permission = operation.interface.is_some();
                p.plate = matches!(
                    operation.interface,
                    Some(ProcessInterfacePermission::HorizontalDiaphragm { .. })
                );
                if let Some(ProcessInterfacePermission::HorizontalDiaphragm { thickness_mm }) =
                    operation.interface
                {
                    p.thickness = thickness_mm;
                }
            } else {
                p.min = [0.0; 3];
                p.max = [0.0; 3];
            }
        }
        let conflict = p.revision != Some(project.revision);
        if conflict {
            ui.colored_label(
                self.prefs.theme.tokens().warning,
                "Process inputs changed while editing. Reload before applying this draft.",
            );
        }
        if ui.button("Reload process values").clicked() {
            p.identity = None;
        }
        ui.add_enabled_ui(writable && !conflict,|ui|{
            let mut changed=false;
            changed|=ui.add(egui::DragValue::new(&mut p.fraction).speed(0.01).prefix("Sparse fraction ")).changed();
            if p.boxed{
                ui.label("Immutable definition-local mm: min inclusive, max exclusive.");
                for (label,values) in [("Minimum",&mut p.min),("Maximum",&mut p.max)] {ui.label(label);ui.horizontal(|ui|{for (axis,value) in values.iter_mut().enumerate(){changed|=ui.add(egui::DragValue::new(value).speed(0.1).prefix(format!("{} ",["X","Y","Z"][axis]))).changed();}});}
                changed|=ui.checkbox(&mut p.permission,"Authorize permanent deposited interface").changed();
                if p.permission{
                    ui.horizontal_wrapped(|ui|{changed|=ui.selectable_value(&mut p.plate,false,"X/Y separator wall").changed();changed|=ui.selectable_value(&mut p.plate,true,"Source-Z plate").changed();});
                    if p.plate{changed|=ui.add(egui::DragValue::new(&mut p.thickness).speed(0.01).suffix(" mm plate thickness")).changed();}
                    if let Some(intent)=&self.state.manufacturing.intent{ui.weak(format!("Permanent separator nominal width {:.4} mm; overlap {:.4} mm. Native error budgets remain separate.",intent.recipe.bead_width_mm,intent.recipe.bead_width_mm*0.5));}
                }else{ui.weak("No construction permission: saved box intent can still be refused at a nonempty join.");}
                ui.weak("Permanent material, not removable support or a strength qualification.");
            }
            p.dirty|=changed;
            let edit=if p.boxed{ProcessEdit::SetBoxSparseFraction{scope:scope.clone(),min:p.min,max:p.max,fraction:p.fraction,interface:if !p.permission{None}else if p.plate{Some(ProcessInterfacePermission::HorizontalDiaphragm{thickness_mm:p.thickness})}else{Some(ProcessInterfacePermission::SeparatorWall)}}}else{ProcessEdit::SetSparseFraction{scope:scope.clone(),fraction:p.fraction}};
            let validity=edit.validate();if let Err(error)=&validity{ui.colored_label(self.prefs.theme.tokens().warning,error.to_string());}
            if ui.add_enabled(validity.is_ok() && p.dirty,egui::Button::new("Apply sparse requirement")).clicked(){p.submitted=Some(self.state.serial);self.queued_project=Some(ProjectCommand::EditProcess{session_id:hello.session_id.clone(),base_revision:p.revision.expect("captured process revision"),edit});}
            if ui.button("Reset this scope to inherit").clicked(){p.submitted=Some(self.state.serial);self.queued_project=Some(ProjectCommand::EditProcess{session_id:hello.session_id.clone(),base_revision:project.revision,edit:ProcessEdit::Reset{scope:scope.clone()}});}
        });
        if let Some(r) = project
            .resolved_process
            .iter()
            .find(|r| r.occurrence_id == o.occurrence_id)
        {
            ui.label(format!(
                "Whole-owned base {:.4} · definition inheritance {:.4} · recipe {:.4}",
                r.value.fraction, r.inherited.fraction, r.global_fraction
            ));
            ui.weak("These are engine-resolved base values; boxes can vary spatial density.");
        }
        ui.collapsing("Authored process layers", |ui| {
            for layer in project
                .process_document
                .layers
                .iter()
                .filter(|l| l.scope == scope)
            {
                ui.push_id(layer.layer_id, |ui| {
                    let mut enabled = layer.enabled;
                    if ui
                        .add_enabled(
                            writable,
                            egui::Checkbox::new(
                                &mut enabled,
                                format!("Layer {}", layer.layer_id.get()),
                            ),
                        )
                        .changed()
                    {
                        self.queued_project = Some(ProjectCommand::EditProcess {
                            session_id: hello.session_id.clone(),
                            base_revision: project.revision,
                            edit: ProcessEdit::SetLayerEnabled {
                                layer_id: layer.layer_id,
                                enabled,
                            },
                        });
                    }
                    for op in &layer.operations {
                        ui.label(format!(
                            "{:?} · {:?} · permission {:?}",
                            op.selector, op.action, op.interface
                        ));
                    }
                    if ui
                        .add_enabled(writable, egui::Button::new("Remove layer"))
                        .clicked()
                    {
                        self.queued_project = Some(ProjectCommand::EditProcess {
                            session_id: hello.session_id.clone(),
                            base_revision: project.revision,
                            edit: ProcessEdit::RemoveLayer {
                                layer_id: layer.layer_id,
                            },
                        });
                    }
                });
            }
        });
    }
}
