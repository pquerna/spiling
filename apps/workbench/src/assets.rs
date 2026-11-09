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

#[derive(Debug)]
pub struct MechanicsCapture {
    pub project: ProjectId,
    pub revision: ProjectRevision,
    pub summary: spiling_contracts::analysis::mechanics::AnalysisRunSummary,
    pub bytes: Vec<u8>,
    pub resource: ArtifactView,
}
pub async fn load_mechanics(
    rpc: EngineRpc,
    session: SessionId,
    project: ProjectId,
    revision: ProjectRevision,
    captured_revision: ProjectRevision,
    input: spiling_contracts::geometry::SourceHash,
    resource: ArtifactView,
) -> Result<Arc<MechanicsCapture>, ClientError> {
    let bytes = rpc
        .fetch_analysis_fields(&input, captured_revision, &resource)
        .await?;
    let summary = spiling_contracts::analysis::fields::AnalysisFields::decode(&bytes)?.summary;
    if summary.project_id != project {
        return Err(ClientError::Protocol(
            "Mechanical fields belong to another project".into(),
        ));
    }
    let status = rpc
        .project(ProjectCommand::Get {
            session_id: session,
        })
        .await?;
    if !matches!(status,ProjectResponse::Status{info} if info.project_id==project && info.revision==revision)
    {
        return Err(ClientError::InvalidRequest(
            "Discarded mechanical fields from changed captured inputs".into(),
        ));
    }
    Ok(Arc::new(MechanicsCapture {
        project,
        revision,
        summary,
        bytes,
        resource,
    }))
}

pub async fn restore_mechanics(
    rpc: EngineRpc,
    session: SessionId,
    project: ProjectId,
    revision: ProjectRevision,
    record: spiling_contracts::analysis::AnalysisResultRecord,
) -> Result<Arc<MechanicsCapture>, ClientError> {
    let response = rpc
        .project(ProjectCommand::GetAnalysisResult {
            session_id: session.clone(),
            base_revision: revision,
            case_id: record.case_id.clone(),
        })
        .await?;
    let ProjectResponse::OperationAccepted { mut operation } = response else {
        return Err(ClientError::Protocol(
            "Saved fields read did not return an operation".into(),
        ));
    };
    let mut watch = rpc.watch_native_operation(operation.name.clone()).await?;
    while !operation.done {
        operation = tokio::time::timeout(std::time::Duration::from_secs(30), watch.next())
            .await
            .map_err(|_| ClientError::Protocol("Saved fields read timed out".into()))??;
    }
    let Some(spiling_contracts::geometry::JobResult::AnalysisResultPrepared {
        project_id,
        read_revision,
        record: returned,
        resource,
    }) = operation.result
    else {
        return Err(ClientError::Protocol(format!(
            "Saved fields read failed: {:?}",
            operation.error
        )));
    };
    if project_id != project
        || read_revision != revision
        || returned != record
        || resource.sha256 != record.hash.as_str()
        || resource.size_bytes != record.byte_count.to_string()
    {
        return Err(ClientError::Protocol(
            "Saved fields descriptor/capture mismatch".into(),
        ));
    }
    load_mechanics(
        rpc,
        session,
        project,
        revision,
        record.captured_revision,
        record.input_hash,
        resource,
    )
    .await
}
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

#[derive(Debug)]
pub struct StockCapture {
    pub project: ProjectId,
    pub revision: ProjectRevision,
    pub record: spiling_contracts::stock::StockMatchRecord,
    pub report: spiling_contracts::stock::StockMatchReport,
}
pub async fn load_stock(
    rpc: EngineRpc,
    session: SessionId,
    project: ProjectId,
    revision: ProjectRevision,
    record: spiling_contracts::stock::StockMatchRecord,
    resource: ArtifactView,
) -> Result<Arc<StockCapture>, ClientError> {
    let report = rpc.fetch_stock_report(&record, &resource).await?;
    if !matches!(rpc.project(ProjectCommand::Get{session_id:session}).await?,ProjectResponse::Status{info} if info.project_id==project && info.revision==revision)
    {
        return Err(ClientError::InvalidRequest(
            "Discarded stock results from a changed project".into(),
        ));
    }
    Ok(Arc::new(StockCapture {
        project,
        revision,
        record,
        report,
    }))
}
