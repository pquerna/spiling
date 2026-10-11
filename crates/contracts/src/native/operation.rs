// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{
    artifact_from_view, artifact_to_view, required, validate_artifact, validate_operation_name,
};
use crate::{
    ArtifactView, NATIVE_METADATA_TYPE, NATIVE_RESULT_TYPE, geometry::*, google, manufacturing::*,
    project::*, rpc,
};
use prost::Message;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NativeOperationView {
    pub name: String,
    pub session_id: SessionId,
    pub captured_scene_revision: SceneRevision,
    pub captured_project_revision: ProjectRevision,
    pub done: bool,
    pub state_version: String,
    pub status: JobStatus,
    pub stage: String,
    pub completed_units: u32,
    pub total_units: u32,
    pub create_time: Option<String>,
    pub update_time: Option<String>,
    pub end_time: Option<String>,
    pub outputs: Vec<ArtifactView>,
    pub result: Option<JobResult>,
    pub error: Option<JobError>,
}
pub fn native_metadata(
    operation: &google::longrunning::Operation,
) -> Result<rpc::NativeOperationMetadata, String> {
    let any = operation
        .metadata
        .as_ref()
        .ok_or("native metadata missing")?;
    if any.type_url != NATIVE_METADATA_TYPE || any.value.len() > crate::MAX_CONTROL_BYTES as usize {
        return Err("unexpected or oversized native metadata".into());
    }
    rpc::NativeOperationMetadata::decode(any.value.as_slice()).map_err(|e| e.to_string())
}
pub fn native_result(
    operation: &google::longrunning::Operation,
) -> Result<Option<JobResult>, String> {
    match &operation.result {
        Some(google::longrunning::operation::Result::Response(any)) => {
            if any.type_url != NATIVE_RESULT_TYPE
                || any.value.len() > crate::MAX_CONTROL_BYTES as usize
            {
                return Err("unexpected or oversized native result".into());
            }
            Ok(Some(
                rpc::NativeOperationResult::decode(any.value.as_slice())
                    .map_err(|e| e.to_string())?
                    .try_into()?,
            ))
        }
        Some(google::longrunning::operation::Result::Error(status)) => {
            let (_, receipt) = super::error::status_error_and_receipt(status)?;
            if receipt.is_some()
                && (!operation.done
                    || native_metadata(operation)?.state
                        != rpc::NativeOperationState::NativeFailed as i32)
            {
                return Err("save receipt requires failed terminal native operation".into());
            }
            Ok(receipt)
        }
        None => Ok(None),
    }
}
fn timestamp(value: &prost_types::Timestamp) -> Result<(), String> {
    if !(0..1_000_000_000).contains(&value.nanos)
        || !(-62_135_596_800..=253_402_300_799).contains(&value.seconds)
    {
        return Err("invalid native timestamp".into());
    }
    Ok(())
}
fn native_artifact_limit(media: &str) -> Result<u64, String> {
    match media {
        "application/x-spiling-mesh" | "application/x-spiling-section" => {
            Ok(u64::from(MAX_GEOMETRY_CHUNK_BYTES))
        }
        "application/x-spiling-manufacturing-bundle" => {
            Ok(u64::from(MAX_MANUFACTURING_BUNDLE_BYTES))
        }
        _ => Err("unexpected native artifact media type".into()),
    }
}
fn validate_verified(
    record: &ManufacturingArtifactRecord,
    report: &VerificationReport,
) -> Result<(), String> {
    report.validate().map_err(|e| e.to_string())?;
    if !report.verified
        || report.deposition_segments != record.summary.deposition_segments
        || report.deposited_volume_mm3 != record.summary.deposited_volume_mm3
        || report.filament_length_mm != record.summary.filament_length_mm
    {
        return Err("verification/record mismatch".into());
    }
    Ok(())
}
pub fn native_operation_view(
    operation: &google::longrunning::Operation,
) -> Result<NativeOperationView, String> {
    validate_operation_name(&operation.name)?;
    let meta = native_metadata(operation)?;
    let session_id = SessionId::parse(meta.session_id.clone()).map_err(|e| e.to_string())?;
    if !operation
        .name
        .starts_with(&format!("sessions/{}/operations/", session_id.as_str()))
    {
        return Err("operation session mismatch".into());
    }
    let status = match rpc::NativeOperationState::try_from(meta.state)
        .map_err(|_| "unknown native state")?
    {
        rpc::NativeOperationState::NativeQueued => JobStatus::Queued,
        rpc::NativeOperationState::NativeRunning => JobStatus::Running,
        rpc::NativeOperationState::NativeCancelling => JobStatus::Cancelling,
        rpc::NativeOperationState::NativeSucceeded => JobStatus::Completed,
        rpc::NativeOperationState::NativeFailed => JobStatus::Failed,
        rpc::NativeOperationState::NativeCancelled => JobStatus::Cancelled,
        rpc::NativeOperationState::NativeInterrupted => JobStatus::Interrupted,
        _ => return Err("unspecified native state".into()),
    };
    if status.is_terminal() != operation.done
        || operation.done != operation.result.is_some()
        || meta.state_version == 0
        || meta.total_units == 0
        || meta.total_units > MAX_DISPLAY_TRIANGLES
        || meta.completed_units > meta.total_units
        || meta.phase.is_empty()
        || meta.phase.len() > MAX_DISPLAY_LABEL_BYTES as usize
        || meta.phase.contains('\0')
        || meta.outputs.len() > 256
        || (status == JobStatus::Completed && meta.completed_units != meta.total_units)
    {
        return Err("inconsistent native snapshot".into());
    }
    let create = required(meta.create_time.as_ref(), "create_time")?;
    let update = required(meta.update_time.as_ref(), "update_time")?;
    timestamp(create)?;
    timestamp(update)?;
    if (create.seconds, create.nanos) > (update.seconds, update.nanos)
        || operation.done != meta.end_time.is_some()
    {
        return Err("inconsistent native timestamps".into());
    }
    if let Some(end) = &meta.end_time {
        timestamp(end)?;
        if (end.seconds, end.nanos) != (update.seconds, update.nanos) {
            return Err("inconsistent native end_time".into());
        }
    }
    let mut names = std::collections::BTreeSet::new();
    let mut bytes = 0u64;
    for output in &meta.outputs {
        validate_artifact(output, native_artifact_limit(&output.media_type)?)?;
        if !names.insert(&output.name) {
            return Err("duplicate native output".into());
        }
        bytes = bytes
            .checked_add(output.size_bytes)
            .ok_or("native output overflow")?;
    }
    if bytes > u64::from(MAX_ENGINE_ARTIFACT_BYTES) {
        return Err("native output budget exceeded".into());
    }
    let (result, error) = match &operation.result {
        Some(google::longrunning::operation::Result::Error(error)) => {
            if status == JobStatus::Completed || error.code == 0 {
                return Err("inconsistent native failure".into());
            }
            if status == JobStatus::Cancelled && error.code != 1
                || status == JobStatus::Interrupted && error.code != 10
            {
                return Err("incorrect cancellation/interruption status".into());
            }
            let (error, receipt) = super::error::status_error_and_receipt(error)?;
            (receipt, Some(error))
        }
        Some(google::longrunning::operation::Result::Response(_)) => {
            if status != JobStatus::Completed {
                return Err("response on unsuccessful native operation".into());
            }
            (native_result(operation)?, None)
        }
        None => (None, None),
    };
    if let Some(result) = &result {
        match result {
            JobResult::Scene { summary } => {
                if summary.session_id != session_id {
                    return Err("scene result session mismatch".into());
                }
            }
            JobResult::ManufacturingCompiled { resource, .. }
            | JobResult::ManufacturingVerified { resource, .. } => {
                let size = super::validate_artifact_view(
                    resource,
                    u64::from(MAX_MANUFACTURING_BUNDLE_BYTES),
                )?;
                if !meta.outputs.iter().any(|a| {
                    a.name == resource.name
                        && a.size_bytes == size
                        && a.sha256 == resource.sha256
                        && a.media_type == resource.media_type
                }) {
                    return Err("manufacturing result not published in outputs".into());
                }
            }
            _ => {}
        }
        if matches!(
            &operation.result,
            Some(google::longrunning::operation::Result::Error(_))
        ) && status != JobStatus::Failed
        {
            return Err("save receipt requires failed native operation".into());
        }
    }
    Ok(NativeOperationView {
        name: operation.name.clone(),
        session_id,
        captured_scene_revision: SceneRevision(meta.captured_scene_revision),
        captured_project_revision: ProjectRevision(meta.captured_project_revision),
        done: operation.done,
        state_version: meta.state_version.to_string(),
        status,
        stage: meta.phase,
        completed_units: meta.completed_units,
        total_units: meta.total_units,
        create_time: meta.create_time.map(|v| v.to_string()),
        update_time: meta.update_time.map(|v| v.to_string()),
        end_time: meta.end_time.map(|v| v.to_string()),
        outputs: meta
            .outputs
            .into_iter()
            .map(|v| {
                let limit = native_artifact_limit(&v.media_type)?;
                artifact_to_view(v, limit)
            })
            .collect::<Result<_, _>>()?,
        result,
        error,
    })
}
impl TryFrom<rpc::NativeOperationResult> for JobResult {
    type Error = String;
    fn try_from(v: rpc::NativeOperationResult) -> Result<Self, String> {
        use rpc::native_operation_result::Result as R;
        Ok(match required(v.result, "native result")? {
            R::Scene(summary) => Self::Scene {
                summary: summary.try_into()?,
            },
            R::SectionArtifactId(id) => Self::Section {
                artifact_id: ArtifactId::new(id).map_err(|e| e.to_string())?,
            },
            R::ProjectSaved(info) => Self::ProjectSaved {
                info: info.try_into()?,
            },
            R::ManufacturingCompiled(v) => {
                let record: ManufacturingArtifactRecord =
                    required(v.record, "manufacturing record")?.try_into()?;
                let resource = required(v.resource, "manufacturing resource")?;
                super::manufacturing::validate_resource(&record, &resource)?;
                Self::ManufacturingCompiled {
                    info: required(v.info, "project info")?.try_into()?,
                    record,
                    resource: artifact_to_view(
                        resource,
                        u64::from(MAX_MANUFACTURING_BUNDLE_BYTES),
                    )?,
                }
            }
            R::ManufacturingVerified(v) => {
                let record: ManufacturingArtifactRecord =
                    required(v.record, "manufacturing record")?.try_into()?;
                let resource = required(v.resource, "manufacturing resource")?;
                super::manufacturing::validate_resource(&record, &resource)?;
                let report: VerificationReport =
                    required(v.report, "verification report")?.try_into()?;
                validate_verified(&record, &report)?;
                Self::ManufacturingVerified {
                    record,
                    report,
                    resource: artifact_to_view(
                        resource,
                        u64::from(MAX_MANUFACTURING_BUNDLE_BYTES),
                    )?,
                }
            }
        })
    }
}
impl TryFrom<JobResult> for rpc::NativeOperationResult {
    type Error = String;
    fn try_from(v: JobResult) -> Result<Self, String> {
        use rpc::native_operation_result::Result as R;
        let result = match v {
            JobResult::Scene { summary } => R::Scene(summary.try_into()?),
            JobResult::Section { artifact_id } => R::SectionArtifactId(artifact_id.get()),
            JobResult::ProjectSaved { info } => R::ProjectSaved(info.try_into()?),
            JobResult::ManufacturingCompiled {
                info,
                record,
                resource,
            } => {
                let resource =
                    artifact_from_view(resource, u64::from(MAX_MANUFACTURING_BUNDLE_BYTES))?;
                super::manufacturing::validate_resource(&record, &resource)?;
                R::ManufacturingCompiled(rpc::ManufacturingCompiledResult {
                    info: Some(info.try_into()?),
                    record: Some(record.try_into()?),
                    resource: Some(resource),
                })
            }
            JobResult::ManufacturingVerified {
                record,
                report,
                resource,
            } => {
                validate_verified(&record, &report)?;
                let resource =
                    artifact_from_view(resource, u64::from(MAX_MANUFACTURING_BUNDLE_BYTES))?;
                super::manufacturing::validate_resource(&record, &resource)?;
                R::ManufacturingVerified(rpc::ManufacturingVerifiedResult {
                    record: Some(record.try_into()?),
                    report: Some(report.try_into()?),
                    resource: Some(resource),
                })
            }
        };
        Ok(Self {
            result: Some(result),
        })
    }
}
