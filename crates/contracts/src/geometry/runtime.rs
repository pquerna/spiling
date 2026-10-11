// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded scene, shared engine jobs and artifact control contracts.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KernelIdentity {
    pub name: String,
    pub version: String,
    pub revision: String,
}

/// Opaque shell-selected source; actual native paths remain private to the shell.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectedSource {
    pub token: String,
    pub session_id: SessionId,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum GeometryCommand {
    ImportPart {
        session_id: SessionId,
        base_revision: SceneRevision,
        source: NativePath,
        initial_pose: RigidPoseMm,
    },
    AddInstance {
        session_id: SessionId,
        base_revision: SceneRevision,
        definition_id: DefinitionId,
        pose: RigidPoseMm,
    },
    SetInstancePose {
        session_id: SessionId,
        base_revision: SceneRevision,
        occurrence_id: OccurrenceId,
        pose: RigidPoseMm,
    },
    RemoveInstance {
        session_id: SessionId,
        base_revision: SceneRevision,
        occurrence_id: OccurrenceId,
    },
    GetScene {
        session_id: SessionId,
    },
    GetScenePage {
        session_id: SessionId,
        revision: SceneRevision,
        kind: ScenePageKind,
        offset: u32,
    },
    GetFaceIndexPage {
        session_id: SessionId,
        definition_id: DefinitionId,
        offset: u32,
    },
    InspectFace {
        reference: FaceRef,
    },
    StartSection {
        session_id: SessionId,
        base_revision: SceneRevision,
        plane: PlaneMm,
    },
    GetArtifactPage {
        session_id: SessionId,
        artifact_id: ArtifactId,
        kind: ArtifactPageKind,
        offset: u32,
    },
    ReleaseArtifact {
        session_id: SessionId,
        artifact_id: ArtifactId,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScenePageKind {
    Definitions,
    Occurrences,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPageKind {
    Chunks,
    Loops,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobResult {
    Scene {
        summary: SceneSummary,
    },
    Section {
        artifact_id: ArtifactId,
    },
    ProjectSaved {
        info: crate::project::ProjectInfo,
    },
    ManufacturingCompiled {
        info: crate::project::ProjectInfo,
        record: crate::manufacturing::ManufacturingArtifactRecord,
        resource: crate::ArtifactView,
    },
    ManufacturingVerified {
        record: crate::manufacturing::ManufacturingArtifactRecord,
        report: crate::manufacturing::VerificationReport,
        resource: crate::ArtifactView,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "domain", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobError {
    Geometry {
        error: GeometryError,
    },
    Project {
        error: crate::project::ProjectError,
    },
    Manufacturing {
        error: crate::manufacturing::ManufacturingError,
    },
}
impl From<GeometryError> for JobError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry { error }
    }
}
impl From<crate::project::ProjectError> for JobError {
    fn from(error: crate::project::ProjectError) -> Self {
        Self::Project { error }
    }
}
impl From<crate::manufacturing::ManufacturingError> for JobError {
    fn from(error: crate::manufacturing::ManufacturingError) -> Self {
        Self::Manufacturing { error }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactSummary {
    Mesh {
        artifact_id: ArtifactId,
        definition_id: DefinitionId,
        chunk_count: u32,
        total_bytes: u32,
    },
    Section {
        summary: SectionSummary,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactChunkMetadata {
    Mesh { metadata: MeshChunkMetadata },
    Section { metadata: SectionChunkMetadata },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GeometryResponse {
    OperationAccepted {
        operation: crate::NativeOperationView,
    },
    SceneChanged {
        summary: SceneSummary,
    },
    Scene {
        summary: SceneSummary,
    },
    ScenePage {
        definitions: Vec<DefinitionRecord>,
        occurrences: Vec<OccurrenceRecord>,
        next_offset: Option<u32>,
    },
    FaceIndexPage {
        faces: Vec<FaceIndexRow>,
        next_offset: Option<u32>,
    },
    FaceInspection {
        inspection: FaceInspection,
    },
    ArtifactPage {
        summary: ArtifactSummary,
        chunks: Vec<ArtifactChunkMetadata>,
        loops: Vec<SectionLoopMetadata>,
        resources: Vec<crate::ArtifactView>,
        next_offset: Option<u32>,
    },
    Released {},
    Error {
        error: GeometryError,
    },
    ProjectError {
        error: crate::project::ProjectError,
    },
}
