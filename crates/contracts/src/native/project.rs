use std::sync::Arc;
// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{request_id, required, session_parent};
use crate::{geometry::*, project::*, rpc, surface::*};

fn validate_option_info(option: &ManufacturingOptionInfo) -> Result<(), String> {
    if option.name.is_empty()
        || option.name.len() > MAX_MANUFACTURING_OPTION_NAME_BYTES
        || option.name.trim() != option.name
        || option.name.chars().any(char::is_control)
    {
        return Err("invalid manufacturing option summary name".into());
    }
    Ok(())
}

impl TryFrom<rpc::ManufacturingOption> for ManufacturingOption {
    type Error = String;
    fn try_from(value: rpc::ManufacturingOption) -> Result<Self, String> {
        let option = Self {
            id: SourceHash::parse(value.id).map_err(|error| error.to_string())?,
            name: value.name,
            context_hash: SourceHash::parse(value.context_hash)
                .map_err(|error| error.to_string())?,
            intent: Arc::new(required(value.intent, "manufacturing option intent")?.try_into()?),
        };
        option.validate().map_err(|error| error.to_string())?;
        Ok(option)
    }
}
impl TryFrom<ManufacturingOption> for rpc::ManufacturingOption {
    type Error = String;
    fn try_from(value: ManufacturingOption) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            id: value.id.as_str().into(),
            name: value.name,
            context_hash: value.context_hash.as_str().into(),
            intent: Some(Arc::unwrap_or_clone(value.intent).try_into()?),
        })
    }
}
impl TryFrom<rpc::ManufacturingOptionInfo> for ManufacturingOptionInfo {
    type Error = String;
    fn try_from(value: rpc::ManufacturingOptionInfo) -> Result<Self, String> {
        let option = Self {
            id: SourceHash::parse(value.id).map_err(|error| error.to_string())?,
            name: value.name,
            context_hash: SourceHash::parse(value.context_hash)
                .map_err(|error| error.to_string())?,
        };
        validate_option_info(&option)?;
        Ok(option)
    }
}
impl TryFrom<ManufacturingOptionInfo> for rpc::ManufacturingOptionInfo {
    type Error = String;
    fn try_from(value: ManufacturingOptionInfo) -> Result<Self, String> {
        validate_option_info(&value)?;
        Ok(Self {
            id: value.id.as_str().into(),
            name: value.name,
            context_hash: value.context_hash.as_str().into(),
        })
    }
}

