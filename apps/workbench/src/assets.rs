// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{
    ArtifactView,
    geometry::{FaceRef, SessionId},
    project::{ProjectCommand, ProjectId, ProjectResponse, ProjectRevision},
    surface::{ImageReliefParameters, SurfaceImageRecord, SurfaceScopeKind},
};
use spiling_engine_client::{ClientError, EngineRpc};
use std::sync::Arc;
#[derive(Debug, Clone)]
pub struct ImageRequest {
    pub session: SessionId,
    pub revision: ProjectRevision,
    pub reference: FaceRef,
    pub scope: SurfaceScopeKind,
    pub parameters: ImageReliefParameters,
}
#[derive(Debug)]
pub struct ImagePreview {
    pub session: SessionId,
    pub project: ProjectId,
    pub revision: ProjectRevision,
    pub record: SurfaceImageRecord,
    pub pixels: egui::ColorImage,
}
pub async fn load_image(
    rpc: EngineRpc,
    session: SessionId,
    project: ProjectId,
    revision: ProjectRevision,
    record: SurfaceImageRecord,
    resource: ArtifactView,
) -> Result<Arc<ImagePreview>, ClientError> {
    let bytes = rpc.fetch_surface_image(&record, &resource).await?;
    let view = record.validate_content(&bytes)?;
    let pixels =
        egui::ColorImage::from_gray([view.width as usize, view.height as usize], view.pixels);
    let status = rpc
        .project(ProjectCommand::Get {
            session_id: session.clone(),
        })
        .await?;
    if !matches!(status,ProjectResponse::Status{info} if info.project_id==project && info.revision==revision)
    {
        return Err(ClientError::InvalidRequest(
            "Discarded canonical preview from a changed project".into(),
        ));
    }
    Ok(Arc::new(ImagePreview {
        session,
        project,
        revision,
        record,
        pixels,
    }))
}

#[derive(Debug)]
pub struct OpticalCapture {
    pub project: ProjectId,
    pub revision: ProjectRevision,
    pub record: spiling_contracts::optical::OpticalReportRecord,
    pub report: spiling_contracts::optical::OpticalReport,
}
pub async fn load_optical(
    rpc: EngineRpc,
    session: SessionId,
    project: ProjectId,
    revision: ProjectRevision,
    record: spiling_contracts::optical::OpticalReportRecord,
    resource: ArtifactView,
) -> Result<Arc<OpticalCapture>, ClientError> {
    let report = rpc.fetch_optical_report(&record, &resource).await?;
    let status = rpc
        .project(ProjectCommand::Get {
            session_id: session,
        })
        .await?;
    let ProjectResponse::Status { info } = status else {
        return Err(ClientError::Protocol(
            "expected project status for optical capture".into(),
        ));
    };
    let capture = OpticalCapture {
        project,
        revision,
        record,
        report,
    };
    if info.revision != revision || !capture.is_current(&info) {
        return Err(ClientError::InvalidRequest(
            "Discarded optical report from changed study inputs".into(),
        ));
    }
    Ok(Arc::new(capture))
}
impl OpticalCapture {
    /// A fresh comparison precedes acceptance. A retained report may survive the acceptance revision.
    pub fn is_current(&self, info: &spiling_contracts::project::ProjectInfo) -> bool {
        info.project_id == self.project
            && info.appearance_study.optical.as_ref().is_some_and(|study| {
                (info.revision == self.revision || study.report.as_ref() == Some(&self.record))
                    && info
                        .appearance_study
                        .palette
                        .as_ref()
                        .is_some_and(|palette| {
                            self.report.matches_inputs(palette, &study.request).is_ok()
                        })
            })
    }
}
