// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use crate::{DOMAIN_ERROR_TYPE, geometry::*, google, manufacturing::*, project::*, rpc};
use prost::Message;
impl TryFrom<rpc::DomainError> for GeometryError {
    type Error = String;
    fn try_from(value: rpc::DomainError) -> Result<Self, String> {
        let error = Self {
            code: geometry_parse(&value.code)?,
            message: value.message,
        };
        error.validate().map_err(|error| error.to_string())?;
        Ok(error)
    }
}
impl TryFrom<GeometryError> for rpc::DomainError {
    type Error = String;
    fn try_from(value: GeometryError) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            code: geometry_code(value.code).into(),
            message: value.message,
        })
    }
}
fn geometry_code(code: GeometryErrorCode) -> &'static str {
    match code {
        GeometryErrorCode::InvalidGeometry => "invalid_geometry",
        GeometryErrorCode::UnsupportedGeometry => "unsupported_geometry",
        GeometryErrorCode::UnsupportedUnits => "unsupported_units",
        GeometryErrorCode::SourceIo => "source_io",
        GeometryErrorCode::SourceChanged => "source_changed",
        GeometryErrorCode::InvalidPose => "invalid_pose",
        GeometryErrorCode::StaleRevision => "stale_revision",
        GeometryErrorCode::UnknownHandle => "unknown_handle",
        GeometryErrorCode::Busy => "busy",
        GeometryErrorCode::ResourceLimit => "resource_limit",
        GeometryErrorCode::Cancelled => "cancelled",
        GeometryErrorCode::DegenerateSection => "degenerate_section",
        GeometryErrorCode::KernelFailure => "kernel_failure",
        GeometryErrorCode::ConditioningFailure => "conditioning_failure",
        GeometryErrorCode::AnalysisIndeterminate => "analysis_indeterminate",
        GeometryErrorCode::SingularGeometry => "singular_geometry",
        GeometryErrorCode::UnsupportedConversion => "unsupported_conversion",
        GeometryErrorCode::UnsupportedOperation => "unsupported_operation",
        GeometryErrorCode::NativeIntersectionFailure => "native_intersection_failure",
        GeometryErrorCode::ParameterCorrespondenceUnavailable => {
            "parameter_correspondence_unavailable"
        }
        GeometryErrorCode::ShapeErrorExceeded => "shape_error_exceeded",
    }
}
fn geometry_parse(code: &str) -> Result<GeometryErrorCode, String> {
    Ok(match code {
        "invalid_geometry" => GeometryErrorCode::InvalidGeometry,
        "unsupported_geometry" => GeometryErrorCode::UnsupportedGeometry,
        "unsupported_units" => GeometryErrorCode::UnsupportedUnits,
        "source_io" => GeometryErrorCode::SourceIo,
        "source_changed" => GeometryErrorCode::SourceChanged,
        "invalid_pose" => GeometryErrorCode::InvalidPose,
        "stale_revision" => GeometryErrorCode::StaleRevision,
        "unknown_handle" => GeometryErrorCode::UnknownHandle,
        "busy" => GeometryErrorCode::Busy,
        "resource_limit" => GeometryErrorCode::ResourceLimit,
        "cancelled" => GeometryErrorCode::Cancelled,
        "degenerate_section" => GeometryErrorCode::DegenerateSection,
        "kernel_failure" => GeometryErrorCode::KernelFailure,
        "conditioning_failure" => GeometryErrorCode::ConditioningFailure,
        "analysis_indeterminate" => GeometryErrorCode::AnalysisIndeterminate,
        "singular_geometry" => GeometryErrorCode::SingularGeometry,
        "unsupported_conversion" => GeometryErrorCode::UnsupportedConversion,
        "unsupported_operation" => GeometryErrorCode::UnsupportedOperation,
        "native_intersection_failure" => GeometryErrorCode::NativeIntersectionFailure,
        "parameter_correspondence_unavailable" => {
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        }
        "shape_error_exceeded" => GeometryErrorCode::ShapeErrorExceeded,
        _ => return Err("unknown domain error code".into()),
    })
}
fn project_code(code: ProjectErrorCode) -> &'static str {
    match code {
        ProjectErrorCode::InvalidProject => "invalid_project",
        ProjectErrorCode::UnsupportedFormat => "unsupported_format",
        ProjectErrorCode::MissingAsset => "missing_asset",
        ProjectErrorCode::CorruptAsset => "corrupt_asset",
        ProjectErrorCode::ProjectLocked => "project_locked",
        ProjectErrorCode::ReadOnly => "read_only",
        ProjectErrorCode::DirtyProject => "dirty_project",
        ProjectErrorCode::NoSavedPath => "no_saved_path",
        ProjectErrorCode::NoUndo => "no_undo",
        ProjectErrorCode::NoRedo => "no_redo",
        ProjectErrorCode::StaleRevision => "stale_revision",
        ProjectErrorCode::Busy => "busy",
        ProjectErrorCode::ResourceLimit => "resource_limit",
        ProjectErrorCode::Cancelled => "cancelled",
        ProjectErrorCode::Io => "io",
    }
}
fn project_parse(code: &str) -> Result<ProjectErrorCode, String> {
    Ok(match code {
        "invalid_project" => ProjectErrorCode::InvalidProject,
        "unsupported_format" => ProjectErrorCode::UnsupportedFormat,
        "missing_asset" => ProjectErrorCode::MissingAsset,
        "corrupt_asset" => ProjectErrorCode::CorruptAsset,
        "project_locked" => ProjectErrorCode::ProjectLocked,
        "read_only" => ProjectErrorCode::ReadOnly,
        "dirty_project" => ProjectErrorCode::DirtyProject,
        "no_saved_path" => ProjectErrorCode::NoSavedPath,
        "no_undo" => ProjectErrorCode::NoUndo,
        "no_redo" => ProjectErrorCode::NoRedo,
        "stale_revision" => ProjectErrorCode::StaleRevision,
        "busy" => ProjectErrorCode::Busy,
        "resource_limit" => ProjectErrorCode::ResourceLimit,
        "cancelled" => ProjectErrorCode::Cancelled,
        "io" => ProjectErrorCode::Io,
        _ => return Err("unknown domain error code".into()),
    })
}
fn manufacturing_code(code: ManufacturingErrorCode) -> &'static str {
    match code {
        ManufacturingErrorCode::InvalidSpecification => "invalid_specification",
        ManufacturingErrorCode::UnsupportedCapability => "unsupported_capability",
        ManufacturingErrorCode::UnsupportedGeometry => "unsupported_geometry",
        ManufacturingErrorCode::ProgramCoordinateConditioningFailure => {
            "program_coordinate_conditioning_failure"
        }
        ManufacturingErrorCode::NoIntent => "no_intent",
        ManufacturingErrorCode::EmptyProject => "empty_project",
        ManufacturingErrorCode::StaleRevision => "stale_revision",
        ManufacturingErrorCode::VerificationFailed => "verification_failed",
        ManufacturingErrorCode::ResourceLimit => "resource_limit",
        ManufacturingErrorCode::Cancelled => "cancelled",
        ManufacturingErrorCode::Io => "io",
        ManufacturingErrorCode::CorruptArtifact => "corrupt_artifact",
        ManufacturingErrorCode::ReadOnly => "read_only",
        ManufacturingErrorCode::Busy => "busy",
        ManufacturingErrorCode::NativeKernelFailure => "native_kernel_failure",
        ManufacturingErrorCode::NativeConditioningFailure => "native_conditioning_failure",
        ManufacturingErrorCode::NativeAnalysisIndeterminate => "native_analysis_indeterminate",
        ManufacturingErrorCode::NativeSingularGeometry => "native_singular_geometry",
        ManufacturingErrorCode::NativeShapeErrorExceeded => "native_shape_error_exceeded",
    }
}
fn manufacturing_parse(code: &str) -> Result<ManufacturingErrorCode, String> {
    Ok(match code {
        "invalid_specification" => ManufacturingErrorCode::InvalidSpecification,
        "unsupported_capability" => ManufacturingErrorCode::UnsupportedCapability,
        "unsupported_geometry" => ManufacturingErrorCode::UnsupportedGeometry,
        "program_coordinate_conditioning_failure" => {
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure
        }
        "no_intent" => ManufacturingErrorCode::NoIntent,
        "empty_project" => ManufacturingErrorCode::EmptyProject,
        "stale_revision" => ManufacturingErrorCode::StaleRevision,
        "verification_failed" => ManufacturingErrorCode::VerificationFailed,
        "resource_limit" => ManufacturingErrorCode::ResourceLimit,
        "cancelled" => ManufacturingErrorCode::Cancelled,
        "io" => ManufacturingErrorCode::Io,
        "corrupt_artifact" => ManufacturingErrorCode::CorruptArtifact,
        "read_only" => ManufacturingErrorCode::ReadOnly,
        "busy" => ManufacturingErrorCode::Busy,
        "native_kernel_failure" => ManufacturingErrorCode::NativeKernelFailure,
        "native_conditioning_failure" => ManufacturingErrorCode::NativeConditioningFailure,
        "native_analysis_indeterminate" => ManufacturingErrorCode::NativeAnalysisIndeterminate,
        "native_singular_geometry" => ManufacturingErrorCode::NativeSingularGeometry,
        "native_shape_error_exceeded" => ManufacturingErrorCode::NativeShapeErrorExceeded,
        _ => return Err("unknown domain error code".into()),
    })
}
pub fn domain_status(error: &JobError) -> google::rpc::Status {
    use rpc::domain_error_detail::Error as E;
    let (detail, code, message) = match error {
        JobError::Geometry { error } => (
            E::Geometry(rpc::DomainError {
                code: geometry_code(error.code).into(),
                message: bounded_text(&error.message, MAX_ERROR_MESSAGE_BYTES as usize),
            }),
            geometry_code(error.code),
            bounded_text(&error.message, MAX_ERROR_MESSAGE_BYTES as usize),
        ),
        JobError::Project { error } => (
            E::Project(rpc::DomainError {
                code: project_code(error.code).into(),
                message: bounded_text(&error.message, MAX_PROJECT_ERROR_BYTES),
            }),
            project_code(error.code),
            bounded_text(&error.message, MAX_PROJECT_ERROR_BYTES),
        ),
        JobError::Manufacturing { error } => (
            E::Manufacturing(rpc::DomainError {
                code: manufacturing_code(error.code).into(),
                message: bounded_text(&error.message, MAX_MANUFACTURING_ERROR_BYTES),
            }),
            manufacturing_code(error.code),
            bounded_text(&error.message, MAX_MANUFACTURING_ERROR_BYTES),
        ),
    };
    let canonical = match code {
        "cancelled" => 1,
        "resource_limit" => 8,
        "busy" | "project_locked" => 9,
        "stale_revision" => 10,
        "unknown_handle" | "missing_asset" | "no_saved_path" => 5,
        "source_io"
        | "io"
        | "kernel_failure"
        | "native_intersection_failure"
        | "native_kernel_failure" => 13,
        "read_only" => 7,
        "no_intent"
        | "empty_project"
        | "dirty_project"
        | "no_undo"
        | "no_redo"
        | "unsupported_capability"
        | "unsupported_geometry"
        | "unsupported_units"
        | "unsupported_format"
        | "verification_failed"
        | "conditioning_failure"
        | "analysis_indeterminate"
        | "singular_geometry"
        | "unsupported_conversion"
        | "unsupported_operation"
        | "parameter_correspondence_unavailable"
        | "shape_error_exceeded"
        | "native_conditioning_failure"
        | "native_analysis_indeterminate"
        | "native_singular_geometry"
        | "native_shape_error_exceeded"
        | "program_coordinate_conditioning_failure" => 9,
        _ => 3,
    };
    google::rpc::Status {
        code: canonical,
        message,
        details: vec![prost_types::Any {
            type_url: DOMAIN_ERROR_TYPE.into(),
            value: rpc::DomainErrorDetail {
                error: Some(detail),
            }
            .encode_to_vec(),
        }],
    }
}
pub fn domain_error(status: &google::rpc::Status) -> Result<JobError, String> {
    status_error_and_receipt(status).map(|(error, _)| error)
}
pub(super) fn status_error_and_receipt(
    status: &google::rpc::Status,
) -> Result<(JobError, Option<JobResult>), String> {
    if status.code == 0
        || !(1..=2).contains(&status.details.len())
        || status.message.len() > MAX_PROJECT_ERROR_BYTES.max(MAX_MANUFACTURING_ERROR_BYTES)
    {
        return Err("invalid domain Status".into());
    }
    let mut domain = None;
    let mut receipt = None;
    for any in &status.details {
        if any.type_url == DOMAIN_ERROR_TYPE {
            if domain.replace(any).is_some() || any.value.len() > 4096 {
                return Err("duplicate or oversized domain error detail".into());
            }
        } else if any.type_url == crate::NATIVE_RESULT_TYPE {
            if receipt.replace(any).is_some() || any.value.len() > crate::MAX_CONTROL_BYTES as usize
            {
                return Err("duplicate or oversized save receipt".into());
            }
        } else {
            return Err("unexpected domain error detail".into());
        }
    }
    let any = domain.ok_or("missing domain error detail")?;
    let detail = rpc::DomainErrorDetail::decode(any.value.as_slice()).map_err(|e| e.to_string())?;
    use rpc::domain_error_detail::Error as E;
    let error = match detail.error.ok_or("missing domain error")? {
        E::Geometry(e) => {
            if e.message.len() > MAX_ERROR_MESSAGE_BYTES as usize || e.message != status.message {
                return Err("invalid geometry error message".into());
            }
            JobError::Geometry {
                error: GeometryError {
                    code: geometry_parse(&e.code)?,
                    message: e.message,
                },
            }
        }
        E::Project(e) => {
            if e.message.len() > MAX_PROJECT_ERROR_BYTES || e.message != status.message {
                return Err("invalid project error message".into());
            }
            JobError::Project {
                error: ProjectError {
                    code: project_parse(&e.code)?,
                    message: e.message,
                },
            }
        }
        E::Manufacturing(e) => {
            if e.message.len() > MAX_MANUFACTURING_ERROR_BYTES || e.message != status.message {
                return Err("invalid manufacturing error message".into());
            }
            JobError::Manufacturing {
                error: ManufacturingError {
                    code: manufacturing_parse(&e.code)?,
                    message: e.message,
                },
            }
        }
    };
    if domain_status(&error).code != status.code
        && !(status.code == 10
            && matches!(
                &error,
                JobError::Geometry {
                    error: GeometryError {
                        code: GeometryErrorCode::Cancelled,
                        ..
                    }
                } | JobError::Project {
                    error: ProjectError {
                        code: ProjectErrorCode::Cancelled,
                        ..
                    }
                } | JobError::Manufacturing {
                    error: ManufacturingError {
                        code: ManufacturingErrorCode::Cancelled,
                        ..
                    }
                }
            ))
    {
        return Err("domain Status code mismatch".into());
    }
    let receipt = receipt
        .map(|any| -> Result<JobResult, String> {
            let result: JobResult = rpc::NativeOperationResult::decode(any.value.as_slice())
                .map_err(|e| e.to_string())?
                .try_into()?;
            if !matches!(
                &error,
                JobError::Project {
                    error: ProjectError {
                        code: ProjectErrorCode::Io,
                        ..
                    }
                }
            ) || !matches!(&result,JobResult::ProjectSaved{info} if info.save_uncertain)
            {
                return Err("receipt requires uncertain ProjectSaved with project Io error".into());
            }
            Ok(result)
        })
        .transpose()?;
    Ok((error, receipt))
}
pub fn geometry_request_status(message: &str) -> google::rpc::Status {
    let (code, text) = message.split_once(": ").unwrap_or(("", message));
    let code = match code {
        "InvalidGeometry" => GeometryErrorCode::InvalidGeometry,
        "UnsupportedGeometry" => GeometryErrorCode::UnsupportedGeometry,
        "UnsupportedUnits" => GeometryErrorCode::UnsupportedUnits,
        "SourceIo" => GeometryErrorCode::SourceIo,
        "SourceChanged" => GeometryErrorCode::SourceChanged,
        "InvalidPose" => GeometryErrorCode::InvalidPose,
        "StaleRevision" => GeometryErrorCode::StaleRevision,
        "UnknownHandle" => GeometryErrorCode::UnknownHandle,
        "Busy" => GeometryErrorCode::Busy,
        "ResourceLimit" => GeometryErrorCode::ResourceLimit,
        "Cancelled" => GeometryErrorCode::Cancelled,
        "DegenerateSection" => GeometryErrorCode::DegenerateSection,
        "KernelFailure" => GeometryErrorCode::KernelFailure,
        "ConditioningFailure" => GeometryErrorCode::ConditioningFailure,
        "AnalysisIndeterminate" => GeometryErrorCode::AnalysisIndeterminate,
        "SingularGeometry" => GeometryErrorCode::SingularGeometry,
        "UnsupportedConversion" => GeometryErrorCode::UnsupportedConversion,
        "UnsupportedOperation" => GeometryErrorCode::UnsupportedOperation,
        "NativeIntersectionFailure" => GeometryErrorCode::NativeIntersectionFailure,
        "ParameterCorrespondenceUnavailable" => {
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        }
        "ShapeErrorExceeded" => GeometryErrorCode::ShapeErrorExceeded,
        _ => GeometryErrorCode::InvalidGeometry,
    };
    domain_status(&JobError::Geometry {
        error: GeometryError::new(code, text),
    })
}
pub fn project_request_status(message: &str) -> google::rpc::Status {
    let (code, text) = message.split_once(": ").unwrap_or(("", message));
    let code = match code {
        "InvalidProject" => ProjectErrorCode::InvalidProject,
        "UnsupportedFormat" => ProjectErrorCode::UnsupportedFormat,
        "MissingAsset" => ProjectErrorCode::MissingAsset,
        "CorruptAsset" => ProjectErrorCode::CorruptAsset,
        "ProjectLocked" => ProjectErrorCode::ProjectLocked,
        "ReadOnly" => ProjectErrorCode::ReadOnly,
        "DirtyProject" => ProjectErrorCode::DirtyProject,
        "NoSavedPath" => ProjectErrorCode::NoSavedPath,
        "NoUndo" => ProjectErrorCode::NoUndo,
        "NoRedo" => ProjectErrorCode::NoRedo,
        "StaleRevision" => ProjectErrorCode::StaleRevision,
        "Busy" => ProjectErrorCode::Busy,
        "ResourceLimit" => ProjectErrorCode::ResourceLimit,
        "Cancelled" => ProjectErrorCode::Cancelled,
        "Io" => ProjectErrorCode::Io,
        _ => ProjectErrorCode::InvalidProject,
    };
    domain_status(&JobError::Project {
        error: ProjectError::new(code, text),
    })
}
pub fn manufacturing_request_status(message: &str) -> google::rpc::Status {
    let (code, text) = message.split_once(": ").unwrap_or(("", message));
    let code = match code {
        "InvalidSpecification" => ManufacturingErrorCode::InvalidSpecification,
        "UnsupportedCapability" => ManufacturingErrorCode::UnsupportedCapability,
        "UnsupportedGeometry" => ManufacturingErrorCode::UnsupportedGeometry,
        "ProgramCoordinateConditioningFailure" => {
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure
        }
        "NoIntent" => ManufacturingErrorCode::NoIntent,
        "EmptyProject" => ManufacturingErrorCode::EmptyProject,
        "StaleRevision" => ManufacturingErrorCode::StaleRevision,
        "VerificationFailed" => ManufacturingErrorCode::VerificationFailed,
        "ResourceLimit" => ManufacturingErrorCode::ResourceLimit,
        "Cancelled" => ManufacturingErrorCode::Cancelled,
        "Io" => ManufacturingErrorCode::Io,
        "CorruptArtifact" => ManufacturingErrorCode::CorruptArtifact,
        "ReadOnly" => ManufacturingErrorCode::ReadOnly,
        "Busy" => ManufacturingErrorCode::Busy,
        "NativeKernelFailure" => ManufacturingErrorCode::NativeKernelFailure,
        "NativeConditioningFailure" => ManufacturingErrorCode::NativeConditioningFailure,
        "NativeAnalysisIndeterminate" => ManufacturingErrorCode::NativeAnalysisIndeterminate,
        "NativeSingularGeometry" => ManufacturingErrorCode::NativeSingularGeometry,
        "NativeShapeErrorExceeded" => ManufacturingErrorCode::NativeShapeErrorExceeded,
        _ => ManufacturingErrorCode::InvalidSpecification,
    };
    domain_status(&JobError::Manufacturing {
        error: ManufacturingError::new(code, text),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numerical_refusals_preserve_typed_codes_through_status() {
        for code in [
            GeometryErrorCode::ConditioningFailure,
            GeometryErrorCode::AnalysisIndeterminate,
            GeometryErrorCode::SingularGeometry,
            GeometryErrorCode::UnsupportedConversion,
            GeometryErrorCode::UnsupportedOperation,
            GeometryErrorCode::NativeIntersectionFailure,
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            GeometryErrorCode::ShapeErrorExceeded,
        ] {
            let name = geometry_code(code);
            assert_eq!(geometry_parse(name).unwrap(), code);
            let status =
                geometry_request_status(&format!("{code:?}: numerical obligation refused"));
            assert_eq!(
                status.code,
                if code == GeometryErrorCode::NativeIntersectionFailure {
                    13
                } else {
                    9
                }
            );
            let detail =
                rpc::DomainErrorDetail::decode(status.details[0].value.as_slice()).unwrap();
            let Some(rpc::domain_error_detail::Error::Geometry(error)) = detail.error else {
                panic!("geometry status detail");
            };
            assert_eq!(geometry_parse(&error.code).unwrap(), code);
            assert_eq!(error.message, "numerical obligation refused");
        }
    }

    #[test]
    fn manufacturing_preserves_native_failure_distinctions_through_status() {
        for code in [
            ManufacturingErrorCode::NativeKernelFailure,
            ManufacturingErrorCode::NativeConditioningFailure,
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            ManufacturingErrorCode::NativeSingularGeometry,
            ManufacturingErrorCode::NativeShapeErrorExceeded,
        ] {
            assert_eq!(manufacturing_parse(manufacturing_code(code)).unwrap(), code);
            let status = manufacturing_request_status(&format!("{code:?}: native slab refused"));
            assert_eq!(
                status.code,
                if code == ManufacturingErrorCode::NativeKernelFailure {
                    13
                } else {
                    9
                }
            );
            let detail =
                rpc::DomainErrorDetail::decode(status.details[0].value.as_slice()).unwrap();
            let Some(rpc::domain_error_detail::Error::Manufacturing(error)) = detail.error else {
                panic!("manufacturing status detail");
            };
            assert_eq!(manufacturing_parse(&error.code).unwrap(), code);
            assert_eq!(error.message, "native slab refused");
        }
    }
}
