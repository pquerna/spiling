// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{request_id, required, session_parent};
use crate::{geometry::*, process::*, project::*, rpc, surface::*};

pub enum ProjectCall {
    Execute(rpc::ProjectRequest),
    Open(rpc::OpenProjectRequest),
    Save(rpc::SaveProjectRequest),
    ImportSurfaceImageFinish(rpc::ImportSurfaceImageFinishRequest),
    GetSurfaceImage(rpc::GetSurfaceImageRequest),
    ImportAppearancePalette(rpc::ImportAppearancePaletteRequest),
    CompareAppearance(rpc::CompareAppearanceRequest),
    GetAppearanceReport(rpc::GetAppearanceReportRequest),
}
pub fn project_call(command: ProjectCommand, id: &str) -> Result<ProjectCall, String> {
    request_id(id)?;
    use rpc::project_request::Command as C;
    let (session_id, command) = match command {
        ProjectCommand::ImportAppearancePalette { session_id, base_revision, source } => {
            return Ok(ProjectCall::ImportAppearancePalette(rpc::ImportAppearancePaletteRequest {
                parent: format!("sessions/{}", session_id.as_str()), request_id: id.into(),
                base_revision: base_revision.0, source: Some(source.try_into()?),
            }));
        }
        ProjectCommand::CompareAppearance { session_id, base_revision } => {
            return Ok(ProjectCall::CompareAppearance(rpc::CompareAppearanceRequest {
                parent: format!("sessions/{}", session_id.as_str()), request_id: id.into(),
                base_revision: base_revision.0,
            }));
        }
        ProjectCommand::GetAppearanceReport { session_id, base_revision } => {
            return Ok(ProjectCall::GetAppearanceReport(rpc::GetAppearanceReportRequest {
                parent: format!("sessions/{}", session_id.as_str()), request_id: id.into(),
                base_revision: base_revision.0,
            }));
        }
        ProjectCommand::SetAppearanceTarget { session_id, base_revision, target, limits } => (
            session_id, C::SetAppearanceTarget(rpc::SetAppearanceTargetRequest {
                base_revision: base_revision.0, target: Some(target.try_into()?),
                limits: Some(limits.try_into()?),
            }),
        ),
        ProjectCommand::AcceptAppearanceReport { session_id, base_revision, report_hash, selection } => (
            session_id, C::AcceptAppearanceReport(rpc::AcceptAppearanceReportRequest {
                base_revision: base_revision.0, report_hash: report_hash.into(), selection,
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
    Ok(ProjectCall::Execute(rpc::ProjectRequest {
        parent: format!("sessions/{}", session_id.as_str()),
        command: Some(command),
    }))
}
impl TryFrom<rpc::ProjectRequest> for ProjectCommand {
    type Error = String;
    fn try_from(v: rpc::ProjectRequest) -> Result<Self, String> {
        let session_id = session_parent(&v.parent)?;
        use rpc::project_request::Command as C;
        Ok(match required(v.command, "project command")? {
            C::SetAppearanceTarget(v) => Self::SetAppearanceTarget {
                session_id, base_revision: ProjectRevision(v.base_revision),
                target: required(v.target, "appearance target")?.try_into()?,
                limits: required(v.limits, "appearance limits")?.try_into()?,
            },
            C::AcceptAppearanceReport(v) => {
                if v.selection.as_ref().is_some_and(|key| key.is_empty() || key.len() > 64 || key.chars().any(char::is_control)) {
                    return Err("invalid appearance selection key".into());
                }
                Self::AcceptAppearanceReport {
                    session_id, base_revision: ProjectRevision(v.base_revision),
                    report_hash: SourceHash::parse(v.report_hash).map_err(|error| error.to_string())?,
                    selection: v.selection,
                }
            },
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
            session_id: session_parent(&value.parent)?, base_revision: ProjectRevision(value.base_revision),
            source: required(value.source, "appearance palette source")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::CompareAppearanceRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::CompareAppearanceRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::CompareAppearance { session_id: session_parent(&value.parent)?, base_revision: ProjectRevision(value.base_revision) })
    }
}
impl TryFrom<rpc::GetAppearanceReportRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::GetAppearanceReportRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::GetAppearanceReport { session_id: session_parent(&value.parent)?, base_revision: ProjectRevision(value.base_revision) })
    }
}
pub fn validate_project_info(v: &ProjectInfo) -> Result<(), String> {
    v.surface_overlays.validate().map_err(|e| e.to_string())?;
    v.process_document.validate().map_err(|e| e.to_string())?;
    v.appearance_study.validate().map_err(|error| error.to_string())?;
    if v.resolved_process.len() > MAX_OCCURRENCES as usize {
        return Err("resolved process occurrence budget exceeded".into());
    }
    let mut previous = None;
    for row in &v.resolved_process {
        ProcessValue::SparseFraction {
            fraction: row.global_fraction,
        }
        .validate()
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
        if v.resolved_process.len() > MAX_OCCURRENCES as usize {
            return Err("resolved process occurrence budget exceeded".into());
        }
        let info = Self {
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
            appearance_study: required(v.appearance_study, "project appearance study")?.try_into()?,
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
