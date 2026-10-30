// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Immutable, revision-bound display data. No kernel, writer or renderer authority.
use spiling_contracts::{display::validate_mesh_manifest, geometry::*};
use spiling_engine_client::{ClientError, EngineRpc};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug)]
pub struct Chunk {
    pub metadata: MeshChunkMetadata,
    pub bytes: Arc<[u8]>,
}
#[derive(Debug)]
pub struct Definition {
    pub record: DefinitionRecord,
    pub faces: Vec<FaceIndexRow>,
    pub chunks: Vec<Chunk>,
}
#[derive(Debug)]
pub struct Scene {
    pub summary: SceneSummary,
    pub definitions: Vec<Arc<Definition>>,
    pub occurrences: Vec<OccurrenceRecord>,
    pub raw_bytes: usize,
}
impl Scene {
    pub fn definition(&self, id: &DefinitionId) -> Option<&Arc<Definition>> {
        self.definitions
            .iter()
            .find(|d| &d.record.definition_id == id)
    }
    pub fn reference(&self, occurrence: &OccurrenceRecord, ordinal: usize) -> Option<FaceRef> {
        let definition = self.definition(&occurrence.definition_id)?;
        Some(FaceRef {
            session_id: self.summary.session_id.clone(),
            scene_revision: self.summary.revision,
            occurrence_id: occurrence.occurrence_id,
            definition_id: occurrence.definition_id.clone(),
            face_id: definition.faces.get(ordinal)?.face_id.clone(),
        })
    }
    pub fn same_revision(&self, summary: &SceneSummary) -> bool {
        self.summary.session_id == summary.session_id && self.summary.revision == summary.revision
    }
}
fn invalid(message: &str) -> ClientError {
    ClientError::Protocol(message.into())
}
fn check(cancel: &AtomicBool) -> Result<(), ClientError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ClientError::InvalidRequest(
            "Display transfer paused; previous display retained".into(),
        ))
    } else {
        Ok(())
    }
}

