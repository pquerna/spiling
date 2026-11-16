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

/// Explicit source interpretation choices. Defaults apply to new imports only.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelImportOptions {
    pub stl_unit: Option<SourceUnit>,
    pub source_plate: Option<u32>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceDisplayDemand {
    None,
    Preview,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum GeometryCommand {
    ImportPart {
        display: SourceDisplayDemand,
        session_id: SessionId,
        base_revision: SceneRevision,
        source: NativePath,
        options: ModelImportOptions,
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
    GetLayoutOverview {
        session_id: SessionId,
        scene_revision: SceneRevision,
        project_revision: crate::project::ProjectRevision,
    },
    GetLayoutOverviewPage {
        session_id: SessionId,
        scene_revision: SceneRevision,
        project_revision: crate::project::ProjectRevision,
        kind: LayoutOverviewPageKind,
        offset: u32,
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
    PrepareLayout {
        session_id: SessionId,
        base_revision: SceneRevision,
        project_revision: crate::project::ProjectRevision,
        request: LayoutRequest,
    },
    ApplyLayout {
        session_id: SessionId,
        base_revision: SceneRevision,
        project_revision: crate::project::ProjectRevision,
        proposal_id: SourceHash,
    },
    /// Rests the scene's parts on the bed as one group (one undo step), and
    /// centres it on the selected plate's usable region when asked or when
    /// the printer would not admit it where it is. `occurrence_id` places only
    /// that part. The one placement rule both clients use.
    PlaceOnBed {
        session_id: SessionId,
        base_revision: SceneRevision,
        project_revision: crate::project::ProjectRevision,
        occurrence_id: Option<OccurrenceId>,
        center_xy: bool,
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
pub enum LayoutOverviewPageKind {
    Definitions,
    Parts,
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
    AnalysisFastenerPrepared {
        preparation: crate::analysis::fastener::AnalysisFastenerPreparation,
    },
    AnalysisFacePrepared {
        preparation: crate::analysis::load::AnalysisFacePreparation,
    },
    AnalysisResultPrepared {
        project_id: crate::project::ProjectId,
        read_revision: crate::project::ProjectRevision,
        record: crate::analysis::AnalysisResultRecord,
        resource: crate::ArtifactView,
    },
    AnalysisCompleted {
        project_id: crate::project::ProjectId,
        captured_revision: crate::project::ProjectRevision,
        input_hash: SourceHash,
        resource: crate::ArtifactView,
    },
    ProjectStorageMaintained {
        action: crate::project::StorageMaintenanceAction,
        stats: crate::project::ProjectStorageStats,
    },
    OpticalConstructionApplied {
        info: crate::project::ProjectInfo,
        construction_index: u16,
    },
    Scene {
        summary: SceneSummary,
    },
    Section {
        artifact_id: ArtifactId,
    },
    LayoutPrepared {
        proposal: LayoutProposal,
    },
    BedPlaced {
        summary: SceneSummary,
        placement: BedGroupPlacement,
    },
    ProjectSaved {
        info: crate::project::ProjectInfo,
    },
    ProjectSaveUncertain {
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
    StockMatched {
        project_id: crate::project::ProjectId,
        captured_revision: crate::project::ProjectRevision,
        record: crate::stock::StockMatchRecord,
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
    ManufacturingAlternativeEvaluated {
        evaluation: crate::manufacturing::ManufacturingAlternativeEvaluation,
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
        delivery_package: Option<crate::manufacturing::DeliveryPackageDescriptor>,
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
    Library {
        error: crate::library::LibraryError,
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
    LayoutOverview {
        summary: LayoutOverviewSummary,
    },
    LayoutOverviewPage {
        definitions: Vec<DefinitionRecord>,
        parts: Vec<LayoutOverviewPart>,
        next_offset: Option<u32>,
    },
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BedGroupOutcome {
    /// Already resting on the bed where the printer admits it; nothing moved.
    Unchanged,
    /// Lowered or raised onto the bed; XY kept.
    Rested,
    /// Rested and centred on the selected plate's usable region.
    Centred,
    /// Does not fit the build envelope even when centred; nothing moved.
    DoesNotFit,
}
/// Outcome of `PlaceOnBed`: the group's placed bounds (after any move, or as
/// they would be when centred for `DoesNotFit`) and the envelope used.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BedGroupPlacement {
    pub outcome: BedGroupOutcome,
    pub placed_bounds_mm: AabbMm,
    pub build_envelope_mm: AabbMm,
}
impl BedGroupPlacement {
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.placed_bounds_mm.validate()?;
        self.build_envelope_mm.validate()?;
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
