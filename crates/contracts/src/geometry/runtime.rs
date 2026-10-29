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
        project_revision: crate::project::ProjectRevision,
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
    PrepareBedPose {
        session_id: SessionId,
        base_revision: SceneRevision,
        project_revision: crate::project::ProjectRevision,
        occurrence_id: OccurrenceId,
        expected_definition_id: DefinitionId,
        expected_pose: RigidPoseMm,
        envelope_mm: AabbMm,
        center_xy: bool,
        contact_tolerance_mm: f64,
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
    BedPosePrepared {
        preparation: BedPosePreparation,
    },
    ProjectSaved {
        info: crate::project::ProjectInfo,
    },
    SurfaceImageImported {
        info: crate::project::ProjectInfo,
        image: crate::surface::SurfaceImageRecord,
        layer_id: crate::surface::SurfaceLayerId,
    },
    SurfaceImagePrepared {
        project_id: crate::project::ProjectId,
        captured_revision: crate::project::ProjectRevision,
        record: crate::surface::SurfaceImageRecord,
        resource: crate::ArtifactView,
    },
    AppearancePaletteImported {
        info: crate::project::ProjectInfo,
        palette: crate::appearance::PaletteRecord,
    },
    AppearanceCompared {
        project_id: crate::project::ProjectId,
        captured_revision: crate::project::ProjectRevision,
        record: crate::appearance::AppearanceReportRecord,
        resource: crate::ArtifactView,
    },
    OpticalCompared {
        project_id: crate::project::ProjectId,
        captured_revision: crate::project::ProjectRevision,
        record: crate::optical::OpticalReportRecord,
        resource: crate::ArtifactView,
    },
    ManufacturingPreflight {
        report: crate::manufacturing::PreflightReport,
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
        provenance: crate::manufacturing::ManufacturingInspectionProvenance,
        replay_display: Option<crate::manufacturing::ReplayDisplayDescriptor>,
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
#[expect(
    clippy::large_enum_variant,
    reason = "Bounded control replies stay inline without a per-operation allocation"
)]
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

/// Read-only native contact evidence; applying the pose still requires both
/// captured revisions. Bounds enclose the original posed source, not a mesh.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BedPosePreparation {
    pub session_id: SessionId,
    pub scene_revision: SceneRevision,
    pub project_revision: crate::project::ProjectRevision,
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub original_pose: RigidPoseMm,
    pub pose: RigidPoseMm,
    pub envelope_mm: AabbMm,
    pub minimum_lower_mm: [f64; 3],
    pub minimum_upper_mm: [f64; 3],
    pub maximum_lower_mm: [f64; 3],
    pub maximum_upper_mm: [f64; 3],
    pub center_xy: bool,
    pub contact_tolerance_mm: f64,
    pub maximum_contact_gap_mm: f64,
}

impl BedPosePreparation {
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.original_pose.validate()?;
        self.pose.validate()?;
        self.envelope_mm.validate()?;
        if self.pose.rotation_xyzw != self.original_pose.rotation_xyzw
            || !self.contact_tolerance_mm.is_finite()
            || self.contact_tolerance_mm <= 0.0
            || self.contact_tolerance_mm > SECTION_PLANE_TOLERANCE_MM
            || !self.maximum_contact_gap_mm.is_finite()
            || self.maximum_contact_gap_mm < 0.0
            || self.maximum_contact_gap_mm > self.contact_tolerance_mm
        {
            return Err(invalid("invalid native bed-contact evidence"));
        }
        for axis in 0..3 {
            let values = [
                self.minimum_lower_mm[axis],
                self.minimum_upper_mm[axis],
                self.maximum_lower_mm[axis],
                self.maximum_upper_mm[axis],
            ];
            if values.iter().any(|value| !value.is_finite())
                || values[0] > values[1]
                || values[2] > values[3]
                || values[0] > values[3]
                || self.envelope_mm.min[axis] >= self.envelope_mm.max[axis]
            {
                return Err(invalid("invalid native support bracket"));
            }
            let delta = rounded_sum_interval(
                self.pose.translation_mm[axis],
                -self.original_pose.translation_mm[axis],
            );
            let placed_min = rounded_sum_interval(values[0], delta[0])[0];
            let placed_max = rounded_sum_interval(values[3], delta[1])[1];
            if delta.iter().any(|value| !value.is_finite())
                || !placed_min.is_finite()
                || !placed_max.is_finite()
                || placed_min < self.envelope_mm.min[axis]
                || placed_max > self.envelope_mm.max[axis]
                || (axis < 2
                    && !self.center_xy
                    && self.pose.translation_mm[axis] != self.original_pose.translation_mm[axis])
            {
                return Err(invalid("prepared native pose exceeds its envelope"));
            }
        }
        let delta = rounded_sum_interval(
            self.pose.translation_mm[2],
            -self.original_pose.translation_mm[2],
        );
        let gap = rounded_sum_interval(
            rounded_sum_interval(self.minimum_upper_mm[2], delta[1])[1],
            -self.envelope_mm.min[2],
        )[1];
        if !gap.is_finite() || gap < 0.0 || gap > self.maximum_contact_gap_mm {
            return Err(invalid("prepared native contact gap is not enclosed"));
        }
        Ok(())
    }
}

/// Enclose the exact sum of binary64 millimetre translation operands.
///
/// Error-free TwoSum preserves exact contact and directs only actual roundoff
/// outward. Callers must reject nonfinite inputs/results before publication.
pub fn rounded_sum_interval(a: f64, b: f64) -> [f64; 2] {
    let sum = a + b;
    if !sum.is_finite() {
        return [sum, sum];
    }
    let shifted = sum - a;
    let residual = (a - (sum - shifted)) + (b - shifted);
    if residual > 0.0 {
        [sum, sum.next_up()]
    } else if residual < 0.0 {
        [sum.next_down(), sum]
    } else {
        [sum, sum]
    }
}
