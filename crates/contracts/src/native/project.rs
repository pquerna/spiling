// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{request_id, required, session_parent};
use crate::{geometry::*, project::*, rpc};

pub enum ProjectCall {
    Execute(rpc::ProjectRequest),
    Open(rpc::OpenProjectRequest),
    Save(rpc::SaveProjectRequest),
}
pub fn project_call(command: ProjectCommand, id: &str) -> Result<ProjectCall, String> {
    request_id(id)?;
    use rpc::project_request::Command as C;
    let (session_id, command) = match command {
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
pub fn validate_project_info(v: &ProjectInfo) -> Result<(), String> {
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
