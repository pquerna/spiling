// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::required;
use crate::{
    geometry::{DefinitionId, OccurrenceId},
    process::*,
    rpc,
};

impl TryFrom<rpc::ProcessSelector> for ProcessSelector {
    type Error = String;
    fn try_from(v: rpc::ProcessSelector) -> Result<Self, String> {
        let selector = match required(v.kind, "process selector")? {
            rpc::process_selector::Kind::WholeOwnedVolume(_) => Self::WholeOwnedVolume,
            rpc::process_selector::Kind::SourceBoxMm(v) => Self::SourceBoxMm {
                min: required(v.min, "source box min")?.try_into()?,
                max: required(v.max, "source box max")?.try_into()?,
            },
        };
        selector.validate().map_err(|e| e.to_string())?;
        Ok(selector)
    }
}
impl TryFrom<ProcessSelector> for rpc::ProcessSelector {
    type Error = String;
    fn try_from(v: ProcessSelector) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            kind: Some(match v {
                ProcessSelector::WholeOwnedVolume => {
                    rpc::process_selector::Kind::WholeOwnedVolume(rpc::WholeOwnedVolume {})
                }
                ProcessSelector::SourceBoxMm { min, max } => {
                    rpc::process_selector::Kind::SourceBoxMm(rpc::SourceBoxMm {
                        min: Some(min.try_into()?),
                        max: Some(max.try_into()?),
                    })
                }
            }),
        })
    }
}
impl TryFrom<rpc::ProcessInterfacePermission> for ProcessInterfacePermission {
    type Error = String;
    fn try_from(value: rpc::ProcessInterfacePermission) -> Result<Self, String> {
        let permission = match required(value.kind, "process interface permission")? {
            rpc::process_interface_permission::Kind::SeparatorWall(_) => Self::SeparatorWall,
            rpc::process_interface_permission::Kind::HorizontalDiaphragm(value) => {
                Self::HorizontalDiaphragm {
                    thickness_mm: value.thickness_mm,
                }
            }
        };
        permission.validate().map_err(|error| error.to_string())?;
        Ok(permission)
    }
}
impl From<ProcessInterfacePermission> for rpc::ProcessInterfacePermission {
    fn from(value: ProcessInterfacePermission) -> Self {
        Self {
            kind: Some(match value {
                ProcessInterfacePermission::SeparatorWall => {
                    rpc::process_interface_permission::Kind::SeparatorWall(
                        rpc::SeparatorWallPermission {},
                    )
                }
                ProcessInterfacePermission::HorizontalDiaphragm { thickness_mm } => {
                    rpc::process_interface_permission::Kind::HorizontalDiaphragm(
                        rpc::HorizontalDiaphragmPermission { thickness_mm },
                    )
                }
            }),
        }
    }
}
fn interface_to_wire(
    value: Option<ProcessInterfacePermission>,
) -> Option<rpc::ProcessInterfacePermission> {
    value.map(Into::into)
}
impl TryFrom<rpc::ProcessValue> for ProcessValue {
    type Error = String;
    fn try_from(v: rpc::ProcessValue) -> Result<Self, String> {
        let value = match required(v.kind, "process value")? {
            rpc::process_value::Kind::SparseFraction(v) => Self::SparseFraction {
                fraction: v.fraction,
            },
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<ProcessValue> for rpc::ProcessValue {
    type Error = String;
    fn try_from(v: ProcessValue) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            kind: Some(match v {
                ProcessValue::SparseFraction { fraction } => {
                    rpc::process_value::Kind::SparseFraction(rpc::SparseFraction { fraction })
                }
            }),
        })
    }
}
impl TryFrom<rpc::ProcessAction> for ProcessAction {
    type Error = String;
    fn try_from(v: rpc::ProcessAction) -> Result<Self, String> {
        Ok(match required(v.kind, "process action")? {
            rpc::process_action::Kind::Replace(v) => Self::Replace {
                value: v.try_into()?,
            },
            rpc::process_action::Kind::Erase(_) => Self::Erase,
        })
    }
}
impl TryFrom<ProcessAction> for rpc::ProcessAction {
    type Error = String;
    fn try_from(v: ProcessAction) -> Result<Self, String> {
        Ok(Self {
            kind: Some(match v {
                ProcessAction::Replace { value } => {
                    rpc::process_action::Kind::Replace(value.try_into()?)
                }
                ProcessAction::Erase => rpc::process_action::Kind::Erase(rpc::ProcessErase {}),
            }),
        })
    }
}
impl TryFrom<rpc::ProcessDocument> for ProcessDocument {
    type Error = String;
    fn try_from(v: rpc::ProcessDocument) -> Result<Self, String> {
        if v.layers.len() > MAX_PROCESS_LAYERS
            || v.layers
                .iter()
                .map(|layer| layer.operations.len())
                .sum::<usize>()
                > MAX_PROCESS_OPERATIONS
        {
            return Err("process document budget exceeded".into());
        }
        let mut layers = Vec::with_capacity(v.layers.len());
        for layer in v.layers {
            let mut operations = Vec::with_capacity(layer.operations.len());
            for operation in layer.operations {
                operations.push(ProcessOperation {
                    operation_id: ProcessOperationId::new(operation.operation_id)
                        .map_err(|e| e.to_string())?,
                    selector: required(operation.selector, "process selector")?.try_into()?,
                    action: required(operation.action, "process action")?.try_into()?,
                    interface: operation.interface.map(TryInto::try_into).transpose()?,
                });
            }
            layers.push(ProcessLayer {
                layer_id: ProcessLayerId::new(layer.layer_id).map_err(|e| e.to_string())?,
                scope: required(layer.scope, "process scope")?.try_into()?,
                enabled: layer.enabled,
                operations,
            });
        }
        let document = Self {
            schema_version: v.schema_version,
            layers,
        };
        document.validate().map_err(|e| e.to_string())?;
        Ok(document)
    }
}
impl TryFrom<ProcessDocument> for rpc::ProcessDocument {
    type Error = String;
    fn try_from(v: ProcessDocument) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        let mut layers = Vec::with_capacity(v.layers.len());
        for layer in v.layers {
            let mut operations = Vec::with_capacity(layer.operations.len());
            for operation in layer.operations {
                operations.push(rpc::ProcessOperation {
                    operation_id: operation.operation_id.get(),
                    selector: Some(operation.selector.try_into()?),
                    action: Some(operation.action.try_into()?),
                    interface: interface_to_wire(operation.interface),
                });
            }
            layers.push(rpc::ProcessLayer {
                layer_id: layer.layer_id.get(),
                scope: Some(layer.scope.try_into()?),
                enabled: layer.enabled,
                operations,
            });
        }
        Ok(Self {
            schema_version: v.schema_version,
            layers,
        })
    }
}
impl TryFrom<rpc::ResolvedProcessValue> for ResolvedProcessValue {
    type Error = String;
    fn try_from(v: rpc::ResolvedProcessValue) -> Result<Self, String> {
        ProcessValue::SparseFraction {
            fraction: v.fraction,
        }
        .validate()
        .map_err(|e| e.to_string())?;
        Ok(Self {
            fraction: v.fraction,
            provenance: v
                .provenance
                .map(|p| {
                    Ok::<_, String>(ProcessProvenance {
                        layer_id: ProcessLayerId::new(p.layer_id).map_err(|e| e.to_string())?,
                        operation_id: ProcessOperationId::new(p.operation_id)
                            .map_err(|e| e.to_string())?,
                    })
                })
                .transpose()?,
        })
    }
}
impl TryFrom<ResolvedProcessValue> for rpc::ResolvedProcessValue {
    type Error = String;
    fn try_from(v: ResolvedProcessValue) -> Result<Self, String> {
        ProcessValue::SparseFraction {
            fraction: v.fraction,
        }
        .validate()
        .map_err(|e| e.to_string())?;
        Ok(Self {
            fraction: v.fraction,
            provenance: v.provenance.map(|p| rpc::ProcessProvenance {
                layer_id: p.layer_id.get(),
                operation_id: p.operation_id.get(),
            }),
        })
    }
}
impl TryFrom<rpc::ResolvedOccurrenceProcess> for ResolvedOccurrenceProcess {
    type Error = String;
    fn try_from(v: rpc::ResolvedOccurrenceProcess) -> Result<Self, String> {
        ProcessValue::SparseFraction {
            fraction: v.global_fraction,
        }
        .validate()
        .map_err(|e| e.to_string())?;
        Ok(Self {
            occurrence_id: OccurrenceId::new(v.occurrence_id).map_err(|e| e.to_string())?,
            definition_id: DefinitionId::parse(v.definition_id).map_err(|e| e.to_string())?,
            global_fraction: v.global_fraction,
            value: required(v.value, "resolved process value")?.try_into()?,
            inherited: required(v.inherited, "inherited process value")?.try_into()?,
        })
    }
}
impl TryFrom<ResolvedOccurrenceProcess> for rpc::ResolvedOccurrenceProcess {
    type Error = String;
    fn try_from(v: ResolvedOccurrenceProcess) -> Result<Self, String> {
        ProcessValue::SparseFraction {
            fraction: v.global_fraction,
        }
        .validate()
        .map_err(|e| e.to_string())?;
        Ok(Self {
            occurrence_id: v.occurrence_id.get(),
            definition_id: v.definition_id.into(),
            global_fraction: v.global_fraction,
            value: Some(v.value.try_into()?),
            inherited: Some(v.inherited.try_into()?),
        })
    }
}
impl TryFrom<rpc::ProcessEdit> for ProcessEdit {
    type Error = String;
    fn try_from(v: rpc::ProcessEdit) -> Result<Self, String> {
        use rpc::process_edit::Op;
        let edit = match required(v.op, "process edit")? {
            Op::SetSparseFraction(v) => {
                ProcessValue::SparseFraction {
                    fraction: v.fraction,
                }
                .validate()
                .map_err(|e| e.to_string())?;
                Self::SetSparseFraction {
                    scope: required(v.scope, "process scope")?.try_into()?,
                    fraction: v.fraction,
                }
            }
            Op::SetBoxSparseFraction(v) => Self::SetBoxSparseFraction {
                scope: required(v.scope, "process scope")?.try_into()?,
                min: required(v.min, "source box min")?.try_into()?,
                max: required(v.max, "source box max")?.try_into()?,
                fraction: v.fraction,
                interface: v.interface.map(TryInto::try_into).transpose()?,
            },
            Op::AddLayer(v) => Self::AddLayer {
                scope: required(v.scope, "process scope")?.try_into()?,
                selector: required(v.selector, "process selector")?.try_into()?,
                action: required(v.action, "process action")?.try_into()?,
                interface: v.interface.map(TryInto::try_into).transpose()?,
            },
            Op::RemoveLayer(v) => Self::RemoveLayer {
                layer_id: ProcessLayerId::new(v.layer_id).map_err(|e| e.to_string())?,
            },
            Op::SetLayerEnabled(v) => Self::SetLayerEnabled {
                layer_id: ProcessLayerId::new(v.layer_id).map_err(|e| e.to_string())?,
                enabled: v.enabled,
            },
            Op::MoveLayer(v) => Self::MoveLayer {
                layer_id: ProcessLayerId::new(v.layer_id).map_err(|e| e.to_string())?,
                index: v.index,
            },
            Op::AddOperation(v) => Self::AddOperation {
                layer_id: ProcessLayerId::new(v.layer_id).map_err(|e| e.to_string())?,
                selector: required(v.selector, "process selector")?.try_into()?,
                action: required(v.action, "process action")?.try_into()?,
                interface: v.interface.map(TryInto::try_into).transpose()?,
            },
            Op::UpdateOperation(v) => Self::UpdateOperation {
                operation_id: ProcessOperationId::new(v.operation_id).map_err(|e| e.to_string())?,
                selector: required(v.selector, "process selector")?.try_into()?,
                action: required(v.action, "process action")?.try_into()?,
                interface: v.interface.map(TryInto::try_into).transpose()?,
            },
            Op::RemoveOperation(v) => Self::RemoveOperation {
                operation_id: ProcessOperationId::new(v.operation_id).map_err(|e| e.to_string())?,
            },
            Op::MoveOperation(v) => Self::MoveOperation {
                operation_id: ProcessOperationId::new(v.operation_id).map_err(|e| e.to_string())?,
                index: v.index,
            },
            Op::Reset(v) => Self::Reset {
                scope: required(v.scope, "process scope")?.try_into()?,
            },
        };
        edit.validate().map_err(|e| e.to_string())?;
        Ok(edit)
    }
}
impl TryFrom<ProcessEdit> for rpc::ProcessEdit {
    type Error = String;
    fn try_from(v: ProcessEdit) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        use rpc::process_edit::Op;
        Ok(Self {
            op: Some(match v {
                ProcessEdit::SetSparseFraction { scope, fraction } => {
                    Op::SetSparseFraction(rpc::SetProcessSparseFraction {
                        scope: Some(scope.try_into()?),
                        fraction,
                    })
                }
                ProcessEdit::SetBoxSparseFraction {
                    scope,
                    min,
                    max,
                    fraction,
                    interface,
                } => Op::SetBoxSparseFraction(rpc::SetProcessBoxSparseFraction {
                    scope: Some(scope.try_into()?),
                    min: Some(min.try_into()?),
                    max: Some(max.try_into()?),
                    fraction,
                    interface: interface_to_wire(interface),
                }),
                ProcessEdit::AddLayer {
                    scope,
                    selector,
                    action,
                    interface,
                } => Op::AddLayer(rpc::AddProcessLayer {
                    scope: Some(scope.try_into()?),
                    selector: Some(selector.try_into()?),
                    action: Some(action.try_into()?),
                    interface: interface_to_wire(interface),
                }),
                ProcessEdit::RemoveLayer { layer_id } => Op::RemoveLayer(rpc::RemoveProcessLayer {
                    layer_id: layer_id.get(),
                }),
                ProcessEdit::SetLayerEnabled { layer_id, enabled } => {
                    Op::SetLayerEnabled(rpc::SetProcessLayerEnabled {
                        layer_id: layer_id.get(),
                        enabled,
                    })
                }
                ProcessEdit::MoveLayer { layer_id, index } => {
                    Op::MoveLayer(rpc::MoveProcessLayer {
                        layer_id: layer_id.get(),
                        index,
                    })
                }
                ProcessEdit::AddOperation {
                    layer_id,
                    selector,
                    action,
                    interface,
                } => Op::AddOperation(rpc::AddProcessOperation {
                    layer_id: layer_id.get(),
                    selector: Some(selector.try_into()?),
                    action: Some(action.try_into()?),
                    interface: interface_to_wire(interface),
                }),
                ProcessEdit::UpdateOperation {
                    operation_id,
                    selector,
                    action,
                    interface,
                } => Op::UpdateOperation(rpc::UpdateProcessOperation {
                    operation_id: operation_id.get(),
                    selector: Some(selector.try_into()?),
                    action: Some(action.try_into()?),
                    interface: interface_to_wire(interface),
                }),
                ProcessEdit::RemoveOperation { operation_id } => {
                    Op::RemoveOperation(rpc::RemoveProcessOperation {
                        operation_id: operation_id.get(),
                    })
                }
                ProcessEdit::MoveOperation {
                    operation_id,
                    index,
                } => Op::MoveOperation(rpc::MoveProcessOperation {
                    operation_id: operation_id.get(),
                    index,
                }),
                ProcessEdit::Reset { scope } => Op::Reset(rpc::ResetProcessScope {
                    scope: Some(scope.try_into()?),
                }),
            }),
        })
    }
}
