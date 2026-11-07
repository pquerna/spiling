// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

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
fn compilation_byte_budget(retained_bytes: u64) -> Result<u32, ManufacturingError> {
    MAX_MANUFACTURING_RETAINED_BYTES
        .checked_sub(retained_bytes)
        .filter(|remaining| *remaining > 0)
        .map(|remaining| remaining.min(u64::from(MAX_MANUFACTURING_BUNDLE_BYTES)) as u32)
        .ok_or_else(|| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "no remaining retained/staged manufacturing budget",
            )
        })
}
impl Runtime {
    pub(super) fn capture_compile_input(
        &self,
        snapshot: &spiling_core::Snapshot,
        revision: ProjectRevision,
        intent: Arc<ManufacturingIntent>,
        byte_budget: u32,
    ) -> Result<super::worker::CompileRequest, ManufacturingError> {
        let (surfaces, process) = snapshot.scoped_documents();
        let surface_images = self
            .capture_surface_images()?
            .into_iter()
            .filter(|(hash, _)| surfaces.images.iter().any(|i| &i.image_hash == hash))
            .collect();
        Ok(super::worker::CompileRequest {
            project_id: self.project.info().project_id,
            revision,
            definition_ids: snapshot
                .active_sources()
                .map(|a| a.record.definition_id.clone())
                .collect(),
            stored_definitions: snapshot
                .active_sources()
                .map(|a| a.record.clone())
                .collect(),
            occurrences: snapshot.occurrences.values().cloned().collect(),
            intent,
            surface_overlays: Arc::new(surfaces),
            process_document: Arc::new(process),
            surface_images,
            byte_budget,
        })
    }

