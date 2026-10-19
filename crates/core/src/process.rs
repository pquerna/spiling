// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;

pub(super) fn apply_process_edit(
    doc: &mut ProcessDocument,
    edit: ProcessEdit,
    next_layer: &mut u32,
    next_operation: &mut u32,
) -> Result<(), ProjectError> {
    edit.validate()?;
    fn operation(
        next: &mut u32,
        selector: ProcessSelector,
        action: ProcessAction,
        interface: Option<ProcessInterfacePermission>,
    ) -> Result<ProcessOperation, ProjectError> {
        action.validate()?;
        let operation_id = ProcessOperationId::new(*next)?;
        *next = next.checked_add(1).ok_or_else(|| {
            error(
                ProjectErrorCode::ResourceLimit,
                "process operation allocator exhausted",
            )
        })?;
        Ok(ProcessOperation {
            operation_id,
            selector,
            action,
            interface,
        })
    }
    fn layer_index(doc: &ProcessDocument, id: ProcessLayerId) -> Result<usize, ProjectError> {
        doc.layers
            .iter()
            .position(|layer| layer.layer_id == id)
            .ok_or_else(|| invalid("unknown process layer"))
    }
    fn operation_index(
        doc: &ProcessDocument,
        id: ProcessOperationId,
    ) -> Result<(usize, usize), ProjectError> {
        doc.layers
            .iter()
            .enumerate()
            .find_map(|(layer, value)| {
                value
                    .operations
                    .iter()
                    .position(|operation| operation.operation_id == id)
                    .map(|index| (layer, index))
            })
            .ok_or_else(|| invalid("unknown process operation"))
    }
    match edit {
        ProcessEdit::SetBoxSparseFraction {
            scope,
            min,
            max,
            fraction,
            interface,
        } => {
            let selector = ProcessSelector::SourceBoxMm { min, max };
            let value = ProcessValue::SparseFraction { fraction };
            let contains = |selector: ProcessSelector, min: [f64; 3], max: [f64; 3]| match selector
            {
                ProcessSelector::WholeOwnedVolume => true,
                ProcessSelector::SourceBoxMm { min: lo, max: hi } => {
                    (0..3).all(|axis| lo[axis] <= min[axis] && hi[axis] >= max[axis])
                }
            };
            let mut winner = None;
            'layers: for (layer, entry) in doc.layers.iter().enumerate().rev() {
                if !entry.enabled || entry.scope != scope {
                    continue;
                }
                for (index, operation) in entry.operations.iter().enumerate().rev() {
                    match (operation.selector, operation.action) {
                        (ProcessSelector::WholeOwnedVolume, ProcessAction::Erase) => break,
                        (ProcessSelector::WholeOwnedVolume, ProcessAction::Replace { .. }) => {
                            if !entry.operations[index + 1..].iter().any(|later| {
                                matches!(later.selector, ProcessSelector::SourceBoxMm { .. })
                                    && later.action == ProcessAction::Erase
                            }) {
                                break 'layers;
                            }
                        }
                        (
                            ProcessSelector::SourceBoxMm { min, max },
                            ProcessAction::Replace { .. },
                        ) => {
                            let hidden = entry.operations[index + 1..]
                                .iter()
                                .any(|later| contains(later.selector, min, max))
                                || doc.layers[layer + 1..].iter().any(|later| {
                                    later.enabled
                                        && later.scope == scope
                                        && later.operations.last().is_some_and(|operation| {
                                            matches!(
                                                operation.action,
                                                ProcessAction::Replace { .. }
                                            ) && contains(operation.selector, min, max)
                                        })
                                });
                            if hidden {
                                continue;
                            }
                            if winner.replace((layer, index)).is_some() {
                                return Err(invalid(
                                    "box adjustment has multiple possible exact-scope targets",
                                ));
                            }
                        }
                        (ProcessSelector::SourceBoxMm { .. }, ProcessAction::Erase) => {}
                    }
                }
            }
            if let Some((layer, index)) = winner {
                let operation = &mut doc.layers[layer].operations[index];
                operation.selector = selector;
                operation.action = ProcessAction::Replace { value };
                operation.interface = interface;
            } else {
                return apply_process_edit(
                    doc,
                    ProcessEdit::AddLayer {
                        scope,
                        selector,
                        action: ProcessAction::Replace { value },
                        interface,
                    },
                    next_layer,
                    next_operation,
                );
            }
        }
        ProcessEdit::SetSparseFraction { scope, fraction } => {
            let value = ProcessValue::SparseFraction { fraction };
            value.validate()?;
            // Only the winner provenance is used. The fallback is never persisted and
            // permits process authoring before a global manufacturing intent exists.
            let resolved = match &scope {
                SurfaceScope::Definition { definition_id } => {
                    doc.resolve_definition(definition_id, 1.0)
                }
                SurfaceScope::Occurrence {
                    occurrence_id,
                    expected_definition_id,
                } => doc.resolve(expected_definition_id, *occurrence_id, 1.0),
            };
            let winner = resolved.provenance.and_then(|provenance| {
                doc.layers
                    .iter()
                    .position(|layer| layer.layer_id == provenance.layer_id && layer.scope == scope)
                    .map(|layer| (layer, provenance.operation_id))
            });
            if let Some((layer, operation_id)) = winner {
                let operation = doc.layers[layer]
                    .operations
                    .iter_mut()
                    .find(|operation| operation.operation_id == operation_id)
                    .ok_or_else(|| invalid("resolved process operation is missing"))?;
                operation.action = ProcessAction::Replace { value };
            } else {
                return apply_process_edit(
                    doc,
                    ProcessEdit::AddLayer {
                        scope,
                        selector: ProcessSelector::WholeOwnedVolume,
                        action: ProcessAction::Replace { value },
                        interface: None,
                    },
                    next_layer,
                    next_operation,
                );
            }
        }
        ProcessEdit::AddLayer {
            scope,
            selector,
            action,
            interface,
        } => {
            if doc.layers.len() == MAX_PROCESS_LAYERS
                || doc
                    .layers
                    .iter()
                    .map(|layer| layer.operations.len())
                    .sum::<usize>()
                    == MAX_PROCESS_OPERATIONS
            {
                return Err(error(
                    ProjectErrorCode::ResourceLimit,
                    "process document budget exceeded",
                ));
            }
            let layer_id = ProcessLayerId::new(*next_layer)?;
            *next_layer = next_layer.checked_add(1).ok_or_else(|| {
                error(
                    ProjectErrorCode::ResourceLimit,
                    "process layer allocator exhausted",
                )
            })?;
            let first = operation(next_operation, selector, action, interface)?;
            doc.layers.reserve_exact(1);
            doc.layers.push(ProcessLayer {
                layer_id,
                scope,
                enabled: true,
                operations: vec![first],
            });
        }
        ProcessEdit::RemoveLayer { layer_id } => {
            let index = layer_index(doc, layer_id)?;
            doc.layers.remove(index);
        }
        ProcessEdit::SetLayerEnabled { layer_id, enabled } => {
            let index = layer_index(doc, layer_id)?;
            doc.layers[index].enabled = enabled;
        }
        ProcessEdit::MoveLayer { layer_id, index } => {
            let old = layer_index(doc, layer_id)?;
            let index = index as usize;
            if index >= doc.layers.len() {
                return Err(invalid("process layer position outside document"));
            }
            let layer = doc.layers.remove(old);
            doc.layers.insert(index, layer);
        }
        ProcessEdit::AddOperation {
            layer_id,
            selector,
            action,
            interface,
        } => {
            let index = layer_index(doc, layer_id)?;
            if doc
                .layers
                .iter()
                .map(|layer| layer.operations.len())
                .sum::<usize>()
                == MAX_PROCESS_OPERATIONS
            {
                return Err(error(
                    ProjectErrorCode::ResourceLimit,
                    "process operation budget exceeded",
                ));
            }
            let operation = operation(next_operation, selector, action, interface)?;
            doc.layers[index].operations.reserve_exact(1);
            doc.layers[index].operations.push(operation);
        }
        ProcessEdit::UpdateOperation {
            operation_id,
            selector,
            action,
            interface,
        } => {
            action.validate()?;
            let (layer, index) = operation_index(doc, operation_id)?;
            doc.layers[layer].operations[index] = ProcessOperation {
                operation_id,
                selector,
                action,
                interface,
            };
        }
        ProcessEdit::RemoveOperation { operation_id } => {
            let (layer, index) = operation_index(doc, operation_id)?;
            doc.layers[layer].operations.remove(index);
        }
        ProcessEdit::MoveOperation {
            operation_id,
            index,
        } => {
            let (layer, old) = operation_index(doc, operation_id)?;
            let operations = &mut doc.layers[layer].operations;
            let index = index as usize;
            if index >= operations.len() {
                return Err(invalid("process operation position outside layer"));
            }
            let operation = operations.remove(old);
            operations.insert(index, operation);
        }
        ProcessEdit::Reset { scope } => {
            // Clear every exact-scope contribution, including disabled/erased
            // whole-volume and box layers, so reset cannot revive a prior override.
            doc.layers.retain(|layer| layer.scope != scope);
        }
    }
    doc.validate()
}
