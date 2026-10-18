// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Ordered source-bound whole-owned-volume process intent, independent of face finishes.
use crate::{
    geometry::{DefinitionId, OccurrenceId, OccurrenceRecord},
    project::{ProjectError, ProjectErrorCode, StoredDefinition},
    surface::SurfaceScope,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use ts_rs::TS;

pub const PROCESS_SCHEMA_VERSION: u32 = 1;
pub const MAX_PROCESS_LAYERS: usize = 64;
pub const MAX_PROCESS_OPERATIONS: usize = 256;
pub const MAX_PROCESS_DOCUMENT_BYTES: usize = 64 * 1024;
pub const MAX_PROCESS_RETAINED_BYTES: usize = 16 * 1024 * 1024;
fn invalid(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::InvalidProject, message)
}
fn resource(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::ResourceLimit, message)
}

macro_rules! identity {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[serde(transparent)]
        #[ts(type = "number")]
        pub struct $name(u32);
        impl $name {
            pub fn new(value: u32) -> Result<Self, ProjectError> {
                if value == 0 {
                    return Err(invalid("process identity must be nonzero"));
                }
                Ok(Self(value))
            }
            pub fn get(self) -> u32 {
                self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::new(u32::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}
identity!(ProcessLayerId);
identity!(ProcessOperationId);

#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessSelector {
    WholeOwnedVolume,
}
impl<'de> Deserialize<'de> for ProcessSelector {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Checked {
            WholeOwnedVolume {},
        }
        match Checked::deserialize(d)? {
            Checked::WholeOwnedVolume {} => Ok(Self::WholeOwnedVolume),
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessValue {
    SparseFraction { fraction: f64 },
}
impl<'de> Deserialize<'de> for ProcessValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Checked {
            SparseFraction { fraction: f64 },
        }
        let Checked::SparseFraction { fraction } = Checked::deserialize(d)?;
        let value = Self::SparseFraction { fraction };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl ProcessValue {
    pub fn validate(&self) -> Result<(), ProjectError> {
        let Self::SparseFraction { fraction } = self;
        if !fraction.is_finite() || *fraction <= 0.0 || *fraction > 1.0 {
            return Err(invalid("sparse fraction must be finite and in (0,1]"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessAction {
    Replace { value: ProcessValue },
    Erase,
}
impl<'de> Deserialize<'de> for ProcessAction {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Checked {
            Replace { value: ProcessValue },
            Erase {},
        }
        let action = match Checked::deserialize(d)? {
            Checked::Replace { value } => Self::Replace { value },
            Checked::Erase {} => Self::Erase,
        };
        action.validate().map_err(serde::de::Error::custom)?;
        Ok(action)
    }
}
impl ProcessAction {
    pub fn validate(&self) -> Result<(), ProjectError> {
        match self {
            Self::Replace { value } => value.validate(),
            Self::Erase => Ok(()),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessOperation {
    pub operation_id: ProcessOperationId,
    pub selector: ProcessSelector,
    pub action: ProcessAction,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessLayer {
    pub layer_id: ProcessLayerId,
    pub scope: SurfaceScope,
    pub enabled: bool,
    #[serde(deserialize_with = "bounded_operations")]
    pub operations: Vec<ProcessOperation>,
}
fn bounded_operations<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ProcessOperation>, D::Error> {
    struct Operations;
    impl<'de> serde::de::Visitor<'de> for Operations {
        type Value = Vec<ProcessOperation>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 256 process operations")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut operations = Vec::new();
            while let Some(operation) = seq.next_element()? {
                if operations.len() == MAX_PROCESS_OPERATIONS {
                    return Err(serde::de::Error::custom(
                        "process operation budget exceeded",
                    ));
                }
                operations.reserve_exact(1);
                operations.push(operation);
            }
            Ok(operations)
        }
    }
    d.deserialize_seq(Operations)
}
fn bounded_layers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ProcessLayer>, D::Error> {
    struct Layers;
    impl<'de> serde::de::Visitor<'de> for Layers {
        type Value = Vec<ProcessLayer>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 64 layers and 256 process operations")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut layers = Vec::new();
            let mut count = 0;
            while let Some(layer) = seq.next_element::<ProcessLayer>()? {
                count += layer.operations.len();
                if layers.len() == MAX_PROCESS_LAYERS || count > MAX_PROCESS_OPERATIONS {
                    return Err(serde::de::Error::custom("process document budget exceeded"));
                }
                layers.reserve_exact(1);
                layers.push(layer);
            }
            Ok(layers)
        }
    }
    d.deserialize_seq(Layers)
}
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessDocument {
    pub schema_version: u32,
    pub layers: Vec<ProcessLayer>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDocument {
    schema_version: u32,
    #[serde(deserialize_with = "bounded_layers")]
    layers: Vec<ProcessLayer>,
}
impl<'de> Deserialize<'de> for ProcessDocument {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawDocument::deserialize(d)?;
        let doc = Self {
            schema_version: raw.schema_version,
            layers: raw.layers,
        };
        doc.validate().map_err(serde::de::Error::custom)?;
        Ok(doc)
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessProvenance {
    pub layer_id: ProcessLayerId,
    pub operation_id: ProcessOperationId,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedProcessValue {
    pub fraction: f64,
    pub provenance: Option<ProcessProvenance>,
}
impl Default for ProcessDocument {
    fn default() -> Self {
        Self::empty()
    }
}
impl ProcessDocument {
    pub fn empty() -> Self {
        Self {
            schema_version: PROCESS_SCHEMA_VERSION,
            layers: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.schema_version != PROCESS_SCHEMA_VERSION {
            return Err(invalid("unsupported process schema"));
        }
        if self.layers.len() > MAX_PROCESS_LAYERS || self.layers.capacity() > MAX_PROCESS_LAYERS {
            return Err(resource("process layer allocation budget exceeded"));
        }
        let mut layers = BTreeSet::new();
        let mut operations = BTreeSet::new();
        let mut capacity = 0;
        for layer in &self.layers {
            if !layers.insert(layer.layer_id) {
                return Err(invalid("duplicate process layer identity"));
            }
            capacity += layer.operations.capacity();
            for operation in &layer.operations {
                if !operations.insert(operation.operation_id) {
                    return Err(invalid("duplicate process operation identity"));
                }
                operation.action.validate()?;
            }
        }
        if operations.len() > MAX_PROCESS_OPERATIONS
            || capacity > MAX_PROCESS_OPERATIONS
            || self.allocated_bytes() > MAX_PROCESS_DOCUMENT_BYTES
        {
            return Err(resource("process operation allocation budget exceeded"));
        }
        Ok(())
    }
    pub fn validate_targets(
        &self,
        definitions: &[StoredDefinition],
        occurrences: &[OccurrenceRecord],
        next_layer: u32,
        next_operation: u32,
    ) -> Result<(), ProjectError> {
        self.validate()?;
        if next_layer == 0 || next_operation == 0 {
            return Err(invalid("invalid process allocator"));
        }
        for layer in &self.layers {
            if layer.layer_id.get() >= next_layer
                || layer
                    .operations
                    .iter()
                    .any(|operation| operation.operation_id.get() >= next_operation)
            {
                return Err(invalid("process identity exceeds allocator high-water"));
            }
            let valid = match &layer.scope {
                SurfaceScope::Definition { definition_id } => definitions
                    .iter()
                    .any(|d| &d.definition_id == definition_id),
                SurfaceScope::Occurrence {
                    occurrence_id,
                    expected_definition_id,
                } => occurrences.iter().any(|o| {
                    o.occurrence_id == *occurrence_id && &o.definition_id == expected_definition_id
                }),
            };
            if !valid {
                return Err(invalid(
                    "process scope references unknown or inconsistent source identity",
                ));
            }
        }
        Ok(())
    }
    pub fn resolve(
        &self,
        definition_id: &DefinitionId,
        occurrence_id: OccurrenceId,
        global_fraction: f64,
    ) -> ResolvedProcessValue {
        self.resolve_scopes(definition_id, Some(occurrence_id), global_fraction)
    }
    pub fn resolve_definition(
        &self,
        definition_id: &DefinitionId,
        global_fraction: f64,
    ) -> ResolvedProcessValue {
        self.resolve_scopes(definition_id, None, global_fraction)
    }
    fn resolve_scopes(
        &self,
        definition_id: &DefinitionId,
        occurrence_id: Option<OccurrenceId>,
        global_fraction: f64,
    ) -> ResolvedProcessValue {
        let mut result = ResolvedProcessValue {
            fraction: global_fraction,
            provenance: None,
        };
        for occurrence_scope in [false, true] {
            for layer in &self.layers {
                let matches = match &layer.scope {
                    SurfaceScope::Definition { definition_id: id } => {
                        !occurrence_scope && id == definition_id
                    }
                    SurfaceScope::Occurrence {
                        occurrence_id: id,
                        expected_definition_id,
                    } => {
                        occurrence_scope
                            && Some(*id) == occurrence_id
                            && expected_definition_id == definition_id
                    }
                };
                if !layer.enabled || !matches {
                    continue;
                }
                let mut contribution = None;
                for operation in &layer.operations {
                    contribution = match operation.action {
                        ProcessAction::Replace {
                            value: ProcessValue::SparseFraction { fraction },
                        } => Some(ResolvedProcessValue {
                            fraction,
                            provenance: Some(ProcessProvenance {
                                layer_id: layer.layer_id,
                                operation_id: operation.operation_id,
                            }),
                        }),
                        ProcessAction::Erase => None,
                    };
                }
                if let Some(value) = contribution {
                    result = value;
                }
            }
        }
        result
    }
    pub fn allocated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.layers.capacity() * std::mem::size_of::<ProcessLayer>()
            + self
                .layers
                .iter()
                .map(|layer| {
                    layer.operations.capacity() * std::mem::size_of::<ProcessOperation>()
                        + match &layer.scope {
                            SurfaceScope::Definition { definition_id } => {
                                definition_id.allocated_bytes()
                            }
                            SurfaceScope::Occurrence {
                                expected_definition_id,
                                ..
                            } => expected_definition_id.allocated_bytes(),
                        }
                })
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessEdit {
    SetSparseFraction {
        scope: SurfaceScope,
        #[serde(deserialize_with = "sparse_fraction")]
        fraction: f64,
    },
    AddLayer {
        scope: SurfaceScope,
        selector: ProcessSelector,
        action: ProcessAction,
    },
    RemoveLayer {
        layer_id: ProcessLayerId,
    },
    SetLayerEnabled {
        layer_id: ProcessLayerId,
        enabled: bool,
    },
    MoveLayer {
        layer_id: ProcessLayerId,
        index: u32,
    },
    AddOperation {
        layer_id: ProcessLayerId,
        selector: ProcessSelector,
        action: ProcessAction,
    },
    UpdateOperation {
        operation_id: ProcessOperationId,
        selector: ProcessSelector,
        action: ProcessAction,
    },
    RemoveOperation {
        operation_id: ProcessOperationId,
    },
    MoveOperation {
        operation_id: ProcessOperationId,
        index: u32,
    },
    Reset {
        scope: SurfaceScope,
    },
}
impl ProcessEdit {
    pub fn validate(&self) -> Result<(), ProjectError> {
        match self {
            Self::SetSparseFraction { fraction, .. } => ProcessValue::SparseFraction {
                fraction: *fraction,
            }
            .validate(),
            Self::AddLayer { action, .. }
            | Self::AddOperation { action, .. }
            | Self::UpdateOperation { action, .. } => action.validate(),
            _ => Ok(()),
        }
    }
}
fn sparse_fraction<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let fraction = f64::deserialize(d)?;
    ProcessValue::SparseFraction { fraction }
        .validate()
        .map_err(serde::de::Error::custom)?;
    Ok(fraction)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedOccurrenceProcess {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub global_fraction: f64,
    pub inherited: ResolvedProcessValue,
    pub value: ResolvedProcessValue,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layer(id: u32, scope: SurfaceScope, actions: &[ProcessAction]) -> ProcessLayer {
        ProcessLayer {
            layer_id: ProcessLayerId::new(id).unwrap(),
            scope,
            enabled: true,
            operations: actions
                .iter()
                .enumerate()
                .map(|(index, action)| ProcessOperation {
                    operation_id: ProcessOperationId::new(id * 10 + index as u32).unwrap(),
                    selector: ProcessSelector::WholeOwnedVolume,
                    action: *action,
                })
                .collect(),
        }
    }
    fn replace(fraction: f64) -> ProcessAction {
        ProcessAction::Replace {
            value: ProcessValue::SparseFraction { fraction },
        }
    }
    #[test]
    fn ordered_definition_occurrence_and_layer_local_erase_resolution() {
        let definition_id = DefinitionId::from_source_sha256(&[7; 32]);
        let occurrence_id = OccurrenceId::new(1).unwrap();
        let definition = SurfaceScope::Definition {
            definition_id: definition_id.clone(),
        };
        let occurrence = SurfaceScope::Occurrence {
            occurrence_id,
            expected_definition_id: definition_id.clone(),
        };
        let mut doc = ProcessDocument {
            schema_version: PROCESS_SCHEMA_VERSION,
            layers: vec![
                layer(1, occurrence, &[replace(0.6), ProcessAction::Erase]),
                layer(2, definition.clone(), &[replace(0.3)]),
            ],
        };
        doc.validate().unwrap();
        assert_eq!(
            doc.resolve(&definition_id, occurrence_id, 0.2).fraction,
            0.3
        );
        doc.layers[0].operations.pop();
        let resolved = doc.resolve(&definition_id, occurrence_id, 0.2);
        assert_eq!(resolved.fraction, 0.6);
        assert_eq!(
            resolved.provenance.unwrap().layer_id,
            doc.layers[0].layer_id
        );
        assert_eq!(doc.resolve_definition(&definition_id, 0.2).fraction, 0.3);
        doc.layers.push(layer(3, definition, &[replace(0.4)]));
        assert_eq!(doc.resolve_definition(&definition_id, 0.2).fraction, 0.4);
        assert_eq!(
            doc.resolve(&definition_id, occurrence_id, 0.2).fraction,
            0.6
        );
        doc.layers[0].enabled = false;
        assert_eq!(
            doc.resolve(&definition_id, occurrence_id, 0.2).fraction,
            0.4
        );
        doc.layers.reverse();
        assert_eq!(
            doc.resolve(&definition_id, occurrence_id, 0.2).fraction,
            0.3
        );
    }
    #[test]
    fn checked_process_json_rejects_unit_extras_invalid_values_and_budget_overflow() {
        for json in [
            r#"{"kind":"whole_owned_volume","future":1}"#,
            r#"{"kind":"whole_owned_volume","face_id":"face"}"#,
            r#"{"kind":"unknown"}"#,
        ] {
            assert!(serde_json::from_str::<ProcessSelector>(json).is_err());
        }
        for json in [
            r#"{"kind":"erase","value":{"kind":"sparse_fraction","fraction":0.5}}"#,
            r#"{"kind":"erase","future":1}"#,
            r#"{"kind":"replace","value":{"kind":"sparse_fraction","fraction":0}}"#,
            r#"{"kind":"replace","value":{"kind":"sparse_fraction","fraction":1.01}}"#,
        ] {
            assert!(serde_json::from_str::<ProcessAction>(json).is_err());
        }
        let scope = SurfaceScope::Definition {
            definition_id: DefinitionId::from_source_sha256(&[9; 32]),
        };
        let set = ProcessEdit::SetSparseFraction {
            scope,
            fraction: 0.5,
        };
        let valid = serde_json::to_value(&set).unwrap();
        assert_eq!(
            serde_json::from_value::<ProcessEdit>(valid.clone()).unwrap(),
            set
        );
        for fraction in [
            serde_json::json!(0),
            serde_json::json!(-0.1),
            serde_json::json!(1.01),
            serde_json::Value::Null,
            serde_json::json!("0.5"),
        ] {
            let mut invalid = valid.clone();
            invalid["fraction"] = fraction;
            assert!(serde_json::from_value::<ProcessEdit>(invalid).is_err());
        }
        let mut extra = valid;
        extra["selector"] = serde_json::json!({"kind":"whole_owned_volume"});
        assert!(serde_json::from_value::<ProcessEdit>(extra).is_err());
        assert!(
            ProcessEdit::try_from(crate::rpc::ProcessEdit {
                op: Some(crate::rpc::process_edit::Op::SetSparseFraction(
                    crate::rpc::SetProcessSparseFraction {
                        scope: None,
                        fraction: 0.5
                    }
                ))
            })
            .is_err()
        );
        assert!(
            ProcessEdit::try_from(crate::rpc::ProcessEdit {
                op: Some(crate::rpc::process_edit::Op::SetSparseFraction(
                    crate::rpc::SetProcessSparseFraction {
                        scope: Some(
                            SurfaceScope::Definition {
                                definition_id: DefinitionId::from_source_sha256(&[9; 32])
                            }
                            .try_into()
                            .unwrap()
                        ),
                        fraction: f64::NAN
                    }
                ))
            })
            .is_err()
        );
        for fraction in [0.0, -0.1, 1.1, f64::NAN, f64::INFINITY] {
            assert!(replace(fraction).validate().is_err());
        }
        assert!(ProcessLayerId::new(0).is_err());
        assert!(ProcessOperationId::new(0).is_err());
        let scope = SurfaceScope::Definition {
            definition_id: DefinitionId::from_source_sha256(&[9; 32]),
        };
        let doc = ProcessDocument {
            schema_version: PROCESS_SCHEMA_VERSION,
            layers: vec![layer(1, scope.clone(), &[replace(0.5)])],
        };
        let wire: crate::rpc::ProcessDocument = doc.clone().try_into().unwrap();
        assert_eq!(ProcessDocument::try_from(wire).unwrap(), doc);
        let mut duplicate = doc.clone();
        duplicate.layers.push(doc.layers[0].clone());
        assert!(duplicate.validate().is_err());
        let mut over = serde_json::to_value(&doc).unwrap();
        over["layers"] = serde_json::json!(vec![&doc.layers[0]; MAX_PROCESS_LAYERS + 1]);
        assert!(serde_json::from_value::<ProcessDocument>(over).is_err());
        let mut over = serde_json::to_value(&doc).unwrap();
        over["layers"][0]["operations"] = serde_json::json!(vec![
            &doc.layers[0].operations[0];
            MAX_PROCESS_OPERATIONS + 1
        ]);
        assert!(serde_json::from_value::<ProcessDocument>(over).is_err());
        assert!(
            ProcessDocument::try_from(crate::rpc::ProcessDocument {
                schema_version: PROCESS_SCHEMA_VERSION,
                layers: vec![crate::rpc::ProcessLayer {
                    layer_id: 1,
                    scope: Some(scope.try_into().unwrap()),
                    enabled: true,
                    operations: vec![crate::rpc::ProcessOperation {
                        operation_id: 1,
                        selector: None,
                        action: None
                    }]
                }]
            })
            .is_err()
        );
        assert!(ProcessDocument::try_from(crate::rpc::ProcessDocument::default()).is_err());
    }
    #[test]
    fn every_process_edit_uses_checked_projects_native_roundtrip() {
        use crate::{
            geometry::SessionId,
            native::{ProjectCall, project_call},
            project::{ProjectCommand, ProjectRevision},
        };
        let layer_id = ProcessLayerId::new(1).unwrap();
        let operation_id = ProcessOperationId::new(1).unwrap();
        let scope = SurfaceScope::Definition {
            definition_id: DefinitionId::from_source_sha256(&[9; 32]),
        };
        for edit in [
            ProcessEdit::SetSparseFraction {
                scope: scope.clone(),
                fraction: 0.4,
            },
            ProcessEdit::AddLayer {
                scope: scope.clone(),
                selector: ProcessSelector::WholeOwnedVolume,
                action: replace(0.4),
            },
            ProcessEdit::RemoveLayer { layer_id },
            ProcessEdit::SetLayerEnabled {
                layer_id,
                enabled: false,
            },
            ProcessEdit::MoveLayer { layer_id, index: 0 },
            ProcessEdit::AddOperation {
                layer_id,
                selector: ProcessSelector::WholeOwnedVolume,
                action: ProcessAction::Erase,
            },
            ProcessEdit::UpdateOperation {
                operation_id,
                selector: ProcessSelector::WholeOwnedVolume,
                action: replace(0.5),
            },
            ProcessEdit::RemoveOperation { operation_id },
            ProcessEdit::MoveOperation {
                operation_id,
                index: 0,
            },
            ProcessEdit::Reset { scope },
        ] {
            let command = ProjectCommand::EditProcess {
                session_id: SessionId::new(),
                base_revision: ProjectRevision(5),
                edit,
            };
            let ProjectCall::Execute(wire) = project_call(command.clone(), "").unwrap() else {
                panic!("process edit must use Projects.Execute")
            };
            assert_eq!(ProjectCommand::try_from(wire).unwrap(), command);
        }
        assert!(ProcessEdit::try_from(crate::rpc::ProcessEdit { op: None }).is_err());
        assert!(ProcessAction::try_from(crate::rpc::ProcessAction { kind: None }).is_err());
        assert!(ProcessSelector::try_from(crate::rpc::ProcessSelector { kind: None }).is_err());
        assert!(
            ProcessValue::try_from(crate::rpc::ProcessValue {
                kind: Some(crate::rpc::process_value::Kind::SparseFraction(
                    crate::rpc::SparseFraction { fraction: f64::NAN }
                ))
            })
            .is_err()
        );
    }
}

#[cfg(test)]
mod public_state_tests {
    use super::*;
    use crate::project::{ProjectId, ProjectInfo, ProjectRevision};
    #[test]
    fn checked_project_state_requires_process_document_and_authoritative_inheritance() {
        let definition_id = DefinitionId::from_source_sha256(&[3; 32]);
        let occurrence_id = OccurrenceId::new(1).unwrap();
        let document = ProcessDocument {
            schema_version: PROCESS_SCHEMA_VERSION,
            layers: vec![ProcessLayer {
                layer_id: ProcessLayerId::new(1).unwrap(),
                scope: SurfaceScope::Definition {
                    definition_id: definition_id.clone(),
                },
                enabled: true,
                operations: vec![ProcessOperation {
                    operation_id: ProcessOperationId::new(1).unwrap(),
                    selector: ProcessSelector::WholeOwnedVolume,
                    action: ProcessAction::Replace {
                        value: ProcessValue::SparseFraction { fraction: 0.3 },
                    },
                }],
            }],
        };
        let info = ProjectInfo {
            project_id: ProjectId::new(),
            revision: ProjectRevision(1),
            saved_revision: None,
            path_label: None,
            dirty: true,
            save_uncertain: false,
            read_only: false,
            recovered_previous: false,
            can_undo: true,
            can_redo: false,
            surface_overlays: crate::surface::SurfaceOverlays::empty(),
            resolved_process: vec![ResolvedOccurrenceProcess {
                occurrence_id,
                definition_id: definition_id.clone(),
                global_fraction: 0.2,
                inherited: document.resolve_definition(&definition_id, 0.2),
                value: document.resolve(&definition_id, occurrence_id, 0.2),
            }],
            process_document: document,
        };
        let wire: crate::rpc::ProjectInfo = info.clone().try_into().unwrap();
        assert_eq!(ProjectInfo::try_from(wire.clone()).unwrap(), info);
        let mut missing = wire.clone();
        missing.process_document = None;
        assert!(ProjectInfo::try_from(missing).is_err());
        let mut wrong = wire;
        wrong.resolved_process[0]
            .inherited
            .as_mut()
            .unwrap()
            .fraction = 0.2;
        assert!(ProjectInfo::try_from(wrong).is_err());
        let mut wrong = info;
        wrong.resolved_process[0].value.provenance = None;
        assert!(crate::rpc::ProjectInfo::try_from(wrong).is_err());
    }
}
