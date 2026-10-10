// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::MAX_BINARY_BYTES;

pub(super) fn project_error(error: ProjectError) -> ManufacturingError {
    let code = match error.code {
        ProjectErrorCode::ReadOnly => ManufacturingErrorCode::ReadOnly,
        ProjectErrorCode::Busy => ManufacturingErrorCode::Busy,
        ProjectErrorCode::StaleRevision => ManufacturingErrorCode::StaleRevision,
        ProjectErrorCode::ResourceLimit => ManufacturingErrorCode::ResourceLimit,
        ProjectErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
        ProjectErrorCode::Io => ManufacturingErrorCode::Io,
        _ => ManufacturingErrorCode::CorruptArtifact,
    };
    ManufacturingError::new(code, error.message)
}
fn geometry_error(error: GeometryError) -> ManufacturingError {
    let code = match error.code {
        GeometryErrorCode::StaleRevision => ManufacturingErrorCode::StaleRevision,
        GeometryErrorCode::Busy => ManufacturingErrorCode::Busy,
        GeometryErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
        _ => ManufacturingErrorCode::ResourceLimit,
    };
    ManufacturingError::new(code, error.message)
}
impl Runtime {
    pub(super) fn manufacturing_bytes(&self) -> u64 {
        self.project
            .retained_manufacturing_assets()
            .iter()
            .map(|asset| asset.bytes.capacity() as u64)
            .sum()
    }
    fn manufacturing_session(&self, session: &SessionId) -> Result<(), ManufacturingError> {
        self.session(session).map_err(geometry_error)
    }
    fn manufacturing_base(
        &self,
        session: &SessionId,
        base: ProjectRevision,
    ) -> Result<(), ManufacturingError> {
        self.manufacturing_session(session)?;
        if base != self.project.info().revision {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::StaleRevision,
                "project input revision is stale",
            ));
        }
        if self.active.as_ref().is_some_and(|active| {
            active.deciding || matches!(active.purpose, Purpose::Open | Purpose::Save)
        }) {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::Busy,
                "a project job or promotion is active",
            ));
        }
        Ok(())
    }
    fn manufacturing_status(&self) -> ManufacturingResponse {
        let snapshot = self.project.snapshot();
        ManufacturingResponse::Status {
            info: self.project.info(),
            intent: snapshot.manufacturing_intent.clone(),
            artifact: snapshot
                .manufacturing_artifact
                .as_ref()
                .map(|asset| asset.record.clone()),
        }
    }
    pub fn manufacturing_control(
        &mut self,
        command: ManufacturingCommand,
    ) -> ManufacturingResponse {
        match self.manufacturing_inner(command) {
            Ok(response) => response,
            Err(error) => ManufacturingResponse::Error { error },
        }
    }
    fn manufacturing_inner(
        &mut self,
        command: ManufacturingCommand,
    ) -> Result<ManufacturingResponse, ManufacturingError> {
        match command {
            ManufacturingCommand::Get { session_id } => {
                self.manufacturing_session(&session_id)?;
                Ok(self.manufacturing_status())
            }
            ManufacturingCommand::SetIntent {
                session_id,
                base_revision,
                intent,
            } => {
                self.manufacturing_base(&session_id, base_revision)?;
                intent.validate()?;
                let edit = self
                    .project
                    .prepare(Edit::SetManufacturingIntent { intent })
                    .map_err(project_error)?;
                self.project.commit(edit).map_err(project_error)?;
                self.synchronize_native_pins();
                // Persistent intent changes are not geometry/scene changes. An active
                // compiler sees its captured ProjectRevision become stale at staging.
                Ok(self.manufacturing_status())
            }
            ManufacturingCommand::Compile {
                session_id,
                base_revision,
            } => {
                self.manufacturing_base(&session_id, base_revision)?;
                if self.project.info().read_only {
                    return Err(ManufacturingError::new(
                        ManufacturingErrorCode::ReadOnly,
                        "read-only projects cannot publish compilation",
                    ));
                }
                if self.active.is_some() {
                    return Err(ManufacturingError::new(
                        ManufacturingErrorCode::Busy,
                        "an engine job is active",
                    ));
                }
                let snapshot = self.project.snapshot();
                let intent = snapshot.manufacturing_intent.clone().ok_or_else(|| {
                    ManufacturingError::new(
                        ManufacturingErrorCode::NoIntent,
                        "an explicit printer specification and recipe are required",
                    )
                })?;
                if snapshot.occurrences.is_empty() {
                    return Err(ManufacturingError::new(
                        ManufacturingErrorCode::EmptyProject,
                        "manufacturing requires source-backed occurrences",
                    ));
                }
                let project_id = self.project.info().project_id;
                let definition_ids = snapshot.sources.keys().cloned().collect();
                let stored_definitions = snapshot
                    .sources
                    .values()
                    .map(|source| source.record.clone())
                    .collect();
                let occurrences = snapshot.occurrences.values().cloned().collect();
                let byte_budget = MAX_MANUFACTURING_RETAINED_BYTES
                    .checked_sub(self.manufacturing_bytes())
                    .filter(|budget| *budget > 0)
                    .ok_or_else(|| {
                        ManufacturingError::new(
                            ManufacturingErrorCode::ResourceLimit,
                            "no remaining retained/staged manufacturing budget",
                        )
                    })?;
                let response = self
                    .start(Purpose::Compile, 0, move |_, _, _| {
                        Work::Compile(super::worker::CompileRequest {
                            project_id,
                            revision: base_revision,
                            definition_ids,
                            stored_definitions,
                            occurrences,
                            intent,
                            byte_budget: byte_budget.min(MAX_MANUFACTURING_BUNDLE_BYTES as u64)
                                as u32,
                        })
                    })
                    .map_err(geometry_error)?;
                let GeometryResponse::JobAccepted { job_id } = response else {
                    unreachable!("start only accepts a job")
                };
                Ok(ManufacturingResponse::JobAccepted { job_id })
            }
            ManufacturingCommand::Inspect { session_id } => {
                self.manufacturing_session(&session_id)?;
                if self.active.is_some() {
                    return Err(ManufacturingError::new(
                        ManufacturingErrorCode::Busy,
                        "an engine job is active",
                    ));
                }
                let asset = self
                    .project
                    .snapshot()
                    .manufacturing_artifact
                    .clone()
                    .ok_or_else(|| {
                        ManufacturingError::new(
                            ManufacturingErrorCode::NoIntent,
                            "project has no current manufacturing artifact",
                        )
                    })?;
                let response = self
                    .start(Purpose::Verify, 0, move |_, _, _| Work::Verify { asset })
                    .map_err(geometry_error)?;
                let GeometryResponse::JobAccepted { job_id } = response else {
                    unreachable!("start only accepts a job")
                };
                Ok(ManufacturingResponse::JobAccepted { job_id })
            }
            ManufacturingCommand::ReadArtifactChunk { .. } => unreachable!("binary dispatch"),
        }
    }
    pub fn write_manufacturing_chunk(
        &self,
        session: &SessionId,
        hash: &SourceHash,
        offset: u32,
        max_bytes: u32,
        request: u32,
        output: &mut impl Write,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let result = (|| {
            self.manufacturing_session(session)?;
            if max_bytes == 0 || max_bytes > MAX_BINARY_BYTES {
                return Err(ManufacturingError::new(
                    ManufacturingErrorCode::ResourceLimit,
                    "manufacturing chunk must fit the negotiated binary cap",
                ));
            }
            let asset = self
                .project
                .retained_manufacturing_assets()
                .into_iter()
                .find(|asset| &asset.record.hash == hash)
                .ok_or_else(|| {
                    ManufacturingError::new(
                        ManufacturingErrorCode::CorruptArtifact,
                        "manufacturing artifact is not retained in this session",
                    )
                })?;
            let total_bytes = asset.bytes.len() as u32;
            if offset >= total_bytes {
                return Err(ManufacturingError::new(
                    ManufacturingErrorCode::CorruptArtifact,
                    "manufacturing chunk offset is outside the immutable bundle",
                ));
            }
            Ok((asset, (total_bytes - offset).min(max_bytes), total_bytes))
        })();
        match result {
            Ok((asset, byte_count, total_bytes)) => {
                Frame::control(
                    request,
                    &Response::Manufacturing {
                        response: ManufacturingResponse::ArtifactChunk {
                            hash: hash.clone(),
                            offset,
                            total_bytes,
                            byte_count,
                        },
                    },
                )?
                .write(output)?;
                Frame::write_payload(
                    FrameKind::ManufacturingChunk,
                    request,
                    &asset.bytes[offset as usize..(offset + byte_count) as usize],
                    output,
                )?;
            }
            Err(error) => Frame::control(
                request,
                &Response::Manufacturing {
                    response: ManufacturingResponse::Error { error },
                },
            )?
            .write(output)?,
        }
        Ok(())
    }
}