    pub(super) fn capture_surface_images(
        &self,
    ) -> Result<BTreeMap<SourceHash, Arc<spiling_surface_fields::HeightImage>>, ManufacturingError>
    {
        self.project
            .surface_images()
            .iter()
            .map(|(hash, asset)| {
                spiling_surface_fields::HeightImage::from_canonical(
                    asset.record().clone(),
                    asset.bytes().clone(),
                )
                .map(|image| (hash.clone(), Arc::new(image)))
                .map_err(project_error)
            })
            .collect()
    }
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
            active.deciding
                || matches!(
                    active.purpose,
                    Purpose::Open | Purpose::Save | Purpose::StorageMaintenance
                )
        }) {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::Busy,
                "a project job or promotion is active",
            ));
        }
        Ok(())
    }
    fn manufacturing_status(&self) -> Reply {
        let snapshot = self.project.snapshot();
        Reply::Manufacturing(ManufacturingResponse::Status {
            info: self.project.info(),
            context_hash: match spiling_core::context_hash(&snapshot) {
                Ok(hash) => hash,
                Err(error) => {
                    return Reply::Manufacturing(ManufacturingResponse::Error {
                        error: project_error(error),
                    });
                }
            },
            intent: snapshot.manufacturing_intent.clone(),
            artifact: snapshot
                .manufacturing_artifact
                .as_ref()
                .map(|asset| asset.record.clone()),
            resource: None,
        })
    }
    pub fn manufacturing_control(&mut self, command: ManufacturingCommand) -> Reply {
        match self.manufacturing_inner(command) {
            Ok(response) => response,
            Err(error) => Reply::Manufacturing(ManufacturingResponse::Error { error }),
        }
    }
    fn manufacturing_inner(
        &mut self,
        command: ManufacturingCommand,
    ) -> Result<Reply, ManufacturingError> {
        if command.layout_id() != &self.project.snapshot().layout_id {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::StaleRevision,
                "the addressed print layout is not the current scene",
            ));
        }
        match command {
            ManufacturingCommand::Get { session_id, .. } => {
                self.manufacturing_session(&session_id)?;
                Ok(self.manufacturing_status())
            }
            ManufacturingCommand::SetIntent {
                layout_id: _,
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
            ManufacturingCommand::EditInteriorPolicy {
                session_id,
                base_revision,
                edit,
                ..
            } => {
                self.manufacturing_base(&session_id, base_revision)?;
                let edit = self
                    .project
                    .prepare(Edit::SetInteriorToolPolicy { edit })
                    .map_err(project_error)?;
                self.project.commit(edit).map_err(project_error)?;
                self.synchronize_native_pins();
                Ok(self.manufacturing_status())
            }
            ManufacturingCommand::ReviewMaterials {
                layout_id: _,
                session_id,
                base_revision,
                request,
            } => {
                self.manufacturing_base(&session_id, base_revision)?;
                let current = self
                    .project
                    .snapshot()
                    .manufacturing_intent
                    .clone()
                    .ok_or_else(|| {
                        ManufacturingError::new(
                            ManufacturingErrorCode::NoIntent,
                            "set an explicit manufacturing intent before reviewing materials",
                        )
                    })?;
                let report =
                    spiling_manufacturing::review_materials(&current, base_revision, request)?;
                Ok(Reply::Manufacturing(
                    ManufacturingResponse::MaterialReview { report },
                ))
            }
            ManufacturingCommand::ApplyMaterialProfile {
                tool_id,
                layout_id: _,
                session_id,
                base_revision,
                selection,
            } => {
                self.manufacturing_base(&session_id, base_revision)?;
                let current = self.project.snapshot().manufacturing_intent.clone().ok_or_else(||
                    ManufacturingError::new(ManufacturingErrorCode::NoIntent,"an explicit printer/recipe intent is required before material selection"))?;
                let intent = Arc::new(if let Some(tool_id) = tool_id {
                    spiling_manufacturing::resolve_tool_material_profile(
                        &current, &tool_id, selection,
                    )?
                } else {
                    spiling_manufacturing::resolve_material_profile(&current, selection)?
                });
                let edit = self
                    .project
                    .prepare(Edit::SetManufacturingIntent { intent })
                    .map_err(project_error)?;
                self.project.commit(edit).map_err(project_error)?;
                self.synchronize_native_pins();
                Ok(self.manufacturing_status())
            }
            ManufacturingCommand::Preflight {
                layout_id: _,
                session_id,
                base_revision,
            } => {
                self.manufacturing_base(&session_id, base_revision)?;
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
                let request = self.capture_compile_input(&snapshot, base_revision, intent, 0)?;
                self.start(Purpose::Preflight, 0, move |_, _, _| {
                    Work::Preflight(request)
                })
                .map_err(geometry_error)
            }
            ManufacturingCommand::EvaluateAlternative {
                layout_id: _,
                session_id,
                base_revision,
                expected_context_hash,
                intent,
            } => {
                self.manufacturing_base(&session_id, base_revision)?;
                if self.active.is_some() {
                    return Err(ManufacturingError::new(
                        ManufacturingErrorCode::Busy,
                        "an engine job is active",
                    ));
                }
                let baseline = self.project.snapshot();
                let context_hash = spiling_core::context_hash(&baseline).map_err(project_error)?;
                if context_hash != expected_context_hash {
                    return Err(ManufacturingError::new(
                        ManufacturingErrorCode::StaleRevision,
                        "alternative source context is stale",
                    ));
                }
                let (changed_inputs, changed_inputs_complete) =
                    manufacturing_input_changes(baseline.manufacturing_intent.as_deref(), &intent)?;
                let snapshot = spiling_core::trial_snapshot(&baseline, intent.clone())
                    .map_err(project_error)?;
                if snapshot.occurrences.is_empty() {
                    return Err(ManufacturingError::new(
                        ManufacturingErrorCode::EmptyProject,
                        "manufacturing requires source-backed occurrences",
                    ));
                }
                let request = self.capture_compile_input(
                    &snapshot,
                    base_revision,
                    intent,
                    compilation_byte_budget(self.manufacturing_bytes())?,
                )?;
                self.start(Purpose::EvaluateAlternative, 0, move |_, _, _| {
                    Work::EvaluateAlternative {
                        request,
                        context_hash,
                        changed_inputs,
                        changed_inputs_complete,
                    }
                })
                .map_err(geometry_error)
            }
            ManufacturingCommand::Compile {
                layout_id: _,
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
                let request = self.capture_compile_input(
                    &snapshot,
                    base_revision,
                    intent,
                    compilation_byte_budget(self.manufacturing_bytes())?,
                )?;
                let response = self
                    .start(Purpose::Compile, 0, move |_, _, _| Work::Compile(request))
                    .map_err(geometry_error)?;
                Ok(response)
            }
            ManufacturingCommand::Inspect {
                layout_id: _,
                session_id,
                base_revision,
                output,
            } => {
                self.manufacturing_base(&session_id, base_revision)?;
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
                    .start(Purpose::Verify, 0, move |_, _, _| Work::Verify {
                        asset,
                        output,
                    })
                    .map_err(geometry_error)?;
                Ok(response)
            }
        }
    }
    pub fn layout_manufacturing_assets(&self) -> Vec<Arc<spiling_core::ManufacturingAsset>> {
        self.project
            .snapshot()
            .layouts
            .iter()
            .filter_map(|layout| layout.manufacturing_artifact.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compilation_reserves_only_remaining_current_and_history_headroom() {
        let retained = 60 * 1024 * 1024;
        assert_eq!(compilation_byte_budget(retained).unwrap(), 4 * 1024 * 1024);
        assert_eq!(
            compilation_byte_budget(0).unwrap(),
            MAX_MANUFACTURING_BUNDLE_BYTES
        );
        assert_eq!(
            compilation_byte_budget(MAX_MANUFACTURING_RETAINED_BYTES - 1).unwrap(),
            1
        );
        for retained in [MAX_MANUFACTURING_RETAINED_BYTES, u64::MAX] {
            assert_eq!(
                compilation_byte_budget(retained).unwrap_err().code,
                ManufacturingErrorCode::ResourceLimit
            );
        }
    }
}