/// Fetch one checked chunk at a time. Reuse immutable definitions on pose-only revisions.
/// Capacity includes active/staged raw bytes and their GPU copies, plus instance matrices.
pub async fn load(
    rpc: EngineRpc,
    summary: SceneSummary,
    previous: Option<Arc<Scene>>,
    cancel: Arc<AtomicBool>,
    reserved: usize,
) -> Result<Arc<Scene>, ClientError> {
    let mut definitions = Vec::new();
    let mut occurrences = Vec::new();
    for kind in [ScenePageKind::Definitions, ScenePageKind::Occurrences] {
        let mut offset = 0;
        loop {
            check(&cancel)?;
            let response = rpc
                .geometry(GeometryCommand::GetScenePage {
                    session_id: summary.session_id.clone(),
                    revision: summary.revision,
                    kind,
                    offset,
                })
                .await?;
            let GeometryResponse::ScenePage {
                definitions: ds,
                occurrences: os,
                next_offset,
            } = response
            else {
                return Err(invalid(
                    "Native scene page refused or returned an unexpected reply",
                ));
            };
            definitions.extend(ds);
            occurrences.extend(os);
            if definitions.len() > summary.definition_count as usize
                || occurrences.len() > summary.occurrence_count as usize
            {
                return Err(invalid("Scene page exceeds captured counts"));
            }
            match next_offset {
                Some(next) if next > offset => offset = next,
                Some(_) => return Err(invalid("Scene page did not advance")),
                None => break,
            }
        }
    }
    if definitions.len() != summary.definition_count as usize
        || occurrences.len() != summary.occurrence_count as usize
    {
        return Err(invalid("Incomplete native scene pages"));
    }
    let mut loaded = Vec::new();
    let mut raw_bytes = 0usize;
    let active_bytes = previous.as_ref().map_or(0, |s| s.raw_bytes);
    let mut newly_retained = 0usize;
    for record in definitions {
        check(&cancel)?;
        if let Some(existing) = previous
            .as_ref()
            .filter(|s| s.summary.session_id == summary.session_id)
            .and_then(|s| s.definition(&record.definition_id))
            .filter(|d| d.record == record)
        {
            raw_bytes += existing.chunks.iter().map(|c| c.bytes.len()).sum::<usize>();
            loaded.push(existing.clone());
            continue;
        }
        let mut faces = Vec::new();
        let mut offset = 0;
        loop {
            check(&cancel)?;
            let GeometryResponse::FaceIndexPage {
                faces: page,
                next_offset,
            } = rpc
                .geometry(GeometryCommand::GetFaceIndexPage {
                    session_id: summary.session_id.clone(),
                    definition_id: record.definition_id.clone(),
                    offset,
                })
                .await?
            else {
                return Err(invalid("Native face page refused"));
            };
            faces.extend(page);
            if faces.len() > record.face_count as usize {
                return Err(invalid("Face table exceeds native count"));
            }
            match next_offset {
                Some(next) if next > offset => offset = next,
                Some(_) => return Err(invalid("Face page did not advance")),
                None => break,
            }
        }
        if faces.len() != record.face_count as usize
            || faces
                .iter()
                .enumerate()
                .any(|(i, f)| i != f.ordinal as usize)
        {
            return Err(invalid("Incomplete native face table"));
        }
        let mut chunks = Vec::new();
        if let DefinitionDisplay::Mesh { artifact_id } = record.display {
            let mut offset = 0;
            loop {
                check(&cancel)?;
                let GeometryResponse::ArtifactPage {
                    summary: artifact,
                    chunks: metadata,
                    resources,
                    next_offset,
                    ..
                } = rpc
                    .geometry(GeometryCommand::GetArtifactPage {
                        session_id: summary.session_id.clone(),
                        artifact_id,
                        kind: ArtifactPageKind::Chunks,
                        offset,
                    })
                    .await?
                else {
                    return Err(invalid("Native mesh page refused"));
                };
                if metadata.len() != resources.len() {
                    return Err(invalid("Mesh descriptor count mismatch"));
                }
                for (metadata, resource) in metadata.into_iter().zip(resources) {
                    check(&cancel)?;
                    let ArtifactChunkMetadata::Mesh { metadata: mesh } = &metadata else {
                        return Err(invalid("Mesh page contained another artifact kind"));
                    };
                    let next = newly_retained
                        .checked_add(mesh.byte_count as usize)
                        .ok_or_else(|| invalid("Display capacity overflow"))?;
                    if (active_bytes + next).saturating_mul(2) + occurrences.len() * 128 + reserved
                        > MAX_DISPLAY_BUFFER_BYTES as usize
                    {
                        return Err(invalid(
                            "Active and staged CPU/GPU display buffers exceed capacity; close the project before admitting this scene",
                        ));
                    }
                    let bytes = rpc
                        .read_geometry_chunk(&resource, &metadata, &artifact)
                        .await?;
                    newly_retained = next;
                    raw_bytes += bytes.len();
                    chunks.push(Chunk {
                        metadata: mesh.clone(),
                        bytes: bytes.into(),
                    });
                }
                if chunks.len() > MAX_SCENE_MESH_BYTES as usize / 64 {
                    return Err(invalid("Too many display chunks"));
                }
                match next_offset {
                    Some(next) if next > offset => offset = next,
                    Some(_) => return Err(invalid("Mesh page did not advance")),
                    None => break,
                }
            }
            validate_mesh_manifest(
                &chunks
                    .iter()
                    .map(|c| c.metadata.clone())
                    .collect::<Vec<_>>(),
            )?;
        }
        loaded.push(Arc::new(Definition {
            record,
            faces,
            chunks,
        }));
    }
    if raw_bytes != summary.unique_mesh_bytes as usize {
        return Err(invalid(
            "Scene mesh byte total disagrees with native summary",
        ));
    }
    for occurrence in &occurrences {
        if !loaded
            .iter()
            .any(|d| d.record.definition_id == occurrence.definition_id)
        {
            return Err(invalid("Occurrence references an absent definition"));
        }
    }
    let center = summary.bounds_mm.map_or(glam::DVec3::ZERO, |b| {
        (glam::DVec3::from_array(b.min) + glam::DVec3::from_array(b.max)) * 0.5
    });
    let ids = loaded
        .iter()
        .map(|d| &d.record.definition_id)
        .collect::<std::collections::HashSet<_>>();
    if ids.len() != loaded.len()
        || occurrences
            .iter()
            .map(|o| o.occurrence_id)
            .collect::<std::collections::HashSet<_>>()
            .len()
            != occurrences.len()
    {
        return Err(invalid("Duplicate native scene identities"));
    }
    for definition in &loaded {
        if definition
            .faces
            .iter()
            .map(|f| &f.face_id)
            .collect::<std::collections::HashSet<_>>()
            .len()
            != definition.faces.len()
        {
            return Err(invalid("Duplicate source-face identities"));
        }
        if !occurrences
            .iter()
            .any(|o| o.definition_id == definition.record.definition_id)
        {
            return Err(invalid("Unreferenced live definition"));
        }
        for chunk in &definition.chunks {
            let view = spiling_contracts::display::validate_mesh_chunk(
                &chunk.bytes,
                &chunk.metadata,
                &summary.session_id,
                chunk.metadata.artifact_id,
                &definition.record.definition_id,
                chunk.metadata.chunk_index,
            )?;
            for occurrence in occurrences
                .iter()
                .filter(|o| o.definition_id == definition.record.definition_id)
            {
                let q = glam::DQuat::from_array(occurrence.pose.rotation_xyzw);
                let translation = q * glam::DVec3::from_array(view.local_origin_mm)
                    + glam::DVec3::from_array(occurrence.pose.translation_mm)
                    - center;
                let translated = translation.as_vec3().as_dvec3();
                let rounded_q = q.as_quat().as_dquat();
                for position in view.positions.iter() {
                    let p = glam::DVec3::from_array(position.map(f64::from));
                    if ((rounded_q * p + translated) - (q * p + translation)).length() > 0.001 {
                        return Err(invalid(
                            "Source display projection exceeds 0.001 mm; previous display retained",
                        ));
                    }
                }
            }
        }
    }
    let current = rpc
        .geometry(GeometryCommand::GetScene {
            session_id: summary.session_id.clone(),
        })
        .await?;
    if !matches!(current,GeometryResponse::Scene { summary: ref s } if s==&summary) {
        return Err(invalid("Discarded stale display transfer"));
    }
    check(&cancel)?;
    Ok(Arc::new(Scene {
        summary,
        definitions: loaded,
        occurrences,
        raw_bytes,
    }))
}

