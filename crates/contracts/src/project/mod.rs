// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Persistent source-backed project contracts, independent of storage and BREP.
use crate::geometry::*;
use crate::manufacturing::{ManufacturingArtifactRecord, ManufacturingIntent, input_fingerprint};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};
use ts_rs::TS;

pub const PROJECT_FORMAT_VERSION: u32 = 2;
pub const MAX_PROJECT_MANIFEST_BYTES: u32 = 1_048_576;
pub const MAX_PROJECT_HISTORY: usize = 64;
pub const MAX_PROJECT_STORED_SOURCE_BYTES: u64 = 268_435_456;
pub const MAX_PROJECT_STORED_ASSETS: usize = 128;
pub const MAX_PROJECT_ERROR_BYTES: usize = 2048;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectErrorCode {
    InvalidProject,
    UnsupportedFormat,
    MissingAsset,
    CorruptAsset,
    ProjectLocked,
    ReadOnly,
    DirtyProject,
    NoSavedPath,
    NoUndo,
    NoRedo,
    StaleRevision,
    Busy,
    ResourceLimit,
    Cancelled,
    Io,
}
#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {message}")]
#[serde(deny_unknown_fields)]
pub struct ProjectError {
    pub code: ProjectErrorCode,
    pub message: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProjectError {
    code: ProjectErrorCode,
    message: String,
}
impl<'de> Deserialize<'de> for ProjectError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawProjectError::deserialize(deserializer)?;
        if raw.message.len() > MAX_PROJECT_ERROR_BYTES {
            return Err(serde::de::Error::custom(
                "project error message exceeds limit",
            ));
        }
        Ok(Self {
            code: raw.code,
            message: raw.message,
        })
    }
}
impl ProjectError {
    pub fn new(code: ProjectErrorCode, message: impl AsRef<str>) -> Self {
        let message = message.as_ref();
        let mut end = message.len().min(MAX_PROJECT_ERROR_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            code,
            message: message[..end].to_owned(),
        }
    }
}
fn invalid(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::InvalidProject, message)
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
#[ts(type = "string")]
pub struct ProjectId(String);
impl ProjectId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }
    pub fn parse(value: impl Into<String>) -> Result<Self, ProjectError> {
        let value = value.into();
        let uuid = uuid::Uuid::parse_str(&value).map_err(|_| invalid("invalid project UUID"))?;
        let mut canonical = [0; 36];
        if uuid.is_nil()
            || uuid.hyphenated().encode_lower(&mut canonical).as_bytes() != value.as_bytes()
        {
            return Err(invalid("project UUID must be canonical and nonnil"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}
impl<'de> Deserialize<'de> for ProjectId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
)]
#[serde(transparent)]
#[ts(type = "number")]
pub struct ProjectRevision(pub u32);
impl ProjectRevision {
    pub const ZERO: Self = Self(0);
    pub fn get(self) -> u32 {
        self.0
    }
    pub fn next(self) -> Result<Self, ProjectError> {
        self.0.checked_add(1).map(Self).ok_or_else(|| {
            ProjectError::new(
                ProjectErrorCode::ResourceLimit,
                "project revision exhausted",
            )
        })
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
pub enum ProjectUnits {
    #[serde(rename = "mm")]
    Millimetres,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFrame {
    RightHanded,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StoredDefinition {
    pub definition_id: DefinitionId,
    pub provenance: SourceProvenance,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    pub format_version: u32,
    pub project_id: ProjectId,
    pub revision: ProjectRevision,
    pub units: ProjectUnits,
    pub frame: ProjectFrame,
    pub next_occurrence: u32,
    pub definitions: Vec<StoredDefinition>,
    pub occurrences: Vec<OccurrenceRecord>,
    #[serde(deserialize_with = "required_option")]
    pub manufacturing_intent: Option<Arc<ManufacturingIntent>>,
    #[serde(deserialize_with = "required_option")]
    pub manufacturing_artifact: Option<ManufacturingArtifactRecord>,
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
impl ProjectManifest {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.format_version != PROJECT_FORMAT_VERSION {
            return Err(ProjectError::new(
                ProjectErrorCode::UnsupportedFormat,
                "unsupported project manifest format",
            ));
        }
        if self.definitions.len() > MAX_DEFINITIONS as usize
            || self.occurrences.len() > MAX_OCCURRENCES as usize
        {
            return Err(ProjectError::new(
                ProjectErrorCode::ResourceLimit,
                "project record budget exceeded",
            ));
        }
        let mut definitions = BTreeSet::new();
        for definition in &self.definitions {
            definition
                .provenance
                .validate()
                .map_err(|error| invalid(&error.message))?;
            let hash = definition.provenance.source_hash.as_str();
            let mut bytes = [0u8; 32];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hash[index * 2..index * 2 + 2], 16)
                    .map_err(|_| invalid("invalid source hash"))?;
            }
            if DefinitionId::from_source_sha256(&bytes) != definition.definition_id
                || !definitions.insert(&definition.definition_id)
            {
                return Err(invalid(
                    "duplicate or source-inconsistent definition identity",
                ));
            }
        }
        let mut occurrences = BTreeSet::new();
        let mut used = BTreeSet::new();
        for occurrence in &self.occurrences {
            occurrence
                .pose
                .validate()
                .map_err(|error| invalid(&error.message))?;
            if !definitions.contains(&occurrence.definition_id)
                || !occurrences.insert(occurrence.occurrence_id)
                || occurrence.occurrence_id.get() >= self.next_occurrence
            {
                return Err(invalid(
                    "invalid project occurrence reference, identity or counter",
                ));
            }
            used.insert(&occurrence.definition_id);
        }
        if self.next_occurrence == 0 || definitions != used {
            return Err(invalid(
                "invalid next occurrence counter or unreferenced definition",
            ));
        }
        if let Some(intent) = &self.manufacturing_intent {
            intent.validate().map_err(|e| invalid(&e.message))?;
        }
        if let Some(record) = &self.manufacturing_artifact {
            if self.definitions.is_empty() || self.occurrences.is_empty() {
                return Err(invalid(
                    "manufacturing artifact requires nonempty project geometry",
                ));
            }
            record.validate().map_err(|e| invalid(&e.message))?;
            let intent = self
                .manufacturing_intent
                .as_ref()
                .ok_or_else(|| invalid("manufacturing artifact requires intent"))?;
            let hash = input_fingerprint(
                &self.project_id,
                &self.definitions,
                &self.occurrences,
                intent,
            )
            .map_err(|e| invalid(&e.message))?;
            if hash != record.input_hash {
                return Err(invalid("manufacturing artifact input hash is not current"));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectInfo {
    pub project_id: ProjectId,
    pub revision: ProjectRevision,
    pub saved_revision: Option<ProjectRevision>,
    pub path_label: Option<String>,
    pub dirty: bool,
    pub save_uncertain: bool,
    pub read_only: bool,
    pub recovered_previous: bool,
    pub can_undo: bool,
    pub can_redo: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectCommand {
    Get {
        session_id: SessionId,
    },
    New {
        session_id: SessionId,
        base_revision: SceneRevision,
        discard_changes: bool,
    },
    Open {
        session_id: SessionId,
        base_revision: SceneRevision,
        path: NativePath,
        read_only: bool,
        recover_previous: bool,
        discard_changes: bool,
    },
    Save {
        session_id: SessionId,
        base_revision: SceneRevision,
        target: Option<NativePath>,
    },
    Undo {
        session_id: SessionId,
        base_revision: SceneRevision,
    },
    Redo {
        session_id: SessionId,
        base_revision: SceneRevision,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectResponse {
    Status {
        info: ProjectInfo,
    },
    SceneChanged {
        info: ProjectInfo,
        summary: SceneSummary,
    },
    JobAccepted {
        job_id: JobId,
    },
    Error {
        error: ProjectError,
    },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPathIntent {
    Open,
    Save,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectedProjectPath {
    pub token: String,
    pub session_id: SessionId,
    pub label: String,
    pub intent: ProjectPathIntent,
}

#[cfg(test)]
mod tests;
