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
mod focus;
pub use focus::FocusTarget;

#[derive(Debug)]
pub struct Chunk {
    pub metadata: MeshChunkMetadata,
    pub bytes: Arc<[u8]>,
    checked: spiling_contracts::display::CheckedMeshChunk,
}
impl Chunk {
    pub(crate) fn from_checked(checked: spiling_contracts::display::CheckedMeshChunk) -> Self {
        Self {
            metadata: checked.metadata().clone(),
            bytes: checked.bytes().clone(),
            checked,
        }
    }
    pub(crate) fn view(
        &self,
        session: &SessionId,
        definition: &DefinitionId,
    ) -> Result<spiling_contracts::display::ValidatedMeshChunk<'_>, GeometryError> {
        if &self.metadata != self.checked.metadata()
            || !Arc::ptr_eq(&self.bytes, self.checked.bytes())
            || &self.metadata.session_id != session
            || &self.metadata.definition_id != definition
        {
            return Err(GeometryError::new(
                GeometryErrorCode::UnknownHandle,
                "checked mesh binding changed",
            ));
        }
        Ok(self.checked.view())
    }
    pub(crate) fn radius_mm(&self) -> f64 {
        self.checked.radius_mm()
    }
}
#[derive(Debug)]
pub struct Definition {
    pub record: DefinitionRecord,
    pub faces: Vec<FaceIndexRow>,
    pub chunks: Vec<Chunk>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneKind {
    Source,
    UnresolvedImport,
}
#[derive(Debug)]
pub struct Scene {
    pub kind: SceneKind,
    pub summary: SceneSummary,
    pub definitions: Vec<Arc<Definition>>,
    pub occurrences: Vec<OccurrenceRecord>,
    /// Global source display closure, shared with the active definitions above.
    pub context: Arc<PrintLayoutOverview>,
    pub context_definitions: Vec<Arc<Definition>>,
    pub context_error: Option<String>,
    pub raw_bytes: usize,
}
impl Scene {
    pub fn definition(&self, id: &DefinitionId) -> Option<&Arc<Definition>> {
        self.definitions
            .iter()
            .find(|d| &d.record.definition_id == id)
    }
    pub fn reference(&self, occurrence: &OccurrenceRecord, ordinal: usize) -> Option<FaceRef> {
        if self.kind == SceneKind::UnresolvedImport {
            return None;
        }
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
        self.summary.layout_id == summary.layout_id
            && self.summary.session_id == summary.session_id
            && self.summary.revision == summary.revision
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

/// Hardware primitives, bounded layout previews and
/// active/staged CPU/GPU construction outlines and mechanical fields. Shared raw meshes are charged
/// separately. This pool is reserved beside every source/section/replay load.
pub const CONTEXT_DISPLAY_RESERVE_BYTES: usize = 4 * 1024 * 1024
    + crate::inspection::INSPECTION_RESERVE_BYTES
    + crate::mechanics::DISPLAY_RESERVE_BYTES
    + crate::viewport::fastener::DISPLAY_RESERVE_BYTES;
/// Display-pool charge per placed part on any plate: CPU/GPU instance
/// matrices, selection and participant state. Instance buffers are admitted
/// through this shared byte pool, never a separate instance count ceiling.
pub const INSTANCE_DISPLAY_BYTES: usize = 128;

/// Shared display-pool bytes charged for every part of every plate.
pub fn instance_display_bytes(overview: &PrintLayoutOverview) -> Option<usize> {
    overview
        .layouts
        .iter()
        .try_fold(0usize, |total, plate| total.checked_add(plate.parts.len()))?
        .checked_mul(INSTANCE_DISPLAY_BYTES)
}

/// Fetch one checked chunk at a time. Reuse immutable definitions on pose-only revisions.
/// Capacity includes active/staged raw bytes and their GPU copies, plus instance matrices.
pub async fn load(
    rpc: EngineRpc,
    summary: SceneSummary,
    previous: Option<Arc<Scene>>,
    overview: Arc<PrintLayoutOverview>,
    cancel: Arc<AtomicBool>,
    reserved: usize,
) -> Result<Arc<Scene>, ClientError> {
    let _opening = crate::performance::span("source_transfer");
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
    if overview.session_id != summary.session_id || overview.scene_revision != summary.revision {
        return Err(invalid("Plate display context does not match the scene"));
    }
    let active_ids = definitions
        .iter()
        .map(|d| d.definition_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if definitions
        .iter()
        .any(|d| !overview.definitions.contains(d))
    {
        return Err(invalid("Active and overview definitions disagree"));
    }
    let plate = overview
        .layouts
        .iter()
        .find(|l| l.layout_id == summary.layout_id)
        .ok_or_else(|| invalid("Active plate absent from overview"))?;
    if plate.parts.len() != occurrences.len()
        || occurrences.iter().any(|o| {
            !plate.parts.iter().any(|p| {
                p.occurrence_id == o.occurrence_id
                    && p.definition_id == o.definition_id
                    && p.pose == o.pose
            })
        })
    {
        return Err(invalid("Active and overview occurrences disagree"));
    }
    definitions.extend(
        overview
            .definitions
            .iter()
            .filter(|d| !active_ids.contains(&d.definition_id))
            .cloned(),
    );
    let mut context_error = None;
    let mut loaded = Vec::new();
    let active_bytes = previous.as_ref().map_or(0, |s| s.raw_bytes);
    let instance_bytes =
        instance_display_bytes(&overview).ok_or_else(|| invalid("Display instance overflow"))?;
    let mut newly_retained = 0usize;
    'definitions: for record in definitions {
        check(&cancel)?;
        if let Some(existing) = previous
            .as_ref()
            .filter(|s| s.summary.session_id == summary.session_id)
            .and_then(|s| {
                s.context_definitions
                    .iter()
                    .find(|d| d.record.definition_id == record.definition_id)
            })
            .filter(|d| d.record == record)
        {
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
                    if (active_bytes + next).saturating_mul(2) + instance_bytes + reserved
                        > MAX_DISPLAY_BUFFER_BYTES as usize
                    {
                        if active_ids.contains(&record.definition_id) {
                            return Err(invalid(
                                "Active and staged CPU/GPU display buffers exceed capacity; close the project before admitting this scene",
                            ));
                        }
                        context_error = Some("Additional plate previews exceed the shared display budget. Focus a plate to inspect it.".into());
                        break 'definitions;
                    }
                    let checked = rpc
                        .read_checked_mesh_chunk(&resource, &metadata, &artifact)
                        .await?;
                    newly_retained = next;
                    chunks.push(Chunk::from_checked(checked));
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
    // The active view and every card share these exact immutable allocations.
    let context_definitions = loaded;
    let raw_bytes = context_definitions
        .iter()
        .flat_map(|d| &d.chunks)
        .map(|c| c.bytes.len())
        .sum();
    let loaded: Vec<_> = context_definitions
        .iter()
        .filter(|d| active_ids.contains(&d.record.definition_id))
        .cloned()
        .collect();
    let active_raw: usize = loaded
        .iter()
        .flat_map(|d| &d.chunks)
        .map(|c| c.bytes.len())
        .sum();
    if active_raw != summary.unique_mesh_bytes as usize {
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
            let view = chunk.view(&summary.session_id, &definition.record.definition_id)?;
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
                let radius = chunk.radius_mm();
                let rotation_error = [glam::DVec3::X, glam::DVec3::Y, glam::DVec3::Z]
                    .iter()
                    .map(|v| ((rounded_q * *v) - (q * *v)).length_squared())
                    .sum::<f64>()
                    .sqrt()
                    * radius;
                if rotation_error + (translated - translation).length() > 0.001 {
                    return Err(invalid(
                        "Source display projection exceeds 0.001 mm; previous display retained",
                    ));
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
        kind: SceneKind::Source,
        summary,
        definitions: loaded,
        occurrences,
        raw_bytes,
        context: overview,
        context_definitions,
        context_error,
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
/// Accounts for raw chunks, active/staged lines, GPU copies and bounded 2D contour inspection.
pub(crate) const SECTION_DISPLAY_MULTIPLIER: usize = 10;

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
                    .checked_add(summary.total_bytes as usize * SECTION_DISPLAY_MULTIPLIER)
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
