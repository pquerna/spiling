// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native execution correlation only; durable public handles are Google Operations.
use spiling_contracts::{geometry::*, manufacturing::*, project::*};

pub(crate) enum Control {
    Geometry(GeometryCommand),
    Project(ProjectCommand),
    Manufacturing(ManufacturingCommand),
}

pub(crate) enum Reply {
    Geometry(GeometryResponse),
    Project(ProjectResponse),
    Manufacturing(ManufacturingResponse),
    Accepted(JobId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct JobId(u32);
impl JobId {
    pub fn new(value: u32) -> Result<Self, GeometryError> {
        if value == 0 {
            Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "JobId must be nonzero",
            ))
        } else {
            Ok(Self(value))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobStatus {
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}
impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JobResult {
    Scene {
        summary: SceneSummary,
    },
    Section {
        artifact_id: ArtifactId,
    },
    ProjectSaved {
        info: ProjectInfo,
    },
    ManufacturingCompiled {
        info: ProjectInfo,
        record: ManufacturingArtifactRecord,
    },
    ManufacturingVerified {
        record: ManufacturingArtifactRecord,
        report: VerificationReport,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JobError {
    Geometry { error: GeometryError },
    Project { error: ProjectError },
    Manufacturing { error: ManufacturingError },
}
impl From<GeometryError> for JobError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry { error }
    }
}
impl From<ProjectError> for JobError {
    fn from(error: ProjectError) -> Self {
        Self::Project { error }
    }
}
impl From<ManufacturingError> for JobError {
    fn from(error: ManufacturingError) -> Self {
        Self::Manufacturing { error }
    }
}
impl From<JobError> for spiling_contracts::geometry::JobError {
    fn from(error: JobError) -> Self {
        match error {
            JobError::Geometry { error } => Self::Geometry { error },
            JobError::Project { error } => Self::Project { error },
            JobError::Manufacturing { error } => Self::Manufacturing { error },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EngineJob {
    pub job_id: JobId,
    pub status: JobStatus,
    pub stage: String,
    pub result: Option<JobResult>,
    pub error: Option<JobError>,
}
