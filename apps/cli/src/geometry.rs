// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{Error, output::OutputDirectory};
use serde::{Deserialize, Serialize};
use spiling_contracts::{
    ArtifactView, NativeOperationView,
    display::{
        validate_mesh_chunk, validate_mesh_manifest, validate_section_chunk,
        validate_section_loop_metadata, validate_section_manifest,
    },
    geometry::*,
};
use spiling_engine_client::EngineClient;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    schema_version: u32,
    parts: Vec<RecipePart>,
    instances: Vec<RecipeInstance>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipePart {
    key: String,
    path: PathBuf,
    pose: RigidPoseMm,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeInstance {
    key: String,
    part: String,
    pose: RigidPoseMm,
}
pub struct Part {
    key: String,
    path: PathBuf,
    source: NativePath,
    pose: RigidPoseMm,
}
pub struct SceneInput {
    parts: Vec<Part>,
    instances: Vec<RecipeInstance>,
}
const MAX_RECIPE_BYTES: u64 = 1024 * 1024;

impl SceneInput {
    pub fn load(recipe: Option<&Path>, sources: &[PathBuf]) -> Result<Self, Error> {
        if recipe.is_some() == !sources.is_empty() {
            return Err("geometry requires either --scene or one or more --source paths".into());
        }
        let (parts, instances) = if let Some(path) = recipe {
            let mut bytes = Vec::new();
            File::open(path)?
                .take(MAX_RECIPE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_RECIPE_BYTES as usize {
                return Err("scene recipe exceeds 1 MiB".into());
            }
            let recipe: Recipe = serde_json::from_slice(&bytes)?;
            if recipe.schema_version != 1 {
                return Err("unsupported scene recipe schema_version (expected 1)".into());
            }
            let root = path.parent().unwrap_or(Path::new("."));
            let parts = recipe
                .parts
                .into_iter()
                .map(|part| {
                    let path = root.join(part.path);
                    Ok(Part {
                        source: NativePath::from_os_str(path.as_os_str())?,
                        key: part.key,
                        path,
                        pose: part.pose,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            (parts, recipe.instances)
        } else {
            let parts = sources
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    Ok(Part {
                        key: format!("source-{}", index + 1),
                        path: path.clone(),
                        source: NativePath::from_os_str(path.as_os_str())?,
                        pose: RigidPoseMm::IDENTITY,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            (parts, Vec::new())
        };
        let result = Self { parts, instances };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), Error> {
        if self.parts.is_empty()
            || self.parts.len() > MAX_DEFINITIONS as usize
            || self.parts.len() + self.instances.len() > MAX_OCCURRENCES as usize
        {
            return Err("recipe exceeds part/occurrence limits or has no parts".into());
        }
        let mut keys = BTreeSet::new();
        for part in &self.parts {
            validate_key(&part.key)?;
            if !keys.insert(part.key.as_str()) {
                return Err("duplicate recipe key".into());
            }
            part.pose.validate()?;
            part.source.validate()?;
        }
        let parts = keys.clone();
        for instance in &self.instances {
            validate_key(&instance.key)?;
            if !keys.insert(instance.key.as_str()) {
                return Err("duplicate recipe key".into());
            }
            if !parts.contains(instance.part.as_str()) {
                return Err("instance references an unknown part key".into());
            }
            instance.pose.validate()?;
        }
        Ok(())
    }
}
fn validate_key(key: &str) -> Result<(), Error> {
    if key.is_empty() || key.len() > MAX_DISPLAY_LABEL_BYTES as usize {
        return Err("recipe keys require 1..256 UTF-8 bytes".into());
    }
    Ok(())
}
pub fn parse_plane(text: &str) -> Result<PlaneMm, Error> {
    let (origin, normal) = text
        .split_once(':')
        .ok_or("section requires ox,oy,oz:nx,ny,nz")?;
    fn triple(text: &str) -> Result<[f64; 3], Error> {
        let values = text
            .split(',')
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()?;
        values
            .try_into()
            .map_err(|_| "section requires two triples".into())
    }
    let plane = PlaneMm {
        origin_mm: triple(origin)?,
        normal: triple(normal)?,
    };
    plane.validate()?;
    Ok(plane)
}
pub(crate) async fn control(
    client: &mut EngineClient,
    command: GeometryCommand,
) -> Result<GeometryResponse, Error> {
    match client.geometry(command).await? {
        GeometryResponse::Error { error } => {
            Err(spiling_engine_client::ClientError::Geometry(error).into())
        }
        response => Ok(response),
    }
}
fn unexpected() -> Error {
    "unexpected geometry response".into()
}
pub(crate) async fn scene(
    client: &mut EngineClient,
    session: &SessionId,
) -> Result<SceneSummary, Error> {
    match control(
        client,
        GeometryCommand::GetScene {
            session_id: session.clone(),
        },
    )
    .await?
    {
        GeometryResponse::Scene { summary } => {
            summary.validate()?;
            if &summary.session_id != session {
                return Err("scene session mismatch".into());
            }
            Ok(summary)
        }
        _ => Err(unexpected()),
    }
}
pub(crate) async fn wait_operation(
    client: &mut EngineClient,
    session: &SessionId,
    mut operation: NativeOperationView,
) -> Result<JobResult, Error> {
    let name = operation.name.clone();
    loop {
        if operation.name != name || &operation.session_id != session {
            return Err("native operation name/session mismatch".into());
        }
        match operation.status {
            JobStatus::Completed => return operation.result.ok_or_else(unexpected),
            JobStatus::Failed | JobStatus::Cancelled | JobStatus::Interrupted => {
                return Err(operation
                    .error
                    .map(|error| match error {
                        JobError::Geometry { error } => {
                            Error::from(spiling_engine_client::ClientError::Geometry(error))
                        }
                        JobError::Project { error } => {
                            Error::from(spiling_engine_client::ClientError::Project(error))
                        }
                        JobError::Manufacturing { error } => {
                            Error::from(spiling_engine_client::ClientError::Manufacturing(error))
                        }
                    })
                    .unwrap_or_else(|| "native operation ended without result".into()));
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(JOB_POLL_INTERVAL_MS.into())).await;
        operation = client.native_operation(name.clone()).await?;
    }
}
fn next_page(
    offset: u32,
    count: usize,
    next: Option<u32>,
    limit: u32,
) -> Result<Option<u32>, Error> {
    let end = offset
        .checked_add(u32::try_from(count)?)
        .ok_or("page count overflow")?;
    if end > limit || next.is_some_and(|next| count == 0 || next != end || next >= limit) {
        return Err("invalid page range or nonprogressing pagination".into());
    }
    Ok(next)
}
pub(crate) async fn records(
    client: &mut EngineClient,
    summary: &SceneSummary,
) -> Result<(Vec<DefinitionRecord>, Vec<OccurrenceRecord>), Error> {
    let mut definitions = Vec::new();
    let mut occurrences = Vec::new();
    for kind in [ScenePageKind::Definitions, ScenePageKind::Occurrences] {
        let mut offset = 0;
        loop {
            let GeometryResponse::ScenePage {
                definitions: defs,
                occurrences: occs,
                next_offset,
            } = control(
                client,
                GeometryCommand::GetScenePage {
                    session_id: summary.session_id.clone(),
                    revision: summary.revision,
                    kind,
                    offset,
                },
            )
            .await?
            else {
                return Err(unexpected());
            };
            if defs.len() + occs.len() > SCENE_PAGE_SIZE as usize {
                return Err("oversized scene page".into());
            }
            let (count, limit) = match kind {
                ScenePageKind::Definitions if occs.is_empty() => {
                    (defs.len(), summary.definition_count)
                }
                ScenePageKind::Occurrences if defs.is_empty() => {
                    (occs.len(), summary.occurrence_count)
                }
                _ => return Err("scene page kind mismatch".into()),
            };
            let next = next_page(offset, count, next_offset, limit)?;
            definitions.extend(defs);
            occurrences.extend(occs);
            if let Some(next) = next {
                offset = next;
            } else {
                break;
            }
        }
    }
    if definitions.len() != summary.definition_count as usize
        || occurrences.len() != summary.occurrence_count as usize
    {
        return Err("scene page totals mismatch".into());
    }
    let mut definition_ids = BTreeSet::new();
    let mut artifacts = BTreeSet::new();
    let mut face_count = 0_u32;
    for definition in &definitions {
        definition.validate()?;
        face_count = face_count
            .checked_add(definition.face_count)
            .ok_or("face count overflow")?;
        if !definition_ids.insert(definition.definition_id.clone())
            || !artifacts.insert(definition.mesh_artifact_id)
        {
            return Err("duplicate definition or artifact".into());
        }
    }
    if face_count > MAX_NATIVE_FACES {
        return Err("scene exceeds unique face cap".into());
    }
    let mut occurrence_ids = BTreeSet::new();
    let mut live = BTreeSet::new();
    for occurrence in &occurrences {
        occurrence.pose.validate()?;
        if !definition_ids.contains(&occurrence.definition_id)
            || !occurrence_ids.insert(occurrence.occurrence_id)
        {
            return Err("invalid scene occurrence reference".into());
        }
        live.insert(occurrence.definition_id.clone());
    }
    if live != definition_ids {
        return Err("unreferenced scene definition".into());
    }
    Ok((definitions, occurrences))
}
async fn faces(
    client: &mut EngineClient,
    session: &SessionId,
    definition: &DefinitionRecord,
) -> Result<Vec<FaceIndexRow>, Error> {
    let mut faces = Vec::new();
    let mut ids = BTreeSet::new();
    let mut offset = 0;
    loop {
        let GeometryResponse::FaceIndexPage {
            faces: rows,
            next_offset,
        } = control(
            client,
            GeometryCommand::GetFaceIndexPage {
                session_id: session.clone(),
                definition_id: definition.definition_id.clone(),
                offset,
            },
        )
        .await?
        else {
            return Err(unexpected());
        };
        if rows.len() > FACE_PAGE_SIZE as usize {
            return Err("oversized face page".into());
        }
        let next = next_page(offset, rows.len(), next_offset, definition.face_count)?;
        for row in rows {
            if row.ordinal as usize != faces.len() || !ids.insert(row.face_id.clone()) {
                return Err("invalid face index order or duplicate source face".into());
            }
            faces.push(row);
        }
        if let Some(next) = next {
            offset = next;
        } else {
            break;
        }
    }
    if faces.len() != definition.face_count as usize {
        return Err("missing source face rows".into());
    }
    Ok(faces)
}
async fn artifact_pages(
    client: &mut EngineClient,
    session: &SessionId,
    artifact: ArtifactId,
    kind: ArtifactPageKind,
    limit: u32,
) -> Result<
    (
        ArtifactSummary,
        Vec<ArtifactChunkMetadata>,
        Vec<SectionLoopMetadata>,
        Vec<ArtifactView>,
    ),
    Error,
> {
    let mut summary = None;
    let mut chunks = Vec::new();
    let mut loops = Vec::new();
    let mut resources = Vec::new();
    let mut resource_names = BTreeSet::new();
    let mut offset = 0;
    loop {
        let GeometryResponse::ArtifactPage {
            summary: current,
            chunks: page_chunks,
            loops: page_loops,
            resources: page_resources,
            next_offset,
        } = control(
            client,
            GeometryCommand::GetArtifactPage {
                session_id: session.clone(),
                artifact_id: artifact,
                kind,
                offset,
            },
        )
        .await?
        else {
            return Err(unexpected());
        };
        if summary.as_ref().is_some_and(|summary| summary != &current) {
            return Err("artifact summary changed during pagination".into());
        }
        if page_chunks.len() + page_loops.len() > ARTIFACT_PAGE_SIZE as usize {
            return Err("oversized artifact page".into());
        }
        if page_resources.len() != page_chunks.len()
            || page_resources
                .iter()
                .any(|resource| !resource_names.insert(resource.name.clone()))
        {
            return Err("artifact resources must uniquely correspond to chunk metadata".into());
        }
        let count = match kind {
            ArtifactPageKind::Chunks if page_loops.is_empty() => page_chunks.len(),
            ArtifactPageKind::Loops if page_chunks.is_empty() => page_loops.len(),
            _ => return Err("artifact page kind mismatch".into()),
        };
        let next = next_page(offset, count, next_offset, limit)?;
        summary = Some(current);
        chunks.extend(page_chunks);
        loops.extend(page_loops);
        resources.extend(page_resources);
        if let Some(next) = next {
            offset = next;
        } else {
            break;
        }
    }
    Ok((summary.ok_or_else(unexpected)?, chunks, loops, resources))
}
async fn release(
    client: &mut EngineClient,
    session: &SessionId,
    artifact: ArtifactId,
) -> Result<(), Error> {
    if !matches!(
        control(
            client,
            GeometryCommand::ReleaseArtifact {
                session_id: session.clone(),
                artifact_id: artifact
            }
        )
        .await?,
        GeometryResponse::Released {}
    ) {
        return Err(unexpected());
    }
    Ok(())
}
#[derive(Serialize)]
struct PartMapping {
    key: String,
    source_label: String,
    definition_id: DefinitionId,
    occurrence_id: OccurrenceId,
}
#[derive(Serialize)]
struct InstanceMapping {
    key: String,
    part: String,
    occurrence_id: OccurrenceId,
}

pub async fn run(
    client: &mut EngineClient,
    input: SceneInput,
    plane: Option<PlaneMm>,
    output: Option<&mut OutputDirectory>,
) -> Result<serde_json::Value, Error> {
    let session = client.hello().session_id.clone();
    let mut summary = scene(client, &session).await?;
    let mut part_map = BTreeMap::new();
    let mut part_mappings = Vec::new();
    let mut instance_mappings = Vec::new();
    for part in input.parts {
        let before = records(client, &summary)
            .await?
            .1
            .into_iter()
            .map(|row| row.occurrence_id)
            .collect::<BTreeSet<_>>();
        let expected_revision = summary.revision.next()?;
        let expected_occurrences = summary.occurrence_count + 1;
        let accepted = control(
            client,
            GeometryCommand::ImportPart {
                session_id: session.clone(),
                base_revision: summary.revision,
                source: part.source,
                initial_pose: part.pose,
            },
        )
        .await?;
        let GeometryResponse::OperationAccepted { operation } = accepted else {
            return Err(unexpected());
        };
        summary = match wait_operation(client, &session, operation).await? {
            JobResult::Scene { summary } => summary,
            _ => return Err(unexpected()),
        };
        summary.validate()?;
        if summary.session_id != session
            || summary.revision != expected_revision
            || summary.occurrence_count != expected_occurrences
        {
            return Err("import scene identity/revision/count mismatch".into());
        }
        let (_, occurrences) = records(client, &summary).await?;
        let added = occurrences
            .iter()
            .filter(|row| !before.contains(&row.occurrence_id))
            .collect::<Vec<_>>();
        if added.len() != 1 || added[0].pose != part.pose {
            return Err("import did not add exactly one correctly placed occurrence".into());
        }
        let added = added[0];
        part_map.insert(part.key.clone(), added.definition_id.clone());
        part_mappings.push(PartMapping {
            key: part.key,
            source_label: bounded_text(
                &part.path.to_string_lossy(),
                MAX_DISPLAY_LABEL_BYTES as usize,
            ),
            definition_id: added.definition_id.clone(),
            occurrence_id: added.occurrence_id,
        });
    }
    for instance in input.instances {
        let before = records(client, &summary)
            .await?
            .1
            .into_iter()
            .map(|row| row.occurrence_id)
            .collect::<BTreeSet<_>>();
        let expected_revision = summary.revision.next()?;
        let expected_occurrences = summary.occurrence_count + 1;
        let definition = part_map
            .get(&instance.part)
            .ok_or("unknown recipe part")?
            .clone();
        summary = match control(
            client,
            GeometryCommand::AddInstance {
                session_id: session.clone(),
                base_revision: summary.revision,
                definition_id: definition.clone(),
                pose: instance.pose,
            },
        )
        .await?
        {
            GeometryResponse::SceneChanged { summary } => summary,
            _ => return Err(unexpected()),
        };
        summary.validate()?;
        if summary.session_id != session
            || summary.revision != expected_revision
            || summary.occurrence_count != expected_occurrences
        {
            return Err("instance scene identity/revision/count mismatch".into());
        }
        let (_, occurrences) = records(client, &summary).await?;
        let added = occurrences
            .iter()
            .filter(|row| !before.contains(&row.occurrence_id))
            .collect::<Vec<_>>();
        if added.len() != 1
            || added[0].definition_id != definition
            || added[0].pose != instance.pose
        {
            return Err("instance admission mismatch".into());
        }
        instance_mappings.push(InstanceMapping {
            key: instance.key,
            part: instance.part,
            occurrence_id: added[0].occurrence_id,
        });
    }
    let mut report = inspect(client, plane, output).await?;
    report["parts"] = serde_json::json!(part_mappings);
    report["instances"] = serde_json::json!(instance_mappings);
    Ok(report)
}

/// Inspection is shared by ephemeral evaluation scenes and durable projects.
pub(crate) async fn inspect(
    client: &mut EngineClient,
    plane: Option<PlaneMm>,
    mut output: Option<&mut OutputDirectory>,
) -> Result<serde_json::Value, Error> {
    let session = client.hello().session_id.clone();
    let summary = scene(client, &session).await?;
    let (definitions, occurrences) = records(client, &summary).await?;
    let mut definition_reports = Vec::new();
    let mut bytes = 0_u32;
    let mut chunk_count = 0_u32;
    let mut vertices = 0_usize;
    let mut triangles = 0_usize;
    for definition in &definitions {
        let face_index = faces(client, &session, definition).await?;
        let (artifact_summary, rows, _, resources) = artifact_pages(
            client,
            &session,
            definition.mesh_artifact_id,
            ArtifactPageKind::Chunks,
            MAX_SCENE_MESH_BYTES / 64,
        )
        .await?;
        let metadata = rows
            .into_iter()
            .map(|row| match row {
                ArtifactChunkMetadata::Mesh { metadata } => Ok(metadata),
                _ => Err(unexpected()),
            })
            .collect::<Result<Vec<_>, Error>>()?;
        validate_mesh_manifest(&metadata)?;
        let artifact_bytes: u32 = metadata.iter().map(|row| row.byte_count).sum();
        match &artifact_summary {
            ArtifactSummary::Mesh {
                artifact_id,
                definition_id,
                chunk_count,
                total_bytes,
            } if *artifact_id == definition.mesh_artifact_id
                && definition_id == &definition.definition_id
                && *chunk_count as usize == metadata.len()
                && *total_bytes == artifact_bytes => {}
            _ => return Err("mesh artifact summary mismatch".into()),
        }
        bytes = bytes
            .checked_add(artifact_bytes)
            .ok_or("mesh byte count overflow")?;
        if bytes > MAX_SCENE_MESH_BYTES {
            return Err("unique scene mesh cap exceeded".into());
        }
        for (row, resource) in metadata.iter().zip(&resources) {
            if row.face_count != definition.face_count {
                return Err("mesh source face count mismatch".into());
            }
            let payload = client
                .read_geometry_chunk(
                    resource,
                    &ArtifactChunkMetadata::Mesh {
                        metadata: row.clone(),
                    },
                    &artifact_summary,
                )
                .await?;
            let decoded = validate_mesh_chunk(
                &payload,
                row,
                &session,
                definition.mesh_artifact_id,
                &definition.definition_id,
                row.chunk_index,
            )?;
            vertices += decoded.positions.len();
            triangles += decoded.face_ordinals.len();
            if vertices > MAX_DISPLAY_VERTICES as usize
                || triangles > MAX_DISPLAY_TRIANGLES as usize
            {
                return Err("unique display count cap exceeded".into());
            }
            if let Some(output) = output.as_deref_mut() {
                output.write(
                    &format!(
                        "mesh-{:06}-{:06}.splm",
                        definition.mesh_artifact_id.get(),
                        row.chunk_index
                    ),
                    &payload,
                )?;
            }
        }
        chunk_count = chunk_count
            .checked_add(metadata.len() as u32)
            .ok_or("chunk count overflow")?;
        release(client, &session, definition.mesh_artifact_id).await?;
        definition_reports.push(serde_json::json!({"record": definition, "faces": face_index, "chunks": metadata, "total_bytes": artifact_bytes}));
    }
    if bytes != summary.unique_mesh_bytes {
        return Err("scene mesh bytes disagree with verified transfer".into());
    }
    let section = if let Some(plane) = plane {
        let accepted = control(
            client,
            GeometryCommand::StartSection {
                session_id: session.clone(),
                base_revision: summary.revision,
                plane,
            },
        )
        .await?;
        let GeometryResponse::OperationAccepted { operation } = accepted else {
            return Err(unexpected());
        };
        let artifact = match wait_operation(client, &session, operation).await? {
            JobResult::Section { artifact_id } => artifact_id,
            _ => return Err(unexpected()),
        };
        let (artifact_summary, rows, _, resources) = artifact_pages(
            client,
            &session,
            artifact,
            ArtifactPageKind::Chunks,
            MAX_SECTION_BYTES / 64,
        )
        .await?;
        let ArtifactSummary::Section { summary: section } = &artifact_summary else {
            return Err(unexpected());
        };
        if section.session_id != session
            || section.artifact_id != artifact
            || section.revision != summary.revision
            || section.plane != plane.normalized()?
        {
            return Err("section identity/plane mismatch".into());
        }
        let (loop_summary, _, loops, _) = artifact_pages(
            client,
            &session,
            artifact,
            ArtifactPageKind::Loops,
            MAX_SECTION_BYTES / 100,
        )
        .await?;
        if loop_summary
            != (ArtifactSummary::Section {
                summary: section.clone(),
            })
        {
            return Err("section loop summary mismatch".into());
        }
        let metadata = rows
            .into_iter()
            .map(|row| match row {
                ArtifactChunkMetadata::Section { metadata } => Ok(metadata),
                _ => Err(unexpected()),
            })
            .collect::<Result<Vec<_>, Error>>()?;
        validate_section_manifest(section, &metadata, &loops)?;
        for row in &loops {
            if !occurrences.iter().any(|occurrence| {
                occurrence.occurrence_id == row.occurrence_id
                    && occurrence.definition_id == row.definition_id
            }) {
                return Err("section loop occurrence mismatch".into());
            }
        }
        let mut reports = Vec::new();
        let mut occurrence_areas = BTreeMap::<OccurrenceId, f64>::new();
        for (row, resource) in metadata.iter().zip(&resources) {
            let payload = client
                .read_geometry_chunk(
                    resource,
                    &ArtifactChunkMetadata::Section {
                        metadata: row.clone(),
                    },
                    &artifact_summary,
                )
                .await?;
            let decoded = validate_section_chunk(
                &payload,
                row,
                section,
                &session,
                artifact,
                row.chunk_index,
            )?;
            let first = row.first_loop_ordinal as usize;
            let rows = &loops[first..first + row.loop_count as usize];
            validate_section_loop_metadata(&decoded, section, rows)?;
            for (local, row) in rows.iter().enumerate() {
                let start = decoded.loop_offsets.get(local).unwrap() as usize;
                let end = decoded.loop_offsets.get(local + 1).unwrap() as usize;
                let mut area: f64 = 0.0;
                let mut perimeter: f64 = 0.0;
                let mut residual: f64 = 0.0;
                let first_point = decoded.points_relative_mm.get(start).unwrap();
                let mut bounds = AabbMm::new(
                    std::array::from_fn(|axis| first_point[axis] + section.plane.origin_mm[axis]),
                    std::array::from_fn(|axis| first_point[axis] + section.plane.origin_mm[axis]),
                )?;
                let dot = |point: [f64; 3], axis: [f64; 3]| {
                    point.iter().zip(axis).map(|(a, b)| a * b).sum::<f64>()
                };
                for index in start..end {
                    let a = decoded.points_relative_mm.get(index).unwrap();
                    bounds.include(std::array::from_fn(|axis| {
                        a[axis] + section.plane.origin_mm[axis]
                    }))?;
                    residual = residual.max(dot(a, section.plane.normal).abs());
                    if index + 1 < end {
                        let b = decoded.points_relative_mm.get(index + 1).unwrap();
                        // Subtract the first point before shoelace accumulation to retain
                        // precision for far-from-origin loops and large-origin scenes.
                        let a0 = std::array::from_fn(|axis| a[axis] - first_point[axis]);
                        let b0 = std::array::from_fn(|axis| b[axis] - first_point[axis]);
                        area += dot(a0, section.frame.x_axis) * dot(b0, section.frame.y_axis)
                            - dot(b0, section.frame.x_axis) * dot(a0, section.frame.y_axis);
                        perimeter += (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2]);
                    }
                }
                area *= 0.5;
                if !area.is_finite()
                    || !perimeter.is_finite()
                    || residual > SECTION_PLANE_TOLERANCE_MM
                {
                    return Err("invalid decoded section measurements".into());
                }
                *occurrence_areas.entry(row.occurrence_id).or_default() += area;
                reports.push(serde_json::json!({"metadata": row, "signed_area_mm2": area, "perimeter_mm": perimeter, "max_plane_residual_mm": residual, "bounds_mm": bounds}));
            }
            if let Some(output) = output.as_deref_mut() {
                output.write(
                    &format!("section-{:06}-{:06}.spls", artifact.get(), row.chunk_index),
                    &payload,
                )?;
            }
        }
        release(client, &session, artifact).await?;
        // Include empty occurrence sections explicitly, not just occurrences with loops.
        let areas = occurrences.iter().map(|occurrence| serde_json::json!({"occurrence_id": occurrence.occurrence_id, "definition_id": occurrence.definition_id, "area_mm2": occurrence_areas.get(&occurrence.occurrence_id).copied().unwrap_or(0.0), "loop_count": loops.iter().filter(|row| row.occurrence_id == occurrence.occurrence_id).count()})).collect::<Vec<_>>();
        Some(
            serde_json::json!({"summary": section, "chunks": metadata, "loops": reports, "occurrences": areas}),
        )
    } else {
        None
    };
    Ok(
        serde_json::json!({"command": "geometry", "hello": client.hello(), "scene": summary, "definitions": definition_reports, "occurrences": occurrences, "unique_mesh_bytes": bytes, "unique_mesh_chunks": chunk_count, "unique_mesh_vertices": vertices, "unique_mesh_triangles": triangles, "section": section}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_plane_numbers_and_zero_normal() {
        for text in [
            "0,0,0:0,0,0",
            "NaN,0,0:0,0,1",
            "0,0:0,0,1",
            "0,0,0:0,0,inf",
            "0,0,0:0,0,1:2",
        ] {
            assert!(parse_plane(text).is_err(), "{text}");
        }
        assert_eq!(
            parse_plane("1000000000,0,4:0,0,2")
                .unwrap()
                .normalized()
                .unwrap()
                .normal,
            [0.0, 0.0, 1.0]
        );
    }
    #[test]
    fn rejects_recipe_duplicate_keys_references_and_invalid_pose() {
        let source = NativePath::from_os_str(Path::new("part.step").as_os_str()).unwrap();
        let mut input = SceneInput {
            parts: vec![Part {
                key: "part".into(),
                path: "part.step".into(),
                source,
                pose: RigidPoseMm::IDENTITY,
            }],
            instances: vec![RecipeInstance {
                key: "part".into(),
                part: "part".into(),
                pose: RigidPoseMm::IDENTITY,
            }],
        };
        assert!(input.validate().is_err());
        input.instances[0].key = "second".into();
        input.instances[0].part = "missing".into();
        assert!(input.validate().is_err());
        input.instances[0].part = "part".into();
        input.instances[0].pose.rotation_xyzw = [0.0; 4];
        assert!(input.validate().is_err());
        input.instances[0].pose = RigidPoseMm::IDENTITY;
        assert!(input.validate().is_ok());
    }
    #[test]
    fn rejects_nonprogressing_and_out_of_bound_pages() {
        assert!(next_page(0, 0, Some(0), 6).is_err());
        assert!(next_page(0, 3, Some(2), 6).is_err());
        assert!(next_page(4, 3, None, 6).is_err());
        assert_eq!(next_page(0, 3, Some(3), 6).unwrap(), Some(3));
    }
}
