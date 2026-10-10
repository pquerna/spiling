// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod manufacturing;
mod placement;
mod raw;
mod source;
mod worker;
pub use raw::{Decoded, decode_request};

use crate::session::Watchdog;
use spiling_contracts::manufacturing::*;
use spiling_contracts::project::*;
use spiling_contracts::{Frame, FrameKind, Response, display::EncodedSection, geometry::*};
use spiling_core::storage::SaveReceipt;
use spiling_core::{Edit, PreparedEdit, Project};
use std::{
    collections::{BTreeMap, VecDeque},
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::TryRecvError,
    },
};
use worker::{Command, Event, Product, Transition, Work, Worker};

type Domain<T> = Result<T, GeometryError>;
pub fn error(code: GeometryErrorCode, message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(code, message)
}
fn unknown() -> GeometryError {
    error(
        GeometryErrorCode::UnknownHandle,
        "handle is not live in this session",
    )
}
fn resource(message: &str) -> GeometryError {
    error(GeometryErrorCode::ResourceLimit, message)
}
fn busy() -> GeometryError {
    error(
        GeometryErrorCode::Busy,
        "a geometry job or promotion is active",
    )
}
fn stale() -> GeometryError {
    error(
        GeometryErrorCode::StaleRevision,
        "session or scene revision is stale",
    )
}
fn check_cancel(cancel: &AtomicBool) -> Domain<()> {
    if cancel.load(Ordering::Acquire) {
        Err(error(
            GeometryErrorCode::Cancelled,
            "geometry job cancelled",
        ))
    } else {
        Ok(())
    }
}

struct DefinitionState {
    record: DefinitionRecord,
    faces: Vec<FaceInfo>,
    vertices: u32,
    triangles: u32,
    bytes: u32,
}
struct Artifact {
    summary: ArtifactSummary,
    chunks: Vec<ArtifactChunkMetadata>,
    bytes: Vec<Vec<u8>>,
    loops: Vec<SectionLoopMetadata>,
    leased: bool,
    pinned: bool,
}
impl Artifact {
    fn total_bytes(&self) -> u32 {
        match &self.summary {
            ArtifactSummary::Mesh { total_bytes, .. } => *total_bytes,
            ArtifactSummary::Section { summary } => summary.total_bytes,
        }
    }
    fn section(section: EncodedSection) -> Self {
        let mut metadata = Vec::with_capacity(section.chunks.len());
        let mut bytes = Vec::with_capacity(section.chunks.len());
        for chunk in section.chunks {
            metadata.push(ArtifactChunkMetadata::Section {
                metadata: chunk.metadata,
            });
            bytes.push(chunk.bytes);
        }
        Self {
            summary: ArtifactSummary::Section {
                summary: section.summary,
            },
            chunks: metadata,
            bytes,
            loops: section.loops,
            leased: true,
            pinned: false,
        }
    }
}
enum Purpose {
    Import { pose: RigidPoseMm },
    Section,
    Open,
    Save,
    Compile,
    Verify,
}
struct Publication {
    kind: PublicationKind,
    artifact: Option<Artifact>,
}
enum PublicationKind {
    Import {
        edit: PreparedEdit,
        definition: Option<DefinitionState>,
        revision: SceneRevision,
    },
    Open {
        project: Project,
        definitions: Vec<DefinitionState>,
        artifacts: Vec<(ArtifactId, Artifact)>,
        revision: SceneRevision,
    },
    Save(SaveReceipt),
    Section,
    Manufacturing {
        edit: PreparedEdit,
        record: ManufacturingArtifactRecord,
    },
    Verified {
        record: ManufacturingArtifactRecord,
        report: VerificationReport,
    },
}
struct Active {
    id: JobId,
    base: SceneRevision,
    project_base: ProjectRevision,
    artifact: ArtifactId,
    purpose: Purpose,
    cancel: Arc<AtomicBool>,
    deciding: bool,
    promote: bool,
    publication: Option<Publication>,
    failure: Option<JobError>,
}