#[derive(Debug)]
pub struct SectionChunk {
    pub metadata: SectionChunkMetadata,
    pub bytes: Arc<[u8]>,
}
#[derive(Debug)]
pub struct Section {
    pub summary: SectionSummary,
    pub loops: Vec<SectionLoopMetadata>,
    pub chunks: Vec<SectionChunk>,
}
/// Ephemeral native sections are released after checked transfer, including failure/cancel.
pub async fn load_section(
    rpc: EngineRpc,
    scene: SceneSummary,
    id: ArtifactId,
    cancel: Arc<AtomicBool>,
    reserved: usize,
) -> Result<Arc<Section>, ClientError> {
    let result = async {
        let mut chunks = Vec::new();
        let mut loops = Vec::new();
        let mut captured = None;
        for kind in [ArtifactPageKind::Chunks, ArtifactPageKind::Loops] {
            let mut offset = 0;
            loop {
                check(&cancel)?;
                let GeometryResponse::ArtifactPage {
                    summary: ArtifactSummary::Section { summary },
                    chunks: metadata,
                    loops: page,
                    resources,
                    next_offset,
                } = rpc
                    .geometry(GeometryCommand::GetArtifactPage {
                        session_id: scene.session_id.clone(),
                        artifact_id: id,
                        kind,
                        offset,
                    })
                    .await?
                else {
                    return Err(invalid("Section page refused"));
                };
                if summary.session_id != scene.session_id
                    || summary.revision != scene.revision
                    || summary.artifact_id != id
                    || captured.as_ref().is_some_and(|s| s != &summary)
                {
                    return Err(invalid("Stale or inconsistent section manifest"));
                }
                if reserved
                    .checked_add(summary.total_bytes as usize * 6)
                    .is_none_or(|n| n > MAX_DISPLAY_BUFFER_BYTES as usize)
                {
                    return Err(ClientError::InvalidRequest(
                        "Section exceeds the combined active/staged display budget".into(),
                    ));
                }
                captured = Some(summary.clone());
                loops.extend(page);
                if loops.len() > summary.total_loop_count as usize
                    || metadata.len() != resources.len()
                {
                    return Err(invalid("Section page exceeds manifest"));
                }
                for (metadata, resource) in metadata.into_iter().zip(resources) {
                    check(&cancel)?;
                    let ArtifactChunkMetadata::Section { metadata: section } = &metadata else {
                        return Err(invalid("Wrong section chunk kind"));
                    };
                    let bytes = rpc
                        .read_geometry_chunk(
                            &resource,
                            &metadata,
                            &ArtifactSummary::Section {
                                summary: summary.clone(),
                            },
                        )
                        .await?;
                    chunks.push(SectionChunk {
                        metadata: section.clone(),
                        bytes: bytes.into(),
                    });
                    if chunks.len() > summary.chunk_count as usize {
                        return Err(invalid("Section chunks exceed manifest"));
                    }
                }
                match next_offset {
                    Some(next) if next > offset => offset = next,
                    Some(_) => return Err(invalid("Section page did not advance")),
                    None => break,
                }
            }
        }
        let summary = captured.ok_or_else(|| invalid("Missing section summary"))?;
        spiling_contracts::display::validate_section_manifest(
            &summary,
            &chunks
                .iter()
                .map(|c| c.metadata.clone())
                .collect::<Vec<_>>(),
            &loops,
        )?;
        check(&cancel)?;
        Ok(Arc::new(Section {
            summary,
            chunks,
            loops,
        }))
    }
    .await;
    let release = rpc
        .geometry(GeometryCommand::ReleaseArtifact {
            session_id: scene.session_id,
            artifact_id: id,
        })
        .await;
    match (result, release) {
        (Ok(section), Ok(GeometryResponse::Released {})) => Ok(section),
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error),
        _ => Err(invalid("Section artifact release was not acknowledged")),
    }
}
