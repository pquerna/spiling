// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A failed source has an isolated display, never an adopted part or FaceRef.
use crate::scene::{Chunk, Definition, Scene};
use spiling_contracts::{display::validate_mesh_manifest, geometry::*, project::*};
use spiling_engine_client::{ClientError, EngineRpc};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone)]
pub struct Failure {
    pub operation: String,
    pub summary: SceneSummary,
    pub project: ProjectId,
    pub project_revision: ProjectRevision,
    pub error: GeometryError,
    pub preview: Option<Arc<Scene>>,
    pub preview_error: Option<String>,
}
fn invalid(message: &str) -> ClientError {
    ClientError::Protocol(message.into())
}
pub async fn load(
    rpc: EngineRpc,
    failure: Arc<Failure>,
    cancel: Arc<AtomicBool>,
    active_bytes: usize,
) -> Result<(String, Arc<Scene>), ClientError> {
    let diagnostic = failure
        .error
        .import_diagnostic
        .as_ref()
        .ok_or_else(|| invalid("Missing import diagnostic"))?;
    diagnostic.validate().map_err(|e| invalid(&e.to_string()))?;
    let artifact_id = diagnostic
        .preview
        .ok_or_else(|| invalid("Source preview is unavailable"))?;
    let definition_id = diagnostic
        .preview_definition
        .clone()
        .ok_or_else(|| invalid("Missing preview identity"))?;
    let hash = diagnostic
        .source_hash
        .clone()
        .ok_or_else(|| invalid("Missing source identity"))?;
    let provenance = SourceProvenance {
        source_hash: hash,
        source_name: diagnostic.source_name.clone(),
        source_unit: SourceUnit::Millimetre,
        uncertainty_mm: None,
        source_solid: None,
        normalized: None,
    };
    if provenance.definition_id() != definition_id {
        return Err(invalid("Preview does not bind original source"));
    }
    let mut offset = 0;
    let mut chunks = Vec::new();
    let mut total = None;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(invalid("Source preview cancelled"));
        }
        let GeometryResponse::ArtifactPage {
            summary,
            chunks: metadata,
            resources,
            next_offset,
            ..
        } = rpc
            .geometry(GeometryCommand::GetArtifactPage {
                session_id: failure.summary.session_id.clone(),
                artifact_id,
                kind: ArtifactPageKind::Chunks,
                offset,
            })
            .await?
        else {
            return Err(invalid("Import preview expired"));
        };
        let ArtifactSummary::Mesh {
            artifact_id: id,
            definition_id: definition,
            chunk_count,
            total_bytes,
        } = summary
        else {
            return Err(invalid("Import preview is not a mesh"));
        };
        if id != artifact_id
            || definition != definition_id
            || metadata.len() != resources.len()
            || total.is_some_and(|old| old != (chunk_count, total_bytes))
            || (active_bytes.saturating_add(total_bytes as usize))
                .saturating_mul(2)
                .saturating_add(crate::scene::CONTEXT_DISPLAY_RESERVE_BYTES)
                > MAX_DISPLAY_BUFFER_BYTES as usize
        {
            return Err(invalid("Import preview identity or working set changed"));
        }
        total = Some((chunk_count, total_bytes));
        for (metadata, resource) in metadata.into_iter().zip(resources) {
            if cancel.load(Ordering::Acquire) {
                return Err(invalid("Source preview cancelled"));
            }
            let ArtifactChunkMetadata::Mesh { metadata: mesh } = metadata else {
                return Err(invalid("Preview contains a non-mesh chunk"));
            };
            if mesh.session_id != failure.summary.session_id
                || mesh.artifact_id != artifact_id
                || mesh.definition_id != definition_id
            {
                return Err(invalid("Import preview chunk binding changed"));
            }
            let checked = rpc
                .read_checked_mesh_chunk(
                    &resource,
                    &ArtifactChunkMetadata::Mesh {
                        metadata: mesh.clone(),
                    },
                    &ArtifactSummary::Mesh {
                        artifact_id,
                        definition_id: definition_id.clone(),
                        chunk_count,
                        total_bytes,
                    },
                )
                .await?;
            chunks.push(Chunk::from_checked(checked));
            if chunks.len() > chunk_count as usize {
                return Err(invalid("Too many preview chunks"));
            }
        }
        match next_offset {
            Some(next) if next > offset => offset = next,
            Some(_) => return Err(invalid("Preview page did not advance")),
            None => break,
        }
    }
    let (count, bytes) = total.ok_or_else(|| invalid("Missing preview manifest"))?;
    if chunks.len() != count as usize
        || chunks
            .iter()
            .map(|c| c.metadata.byte_count as u64)
            .sum::<u64>()
            != bytes as u64
    {
        return Err(invalid("Incomplete import preview"));
    }
    validate_mesh_manifest(
        &chunks
            .iter()
            .map(|c| c.metadata.clone())
            .collect::<Vec<_>>(),
    )
    .map_err(|e| invalid(&e.to_string()))?;
    let mut bounds = chunks
        .first()
        .ok_or_else(|| invalid("Empty preview"))?
        .metadata
        .bounds_mm;
    for chunk in &chunks {
        for a in 0..3 {
            bounds.min[a] = bounds.min[a].min(chunk.metadata.bounds_mm.min[a]);
            bounds.max[a] = bounds.max[a].max(chunk.metadata.bounds_mm.max[a]);
        }
    }
    let record = DefinitionRecord {
        definition_id: definition_id.clone(),
        provenance,
        face_count: 1,
        bounds_mm: bounds,
        display: DefinitionDisplay::Mesh { artifact_id },
    };
    record.validate().map_err(|e| invalid(&e.to_string()))?;
    let mut summary = failure.summary.clone();
    summary.definition_count = 1;
    summary.occurrence_count = 1;
    summary.bounds_mm = Some(bounds);
    summary.unique_mesh_bytes = bytes;
    let scene = Scene {
        kind: crate::scene::SceneKind::UnresolvedImport,
        summary,
        definitions: vec![Arc::new(Definition {
            record,
            faces: Vec::new(),
            chunks,
        })],
        occurrences: vec![OccurrenceRecord {
            occurrence_id: OccurrenceId::new(1).map_err(|e| invalid(&e.to_string()))?,
            definition_id,
            pose: RigidPoseMm::IDENTITY,
        }],
        context: Arc::new(PrintLayoutOverview {
            session_id: failure.summary.session_id.clone(),
            scene_revision: failure.summary.revision,
            project_id: failure.project.clone(),
            project_revision: failure.project_revision,
            layouts: Vec::new(),
            definitions: Vec::new(),
        }),
        context_definitions: Vec::new(),
        context_error: None,
        raw_bytes: bytes as usize,
    };
    Ok((failure.operation.clone(), Arc::new(scene)))
}