/// The pipe dispatcher owns only bounded metadata and immutable encoded buffers.
/// Native values exist exclusively on Worker; publication is a two-phase ACK boundary.
pub struct Runtime {
    pub session_id: SessionId,
    revision: SceneRevision,
    definitions: BTreeMap<DefinitionId, DefinitionState>,
    project: Project,
    retained_source_bytes: u32,
    artifacts: BTreeMap<ArtifactId, Artifact>,
    released: VecDeque<ArtifactId>,
    jobs: BTreeMap<JobId, EngineJob>,
    terminal: VecDeque<JobId>,
    next_job: u32,
    next_artifact: u32,
    active: Option<Active>,
    worker: Worker,
    watchdog: Watchdog,
    fault: Option<crate::ProjectFault>,
}
impl Runtime {
    pub fn new(fault: Option<crate::ProjectFault>) -> Self {
        Self {
            session_id: SessionId::new(),
            revision: SceneRevision::ZERO,
            definitions: BTreeMap::new(),
            project: Project::new(),
            retained_source_bytes: 0,
            artifacts: BTreeMap::new(),
            released: VecDeque::new(),
            jobs: BTreeMap::new(),
            terminal: VecDeque::new(),
            next_job: 0,
            next_artifact: 0,
            active: None,
            worker: Worker::new(),
            watchdog: Watchdog::new(),
            fault,
        }
    }
    pub fn poll(&mut self) -> Result<(), String> {
        loop {
            match self.worker.result.try_recv() {
                Ok(event) => self.event(event)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err("native worker disconnected; session interrupted".into());
                }
            }
        }
        if let Some(active) = &self.active
            && active.cancel.load(Ordering::Acquire)
            && !active.deciding
        {
            let job = self.jobs.get_mut(&active.id).unwrap();
            job.status = JobStatus::Cancelling;
            job.stage = "cancelling".into();
        }
        Ok(())
    }
    fn event(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event {
                job,
                transition: Transition::Staged,
                staged: Some(result),
            } => {
                let mut active = self
                    .active
                    .take()
                    .ok_or("native result without an active job")?;
                if active.id != job || active.deciding {
                    return Err("native result correlation failure".into());
                }
                // A durable save receipt has already crossed its disk commit point.
                // It must reach core even if cancellation arrived after replacement.
                let durable = matches!(&result, Ok(Product::Saved(_)));
                let publication = if !durable && active.cancel.load(Ordering::Acquire) {
                    Err(cancelled(&active.purpose))
                } else if active.base != self.revision
                    || (matches!(active.purpose, Purpose::Compile | Purpose::Verify)
                        && active.project_base != self.project.info().revision)
                {
                    Err(
                        if matches!(active.purpose, Purpose::Compile | Purpose::Verify) {
                            ManufacturingError::new(
                                ManufacturingErrorCode::StaleRevision,
                                "manufacturing input revision is stale",
                            )
                            .into()
                        } else {
                            stale().into()
                        },
                    )
                } else {
                    result.and_then(|product| self.prepare(&active, product))
                };
                match publication {
                    Ok(publication) => {
                        if durable || self.watchdog.begin_commit(job) {
                            let snapshot = match &publication.kind {
                                PublicationKind::Import { edit, .. } => Some(edit.snapshot()),
                                PublicationKind::Open { project, .. } => Some(project.snapshot()),
                                _ => None,
                            };
                            if let Some(snapshot) = snapshot {
                                let mut live = self.worker.live.lock();
                                for id in snapshot.sources.keys() {
                                    live.reserve(id.clone());
                                }
                            }
                            active.promote = true;
                            active.publication = Some(publication);
                        } else {
                            active.failure = Some(cancelled(&active.purpose));
                        }
                    }
                    Err(failure) => active.failure = Some(failure),
                }
                active.deciding = true;
                let record = self.jobs.get_mut(&job).unwrap();
                record.stage = if active.promote {
                    "committing"
                } else {
                    "discarding"
                }
                .into();
                record.status = if active.failure.as_ref().is_some_and(is_cancelled) {
                    JobStatus::Cancelling
                } else {
                    JobStatus::Running
                };
                self.worker
                    .command
                    .send(Command::decide(job, active.promote))
                    .map_err(|_| "native promotion channel disconnected")?;
                self.active = Some(active);
            }
            Event {
                job,
                transition: Transition::Acknowledged { promoted },
                staged: None,
            } => {
                let mut active = self.active.take().ok_or("native ACK without active job")?;
                if active.id != job || !active.deciding || active.promote != promoted {
                    return Err("native promotion ACK correlation failure".into());
                }
                let result = if promoted {
                    let publication = active
                        .publication
                        .ok_or("native promotion lacks staged metadata")?;
                    match publication.kind {
                        PublicationKind::Import {
                            edit,
                            definition,
                            revision,
                        } => {
                            self.project
                                .commit(edit)
                                .map_err(|e| format!("core import ACK invariant: {e}"))?;
                            if let Some(definition) = definition {
                                self.definitions
                                    .insert(definition.record.definition_id.clone(), definition);
                            }
                            if let Some(artifact) = publication.artifact {
                                self.artifacts.insert(active.artifact, artifact);
                            }
                            self.revision = revision;
                            Some(JobResult::Scene {
                                summary: self.summary(),
                            })
                        }
                        PublicationKind::Open {
                            project,
                            definitions,
                            artifacts,
                            revision,
                        } => {
                            self.project = project;
                            for definition in definitions {
                                self.definitions
                                    .insert(definition.record.definition_id.clone(), definition);
                            }
                            self.artifacts.extend(artifacts);
                            self.revision = revision;
                            Some(JobResult::Scene {
                                summary: self.summary(),
                            })
                        }
                        PublicationKind::Save(receipt) => {
                            let durability_error = receipt.durability_error.clone();
                            self.project
                                .mark_saved(receipt)
                                .map_err(|e| format!("durable save ACK invariant: {e}"))?;
                            if let Some(error) = durability_error {
                                active.failure = Some(error.into());
                            }
                            // The receipt proves publication even when durability failed.
                            Some(JobResult::ProjectSaved {
                                info: self.project.info(),
                            })
                        }
                        PublicationKind::Manufacturing { edit, record } => {
                            self.project
                                .commit(edit)
                                .map_err(|e| format!("core manufacturing ACK invariant: {e}"))?;
                            Some(JobResult::ManufacturingCompiled {
                                info: self.project.info(),
                                record,
                            })
                        }
                        PublicationKind::Verified { record, report } => {
                            Some(JobResult::ManufacturingVerified { record, report })
                        }
                        PublicationKind::Section => {
                            self.artifacts.insert(
                                active.artifact,
                                publication
                                    .artifact
                                    .ok_or("section promotion lacks artifact")?,
                            );
                            Some(JobResult::Section {
                                artifact_id: active.artifact,
                            })
                        }
                    }
                } else {
                    None
                };
                let record = self.jobs.get_mut(&job).unwrap();
                record.status = if promoted && active.failure.is_none() {
                    JobStatus::Completed
                } else if active.failure.as_ref().is_some_and(is_cancelled) {
                    JobStatus::Cancelled
                } else {
                    JobStatus::Failed
                };
                record.stage = match record.status {
                    JobStatus::Completed => "completed",
                    JobStatus::Cancelled => "cancelled",
                    _ => "failed",
                }
                .into();
                record.result = result;
                record.error = active.failure;
                self.watchdog.finish(job);
                self.terminal.push_back(job);
                while self.terminal.len() > MAX_TERMINAL_JOBS as usize {
                    if let Some(old) = self.terminal.pop_front() {
                        self.jobs.remove(&old);
                    }
                }
                self.synchronize_native_pins();
            }
            _ => return Err("native event phase and staged payload disagree".into()),
        }
        Ok(())
    }
    fn prepare(&mut self, active: &Active, product: Product) -> Result<Publication, JobError> {
        match (&active.purpose, product) {
            (Purpose::Import { pose }, Product::Import(imported)) => {
                let revision = self.revision.next()?;
                placement::placed_bounds(*pose, imported.bounds)?;
                let edit = self.project.prepare(Edit::Import {
                    source: imported.source.clone(),
                    pose: *pose,
                })?;
                let (definition, artifact) =
                    self.prepare_definition(imported, active.artifact, 0)?;
                Ok(Publication {
                    kind: PublicationKind::Import {
                        edit,
                        definition,
                        revision,
                    },
                    artifact,
                })
            }
            (
                Purpose::Open,
                Product::Open {
                    mut project,
                    imports,
                },
            ) => {
                let revision = self.revision.next()?;
                project.retain_high_water(&self.project)?;
                let mut definitions = Vec::new();
                let mut artifacts = Vec::new();
                let mut staged_bytes = 0;
                let mut staged_vertices = 0;
                let mut staged_triangles = 0;
                for imported in imports {
                    let artifact_id =
                        ArtifactId::new(allocate(&mut self.next_artifact, "artifact ID")?)?;
                    let (definition, artifact) =
                        self.prepare_definition(imported, artifact_id, staged_bytes)?;
                    if let Some(definition) = definition {
                        staged_vertices += definition.vertices;
                        staged_triangles += definition.triangles;
                        staged_bytes += definition.bytes;
                        definitions.push(definition);
                    }
                    if let Some(artifact) = artifact {
                        artifacts.push((artifact_id, artifact));
                    }
                }
                if self.definitions.len() + definitions.len() > MAX_DEFINITIONS as usize
                    || self.definitions.values().map(|d| d.vertices).sum::<u32>() + staged_vertices
                        > MAX_DISPLAY_VERTICES
                    || self.definitions.values().map(|d| d.triangles).sum::<u32>()
                        + staged_triangles
                        > MAX_DISPLAY_TRIANGLES
                    || self.artifacts.len() + artifacts.len()
                        > (MAX_DEFINITIONS + MAX_TERMINAL_JOBS) as usize
                {
                    return Err(resource("open exceeds retained/staged scene budget").into());
                }
                for occurrence in project.snapshot().occurrences.values() {
                    let definition = definitions
                        .iter()
                        .find(|d| d.record.definition_id == occurrence.definition_id)
                        .or_else(|| self.definitions.get(&occurrence.definition_id))
                        .ok_or_else(stale)?;
                    placement::placed_bounds(occurrence.pose, definition.record.bounds_mm)?;
                }
                Ok(Publication {
                    kind: PublicationKind::Open {
                        project,
                        definitions,
                        artifacts,
                        revision,
                    },
                    artifact: None,
                })
            }
            (Purpose::Save, Product::Saved(receipt)) => Ok(Publication {
                kind: PublicationKind::Save(receipt),
                artifact: None,
            }),
            (Purpose::Compile, Product::Compiled(asset)) => {
                let record = asset.record.clone();
                let edit = self
                    .project
                    .prepare(Edit::PublishManufacturing { asset })
                    .map_err(manufacturing::project_error)?;
                Ok(Publication {
                    kind: PublicationKind::Manufacturing { edit, record },
                    artifact: None,
                })
            }
            (Purpose::Verify, Product::Verified { record, report }) => Ok(Publication {
                kind: PublicationKind::Verified { record, report },
                artifact: None,
            }),
            (Purpose::Section, Product::Section(section)) => {
                if section.summary.total_bytes > MAX_ENGINE_ARTIFACT_BYTES - self.artifact_bytes() {
                    return Err(resource("section exceeds remaining artifact budget").into());
                }
                Ok(Publication {
                    kind: PublicationKind::Section,
                    artifact: Some(Artifact::section(section)),
                })
            }
            _ => Err(error(
                GeometryErrorCode::KernelFailure,
                "native staged product differs from requested job",
            )
            .into()),
        }
    }
    fn prepare_definition(
        &self,
        imported: worker::Imported,
        artifact_id: ArtifactId,
        staged_bytes: u32,
    ) -> Domain<(Option<DefinitionState>, Option<Artifact>)> {
        let definition_id = imported.definition_id;
        let Some(mesh) = imported.mesh else {
            if !self.definitions.contains_key(&definition_id) {
                return Err(stale());
            }
            return Ok((None, None));
        };
        if self.definitions.contains_key(&definition_id) {
            return Err(error(
                GeometryErrorCode::KernelFailure,
                "duplicate native definition stage",
            ));
        }
        let vertices: u32 = self.definitions.values().map(|d| d.vertices).sum();
        let triangles: u32 = self.definitions.values().map(|d| d.triangles).sum();
        let bytes: u32 = self.definitions.values().map(|d| d.bytes).sum();
        if mesh.vertex_count > MAX_DISPLAY_VERTICES - vertices
            || mesh.triangle_count > MAX_DISPLAY_TRIANGLES - triangles
            || mesh.total_bytes > (MAX_SCENE_MESH_BYTES - bytes).saturating_sub(staged_bytes)
            || mesh.total_bytes
                > (MAX_ENGINE_ARTIFACT_BYTES - self.artifact_bytes()).saturating_sub(staged_bytes)
        {
            return Err(resource(
                "staged mesh exceeds remaining scene/artifact budget",
            ));
        }
        let record = DefinitionRecord {
            definition_id: definition_id.clone(),
            provenance: imported.provenance,
            face_count: imported.faces.len() as u32,
            bounds_mm: imported.bounds,
            mesh_artifact_id: artifact_id,
        };
        record.validate()?;
        let definition = DefinitionState {
            record,
            faces: imported.faces,
            vertices: mesh.vertex_count,
            triangles: mesh.triangle_count,
            bytes: mesh.total_bytes,
        };
        let mut chunks = Vec::with_capacity(mesh.chunks.len());
        let mut bytes = Vec::with_capacity(mesh.chunks.len());
        for chunk in mesh.chunks {
            chunks.push(ArtifactChunkMetadata::Mesh {
                metadata: chunk
                    .descriptor
                    .into_metadata(self.session_id.clone(), artifact_id),
            });
            bytes.push(chunk.bytes);
        }
        let artifact = Artifact {
            summary: ArtifactSummary::Mesh {
                artifact_id,
                definition_id,
                chunk_count: chunks.len() as u32,
                total_bytes: mesh.total_bytes,
            },
            chunks,
            bytes,
            loops: vec![],
            leased: true,
            pinned: true,
        };
        Ok((Some(definition), Some(artifact)))
    }
    fn artifact_bytes(&self) -> u32 {
        self.artifacts.values().map(Artifact::total_bytes).sum()
    }
    fn session(&self, session: &SessionId) -> Domain<()> {
        if session == &self.session_id {
            Ok(())
        } else {
            Err(stale())
        }
    }
    fn base(&self, session: &SessionId, revision: SceneRevision) -> Domain<()> {
        self.session(session)?;
        if revision != self.revision {
            return Err(stale());
        }
        if self.active.as_ref().is_some_and(|active| {
            active.deciding || matches!(active.purpose, Purpose::Open | Purpose::Save)
        }) {
            return Err(busy());
        }
        Ok(())
    }
    fn synchronize_native_pins(&mut self) {
        let sources = self.project.retained_sources();
        self.retained_source_bytes = sources.iter().map(|source| source.bytes.len() as u32).sum();
        let ids: std::collections::BTreeSet<_> = sources
            .iter()
            .map(|source| source.record.definition_id.clone())
            .collect();
        for source in &sources {
            if let Some(definition) = self.definitions.get_mut(&source.record.definition_id) {
                definition.record.provenance = source.record.provenance.clone();
            }
        }
        let removed: Vec<_> = self
            .definitions
            .iter()
            .filter(|(id, _)| !ids.contains(*id))
            .map(|(_, definition)| definition.record.mesh_artifact_id)
            .collect();
        self.definitions.retain(|id, _| ids.contains(id));
        for id in removed {
            if let Some(artifact) = self.artifacts.get_mut(&id) {
                artifact.pinned = false;
            }
            self.collect_artifact(id);
        }
        self.worker.live.lock().replace(ids);
    }
    fn summary(&self) -> SceneSummary {
        let snapshot = self.project.snapshot();
        let mut bounds: Option<AabbMm> = None;
        for occurrence in snapshot.occurrences.values() {
            let definition = &self.definitions[&occurrence.definition_id];
            let placed = placement::placed_bounds(occurrence.pose, definition.record.bounds_mm)
                .expect("validated placed bounds");
            if let Some(bounds) = &mut bounds {
                bounds.include(placed.min).expect("finite bounds");
                bounds.include(placed.max).expect("finite bounds");
            } else {
                bounds = Some(placed);
            }
        }
        SceneSummary {
            session_id: self.session_id.clone(),
            revision: self.revision,
            definition_count: snapshot.sources.len() as u32,
            occurrence_count: snapshot.occurrences.len() as u32,
            bounds_mm: bounds,
            unique_mesh_bytes: snapshot
                .sources
                .keys()
                .map(|id| self.definitions[id].bytes)
                .sum(),
        }
    }
    pub fn control(&mut self, command: GeometryCommand) -> GeometryResponse {
        match self.control_inner(command) {
            Ok(response) => response,
            Err(error) => GeometryResponse::Error { error },
        }
    }
    fn apply_edit(&mut self, edit: Edit) -> GeometryResponse {
        let prepared = match self.project.prepare(edit) {
            Ok(prepared) => prepared,
            Err(error) => return GeometryResponse::ProjectError { error },
        };
        let revision = match self.revision.next() {
            Ok(revision) => revision,
            Err(error) => return GeometryResponse::Error { error },
        };
        if let Err(error) = self.project.commit(prepared) {
            return GeometryResponse::ProjectError { error };
        }
        self.revision = revision;
        self.synchronize_native_pins();
        GeometryResponse::SceneChanged {
            summary: self.summary(),
        }
    }
    fn control_inner(&mut self, command: GeometryCommand) -> Domain<GeometryResponse> {
        match command {
            GeometryCommand::ImportPart {
                session_id,
                base_revision,
                source,
                initial_pose,
            } => {
                self.base(&session_id, base_revision)?;
                initial_pose.validate()?;
                source.validate()?;
                if self.active.is_some() {
                    return Err(busy());
                }
                if self.project.info().read_only {
                    return Ok(GeometryResponse::ProjectError {
                        error: project_error(ProjectErrorCode::ReadOnly, "project is read-only"),
                    });
                }
                if self.project.snapshot().occurrences.len() >= MAX_OCCURRENCES as usize {
                    return Err(resource("occurrence cap exceeded"));
                }
                let vertices = self.definitions.values().map(|d| d.vertices).sum::<u32>();
                let triangles = self.definitions.values().map(|d| d.triangles).sum::<u32>();
                let bytes = self.definitions.values().map(|d| d.bytes).sum::<u32>();
                let allowed_reuse = self.definitions.keys().cloned().collect();
                self.start(
                    Purpose::Import { pose: initial_pose },
                    MAX_SCENE_MESH_BYTES,
                    move |_, _, _| Work::Import {
                        source,
                        allowed_reuse,
                        budget: worker::MeshBudget {
                            vertices: MAX_DISPLAY_VERTICES - vertices,
                            triangles: MAX_DISPLAY_TRIANGLES - triangles,
                            bytes: MAX_SCENE_MESH_BYTES - bytes,
                        },
                    },
                )
            }
            GeometryCommand::StartSection {
                session_id,
                base_revision,
                plane,
            } => {
                self.base(&session_id, base_revision)?;
                let plane = plane.normalized()?;
                if self.active.is_some() {
                    return Err(busy());
                }
                let occurrences = self
                    .project
                    .snapshot()
                    .occurrences
                    .values()
                    .cloned()
                    .collect();
                let session = self.session_id.clone();
                let revision = self.revision;
                self.start(
                    Purpose::Section,
                    MAX_SECTION_BYTES,
                    move |_, artifact, _| Work::Section {
                        session,
                        artifact,
                        revision,
                        plane,
                        occurrences,
                    },
                )
            }
            GeometryCommand::AddInstance {
                session_id,
                base_revision,
                definition_id,
                pose,
            } => {
                self.base(&session_id, base_revision)?;
                pose.validate()?;
                let definition = self.definitions.get(&definition_id).ok_or_else(unknown)?;
                placement::placed_bounds(pose, definition.record.bounds_mm).map_err(|_| {
                    error(
                        GeometryErrorCode::InvalidPose,
                        "placed bounds overflow finite scene coordinates",
                    )
                })?;
                Ok(self.apply_edit(Edit::Add {
                    definition_id,
                    pose,
                }))
            }
            GeometryCommand::SetInstancePose {
                session_id,
                base_revision,
                occurrence_id,
                pose,
            } => {
                self.base(&session_id, base_revision)?;
                pose.validate()?;
                let snapshot = self.project.snapshot();
                let occurrence = snapshot
                    .occurrences
                    .get(&occurrence_id)
                    .ok_or_else(unknown)?;
                placement::placed_bounds(
                    pose,
                    self.definitions[&occurrence.definition_id].record.bounds_mm,
                )
                .map_err(|_| {
                    error(
                        GeometryErrorCode::InvalidPose,
                        "placed bounds overflow finite scene coordinates",
                    )
                })?;
                Ok(self.apply_edit(Edit::SetPose {
                    occurrence_id,
                    pose,
                }))
            }
            GeometryCommand::RemoveInstance {
                session_id,
                base_revision,
                occurrence_id,
            } => {
                self.base(&session_id, base_revision)?;
                Ok(self.apply_edit(Edit::Remove { occurrence_id }))
            }
            GeometryCommand::GetScene { session_id } => {
                self.session(&session_id)?;
                Ok(GeometryResponse::Scene {
                    summary: self.summary(),
                })
            }
            GeometryCommand::GetScenePage {
                session_id,
                revision,
                kind,
                offset,
            } => {
                self.session(&session_id)?;
                if revision != self.revision {
                    return Err(stale());
                }
                let snapshot = self.project.snapshot();
                let count = match kind {
                    ScenePageKind::Definitions => snapshot.sources.len(),
                    ScenePageKind::Occurrences => snapshot.occurrences.len(),
                };
                let (start, end, next_offset) = page(count, offset, SCENE_PAGE_SIZE)?;
                let definitions = if kind == ScenePageKind::Definitions {
                    snapshot
                        .sources
                        .keys()
                        .skip(start)
                        .take(end - start)
                        .map(|id| self.definitions[id].record.clone())
                        .collect()
                } else {
                    vec![]
                };
                let occurrences = if kind == ScenePageKind::Occurrences {
                    snapshot
                        .occurrences
                        .values()
                        .skip(start)
                        .take(end - start)
                        .cloned()
                        .collect()
                } else {
                    vec![]
                };
                Ok(GeometryResponse::ScenePage {
                    definitions,
                    occurrences,
                    next_offset,
                })
            }
            GeometryCommand::GetFaceIndexPage {
                session_id,
                definition_id,
                offset,
            } => {
                self.session(&session_id)?;
                if !self.project.snapshot().sources.contains_key(&definition_id) {
                    return Err(unknown());
                }
                let definition = self.definitions.get(&definition_id).ok_or_else(unknown)?;
                let (start, end, next_offset) =
                    page(definition.faces.len(), offset, FACE_PAGE_SIZE)?;
                let faces = definition
                    .faces
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(end - start)
                    .map(|(ordinal, f)| FaceIndexRow {
                        ordinal: ordinal as u32,
                        face_id: f.face_id.clone(),
                    })
                    .collect();
                Ok(GeometryResponse::FaceIndexPage { faces, next_offset })
            }
            GeometryCommand::InspectFace { reference } => {
                self.session(&reference.session_id)?;
                if reference.scene_revision != self.revision {
                    return Err(stale());
                }
                let snapshot = self.project.snapshot();
                let occurrence = snapshot
                    .occurrences
                    .get(&reference.occurrence_id)
                    .ok_or_else(unknown)?;
                if occurrence.definition_id != reference.definition_id {
                    return Err(unknown());
                }
                let definition = self
                    .definitions
                    .get(&reference.definition_id)
                    .ok_or_else(unknown)?;
                let face = definition
                    .faces
                    .iter()
                    .find(|f| f.face_id == reference.face_id)
                    .ok_or_else(unknown)?
                    .clone();
                Ok(GeometryResponse::FaceInspection {
                    inspection: FaceInspection {
                        reference,
                        face,
                        pose: occurrence.pose,
                        provenance: definition.record.provenance.clone(),
                    },
                })
            }
            GeometryCommand::GetJob { session_id, job_id } => {
                self.session(&session_id)?;
                Ok(GeometryResponse::Job {
                    job: self.jobs.get(&job_id).ok_or_else(unknown)?.clone(),
                })
            }
            GeometryCommand::CancelJob { session_id, job_id } => {
                self.session(&session_id)?;
                let job = self.jobs.get_mut(&job_id).ok_or_else(unknown)?;
                if !job.status.is_terminal() {
                    let committing = self.watchdog.cancel(job_id);
                    if committing {
                        job.stage = "committing".into();
                    } else {
                        job.status = JobStatus::Cancelling;
                        job.stage = "cancelling".into();
                    }
                }
                Ok(GeometryResponse::Job { job: job.clone() })
            }
            GeometryCommand::GetArtifactPage {
                session_id,
                artifact_id,
                kind,
                offset,
            } => {
                self.session(&session_id)?;
                let artifact = self.artifacts.get(&artifact_id).ok_or_else(unknown)?;
                let count = match kind {
                    ArtifactPageKind::Chunks => artifact.chunks.len(),
                    ArtifactPageKind::Loops => artifact.loops.len(),
                };
                let (start, end, next_offset) = page(count, offset, ARTIFACT_PAGE_SIZE)?;
                let chunks = if kind == ArtifactPageKind::Chunks {
                    artifact.chunks[start..end].to_vec()
                } else {
                    vec![]
                };
                let loops = if kind == ArtifactPageKind::Loops {
                    artifact.loops[start..end].to_vec()
                } else {
                    vec![]
                };
                Ok(GeometryResponse::ArtifactPage {
                    summary: artifact.summary.clone(),
                    chunks,
                    loops,
                    next_offset,
                })
            }
            GeometryCommand::ReleaseArtifact {
                session_id,
                artifact_id,
            } => {
                self.session(&session_id)?;
                if let Some(artifact) = self.artifacts.get_mut(&artifact_id) {
                    artifact.leased = false;
                    self.collect_artifact(artifact_id);
                } else if !self.released.contains(&artifact_id) {
                    return Err(unknown());
                }
                Ok(GeometryResponse::Released {})
            }
            GeometryCommand::ReadArtifactChunk { .. } => Err(error(
                GeometryErrorCode::InvalidGeometry,
                "chunk reads use the binary response path",
            )),
        }
    }
    pub fn project_control(&mut self, command: ProjectCommand) -> ProjectResponse {
        match self.project_control_inner(command) {
            Ok(response) => response,
            Err(error) => ProjectResponse::Error { error },
        }
    }
    fn project_base(&self, session: &SessionId, base: SceneRevision) -> Result<(), ProjectError> {
        if session != &self.session_id || base != self.revision {
            return Err(project_error(
                ProjectErrorCode::StaleRevision,
                "session or scene revision is stale",
            ));
        }
        if self.active.as_ref().is_some_and(|active| {
            active.deciding || matches!(active.purpose, Purpose::Open | Purpose::Save)
        }) {
            return Err(project_error(
                ProjectErrorCode::Busy,
                "a project job or promotion is active",
            ));
        }
        Ok(())
    }
    fn replace_allowed(&self, discard: bool) -> Result<(), ProjectError> {
        if self.active.is_some() {
            return Err(project_error(
                ProjectErrorCode::Busy,
                "an engine job is active",
            ));
        }
        if self.project.info().dirty && !discard {
            return Err(project_error(
                ProjectErrorCode::DirtyProject,
                "replacement requires explicit discard of unsaved changes",
            ));
        }
        Ok(())
    }
    fn project_control_inner(
        &mut self,
        command: ProjectCommand,
    ) -> Result<ProjectResponse, ProjectError> {
        match command {
            ProjectCommand::Get { session_id } => {
                if session_id != self.session_id {
                    return Err(project_error(
                        ProjectErrorCode::StaleRevision,
                        "session is stale",
                    ));
                }
                Ok(ProjectResponse::Status {
                    info: self.project.info(),
                })
            }
            ProjectCommand::New {
                session_id,
                base_revision,
                discard_changes,
            } => {
                self.project_base(&session_id, base_revision)?;
                self.replace_allowed(discard_changes)?;
                let revision = project_scene_revision(self.revision)?;
                self.project = Project::new();
                self.revision = revision;
                self.synchronize_native_pins();
                Ok(ProjectResponse::SceneChanged {
                    info: self.project.info(),
                    summary: self.summary(),
                })
            }
            ProjectCommand::Open {
                session_id,
                base_revision,
                path,
                read_only,
                recover_previous,
                discard_changes,
            } => {
                self.project_base(&session_id, base_revision)?;
                self.replace_allowed(discard_changes)?;
                path.validate()
                    .map_err(|e| project_error(ProjectErrorCode::InvalidProject, &e.message))?;
                project_scene_revision(self.revision)?;
                let existing = self.project.storage();
                let allowed_reuse = self.definitions.keys().cloned().collect();
                let retained_source_bytes = self.retained_source_bytes;
                let retained_manufacturing_bytes = self.manufacturing_bytes();
                let budget = worker::MeshBudget {
                    vertices: MAX_DISPLAY_VERTICES
                        - self.definitions.values().map(|d| d.vertices).sum::<u32>(),
                    triangles: MAX_DISPLAY_TRIANGLES
                        - self.definitions.values().map(|d| d.triangles).sum::<u32>(),
                    bytes: MAX_SCENE_MESH_BYTES
                        - self.definitions.values().map(|d| d.bytes).sum::<u32>(),
                };
                let response = self
                    .start(Purpose::Open, 0, move |_, _, _| {
                        Work::Open(worker::OpenRequest {
                            path,
                            read_only,
                            recover_previous,
                            existing,
                            allowed_reuse,
                            budget,
                            retained_source_bytes,
                            retained_manufacturing_bytes,
                        })
                    })
                    .map_err(project_geometry_error)?;
                match response {
                    GeometryResponse::JobAccepted { job_id } => {
                        Ok(ProjectResponse::JobAccepted { job_id })
                    }
                    _ => unreachable!("start only accepts a job"),
                }
            }
            ProjectCommand::Save {
                session_id,
                base_revision,
                target,
            } => {
                self.project_base(&session_id, base_revision)?;
                if self.active.is_some() {
                    return Err(project_error(
                        ProjectErrorCode::Busy,
                        "an engine job is active",
                    ));
                }
                if self.project.info().read_only {
                    return Err(project_error(
                        ProjectErrorCode::ReadOnly,
                        "project is read-only",
                    ));
                }
                if let Some(target) = &target {
                    target
                        .validate()
                        .map_err(|e| project_error(ProjectErrorCode::InvalidProject, &e.message))?;
                } else if self.project.storage().is_none() {
                    return Err(project_error(
                        ProjectErrorCode::NoSavedPath,
                        "first save requires a new target",
                    ));
                }
                let snapshot = self.project.capture();
                let existing = self.project.storage();
                let fault = self.fault;
                let response = self
                    .start(Purpose::Save, 0, move |job, _, watchdog| Work::Save {
                        snapshot,
                        existing,
                        target,
                        fault,
                        commit: watchdog.commit_token(job),
                    })
                    .map_err(project_geometry_error)?;
                match response {
                    GeometryResponse::JobAccepted { job_id } => {
                        Ok(ProjectResponse::JobAccepted { job_id })
                    }
                    _ => unreachable!("start only accepts a job"),
                }
            }
            ProjectCommand::Undo {
                session_id,
                base_revision,
            } => {
                self.project_base(&session_id, base_revision)?;
                let revision = project_scene_revision(self.revision)?;
                let before = self.project.snapshot();
                self.project.undo()?;
                if same_geometry(&before, &self.project.snapshot()) {
                    return Ok(ProjectResponse::Status {
                        info: self.project.info(),
                    });
                }
                self.revision = revision;
                self.synchronize_native_pins();
                Ok(ProjectResponse::SceneChanged {
                    info: self.project.info(),
                    summary: self.summary(),
                })
            }
            ProjectCommand::Redo {
                session_id,
                base_revision,
            } => {
                self.project_base(&session_id, base_revision)?;
                let revision = project_scene_revision(self.revision)?;
                let before = self.project.snapshot();
                self.project.redo()?;
                if same_geometry(&before, &self.project.snapshot()) {
                    return Ok(ProjectResponse::Status {
                        info: self.project.info(),
                    });
                }
                self.revision = revision;
                self.synchronize_native_pins();
                Ok(ProjectResponse::SceneChanged {
                    info: self.project.info(),
                    summary: self.summary(),
                })
            }
        }
    }
    fn start(
        &mut self,
        purpose: Purpose,
        reservation: u32,
        work: impl FnOnce(JobId, ArtifactId, &Watchdog) -> Work,
    ) -> Domain<GeometryResponse> {
        if reservation > MAX_ENGINE_ARTIFACT_BYTES - self.artifact_bytes() {
            return Err(resource(
                "encoded artifact staging reservation exceeds budget; release unused artifacts",
            ));
        }
        // Byte limits do not bound empty section handle metadata. Keep a bounded lease table.
        if matches!(purpose, Purpose::Import { .. } | Purpose::Section)
            && self.artifacts.len() >= (MAX_DEFINITIONS + MAX_TERMINAL_JOBS) as usize
        {
            return Err(resource(
                "encoded artifact handle cap exceeded; release unused artifacts",
            ));
        }
        let job = JobId::new(allocate(&mut self.next_job, "job ID")?)?;
        let artifact = ArtifactId::new(allocate(&mut self.next_artifact, "artifact ID")?)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let stage = match &purpose {
            Purpose::Import { .. } => "importing",
            Purpose::Section => "sectioning",
            Purpose::Open => "opening",
            Purpose::Save => "saving",
            Purpose::Compile => "compiling_manufacturing",
            Purpose::Verify => "verifying_manufacturing",
        };
        self.watchdog.start(job, cancel.clone());
        let work = work(job, artifact, &self.watchdog);
        if self
            .worker
            .command
            .try_send(Command::run(job, cancel.clone(), work))
            .is_err()
        {
            crate::diagnostic(
                "error",
                "geometry_worker_disconnect",
                "native worker command slot failed",
            );
            std::process::exit(70);
        }
        self.jobs.insert(
            job,
            EngineJob {
                job_id: job,
                status: JobStatus::Running,
                stage: stage.into(),
                result: None,
                error: None,
            },
        );
        self.active = Some(Active {
            id: job,
            base: self.revision,
            project_base: self.project.info().revision,
            artifact,
            purpose,
            cancel,
            deciding: false,
            promote: false,
            publication: None,
            failure: None,
        });
        Ok(GeometryResponse::JobAccepted { job_id: job })
    }
    fn collect_artifact(&mut self, id: ArtifactId) {
        if self
            .artifacts
            .get(&id)
            .is_some_and(|a| !a.pinned && !a.leased)
        {
            self.artifacts.remove(&id);
            self.released.push_back(id);
            while self.released.len() > MAX_TERMINAL_JOBS as usize {
                self.released.pop_front();
            }
        }
    }
    pub fn write_chunk(
        &self,
        session: &SessionId,
        artifact: ArtifactId,
        chunk: u32,
        request: u32,
        output: &mut impl Write,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let bytes = self
            .session(session)
            .and_then(|_| self.artifacts.get(&artifact).ok_or_else(unknown))
            .and_then(|artifact| artifact.bytes.get(chunk as usize).ok_or_else(unknown));
        match bytes {
            Ok(bytes) => Frame::write_payload(FrameKind::GeometryChunk, request, bytes, output)?,
            Err(error) => Frame::control(
                request,
                &Response::Geometry {
                    response: GeometryResponse::Error { error },
                },
            )?
            .write(output)?,
        }
        Ok(())
    }
    pub fn shutdown(&mut self) -> Result<(), String> {
        if let Some(active) = &self.active {
            self.watchdog.cancel(active.id);
        }
        while self.active.is_some() {
            self.poll()?;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.definitions.clear();
        self.project = Project::new();
        self.artifacts.clear();
        self.synchronize_native_pins();
        self.worker.stop()
    }
}
fn project_geometry_error(error: GeometryError) -> ProjectError {
    let code = match error.code {
        GeometryErrorCode::Busy => ProjectErrorCode::Busy,
        GeometryErrorCode::StaleRevision => ProjectErrorCode::StaleRevision,
        GeometryErrorCode::ResourceLimit => ProjectErrorCode::ResourceLimit,
        GeometryErrorCode::Cancelled => ProjectErrorCode::Cancelled,
        _ => ProjectErrorCode::InvalidProject,
    };
    ProjectError::new(code, error.message)
}
fn project_scene_revision(revision: SceneRevision) -> Result<SceneRevision, ProjectError> {
    revision.next().map_err(project_geometry_error)
}
fn project_error(code: ProjectErrorCode, message: &str) -> ProjectError {
    ProjectError::new(code, message)
}
fn is_cancelled(error: &JobError) -> bool {
    match error {
        JobError::Geometry { error } => error.code == GeometryErrorCode::Cancelled,
        JobError::Project { error } => error.code == ProjectErrorCode::Cancelled,
        JobError::Manufacturing { error } => error.code == ManufacturingErrorCode::Cancelled,
    }
}
fn cancelled(purpose: &Purpose) -> JobError {
    match purpose {
        Purpose::Compile | Purpose::Verify => ManufacturingError::new(
            ManufacturingErrorCode::Cancelled,
            "manufacturing job cancelled before publication",
        )
        .into(),
        Purpose::Open | Purpose::Save => project_error(
            ProjectErrorCode::Cancelled,
            "project job cancelled before publication",
        )
        .into(),
        _ => error(
            GeometryErrorCode::Cancelled,
            "geometry job cancelled before publication",
        )
        .into(),
    }
}
fn allocate(counter: &mut u32, name: &str) -> Domain<u32> {
    *counter = counter
        .checked_add(1)
        .ok_or_else(|| resource(&format!("{name} exhausted")))?;
    Ok(*counter)
}
fn page(count: usize, offset: u32, size: u32) -> Domain<(usize, usize, Option<u32>)> {
    let start = offset as usize;
    if start > count {
        return Err(unknown());
    }
    let end = (start + size as usize).min(count);
    Ok((start, end, (end < count).then_some(end as u32)))
}

fn same_geometry(left: &spiling_core::Snapshot, right: &spiling_core::Snapshot) -> bool {
    left.occurrences == right.occurrences
        && left.sources.len() == right.sources.len()
        && left
            .sources
            .iter()
            .zip(&right.sources)
            .all(|((left_id, left), (right_id, right))| {
                left_id == right_id && left.record == right.record
            })
}
