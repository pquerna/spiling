// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Persistent source-backed project contracts, independent of storage and BREP.
mod layouts;
pub use layouts::*;
mod manufacturing_options;
mod recovery;
mod storage;
use crate::appearance::{AppearanceStudy, AppearanceStudyInfo, AppearanceTarget, SearchLimits};
use crate::geometry::*;
use crate::manufacturing::{
    ApplyOpticalConstruction, ManufacturingArtifactRecord, ManufacturingIntent, input_fingerprint,
};
use crate::optical::{OpticalRequest, OpticalSelection};
use crate::process::*;
use crate::surface::*;
pub use manufacturing_options::*;
pub use recovery::ProjectRecoveryGuard;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};
pub use storage::{ProjectStorageLimits, ProjectStorageStats, StorageMaintenanceAction};
use ts_rs::TS;

pub const PROJECT_FORMAT_VERSION: u32 = 10;
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
    pub next_surface_layer: u32,
    pub surface_overlays: SurfaceOverlays,
    pub next_process_layer: u32,
    pub next_process_operation: u32,
    pub process_document: ProcessDocument,
    pub appearance_study: AppearanceStudy,
    pub analysis: crate::analysis::AnalysisDocument,
    #[serde(deserialize_with = "required_option")]
    pub materials: Option<crate::library::MaterialCollectionRecord>,
    pub definitions: Vec<StoredDefinition>,
    pub layouts: Vec<PrintLayoutRecord>,
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
        let records: Vec<_> = self
            .layouts
            .iter()
            .flat_map(|l| l.occurrences.iter().cloned())
            .collect();
        if self.layouts.is_empty()
            || self.layouts.len() > MAX_PRINT_LAYOUTS
            || self.definitions.len() > MAX_DEFINITIONS as usize
            || records.len() > MAX_OCCURRENCES as usize
        {
            return Err(ProjectError::new(
                ProjectErrorCode::ResourceLimit,
                "project record budget exceeded",
            ));
        }
        let mut layouts = BTreeSet::new();
        for layout in &self.layouts {
            layout.validate(
                &self.project_id,
                &self.definitions,
                &self.surface_overlays,
                &self.process_document,
            )?;
            if !layouts.insert(&layout.layout_id) {
                return Err(invalid("duplicate print layout identity"));
            }
        }
        if let Some(materials) = &self.materials {
            materials.validate().map_err(|e| invalid(&e))?;
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
        for occurrence in &records {
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
        self.surface_overlays.validate_targets(
            &self.definitions,
            &records,
            self.next_surface_layer,
        )?;
        self.process_document.validate_targets(
            &self.definitions,
            &records,
            self.next_process_layer,
            self.next_process_operation,
        )?;
        self.appearance_study.validate()?;
        self.analysis
            .validate_subjects(&self.definitions, &self.layouts, self.revision)?;
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectInfo {
    pub project_id: ProjectId,
    pub revision: ProjectRevision,
    pub layout_id: PrintLayoutId,
    pub layouts: Vec<PrintLayoutInfo>,
    pub saved_revision: Option<ProjectRevision>,
    pub path_label: Option<String>,
    pub dirty: bool,
    pub save_uncertain: bool,
    pub read_only: bool,
    pub recovered_previous: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub surface_overlays: SurfaceOverlays,
    pub process_document: ProcessDocument,
    pub appearance_study: AppearanceStudyInfo,
    pub analysis: crate::analysis::AnalysisDocument,
    pub materials: Option<crate::library::MaterialCollectionRecord>,
    #[serde(deserialize_with = "required_option")]
    pub hardware: Option<Arc<crate::hardware::ProjectHardwareCapture>>,
    pub resolved_process: Vec<ResolvedOccurrenceProcess>,
    pub manufacturing_options: Vec<ManufacturingOptionInfo>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectCommand {
    EditAnalysis {
        session_id: SessionId,
        base_revision: ProjectRevision,
        edit: crate::analysis::AnalysisEdit,
    },
    SelectPrintLayout {
        session_id: SessionId,
        base_revision: ProjectRevision,
        layout_id: PrintLayoutId,
    },
    EditPrintLayout {
        session_id: SessionId,
        base_revision: ProjectRevision,
        edit: PrintLayoutEdit,
    },
    GetManufacturingOption {
        layout_id: PrintLayoutId,
        session_id: SessionId,
        base_revision: ProjectRevision,
        id: SourceHash,
    },
    RetainManufacturingOption {
        layout_id: PrintLayoutId,
        session_id: SessionId,
        base_revision: ProjectRevision,
        option: ManufacturingOption,
    },
    RemoveManufacturingOption {
        layout_id: PrintLayoutId,
        session_id: SessionId,
        base_revision: ProjectRevision,
        id: SourceHash,
    },
    ApplyManufacturingOption {
        layout_id: PrintLayoutId,
        session_id: SessionId,
        base_revision: ProjectRevision,
        id: SourceHash,
    },
    MatchStock {
        session_id: SessionId,
        base_revision: ProjectRevision,
        query: crate::stock::StockQuery,
    },
    SetHardware {
        layout_id: PrintLayoutId,
        session_id: SessionId,
        base_revision: ProjectRevision,
        capture: Option<Arc<crate::hardware::ProjectHardwareCapture>>,
    },
    PutRecipe {
        session_id: SessionId,
        base_revision: ProjectRevision,
        transfer: crate::library::RecipeTransfer,
        expected: Option<SourceHash>,
    },
    GetRecipe {
        session_id: SessionId,
        base_revision: ProjectRevision,
        reference: crate::library::RecipeRef,
    },
    ListRecipes {
        session_id: SessionId,
        base_revision: ProjectRevision,
        offset: u32,
    },
    LoadRecipeStudy {
        session_id: SessionId,
        base_revision: ProjectRevision,
        reference: crate::library::RecipeRef,
        context: crate::library::RecipeStudyContext,
    },
    CaptureRecipe {
        session_id: SessionId,
        base_revision: ProjectRevision,
        id: crate::library::RecipeId,
        name: String,
        ingredients: Vec<crate::library::RecipeIngredient>,
        expected: Option<SourceHash>,
    },
    PutFilament {
        session_id: SessionId,
        base_revision: ProjectRevision,
        revision: crate::library::FilamentRevision,
        origin: Option<crate::library::LibraryOrigin>,
        expected: Option<SourceHash>,
    },
    GetFilament {
        session_id: SessionId,
        base_revision: ProjectRevision,
        reference: crate::library::FilamentRef,
    },
    ListFilaments {
        session_id: SessionId,
        base_revision: ProjectRevision,
        offset: u32,
    },
    StorageMaintenance {
        session_id: SessionId,
        base_revision: ProjectRevision,
        action: StorageMaintenanceAction,
        target: Option<NativePath>,
    },
    ApplyOpticalConstruction {
        session_id: SessionId,
        base_revision: ProjectRevision,
        application: ApplyOpticalConstruction,
    },
    ImportAppearancePalette {
        session_id: SessionId,
        base_revision: ProjectRevision,
        source: NativePath,
    },
    SetAppearanceTarget {
        session_id: SessionId,
        base_revision: ProjectRevision,
        target: AppearanceTarget,
        limits: SearchLimits,
    },
    CompareAppearance {
        session_id: SessionId,
        base_revision: ProjectRevision,
    },
    AttachAnalysisResult {
        session_id: SessionId,
        base_revision: ProjectRevision,
        input_hash: SourceHash,
    },
    GetAnalysisResult {
        session_id: SessionId,
        base_revision: ProjectRevision,
        case_id: crate::analysis::AnalysisCaseId,
    },
    RunAnalysis {
        session_id: SessionId,
        base_revision: ProjectRevision,
        case_id: crate::analysis::AnalysisCaseId,
        settings: Arc<crate::analysis::mechanics::AnalysisRunSettings>,
    },
    AcceptAppearanceReport {
        session_id: SessionId,
        base_revision: ProjectRevision,
        report_hash: SourceHash,
        selection: Option<String>,
    },
    GetAppearanceReport {
        session_id: SessionId,
        base_revision: ProjectRevision,
    },
    SetOpticalStudy {
        session_id: SessionId,
        base_revision: ProjectRevision,
        request: OpticalRequest,
    },
    CompareOptical {
        session_id: SessionId,
        base_revision: ProjectRevision,
    },
    GetOpticalReport {
        session_id: SessionId,
        base_revision: ProjectRevision,
    },
    AcceptOpticalReport {
        session_id: SessionId,
        base_revision: ProjectRevision,
        report_hash: SourceHash,
        selection: Option<OpticalSelection>,
    },
    EditProcess {
        session_id: SessionId,
        base_revision: ProjectRevision,
        edit: ProcessEdit,
    },
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
    ImportSurfaceImageFinish {
        session_id: SessionId,
        base_revision: ProjectRevision,
        reference: FaceRef,
        scope: SurfaceScopeKind,
        source: NativePath,
        parameters: ImageReliefParameters,
    },
    GetSurfaceImage {
        session_id: SessionId,
        base_revision: ProjectRevision,
        image_hash: SourceHash,
    },
    AddSurfaceFinish {
        session_id: SessionId,
        base_revision: ProjectRevision,
        face_ref: FaceRef,
        scope: SurfaceScopeKind,
        finish: SurfaceFinish,
    },
    RemoveSurfaceFinish {
        session_id: SessionId,
        base_revision: ProjectRevision,
        layer_id: SurfaceLayerId,
    },
    SetSurfaceFinishEnabled {
        session_id: SessionId,
        base_revision: ProjectRevision,
        layer_id: SurfaceLayerId,
        enabled: bool,
    },
}
#[expect(
    clippy::large_enum_variant,
    reason = "Bounded control replies stay inline; boxing adds an allocation to each operation observation"
)]
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectResponse {
    ManufacturingOption {
        option: ManufacturingOption,
    },
    Recipe {
        transfer: crate::library::RecipeTransfer,
    },
    Recipes {
        entries: Vec<crate::library::RecipeSummary>,
        total: u32,
        next_offset: Option<u32>,
    },
    Filaments {
        entries: Vec<crate::library::LibraryFilamentSummary>,
        total: u32,
        next_offset: Option<u32>,
    },
    Filament {
        revision: crate::library::FilamentRevision,
    },
    Status {
        info: ProjectInfo,
    },
    SceneChanged {
        info: ProjectInfo,
        summary: SceneSummary,
    },
    OperationAccepted {
        operation: crate::NativeOperationView,
    },
    Error {
        error: ProjectError,
    },
}
#[cfg(test)]
mod tests;