pub enum ProjectCall {
    MatchStock(rpc::MatchStockRequest),
    StorageMaintenance(rpc::StorageMaintenanceRequest),
    ApplyOpticalConstruction(rpc::ApplyOpticalConstructionRequest),
    Execute(Box<rpc::ProjectRequest>),
    Open(rpc::OpenProjectRequest),
    Save(rpc::SaveProjectRequest),
    ImportSurfaceImageFinish(rpc::ImportSurfaceImageFinishRequest),
    GetSurfaceImage(rpc::GetSurfaceImageRequest),
    ImportAppearancePalette(rpc::ImportAppearancePaletteRequest),
    CompareAppearance(rpc::CompareAppearanceRequest),
    GetAppearanceReport(rpc::GetAppearanceReportRequest),
    CompareOptical(rpc::CompareOpticalRequest),
    GetOpticalReport(rpc::GetOpticalReportRequest),
}
pub fn project_call(command: ProjectCommand, id: &str) -> Result<ProjectCall, String> {
    request_id(id)?;
    use rpc::project_request::Command as C;
    let (session_id, command) = match command {
        ProjectCommand::SelectPrintLayout {
            session_id,
            base_revision,
            layout_id,
        } => (
            session_id,
            C::SelectPrintLayout(rpc::SelectPrintLayoutRequest {
                base_revision: base_revision.get(),
                layout_id: layout_id.as_str().into(),
            }),
        ),
        ProjectCommand::EditPrintLayout {
            session_id,
            base_revision,
            edit,
        } => (
            session_id,
            C::EditPrintLayout(rpc::EditPrintLayoutRequest {
                base_revision: base_revision.get(),
                edit: Some(edit.try_into()?),
            }),
        ),
        ProjectCommand::GetManufacturingOption {
            layout_id,
            session_id,
            base_revision,
            id,
        } => (
            session_id,
            C::GetManufacturingOption(rpc::ManufacturingOptionRequest {
                layout_id: layout_id.as_str().into(),
                base_revision: base_revision.get(),
                id: id.as_str().into(),
            }),
        ),
        ProjectCommand::RetainManufacturingOption {
            layout_id,
            session_id,
            base_revision,
            option,
        } => (
            session_id,
            C::RetainManufacturingOption(rpc::RetainManufacturingOptionRequest {
                layout_id: layout_id.as_str().into(),
                base_revision: base_revision.get(),
                option: Some(option.try_into()?),
            }),
        ),
        ProjectCommand::RemoveManufacturingOption {
            layout_id,
            session_id,
            base_revision,
            id,
        } => (
            session_id,
            C::RemoveManufacturingOption(rpc::ManufacturingOptionRequest {
                layout_id: layout_id.as_str().into(),
                base_revision: base_revision.get(),
                id: id.as_str().into(),
            }),
        ),
        ProjectCommand::ApplyManufacturingOption {
            layout_id,
            session_id,
            base_revision,
            id,
        } => (
            session_id,
            C::ApplyManufacturingOption(rpc::ManufacturingOptionRequest {
                layout_id: layout_id.as_str().into(),
                base_revision: base_revision.get(),
                id: id.as_str().into(),
            }),
        ),
        ProjectCommand::MatchStock {
            session_id,
            base_revision,
            query,
        } => {
            return Ok(ProjectCall::MatchStock(rpc::MatchStockRequest {
                parent: format!("sessions/{}", session_id.as_str()),
                request_id: id.into(),
                base_revision: base_revision.0,
                query: Some(query.try_into()?),
            }));
        }
        ProjectCommand::PutRecipe {
            session_id,
            base_revision,
            transfer,
            expected,
        } => (
            session_id,
            C::PutRecipe(rpc::ProjectPutRecipeRequest {
                base_revision: base_revision.0,
                transfer: Some(transfer.try_into()?),
                expected: expected.map(|h| h.as_str().into()),
            }),
        ),
        ProjectCommand::GetRecipe {
            session_id,
            base_revision,
            reference,
        } => (
            session_id,
            C::GetRecipe(rpc::ProjectGetRecipeRequest {
                base_revision: base_revision.0,
                reference: Some(reference.try_into()?),
            }),
        ),
        ProjectCommand::ListRecipes {
            session_id,
            base_revision,
            offset,
        } => (
            session_id,
            C::ListRecipes(rpc::ProjectListRecipesRequest {
                base_revision: base_revision.0,
                offset,
            }),
        ),
        ProjectCommand::LoadRecipeStudy {
            session_id,
            base_revision,
            reference,
            context,
        } => (
            session_id,
            C::LoadRecipeStudy(rpc::ProjectLoadRecipeStudyRequest {
                base_revision: base_revision.0,
                reference: Some(reference.try_into()?),
                context: Some(context.try_into()?),
            }),
        ),
        ProjectCommand::CaptureRecipe {
            session_id,
            base_revision,
            id,
            name,
            ingredients,
            expected,
        } => {
            crate::library::validate_library_uuid(&id.0)?;
            if name.trim().is_empty()
                || name.len() > 256
                || name.chars().any(char::is_control)
                || !(2..=3).contains(&ingredients.len())
            {
                return Err("invalid captured recipe name/ingredients".into());
            }
            (
                session_id,
                C::CaptureRecipe(rpc::ProjectCaptureRecipeRequest {
                    base_revision: base_revision.0,
                    id: id.0,
                    name,
                    ingredients: ingredients
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    expected: expected.map(|h| h.as_str().into()),
                }),
            )
        }
        ProjectCommand::SetHardware {
            layout_id,
            session_id,
            base_revision,
            capture,
        } => (
            session_id,
            C::SetHardware(rpc::ProjectSetHardwareRequest {
                layout_id: layout_id.as_str().into(),
                base_revision: base_revision.0,
                capture: capture.map(|c| (*c).clone().try_into()).transpose()?,
            }),
        ),
        ProjectCommand::PutFilament {
            session_id,
            base_revision,
            revision,
            origin,
            expected,
        } => (
            session_id,
            C::PutFilament(rpc::ProjectPutFilamentRequest {
                base_revision: base_revision.0,
                revision: Some(revision.try_into()?),
                origin: origin.map(TryInto::try_into).transpose()?,
                expected: expected.map(|h| h.as_str().to_owned()),
            }),
        ),
        ProjectCommand::GetFilament {
            session_id,
            base_revision,
            reference,
        } => (
            session_id,
            C::GetFilament(rpc::ProjectGetFilamentRequest {
                base_revision: base_revision.0,
                reference: Some(reference.try_into()?),
            }),
        ),
        ProjectCommand::ListFilaments {
            session_id,
            base_revision,
            offset,
        } => (
            session_id,
            C::ListFilaments(rpc::ProjectListFilamentsRequest {
                base_revision: base_revision.0,
                offset,
            }),
        ),
        ProjectCommand::StorageMaintenance {
            session_id,
            base_revision,
            action,
            target,
        } => {
            if (action == StorageMaintenanceAction::CompactToNew) != target.is_some() {
                return Err("only compact-to-new requires a new target file".into());
            }
            return Ok(ProjectCall::StorageMaintenance(
                rpc::StorageMaintenanceRequest {
                    parent: format!("sessions/{}", session_id.as_str()),
                    request_id: id.into(),
                    base_revision: base_revision.0,
                    action: rpc::StorageMaintenanceAction::from(action) as i32,
                    target: target.map(TryInto::try_into).transpose()?,
                },
            ));
        }
        ProjectCommand::ApplyOpticalConstruction {
            session_id,
            base_revision,
            application,
        } => {
            return Ok(ProjectCall::ApplyOpticalConstruction(
                rpc::ApplyOpticalConstructionRequest {
                    parent: format!("sessions/{}", session_id.as_str()),
                    request_id: id.into(),
                    base_revision: base_revision.0,
                    application: Some(application.try_into()?),
                },
            ));
        }
        ProjectCommand::CompareOptical {
            session_id,
            base_revision,
        } => {
            return Ok(ProjectCall::CompareOptical(rpc::CompareOpticalRequest {
                parent: format!("sessions/{}", session_id.as_str()),
                request_id: id.into(),
                base_revision: base_revision.0,
            }));
        }
        ProjectCommand::GetOpticalReport {
            session_id,
            base_revision,
        } => {
            return Ok(ProjectCall::GetOpticalReport(
                rpc::GetOpticalReportRequest {
                    parent: format!("sessions/{}", session_id.as_str()),
                    request_id: id.into(),
                    base_revision: base_revision.0,
                },
            ));
        }
        ProjectCommand::SetOpticalStudy {
            session_id,
            base_revision,
            request,
        } => (
            session_id,
            C::SetOpticalStudy(rpc::SetOpticalStudyRequest {
                base_revision: base_revision.0,
                request: Some(request.try_into()?),
            }),
        ),
        ProjectCommand::AcceptOpticalReport {
            session_id,
            base_revision,
            report_hash,
            selection,
        } => (
            session_id,
            C::AcceptOpticalReport(rpc::AcceptOpticalReportRequest {
                base_revision: base_revision.0,
                report_hash: report_hash.into(),
                selection: selection.map(TryInto::try_into).transpose()?,
            }),
        ),
        ProjectCommand::ImportAppearancePalette {
            session_id,
            base_revision,
            source,
        } => {
            return Ok(ProjectCall::ImportAppearancePalette(
                rpc::ImportAppearancePaletteRequest {
                    parent: format!("sessions/{}", session_id.as_str()),
                    request_id: id.into(),
                    base_revision: base_revision.0,
                    source: Some(source.try_into()?),
                },
            ));
        }
        ProjectCommand::CompareAppearance {
            session_id,
            base_revision,
        } => {
            return Ok(ProjectCall::CompareAppearance(
                rpc::CompareAppearanceRequest {
                    parent: format!("sessions/{}", session_id.as_str()),
                    request_id: id.into(),
                    base_revision: base_revision.0,
                },
            ));
        }
        ProjectCommand::GetAppearanceReport {
            session_id,
            base_revision,
        } => {
            return Ok(ProjectCall::GetAppearanceReport(
                rpc::GetAppearanceReportRequest {
                    parent: format!("sessions/{}", session_id.as_str()),
                    request_id: id.into(),
                    base_revision: base_revision.0,
                },
            ));
        }
        ProjectCommand::SetAppearanceTarget {
            session_id,
            base_revision,
            target,
            limits,
        } => (
            session_id,
            C::SetAppearanceTarget(rpc::SetAppearanceTargetRequest {
                base_revision: base_revision.0,
                target: Some(target.try_into()?),
                limits: Some(limits.try_into()?),
            }),
        ),
        ProjectCommand::AcceptAppearanceReport {
            session_id,
            base_revision,
            report_hash,
            selection,
        } => (
            session_id,
            C::AcceptAppearanceReport(rpc::AcceptAppearanceReportRequest {
                base_revision: base_revision.0,
                report_hash: report_hash.into(),
                selection,
            }),
        ),
        ProjectCommand::EditProcess {
            session_id,
            base_revision,
            edit,
        } => (
            session_id,
            C::EditProcess(rpc::EditProcessRequest {
                base_revision: base_revision.0,
                edit: Some(edit.try_into()?),
            }),
        ),
        ProjectCommand::Open {
            session_id,
            base_revision,
            path,
            read_only,
            recover_previous,
            discard_changes,
        } => {
            return Ok(ProjectCall::Open(rpc::OpenProjectRequest {
                parent: format!("sessions/{}", session_id.as_str()),
                request_id: id.into(),
                base_revision: base_revision.0,
                path: Some(path.try_into()?),
                read_only,
                recover_previous,
                discard_changes,
            }));
        }
        ProjectCommand::Save {
            session_id,
            base_revision,
            target,
        } => {
            return Ok(ProjectCall::Save(rpc::SaveProjectRequest {
                parent: format!("sessions/{}", session_id.as_str()),
                request_id: id.into(),
                base_revision: base_revision.0,
                target: target.map(TryInto::try_into).transpose()?,
            }));
        }
        ProjectCommand::ImportSurfaceImageFinish {
            session_id,
            base_revision,
            reference,
            scope,
            source,
            parameters,
        } => {
            parameters.validate().map_err(|e| e.to_string())?;
            if reference.session_id != session_id {
                return Err("image face session mismatch".into());
            }
            return Ok(ProjectCall::ImportSurfaceImageFinish(
                rpc::ImportSurfaceImageFinishRequest {
                    parent: format!("sessions/{}", session_id.as_str()),
                    request_id: id.into(),
                    base_revision: base_revision.0,
                    reference: Some(reference.try_into()?),
                    scope: match scope {
                        SurfaceScopeKind::Definition => rpc::SurfaceScopeKind::Definition as i32,
                        SurfaceScopeKind::Occurrence => rpc::SurfaceScopeKind::Occurrence as i32,
                    },
                    source: Some(source.try_into()?),
                    parameters: Some(parameters.try_into()?),
                },
            ));
        }
        ProjectCommand::GetSurfaceImage {
            session_id,
            base_revision,
            image_hash,
        } => {
            return Ok(ProjectCall::GetSurfaceImage(rpc::GetSurfaceImageRequest {
                parent: format!("sessions/{}", session_id.as_str()),
                request_id: id.into(),
                base_revision: base_revision.0,
                image_hash: image_hash.into(),
            }));
        }
        ProjectCommand::Get { session_id } => (session_id, C::Get(rpc::GetProjectRequest {})),
        ProjectCommand::New {
            session_id,
            base_revision,
            discard_changes,
        } => (
            session_id,
            C::New(rpc::NewProjectRequest {
                base_revision: base_revision.0,
                discard_changes,
            }),
        ),
        ProjectCommand::Undo {
            session_id,
            base_revision,
        } => (
            session_id,
            C::Undo(rpc::UndoProjectRequest {
                base_revision: base_revision.0,
            }),
        ),
        ProjectCommand::Redo {
            session_id,
            base_revision,
        } => (
            session_id,
            C::Redo(rpc::RedoProjectRequest {
                base_revision: base_revision.0,
            }),
        ),
        ProjectCommand::AddSurfaceFinish {
            session_id,
            base_revision,
            face_ref,
            scope,
            finish,
        } => (
            session_id,
            C::AddSurfaceFinish(rpc::AddSurfaceFinishRequest {
                base_revision: base_revision.0,
                face_ref: Some(face_ref.try_into()?),
                scope: match scope {
                    SurfaceScopeKind::Definition => rpc::SurfaceScopeKind::Definition as i32,
                    SurfaceScopeKind::Occurrence => rpc::SurfaceScopeKind::Occurrence as i32,
                },
                finish: Some(finish.try_into()?),
            }),
        ),
        ProjectCommand::RemoveSurfaceFinish {
            session_id,
            base_revision,
            layer_id,
        } => (
            session_id,
            C::RemoveSurfaceFinish(rpc::RemoveSurfaceFinishRequest {
                base_revision: base_revision.0,
                layer_id: layer_id.get(),
            }),
        ),
        ProjectCommand::SetSurfaceFinishEnabled {
            session_id,
            base_revision,
            layer_id,
            enabled,
        } => (
            session_id,
            C::SetSurfaceFinishEnabled(rpc::SetSurfaceFinishEnabledRequest {
                base_revision: base_revision.0,
                layer_id: layer_id.get(),
                enabled,
            }),
        ),
    };
    Ok(ProjectCall::Execute(Box::new(rpc::ProjectRequest {
        parent: format!("sessions/{}", session_id.as_str()),
        command: Some(command),
    })))
}
impl TryFrom<rpc::ProjectRequest> for ProjectCommand {
    type Error = String;
    fn try_from(v: rpc::ProjectRequest) -> Result<Self, String> {
        let session_id = session_parent(&v.parent)?;
        use rpc::project_request::Command as C;
        Ok(match required(v.command, "project command")? {
            C::SelectPrintLayout(v) => Self::SelectPrintLayout {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                layout_id: PrintLayoutId::parse(v.layout_id).map_err(|e| e.to_string())?,
            },
            C::EditPrintLayout(v) => Self::EditPrintLayout {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                edit: required(v.edit, "plate edit")?.try_into()?,
            },
            C::GetManufacturingOption(value) => Self::GetManufacturingOption {
                layout_id: PrintLayoutId::parse(value.layout_id).map_err(|e| e.to_string())?,
                session_id,
                base_revision: ProjectRevision(value.base_revision),
                id: SourceHash::parse(value.id).map_err(|error| error.to_string())?,
            },
            C::RetainManufacturingOption(value) => Self::RetainManufacturingOption {
                layout_id: PrintLayoutId::parse(value.layout_id).map_err(|e| e.to_string())?,
                session_id,
                base_revision: ProjectRevision(value.base_revision),
                option: required(value.option, "manufacturing option")?.try_into()?,
            },
            C::RemoveManufacturingOption(value) => Self::RemoveManufacturingOption {
                layout_id: PrintLayoutId::parse(value.layout_id).map_err(|e| e.to_string())?,
                session_id,
                base_revision: ProjectRevision(value.base_revision),
                id: SourceHash::parse(value.id).map_err(|error| error.to_string())?,
            },
            C::ApplyManufacturingOption(value) => Self::ApplyManufacturingOption {
                layout_id: PrintLayoutId::parse(value.layout_id).map_err(|e| e.to_string())?,
                session_id,
                base_revision: ProjectRevision(value.base_revision),
                id: SourceHash::parse(value.id).map_err(|error| error.to_string())?,
            },
            C::PutRecipe(v) => Self::PutRecipe {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                transfer: required(v.transfer, "recipe transfer")?.try_into()?,
                expected: v
                    .expected
                    .map(|s| SourceHash::parse(s).map_err(|e| e.to_string()))
                    .transpose()?,
            },
            C::GetRecipe(v) => Self::GetRecipe {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                reference: required(v.reference, "recipe reference")?.try_into()?,
            },
            C::ListRecipes(v) => Self::ListRecipes {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                offset: v.offset,
            },
            C::LoadRecipeStudy(v) => Self::LoadRecipeStudy {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                reference: required(v.reference, "recipe reference")?.try_into()?,
                context: required(v.context, "recipe context")?.try_into()?,
            },
            C::CaptureRecipe(v) => {
                crate::library::validate_library_uuid(&v.id)?;
                if v.name.trim().is_empty()
                    || v.name.len() > 256
                    || v.name.chars().any(char::is_control)
                    || !(2..=3).contains(&v.ingredients.len())
                {
                    return Err("invalid captured recipe name/ingredients".into());
                }
                Self::CaptureRecipe {
                    session_id,
                    base_revision: ProjectRevision(v.base_revision),
                    id: crate::library::RecipeId(v.id),
                    name: v.name,
                    ingredients: v
                        .ingredients
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    expected: v
                        .expected
                        .map(|s| SourceHash::parse(s).map_err(|e| e.to_string()))
                        .transpose()?,
                }
            }
            C::SetHardware(v) => Self::SetHardware {
                layout_id: PrintLayoutId::parse(v.layout_id).map_err(|e| e.to_string())?,
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                capture: v.capture.map(|c| c.try_into().map(Arc::new)).transpose()?,
            },
            C::PutFilament(v) => Self::PutFilament {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                revision: required(v.revision, "filament revision")?.try_into()?,
                origin: v.origin.map(TryInto::try_into).transpose()?,
                expected: v
                    .expected
                    .map(SourceHash::parse)
                    .transpose()
                    .map_err(|e| e.to_string())?,
            },
            C::GetFilament(v) => Self::GetFilament {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                reference: required(v.reference, "filament reference")?.try_into()?,
            },
            C::ListFilaments(v) => Self::ListFilaments {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                offset: v.offset,
            },
            C::SetOpticalStudy(value) => Self::SetOpticalStudy {
                session_id,
                base_revision: ProjectRevision(value.base_revision),
                request: required(value.request, "optical request")?.try_into()?,
            },
            C::AcceptOpticalReport(value) => Self::AcceptOpticalReport {
                session_id,
                base_revision: ProjectRevision(value.base_revision),
                report_hash: SourceHash::parse(value.report_hash)
                    .map_err(|error| error.to_string())?,
                selection: value.selection.map(TryInto::try_into).transpose()?,
            },
            C::SetAppearanceTarget(v) => Self::SetAppearanceTarget {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                target: required(v.target, "appearance target")?.try_into()?,
                limits: required(v.limits, "appearance limits")?.try_into()?,
            },
            C::AcceptAppearanceReport(v) => {
                if v.selection.as_ref().is_some_and(|key| {
                    key.is_empty() || key.len() > 64 || key.chars().any(char::is_control)
                }) {
                    return Err("invalid appearance selection key".into());
                }
                Self::AcceptAppearanceReport {
                    session_id,
                    base_revision: ProjectRevision(v.base_revision),
                    report_hash: SourceHash::parse(v.report_hash)
                        .map_err(|error| error.to_string())?,
                    selection: v.selection,
                }
            }
            C::EditProcess(v) => Self::EditProcess {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                edit: required(v.edit, "process edit")?.try_into()?,
            },
            C::Get(_) => Self::Get { session_id },
            C::New(v) => Self::New {
                session_id,
                base_revision: SceneRevision(v.base_revision),
                discard_changes: v.discard_changes,
            },
            C::Undo(v) => Self::Undo {
                session_id,
                base_revision: SceneRevision(v.base_revision),
            },
            C::Redo(v) => Self::Redo {
                session_id,
                base_revision: SceneRevision(v.base_revision),
            },
            C::AddSurfaceFinish(v) => Self::AddSurfaceFinish {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                face_ref: required(v.face_ref, "surface source face")?.try_into()?,
                scope: match rpc::SurfaceScopeKind::try_from(v.scope)
                    .map_err(|_| "unknown surface scope")?
                {
                    rpc::SurfaceScopeKind::Definition => SurfaceScopeKind::Definition,
                    rpc::SurfaceScopeKind::Occurrence => SurfaceScopeKind::Occurrence,
                    rpc::SurfaceScopeKind::Unspecified => {
                        return Err("missing surface scope".into());
                    }
                },
                finish: required(v.finish, "surface finish")?.try_into()?,
            },
            C::RemoveSurfaceFinish(v) => Self::RemoveSurfaceFinish {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                layer_id: SurfaceLayerId::new(v.layer_id).map_err(|e| e.to_string())?,
            },
            C::SetSurfaceFinishEnabled(v) => Self::SetSurfaceFinishEnabled {
                session_id,
                base_revision: ProjectRevision(v.base_revision),
                layer_id: SurfaceLayerId::new(v.layer_id).map_err(|e| e.to_string())?,
                enabled: v.enabled,
            },
        })
    }
}
impl TryFrom<rpc::OpenProjectRequest> for ProjectCommand {
    type Error = String;
    fn try_from(v: rpc::OpenProjectRequest) -> Result<Self, String> {
        request_id(&v.request_id)?;
        Ok(Self::Open {
            session_id: session_parent(&v.parent)?,
            base_revision: SceneRevision(v.base_revision),
            path: required(v.path, "project path")?.try_into()?,
            read_only: v.read_only,
            recover_previous: v.recover_previous,
            discard_changes: v.discard_changes,
        })
    }
}
impl TryFrom<rpc::SaveProjectRequest> for ProjectCommand {
    type Error = String;
    fn try_from(v: rpc::SaveProjectRequest) -> Result<Self, String> {
        request_id(&v.request_id)?;
        Ok(Self::Save {
            session_id: session_parent(&v.parent)?,
            base_revision: SceneRevision(v.base_revision),
            target: v.target.map(TryInto::try_into).transpose()?,
        })
    }
}
impl TryFrom<rpc::ImportSurfaceImageFinishRequest> for ProjectCommand {
    type Error = String;
    fn try_from(v: rpc::ImportSurfaceImageFinishRequest) -> Result<Self, String> {
        request_id(&v.request_id)?;
        let session_id = session_parent(&v.parent)?;
        let reference: FaceRef = required(v.reference, "image source face")?.try_into()?;
        if reference.session_id != session_id {
            return Err("image face session mismatch".into());
        }
        let scope =
            match rpc::SurfaceScopeKind::try_from(v.scope).map_err(|_| "unknown surface scope")? {
                rpc::SurfaceScopeKind::Definition => SurfaceScopeKind::Definition,
                rpc::SurfaceScopeKind::Occurrence => SurfaceScopeKind::Occurrence,
                rpc::SurfaceScopeKind::Unspecified => return Err("missing surface scope".into()),
            };
        Ok(Self::ImportSurfaceImageFinish {
            session_id,
            base_revision: ProjectRevision(v.base_revision),
            reference,
            scope,
            source: required(v.source, "image source path")?.try_into()?,
            parameters: required(v.parameters, "image relief parameters")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::GetSurfaceImageRequest> for ProjectCommand {
    type Error = String;
    fn try_from(v: rpc::GetSurfaceImageRequest) -> Result<Self, String> {
        request_id(&v.request_id)?;
        Ok(Self::GetSurfaceImage {
            session_id: session_parent(&v.parent)?,
            base_revision: ProjectRevision(v.base_revision),
            image_hash: SourceHash::parse(v.image_hash).map_err(|e| e.to_string())?,
        })
    }
}
impl TryFrom<rpc::ImportAppearancePaletteRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::ImportAppearancePaletteRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::ImportAppearancePalette {
            session_id: session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
            source: required(value.source, "appearance palette source")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::CompareAppearanceRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::CompareAppearanceRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::CompareAppearance {
            session_id: session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
        })
    }
}
impl TryFrom<rpc::GetAppearanceReportRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::GetAppearanceReportRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::GetAppearanceReport {
            session_id: session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
        })
    }
}
impl TryFrom<rpc::CompareOpticalRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::CompareOpticalRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::CompareOptical {
            session_id: session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
        })
    }
}
impl TryFrom<rpc::GetOpticalReportRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::GetOpticalReportRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::GetOpticalReport {
            session_id: session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
        })
    }
}
pub fn validate_project_info(v: &ProjectInfo) -> Result<(), String> {
    if v.layouts.is_empty()
        || v.layouts.len() > MAX_PRINT_LAYOUTS
        || !v.layouts.iter().any(|l| l.layout_id == v.layout_id)
        || v.layouts
            .iter()
            .map(|l| &l.layout_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != v.layouts.len()
        || v.layouts
            .iter()
            .map(|l| u64::from(l.occurrence_count))
            .sum::<u64>()
            > u64::from(MAX_OCCURRENCES)
    {
        return Err("inconsistent plate summaries".into());
    }
    for l in &v.layouts {
        validate_layout_name(&l.name).map_err(|e| e.to_string())?;
    }

    if v.manufacturing_options.len() > MAX_MANUFACTURING_OPTIONS {
        return Err("manufacturing option summary count exceeds limit".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for option in &v.manufacturing_options {
        validate_option_info(option)?;
        if !ids.insert(&option.id) || !names.insert(&option.name) {
            return Err("invalid or duplicate manufacturing option summary".into());
        }
    }
    if let Some(record) = &v.materials {
        record.validate()?;
    }
    v.surface_overlays.validate().map_err(|e| e.to_string())?;
    v.process_document.validate().map_err(|e| e.to_string())?;
    v.appearance_study
        .validate()
        .map_err(|error| error.to_string())?;
    if v.resolved_process.len() > MAX_OCCURRENCES as usize {
        return Err("resolved process occurrence budget exceeded".into());
    }
    let mut previous = None;
    for row in &v.resolved_process {
        crate::manufacturing::validate_global_infill_fraction(row.global_fraction)
            .map_err(|e| e.to_string())?;
        if previous.is_some_and(|id| id >= row.occurrence_id)
            || row.value
                != v.process_document.resolve(
                    &row.definition_id,
                    row.occurrence_id,
                    row.global_fraction,
                )
            || row.inherited
                != v.process_document
                    .resolve_definition(&row.definition_id, row.global_fraction)
        {
            return Err("inconsistent authoritative process resolution".into());
        }
        previous = Some(row.occurrence_id);
    }
    if v.saved_revision.is_some_and(|r| r.0 > v.revision.0)
        || v.path_label.as_ref().is_some_and(|s| {
            s.is_empty() || s.len() > MAX_NATIVE_PATH_UNITS as usize * 4 || s.contains('\0')
        })
        || ((v.save_uncertain || v.recovered_previous) && !v.dirty)
        || (v.read_only && (v.can_undo || v.can_redo))
    {
        return Err("inconsistent project status".into());
    }
    Ok(())
}
impl TryFrom<rpc::ProjectInfo> for ProjectInfo {
    type Error = String;
    fn try_from(v: rpc::ProjectInfo) -> Result<Self, String> {
        if v.manufacturing_options.len() > MAX_MANUFACTURING_OPTIONS {
            return Err("manufacturing option summary count exceeds limit".into());
        }
        if v.resolved_process.len() > MAX_OCCURRENCES as usize {
            return Err("resolved process occurrence budget exceeded".into());
        }
        if v.layouts.len() > MAX_PRINT_LAYOUTS {
            return Err("plate count exceeds limit".into());
        }
        let info = Self {
            layout_id: PrintLayoutId::parse(v.layout_id).map_err(|e| e.to_string())?,
            layouts: v
                .layouts
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            manufacturing_options: v
                .manufacturing_options
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            project_id: ProjectId::parse(v.project_id).map_err(|e| e.to_string())?,
            revision: ProjectRevision(v.revision),
            saved_revision: v.saved_revision.map(ProjectRevision),
            path_label: v.path_label,
            dirty: v.dirty,
            save_uncertain: v.save_uncertain,
            read_only: v.read_only,
            recovered_previous: v.recovered_previous,
            can_undo: v.can_undo,
            can_redo: v.can_redo,
            surface_overlays: required(v.surface_overlays, "project surface overlays")?
                .try_into()?,
            process_document: required(v.process_document, "project process document")?
                .try_into()?,
            materials: v.materials.map(TryInto::try_into).transpose()?,
            hardware: v.hardware.map(|h| h.try_into().map(Arc::new)).transpose()?,
            appearance_study: required(v.appearance_study, "project appearance study")?
                .try_into()?,
            resolved_process: v
                .resolved_process
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        validate_project_info(&info)?;
        Ok(info)
    }
}
impl TryFrom<ProjectInfo> for rpc::ProjectInfo {
    type Error = String;
    fn try_from(v: ProjectInfo) -> Result<Self, String> {
        validate_project_info(&v)?;
        Ok(Self {
            layout_id: v.layout_id.as_str().into(),
            layouts: v
                .layouts
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            manufacturing_options: v
                .manufacturing_options
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            project_id: v.project_id.as_str().into(),
            revision: v.revision.0,
            saved_revision: v.saved_revision.map(|r| r.0),
            path_label: v.path_label,
            dirty: v.dirty,
            save_uncertain: v.save_uncertain,
            read_only: v.read_only,
            recovered_previous: v.recovered_previous,
            can_undo: v.can_undo,
            can_redo: v.can_redo,
            surface_overlays: Some(v.surface_overlays.try_into()?),
            process_document: Some(v.process_document.try_into()?),
            appearance_study: Some(v.appearance_study.try_into()?),
            materials: v.materials.map(TryInto::try_into).transpose()?,
            hardware: v.hardware.map(|h| (*h).clone().try_into()).transpose()?,
            resolved_process: v
                .resolved_process
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}
impl TryFrom<rpc::ProjectReply> for ProjectResponse {
    type Error = String;
    fn try_from(v: rpc::ProjectReply) -> Result<Self, String> {
        use rpc::project_reply::Response as R;
        Ok(match required(v.response, "project reply")? {
            R::ManufacturingOption(value) => Self::ManufacturingOption {
                option: value.try_into()?,
            },
            R::Recipe(v) => Self::Recipe {
                transfer: v.try_into()?,
            },
            R::Recipes(v) => {
                let entries = v
                    .entries
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<Vec<_>, _>>()?;
                crate::library::validate_recipe_page(&entries, v.total, v.next_offset)?;
                Self::Recipes {
                    entries,
                    total: v.total,
                    next_offset: v.next_offset,
                }
            }
            R::Filament(v) => Self::Filament {
                revision: v.try_into()?,
            },
            R::Filaments(v) => {
                if v.entries.len() > crate::library::MAX_LIBRARY_PAGE as usize
                    || v.total > crate::library::MAX_FILAMENTS as u32
                    || v.entries.len() > v.total as usize
                    || v.next_offset.is_some_and(|n| n == 0 || n >= v.total)
                {
                    return Err("invalid project material page".into());
                }
                Self::Filaments {
                    entries: v
                        .entries
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    total: v.total,
                    next_offset: v.next_offset,
                }
            }
            R::Status(info) => Self::Status {
                info: info.try_into()?,
            },
            R::SceneChanged(v) => Self::SceneChanged {
                info: required(v.info, "project info")?.try_into()?,
                summary: required(v.summary, "scene summary")?.try_into()?,
            },
        })
    }
}
impl TryFrom<ProjectResponse> for rpc::ProjectReply {
    type Error = String;
    fn try_from(v: ProjectResponse) -> Result<Self, String> {
        use rpc::project_reply::Response as R;
        Ok(Self {
            response: Some(match v {
                ProjectResponse::ManufacturingOption { option } => {
                    R::ManufacturingOption(option.try_into()?)
                }
                ProjectResponse::Recipe { transfer } => R::Recipe(transfer.try_into()?),
                ProjectResponse::Recipes {
                    entries,
                    total,
                    next_offset,
                } => {
                    crate::library::validate_recipe_page(&entries, total, next_offset)?;
                    R::Recipes(rpc::ProjectRecipePage {
                        entries: entries
                            .into_iter()
                            .map(TryInto::try_into)
                            .collect::<Result<_, _>>()?,
                        total,
                        next_offset,
                    })
                }
                ProjectResponse::Filament { revision } => R::Filament(revision.try_into()?),
                ProjectResponse::Filaments {
                    entries,
                    total,
                    next_offset,
                } => R::Filaments(rpc::ProjectFilamentPage {
                    entries: entries
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    total,
                    next_offset,
                }),
                ProjectResponse::Status { info } => R::Status(info.try_into()?),
                ProjectResponse::SceneChanged { info, summary } => {
                    R::SceneChanged(rpc::ProjectSceneChanged {
                        info: Some(info.try_into()?),
                        summary: Some(summary.try_into()?),
                    })
                }
                _ => return Err("project response requires Operation or Status".into()),
            }),
        })
    }
}

impl TryFrom<rpc::SurfaceFinish> for SurfaceFinish {
    type Error = String;
    fn try_from(v: rpc::SurfaceFinish) -> Result<Self, String> {
        use rpc::surface_finish::Kind;
        let finish = match required(v.kind, "surface finish kind")? {
            Kind::Smooth(_) => Self::Smooth,
            Kind::VerticalRibs(v) => Self::VerticalRibs {
                depth_mm: v.depth_mm,
                pitch_mm: v.pitch_mm,
                edge_margin_mm: v.edge_margin_mm,
                phase_mm: v.phase_mm,
            },
            Kind::CylindricalRibs(v) => Self::CylindricalRibs {
                depth_mm: v.depth_mm,
                rib_count: v.rib_count,
                phase_fraction: v.phase_fraction,
            },
            Kind::ImageRelief(v) => Self::ImageRelief {
                image_hash: SourceHash::parse(v.image_hash).map_err(|e| e.to_string())?,
                parameters: required(v.parameters, "image relief parameters")?.try_into()?,
            },
        };
        finish.validate().map_err(|e| e.to_string())?;
        Ok(finish)
    }
}
impl TryFrom<SurfaceFinish> for rpc::SurfaceFinish {
    type Error = String;
    fn try_from(v: SurfaceFinish) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        use rpc::surface_finish::Kind;
        Ok(Self {
            kind: Some(match v {
                SurfaceFinish::Smooth => Kind::Smooth(rpc::SurfaceSmooth {}),
                SurfaceFinish::VerticalRibs {
                    depth_mm,
                    pitch_mm,
                    edge_margin_mm,
                    phase_mm,
                } => Kind::VerticalRibs(rpc::SurfaceVerticalRibs {
                    depth_mm,
                    pitch_mm,
                    edge_margin_mm,
                    phase_mm,
                }),
                SurfaceFinish::CylindricalRibs {
                    depth_mm,
                    rib_count,
                    phase_fraction,
                } => Kind::CylindricalRibs(rpc::SurfaceCylindricalRibs {
                    depth_mm,
                    rib_count,
                    phase_fraction,
                }),
                SurfaceFinish::ImageRelief {
                    image_hash,
                    parameters,
                } => Kind::ImageRelief(rpc::SurfaceImageRelief {
                    image_hash: image_hash.into(),
                    parameters: Some(parameters.try_into()?),
                }),
            }),
        })
    }
}
impl TryFrom<rpc::SurfaceScope> for SurfaceScope {
    type Error = String;
    fn try_from(v: rpc::SurfaceScope) -> Result<Self, String> {
        use rpc::surface_scope::Kind;
        Ok(match required(v.kind, "surface scope kind")? {
            Kind::Definition(v) => Self::Definition {
                definition_id: DefinitionId::parse(v.definition_id).map_err(|e| e.to_string())?,
            },
            Kind::Occurrence(v) => Self::Occurrence {
                occurrence_id: OccurrenceId::new(v.occurrence_id).map_err(|e| e.to_string())?,
                expected_definition_id: DefinitionId::parse(v.expected_definition_id)
                    .map_err(|e| e.to_string())?,
            },
        })
    }
}
impl TryFrom<SurfaceScope> for rpc::SurfaceScope {
    type Error = String;
    fn try_from(v: SurfaceScope) -> Result<Self, String> {
        use rpc::surface_scope::Kind;
        Ok(Self {
            kind: Some(match v {
                SurfaceScope::Definition { definition_id } => {
                    Kind::Definition(rpc::SurfaceDefinitionScope {
                        definition_id: definition_id.as_str().into(),
                    })
                }
                SurfaceScope::Occurrence {
                    occurrence_id,
                    expected_definition_id,
                } => Kind::Occurrence(rpc::SurfaceOccurrenceScope {
                    occurrence_id: occurrence_id.get(),
                    expected_definition_id: expected_definition_id.as_str().into(),
                }),
            }),
        })
    }
}
impl TryFrom<rpc::SurfaceOverlays> for SurfaceOverlays {
    type Error = String;
    fn try_from(v: rpc::SurfaceOverlays) -> Result<Self, String> {
        if v.layers.len() > MAX_SURFACE_LAYERS || v.images.len() > MAX_SURFACE_IMAGES {
            return Err("surface layer budget exceeded".into());
        }
        let layers = v
            .layers
            .into_iter()
            .map(|v| {
                Ok(SurfaceLayer {
                    layer_id: SurfaceLayerId::new(v.layer_id).map_err(|e| e.to_string())?,
                    enabled: v.enabled,
                    scope: required(v.scope, "surface scope")?.try_into()?,
                    face_id: FaceId::parse(v.face_id).map_err(|e| e.to_string())?,
                    finish: required(v.finish, "surface finish")?.try_into()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let doc = Self {
            schema_version: v.schema_version,
            images: v
                .images
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, String>>()?,
            layers,
        };
        doc.validate().map_err(|e| e.to_string())?;
        Ok(doc)
    }
}
impl TryFrom<SurfaceOverlays> for rpc::SurfaceOverlays {
    type Error = String;
    fn try_from(v: SurfaceOverlays) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            schema_version: v.schema_version,
            images: v
                .images
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, String>>()?,
            layers: v
                .layers
                .into_iter()
                .map(|v| {
                    Ok(rpc::SurfaceLayer {
                        layer_id: v.layer_id.get(),
                        enabled: v.enabled,
                        scope: Some(v.scope.try_into()?),
                        face_id: v.face_id.as_str().into(),
                        finish: Some(v.finish.try_into()?),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        })
    }
}

impl TryFrom<rpc::ImageReliefParameters> for ImageReliefParameters {
    type Error = String;
    fn try_from(v: rpc::ImageReliefParameters) -> Result<Self, String> {
        let parameters = Self {
            depth_mm: v.depth_mm,
            u_repeats: v.u_repeats,
            u_phase_fraction: v.u_phase_fraction,
            z_period_mm: v.z_period_mm,
            z_phase_mm: v.z_phase_mm,
            edge_margin_mm: v.edge_margin_mm,
            vertical_error_mm: v.vertical_error_mm,
        };
        parameters.validate().map_err(|e| e.to_string())?;
        Ok(parameters)
    }
}
impl TryFrom<ImageReliefParameters> for rpc::ImageReliefParameters {
    type Error = String;
    fn try_from(v: ImageReliefParameters) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            depth_mm: v.depth_mm,
            u_repeats: v.u_repeats,
            u_phase_fraction: v.u_phase_fraction,
            z_period_mm: v.z_period_mm,
            z_phase_mm: v.z_phase_mm,
            edge_margin_mm: v.edge_margin_mm,
            vertical_error_mm: v.vertical_error_mm,
        })
    }
}
impl TryFrom<rpc::SurfaceImageRecord> for SurfaceImageRecord {
    type Error = String;
    fn try_from(v: rpc::SurfaceImageRecord) -> Result<Self, String> {
        let record = Self {
            image_hash: SourceHash::parse(v.image_hash).map_err(|e| e.to_string())?,
            width: v.width,
            height: v.height,
            byte_count: v.byte_count,
            source_hash: SourceHash::parse(v.source_hash).map_err(|e| e.to_string())?,
            source_name: v.source_name,
        };
        record.validate().map_err(|e| e.to_string())?;
        Ok(record)
    }
}
impl TryFrom<SurfaceImageRecord> for rpc::SurfaceImageRecord {
    type Error = String;
    fn try_from(v: SurfaceImageRecord) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            image_hash: v.image_hash.into(),
            width: v.width,
            height: v.height,
            byte_count: v.byte_count,
            source_hash: v.source_hash.into(),
            source_name: v.source_name,
        })
    }
}
pub fn validate_surface_image_prepared_binding(
    project_id: &ProjectId,
    _captured_revision: ProjectRevision,
    record: &SurfaceImageRecord,
    resource: &crate::ArtifactView,
) -> Result<(), String> {
    ProjectId::parse(project_id.as_str()).map_err(|e| e.to_string())?;
    record.validate().map_err(|e| e.to_string())?;
    let size = super::validate_artifact_view(resource, MAX_SURFACE_IMAGE_BYTES as u64)?;
    if size != record.byte_count
        || resource.sha256 != record.image_hash.as_str()
        || resource.media_type != SURFACE_IMAGE_MEDIA_TYPE
    {
        return Err("image resource disagrees with canonical record".into());
    }
    Ok(())
}
pub fn validate_surface_image_imported_binding(
    info: &ProjectInfo,
    image: &SurfaceImageRecord,
    layer_id: SurfaceLayerId,
) -> Result<(), String> {
    validate_project_info(info)?;
    image.validate().map_err(|e| e.to_string())?;
    if info.read_only || !info.surface_overlays.images.iter().any(|record| record == image)
        || !info.surface_overlays.layers.last().is_some_and(|layer| layer.enabled && layer.layer_id == layer_id && matches!(&layer.finish, SurfaceFinish::ImageRelief { image_hash, .. } if image_hash == &image.image_hash)) {
        return Err("imported image result disagrees with project metadata".into());
    }
    Ok(())
}

impl TryFrom<rpc::ApplyOpticalConstructionRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::ApplyOpticalConstructionRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::ApplyOpticalConstruction {
            session_id: session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
            application: required(value.application, "optical construction application")?
                .try_into()?,
        })
    }
}
