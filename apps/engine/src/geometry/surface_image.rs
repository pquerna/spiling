// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_core::SurfaceImageAsset;

impl Runtime {
    pub fn validate_image_admission(&self, command: &Control) -> Result<(), ProjectError> {
        match command {
            Control::Project(ProjectCommand::ImportSurfaceImageFinish {
                session_id,
                base_revision,
                reference,
                parameters,
                source,
                ..
            }) => {
                self.surface_base(session_id, *base_revision)?;
                self.live_surface_target(reference)?;
                parameters.validate()?;
                source.validate().map_err(project_geometry_error)
            }
            Control::Project(ProjectCommand::GetSurfaceImage {
                session_id,
                base_revision,
                image_hash,
            }) => {
                if session_id != &self.session_id || *base_revision != self.project.info().revision
                {
                    return Err(project_error(
                        ProjectErrorCode::StaleRevision,
                        "session or project revision is stale",
                    ));
                }
                if !self.project.surface_images().contains_key(image_hash) {
                    return Err(project_error(
                        ProjectErrorCode::InvalidProject,
                        "project has no referenced surface image",
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(super) fn live_surface_target(&self, reference: &FaceRef) -> Result<(), ProjectError> {
        if reference.session_id != self.session_id || reference.scene_revision != self.revision {
            return Err(project_error(
                ProjectErrorCode::StaleRevision,
                "surface target session or scene revision is stale",
            ));
        }
        let snapshot = self.project.snapshot();
        let valid = snapshot
            .occurrences
            .get(&reference.occurrence_id)
            .is_some_and(|occurrence| occurrence.definition_id == reference.definition_id)
            && snapshot.sources.contains_key(&reference.definition_id)
            && self
                .definitions
                .get(&reference.definition_id)
                .is_some_and(|definition| {
                    definition
                        .faces
                        .iter()
                        .any(|face| face.face_id == reference.face_id)
                });
        if !valid {
            return Err(project_error(
                ProjectErrorCode::InvalidProject,
                "surface target is not a live source occurrence and native face",
            ));
        }
        Ok(())
    }

    pub(super) fn import_surface_image(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        reference: FaceRef,
        scope: SurfaceScopeKind,
        source: NativePath,
        parameters: ImageReliefParameters,
    ) -> Result<Reply, ProjectError> {
        self.surface_base(session, base)?;
        self.live_surface_target(&reference)?;
        parameters.validate()?;
        source.validate().map_err(project_geometry_error)?;
        if self.active.is_some() {
            return Err(project_error(
                ProjectErrorCode::Busy,
                "an engine job is active",
            ));
        }
        let canonical_budget = spiling_contracts::surface::MAX_SURFACE_IMAGE_RETAINED_BYTES
            .checked_sub(self.surface_image_bytes()?)
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::ResourceLimit,
                    "retained surface image budget exceeded",
                )
            })?;
        self.start(
            Purpose::ImportSurfaceImage {
                reference,
                scope,
                parameters,
            },
            0,
            move |_, _, _| Work::ImportSurfaceImage {
                source,
                canonical_budget,
            },
        )
        .map_err(project_geometry_error)
    }

    pub(super) fn prepare_surface_image(
        &self,
        reference: &FaceRef,
        scope: SurfaceScopeKind,
        parameters: &ImageReliefParameters,
        asset: Arc<SurfaceImageAsset>,
    ) -> Result<Publication, JobError> {
        self.live_surface_target(reference)?;
        let image_hash = asset.record().image_hash.clone();
        let scope = match scope {
            SurfaceScopeKind::Definition => SurfaceScope::Definition {
                definition_id: reference.definition_id.clone(),
            },
            SurfaceScopeKind::Occurrence => SurfaceScope::Occurrence {
                occurrence_id: reference.occurrence_id,
                expected_definition_id: reference.definition_id.clone(),
            },
        };
        let edit = self.project.prepare(Edit::AddImageSurfaceFinish {
            asset,
            scope,
            face_id: reference.face_id.clone(),
            parameters: parameters.clone(),
        })?;
        // Core can reuse an asset retained in history. The staged snapshot, not
        // the incoming filename/provenance, owns the first-admission receipt.
        let image = edit
            .surface_images()
            .get(&image_hash)
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::InvalidProject,
                    "prepared image asset missing",
                )
            })?
            .record()
            .clone();
        let layer_id = edit
            .surface_overlays()
            .layers
            .last()
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::InvalidProject,
                    "prepared image layer missing",
                )
            })?
            .layer_id;
        Ok(Publication {
            kind: PublicationKind::SurfaceImageImported {
                edit,
                image,
                layer_id,
            },
            artifact: None,
        })
    }

    pub(super) fn get_surface_image(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        hash: &SourceHash,
    ) -> Result<Reply, ProjectError> {
        if session != &self.session_id || base != self.project.info().revision {
            return Err(project_error(
                ProjectErrorCode::StaleRevision,
                "session or project revision is stale",
            ));
        }
        if self.active.is_some() {
            return Err(project_error(
                ProjectErrorCode::Busy,
                "an engine job is active",
            ));
        }
        let asset = self
            .project
            .surface_images()
            .get(hash)
            .cloned()
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::InvalidProject,
                    "project has no referenced surface image",
                )
            })?;
        self.start(Purpose::GetSurfaceImage, 0, move |_, _, _| {
            Work::GetSurfaceImage { asset }
        })
        .map_err(project_geometry_error)
    }

    pub fn image_publication(&self, id: JobId) -> Option<&Arc<SurfaceImageAsset>> {
        self.image_publication
            .as_ref()
            .filter(|(job, _)| *job == id)
            .map(|(_, asset)| asset)
    }

    pub fn release_image_publication(&mut self, id: JobId) {
        if self
            .image_publication
            .as_ref()
            .is_some_and(|(job, _)| *job == id)
        {
            self.image_publication = None;
        }
    }

    pub(super) fn surface_image_bytes(&self) -> Result<usize, ProjectError> {
        self.project.retained_surface_image_bytes()
    }
}
