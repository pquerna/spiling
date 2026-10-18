// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Runtime-independent authoring authority. Native caches are engine-owned.
mod process;
pub mod storage;
use process::apply_process_edit;
use sha2::{Digest, Sha256};
use spiling_contracts::{geometry::*, manufacturing::*, process::*, project::*, surface::*};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, Weak},
};
use storage::{LoadedProject, SaveReceipt, Storage};

pub(crate) fn error(code: ProjectErrorCode, message: impl AsRef<str>) -> ProjectError {
    ProjectError::new(code, message)
}
pub(crate) fn invalid(message: impl AsRef<str>) -> ProjectError {
    error(ProjectErrorCode::InvalidProject, message)
}
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RETAINED_SOURCE_BYTES: usize = 64 * 1024 * 1024;

/// Checked immutable canonical PGM content. Original encoded paths never enter Core.
#[derive(Debug)]
pub struct SurfaceImageAsset {
    record: SurfaceImageRecord,
    bytes: Arc<Vec<u8>>,
}
impl SurfaceImageAsset {
    pub fn new(record: SurfaceImageRecord, bytes: Vec<u8>) -> Result<Self, ProjectError> {
        record.validate_content(&bytes)?;
        if bytes.capacity() > MAX_SURFACE_IMAGE_RETAINED_BYTES {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "image allocation exceeds retained budget",
            ));
        }
        Ok(Self {
            record,
            bytes: Arc::new(bytes),
        })
    }
    pub fn record(&self) -> &SurfaceImageRecord {
        &self.record
    }
    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }
}

#[derive(Debug, Clone)]
pub struct SourceAsset {
    pub record: StoredDefinition,
    pub bytes: Arc<Vec<u8>>,
}
impl SourceAsset {
    pub fn new(record: StoredDefinition, bytes: Vec<u8>) -> Result<Self, ProjectError> {
        let asset = Self {
            record,
            bytes: Arc::new(bytes),
        };
        asset.validate()?;
        Ok(asset)
    }
    pub(crate) fn validate(&self) -> Result<(), ProjectError> {
        if self.bytes.is_empty() || self.bytes.len() > MAX_SOURCE_BYTES {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "source asset must contain 1..16 MiB",
            ));
        }
        self.record
            .provenance
            .validate()
            .map_err(|e| invalid(e.message))?;
        let digest: [u8; 32] = Sha256::digest(self.bytes.as_slice()).into();
        if SourceHash::from_digest(&digest) != self.record.provenance.source_hash
            || DefinitionId::from_source_sha256(&digest) != self.record.definition_id
        {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "source SHA-256 or definition identity mismatch",
            ));
        }
        Ok(())
    }
}
/// Immutable exact JSON bundle bytes. Persisted verification is metadata, not replay proof.
#[derive(Debug, Clone)]
pub struct ManufacturingAsset {
    pub record: ManufacturingArtifactRecord,
    pub bytes: Arc<Vec<u8>>,
}
impl ManufacturingAsset {
    pub fn new(record: ManufacturingArtifactRecord, bytes: Vec<u8>) -> Result<Self, ProjectError> {
        let asset = Self {
            record,
            bytes: Arc::new(bytes),
        };
        asset.validate()?;
        Ok(asset)
    }
    pub(crate) fn validate(&self) -> Result<ManufacturingBundle, ProjectError> {
        if self.bytes.is_empty() || self.bytes.capacity() > MAX_MANUFACTURING_BUNDLE_BYTES as usize
        {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "manufacturing bundle exceeds byte budget",
            ));
        }
        self.record.validate().map_err(manufacturing_error)?;
        let digest: [u8; 32] = Sha256::digest(self.bytes.as_slice()).into();
        if SourceHash::from_digest(&digest) != self.record.hash
            || self.bytes.len() != self.record.byte_count as usize
        {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "manufacturing hash or byte count mismatch",
            ));
        }
        let bundle = decode_bundle(self.bytes.as_slice()).map_err(manufacturing_error)?;
        if bundle.provenance.input_hash != self.record.input_hash
            || bundle.summary() != self.record.summary
        {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "manufacturing record differs from bundle",
            ));
        }
        Ok(bundle)
    }
}
fn manufacturing_error(value: ManufacturingError) -> ProjectError {
    error(
        if value.code == ManufacturingErrorCode::ResourceLimit {
            ProjectErrorCode::ResourceLimit
        } else {
            ProjectErrorCode::CorruptAsset
        },
        value.message,
    )
}
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub sources: BTreeMap<DefinitionId, Arc<SourceAsset>>,
    pub occurrences: BTreeMap<OccurrenceId, OccurrenceRecord>,
    pub manufacturing_intent: Option<Arc<ManufacturingIntent>>,
    pub manufacturing_artifact: Option<Arc<ManufacturingAsset>>,
    pub surface_overlays: Arc<SurfaceOverlays>,
    pub process_document: Arc<ProcessDocument>,
    pub surface_images: BTreeMap<SourceHash, Arc<SurfaceImageAsset>>,
}
#[derive(Debug, Clone)]
pub enum Edit {
    Process {
        edit: ProcessEdit,
    },
    Import {
        source: Arc<SourceAsset>,
        pose: RigidPoseMm,
    },
    Add {
        definition_id: DefinitionId,
        pose: RigidPoseMm,
    },
    SetPose {
        occurrence_id: OccurrenceId,
        pose: RigidPoseMm,
    },
    Remove {
        occurrence_id: OccurrenceId,
    },
    SetManufacturingIntent {
        intent: Arc<ManufacturingIntent>,
    },
    PublishManufacturing {
        asset: Arc<ManufacturingAsset>,
    },
    AddSurfaceFinish {
        scope: SurfaceScope,
        face_id: FaceId,
        finish: SurfaceFinish,
    },
    AddImageSurfaceFinish {
        asset: Arc<SurfaceImageAsset>,
        scope: SurfaceScope,
        face_id: FaceId,
        parameters: ImageReliefParameters,
    },
    RemoveSurfaceFinish {
        layer_id: SurfaceLayerId,
    },
    SetSurfaceFinishEnabled {
        layer_id: SurfaceLayerId,
        enabled: bool,
    },
}
#[derive(Debug)]
pub struct PreparedEdit {
    owner: Arc<()>,
    revision: ProjectRevision,
    snapshot: Arc<Snapshot>,
    next_occurrence: u32,
    next_surface_layer: u32,
    next_process_layer: u32,
    next_process_operation: u32,
}
impl PreparedEdit {
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.clone()
    }
    pub fn surface_overlays(&self) -> &SurfaceOverlays {
        &self.snapshot.surface_overlays
    }
    pub fn surface_images(&self) -> &BTreeMap<SourceHash, Arc<SurfaceImageAsset>> {
        &self.snapshot.surface_images
    }
}
#[derive(Debug, Clone)]
pub struct SaveSnapshot {
    pub manifest: ProjectManifest,
    pub snapshot: Arc<Snapshot>,
    pub(crate) recovered_previous: bool,
}
#[derive(Debug, Clone, PartialEq)]
struct Content {
    definitions: BTreeMap<DefinitionId, StoredDefinition>,
    occurrences: BTreeMap<OccurrenceId, OccurrenceRecord>,
    manufacturing_intent: Option<Arc<ManufacturingIntent>>,
    manufacturing_artifact: Option<ManufacturingArtifactRecord>,
    surface_overlays: Arc<SurfaceOverlays>,
    process_document: Arc<ProcessDocument>,
}
impl Content {
    fn matches(&self, snapshot: &Snapshot) -> bool {
        self.occurrences == snapshot.occurrences
            && self.manufacturing_intent == snapshot.manufacturing_intent
            && self.surface_overlays == snapshot.surface_overlays
            && self.process_document == snapshot.process_document
            && self.manufacturing_artifact.as_ref()
                == snapshot
                    .manufacturing_artifact
                    .as_ref()
                    .map(|asset| &asset.record)
            && self.definitions.len() == snapshot.sources.len()
            && self.definitions.iter().all(|(id, record)| {
                snapshot
                    .sources
                    .get(id)
                    .is_some_and(|source| source.record == *record)
            })
    }
    fn from_manifest(manifest: &ProjectManifest) -> Self {
        Self {
            definitions: manifest
                .definitions
                .iter()
                .map(|r| (r.definition_id.clone(), r.clone()))
                .collect(),
            occurrences: manifest
                .occurrences
                .iter()
                .map(|r| (r.occurrence_id, r.clone()))
                .collect(),
            manufacturing_intent: manifest.manufacturing_intent.clone(),
            manufacturing_artifact: manifest.manufacturing_artifact.clone(),
            surface_overlays: Arc::new(manifest.surface_overlays.clone()),
            process_document: Arc::new(manifest.process_document.clone()),
        }
    }
}
#[derive(Debug)]
pub struct Project {
    owner: Arc<()>,
    project_id: ProjectId,
    revision: ProjectRevision,
    next_occurrence: u32,
    next_surface_layer: u32,
    next_process_layer: u32,
    next_process_operation: u32,
    current: Arc<Snapshot>,
    undo: VecDeque<Arc<Snapshot>>,
    redo: Vec<Arc<Snapshot>>,
    staged_images: Mutex<Vec<Weak<Vec<u8>>>>,
    staged_process: Mutex<Vec<Weak<ProcessDocument>>>,
    saved: Option<(ProjectRevision, Content)>,
    storage: Option<Arc<Storage>>,
    recovered_previous: bool,
    save_uncertain: bool,
}
impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}
impl Project {
    pub fn new() -> Self {
        Self {
            owner: Arc::new(()),
            project_id: ProjectId::new(),
            revision: ProjectRevision::ZERO,
            next_occurrence: 1,
            next_surface_layer: 1,
            next_process_layer: 1,
            next_process_operation: 1,
            current: Arc::new(Snapshot::default()),
            undo: VecDeque::new(),
            redo: Vec::new(),
            staged_images: Mutex::new(Vec::new()),
            staged_process: Mutex::new(Vec::new()),
            saved: None,
            storage: None,
            recovered_previous: false,
            save_uncertain: false,
        }
    }
    pub fn from_loaded(loaded: LoadedProject) -> Result<Self, ProjectError> {
        loaded.manifest.validate()?;
        let snapshot = Snapshot {
            sources: loaded.sources,
            occurrences: loaded
                .manifest
                .occurrences
                .iter()
                .map(|r| (r.occurrence_id, r.clone()))
                .collect(),
            manufacturing_intent: loaded.manifest.manufacturing_intent.clone(),
            manufacturing_artifact: loaded.manufacturing_artifact,
            surface_overlays: Arc::new(loaded.manifest.surface_overlays.clone()),
            process_document: Arc::new(loaded.manifest.process_document.clone()),
            surface_images: loaded.surface_images,
        };
        validate_snapshot(&snapshot, &loaded.manifest)?;
        let mut project = Self::new();
        project.project_id = loaded.manifest.project_id.clone();
        project.revision = loaded.manifest.revision;
        project.next_occurrence = loaded.manifest.next_occurrence;
        project.next_surface_layer = loaded.manifest.next_surface_layer;
        project.next_process_layer = loaded.manifest.next_process_layer;
        project.next_process_operation = loaded.manifest.next_process_operation;
        project.saved = Some((
            loaded.saved_revision,
            Content::from_manifest(&loaded.manifest),
        ));
        project.current = Arc::new(snapshot);
        project.storage = Some(loaded.storage);
        project.recovered_previous = loaded.recovered_previous;
        Ok(project)
    }
    pub fn info(&self) -> ProjectInfo {
        let read_only = self.storage.as_ref().is_some_and(|s| s.read_only());
        let dirty = self.save_uncertain
            || self.recovered_previous
            || self.saved.as_ref().map_or(
                !self.current.occurrences.is_empty()
                    || self.current.manufacturing_intent.is_some()
                    || !self.current.surface_overlays.layers.is_empty()
                    || !self.current.process_document.layers.is_empty(),
                |(_, content)| !content.matches(&self.current),
            );
        ProjectInfo {
            project_id: self.project_id.clone(),
            revision: self.revision,
            saved_revision: self.saved.as_ref().map(|(r, _)| *r),
            path_label: self
                .storage
                .as_ref()
                .map(|s| s.path().to_string_lossy().into_owned()),
            dirty,
            read_only,
            recovered_previous: self.recovered_previous,
            save_uncertain: self.save_uncertain,
            can_undo: !read_only && !self.undo.is_empty(),
            can_redo: !read_only && !self.redo.is_empty(),
            surface_overlays: (*self.current.surface_overlays).clone(),
            process_document: (*self.current.process_document).clone(),
            resolved_process: self.current.manufacturing_intent.as_ref().map_or_else(
                Vec::new,
                |intent| {
                    self.current
                        .occurrences
                        .values()
                        .map(|occurrence| {
                            let global_fraction = intent.recipe.infill_fraction;
                            ResolvedOccurrenceProcess {
                                occurrence_id: occurrence.occurrence_id,
                                definition_id: occurrence.definition_id.clone(),
                                global_fraction,
                                inherited: self
                                    .current
                                    .process_document
                                    .resolve_definition(&occurrence.definition_id, global_fraction),
                                value: self.current.process_document.resolve(
                                    &occurrence.definition_id,
                                    occurrence.occurrence_id,
                                    global_fraction,
                                ),
                            }
                        })
                        .collect()
                },
            ),
        }
    }
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.current.clone()
    }
    pub fn surface_images(&self) -> &BTreeMap<SourceHash, Arc<SurfaceImageAsset>> {
        &self.current.surface_images
    }
    pub fn retained_surface_images(&self) -> Vec<Arc<SurfaceImageAsset>> {
        let mut images = BTreeMap::new();
        for snapshot in std::iter::once(&self.current)
            .chain(self.undo.iter())
            .chain(self.redo.iter())
        {
            for (hash, asset) in &snapshot.surface_images {
                images.entry(hash.clone()).or_insert_with(|| asset.clone());
            }
        }
        images.into_values().collect()
    }
    /// Includes live prepared candidates, even after their project revision becomes stale.
    pub fn retained_surface_image_bytes(&self) -> Result<usize, ProjectError> {
        let mut allocations = BTreeMap::new();
        for snapshot in std::iter::once(&self.current)
            .chain(self.undo.iter())
            .chain(self.redo.iter())
        {
            for asset in snapshot.surface_images.values() {
                allocations.insert(Arc::as_ptr(asset.bytes()), asset.bytes().capacity());
            }
        }
        let staged = self
            .staged_images
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "image staging lock poisoned"))?;
        for bytes in staged.iter().filter_map(Weak::upgrade) {
            allocations.insert(Arc::as_ptr(&bytes), bytes.capacity());
        }
        Ok(allocations.values().sum())
    }
    pub fn storage(&self) -> Option<Arc<Storage>> {
        self.storage.clone()
    }
    pub fn retained_sources(&self) -> Vec<Arc<SourceAsset>> {
        let mut sources = BTreeMap::new();
        for snapshot in std::iter::once(&self.current)
            .chain(self.undo.iter())
            .chain(self.redo.iter())
        {
            for (id, source) in &snapshot.sources {
                sources.entry(id.clone()).or_insert_with(|| source.clone());
            }
        }
        sources.into_values().collect()
    }
    pub fn retained_manufacturing_assets(&self) -> Vec<Arc<ManufacturingAsset>> {
        let mut assets = BTreeMap::new();
        for snapshot in std::iter::once(&self.current)
            .chain(self.undo.iter())
            .chain(self.redo.iter())
        {
            if let Some(asset) = &snapshot.manufacturing_artifact {
                assets
                    .entry(asset.record.hash.clone())
                    .or_insert_with(|| asset.clone());
            }
        }
        assets.into_values().collect()
    }
    /// Preserve live identity high-water on staged reopen of the same project.
    pub fn retain_high_water(&mut self, previous: &Project) -> Result<(), ProjectError> {
        if self.project_id == previous.project_id {
            self.next_occurrence = self.next_occurrence.max(previous.next_occurrence);
            self.next_surface_layer = self.next_surface_layer.max(previous.next_surface_layer);
            self.next_process_layer = self.next_process_layer.max(previous.next_process_layer);
            self.next_process_operation = self
                .next_process_operation
                .max(previous.next_process_operation);
            self.revision = self.revision.max(previous.revision);
            if previous.save_uncertain
                && self
                    .storage
                    .as_ref()
                    .zip(previous.storage.as_ref())
                    .is_some_and(|(a, b)| a.path() == b.path())
            {
                self.save_uncertain = true;
                self.saved = previous.saved.clone();
            }
        }
        Ok(())
    }
    fn writable(&self) -> Result<(), ProjectError> {
        if self.storage.as_ref().is_some_and(|s| s.read_only()) {
            return Err(error(
                ProjectErrorCode::ReadOnly,
                "read-only project cannot be authored or saved",
            ));
        }
        Ok(())
    }
    pub fn prepare(&self, edit: Edit) -> Result<PreparedEdit, ProjectError> {
        self.writable()?;
        self.revision.next()?;
        let mut snapshot = (*self.current).clone();
        let mut next_occurrence = self.next_occurrence;
        let mut next_surface_layer = self.next_surface_layer;
        let mut next_process_layer = self.next_process_layer;
        let mut next_process_operation = self.next_process_operation;
        let incoming_image = match &edit {
            Edit::AddImageSurfaceFinish { asset, .. } => Some(asset.bytes().clone()),
            _ => None,
        };
        // Invalidation is part of the same atomic edit; history keeps the coherent old pair.
        if !matches!(&edit, Edit::PublishManufacturing { .. }) {
            snapshot.manufacturing_artifact = None;
        }
        match edit {
            Edit::Process { edit } => {
                if let ProcessEdit::AddLayer { scope, .. }
                | ProcessEdit::SetSparseFraction { scope, .. }
                | ProcessEdit::Reset { scope } = &edit
                {
                    let valid = match scope {
                        SurfaceScope::Definition { definition_id } => {
                            snapshot.sources.contains_key(definition_id)
                        }
                        SurfaceScope::Occurrence {
                            occurrence_id,
                            expected_definition_id,
                        } => snapshot
                            .occurrences
                            .get(occurrence_id)
                            .is_some_and(|record| &record.definition_id == expected_definition_id),
                    };
                    if !valid {
                        return Err(invalid("unknown or inconsistent process source scope"));
                    }
                }
                apply_process_edit(
                    Arc::make_mut(&mut snapshot.process_document),
                    edit,
                    &mut next_process_layer,
                    &mut next_process_operation,
                )?;
            }
            Edit::Import { mut source, pose } => {
                source.validate()?;
                if let Some(retained) = std::iter::once(&self.current)
                    .chain(self.undo.iter())
                    .chain(self.redo.iter())
                    .find_map(|snapshot| snapshot.sources.get(&source.record.definition_id))
                {
                    let old = &retained.record.provenance;
                    let new = &source.record.provenance;
                    if old.source_hash != new.source_hash
                        || old.source_unit != new.source_unit
                        || old.uncertainty_mm != new.uncertainty_mm
                        || (!Arc::ptr_eq(&retained.bytes, &source.bytes)
                            && retained.bytes.as_slice() != source.bytes.as_slice())
                    {
                        return Err(invalid(
                            "source identity has conflicting native provenance or bytes",
                        ));
                    }
                    source = retained.clone();
                }
                let id = source.record.definition_id.clone();
                snapshot.sources.entry(id.clone()).or_insert(source);
                add_occurrence(&mut snapshot, id, pose, &mut next_occurrence)?;
            }
            Edit::Add {
                definition_id,
                pose,
            } => {
                if !snapshot.sources.contains_key(&definition_id) {
                    return Err(invalid("unknown definition"));
                }
                add_occurrence(&mut snapshot, definition_id, pose, &mut next_occurrence)?;
            }
            Edit::SetPose {
                occurrence_id,
                pose,
            } => {
                pose.validate().map_err(|e| invalid(e.message))?;
                snapshot
                    .occurrences
                    .get_mut(&occurrence_id)
                    .ok_or_else(|| invalid("unknown occurrence"))?
                    .pose = pose;
            }
            Edit::Remove { occurrence_id } => {
                let removed = snapshot
                    .occurrences
                    .remove(&occurrence_id)
                    .ok_or_else(|| invalid("unknown occurrence"))?;
                if !snapshot
                    .occurrences
                    .values()
                    .any(|r| r.definition_id == removed.definition_id)
                {
                    snapshot.sources.remove(&removed.definition_id);
                }
                let keep = |layer: &SurfaceLayer| match &layer.scope {
                    SurfaceScope::Occurrence {
                        occurrence_id: id, ..
                    } => *id != occurrence_id,
                    SurfaceScope::Definition { definition_id } => {
                        snapshot.sources.contains_key(definition_id)
                    }
                };
                if snapshot
                    .surface_overlays
                    .layers
                    .iter()
                    .any(|layer| !keep(layer))
                {
                    Arc::make_mut(&mut snapshot.surface_overlays)
                        .layers
                        .retain(keep);
                }
                if snapshot
                    .process_document
                    .layers
                    .iter()
                    .any(|layer| match &layer.scope {
                        SurfaceScope::Occurrence {
                            occurrence_id: id, ..
                        } => *id == occurrence_id,
                        SurfaceScope::Definition { definition_id } => {
                            !snapshot.sources.contains_key(definition_id)
                        }
                    })
                {
                    Arc::make_mut(&mut snapshot.process_document).layers.retain(
                        |layer| match &layer.scope {
                            SurfaceScope::Occurrence {
                                occurrence_id: id, ..
                            } => *id != occurrence_id,
                            SurfaceScope::Definition { definition_id } => {
                                snapshot.sources.contains_key(definition_id)
                            }
                        },
                    );
                }
            }
            Edit::SetManufacturingIntent { intent } => {
                intent.validate().map_err(|e| {
                    if e.code == ManufacturingErrorCode::ResourceLimit {
                        error(ProjectErrorCode::ResourceLimit, e.message)
                    } else {
                        invalid(e.message)
                    }
                })?;
                snapshot.manufacturing_intent = Some(intent);
            }
            Edit::PublishManufacturing { mut asset } => {
                let bundle = asset.validate()?;
                if bundle.provenance.input_revision != self.revision {
                    return Err(error(
                        ProjectErrorCode::StaleRevision,
                        "compiled input revision is stale",
                    ));
                }
                validate_manufacturing_input(&bundle, &snapshot, &self.project_id, self.revision)?;
                if let Some(retained) = std::iter::once(&self.current)
                    .chain(self.undo.iter())
                    .chain(self.redo.iter())
                    .filter_map(|s| s.manufacturing_artifact.as_ref())
                    .find(|old| old.record.hash == asset.record.hash)
                {
                    if retained.record != asset.record
                        || retained.bytes.as_slice() != asset.bytes.as_slice()
                    {
                        return Err(invalid("conflicting retained manufacturing identity"));
                    }
                    asset = retained.clone();
                }
                snapshot.manufacturing_artifact = Some(asset);
            }
            Edit::AddSurfaceFinish {
                scope,
                face_id,
                finish,
            } => {
                add_surface_layer(
                    &mut snapshot,
                    scope,
                    face_id,
                    finish,
                    &mut next_surface_layer,
                )?;
            }
            Edit::AddImageSurfaceFinish {
                mut asset,
                scope,
                face_id,
                parameters,
            } => {
                parameters.validate()?;
                if let Some(retained) = std::iter::once(&self.current)
                    .chain(self.undo.iter())
                    .chain(self.redo.iter())
                    .find_map(|snapshot| snapshot.surface_images.get(&asset.record.image_hash))
                {
                    if !Arc::ptr_eq(retained.bytes(), asset.bytes())
                        && retained.bytes().as_slice() != asset.bytes().as_slice()
                    {
                        return Err(error(
                            ProjectErrorCode::CorruptAsset,
                            "conflicting image content identity",
                        ));
                    }
                    // Canonical content owns complete first-admission provenance.
                    asset = retained.clone();
                }
                let image_hash = asset.record.image_hash.clone();
                snapshot
                    .surface_images
                    .entry(image_hash.clone())
                    .or_insert(asset);
                add_surface_layer(
                    &mut snapshot,
                    scope,
                    face_id,
                    SurfaceFinish::ImageRelief {
                        image_hash,
                        parameters,
                    },
                    &mut next_surface_layer,
                )?;
            }
            Edit::RemoveSurfaceFinish { layer_id } => {
                let index = snapshot
                    .surface_overlays
                    .layers
                    .iter()
                    .position(|layer| layer.layer_id == layer_id)
                    .ok_or_else(|| invalid("unknown surface layer"))?;
                Arc::make_mut(&mut snapshot.surface_overlays)
                    .layers
                    .remove(index);
            }
            Edit::SetSurfaceFinishEnabled { layer_id, enabled } => {
                let index = snapshot
                    .surface_overlays
                    .layers
                    .iter()
                    .position(|layer| layer.layer_id == layer_id)
                    .ok_or_else(|| invalid("unknown surface layer"))?;
                Arc::make_mut(&mut snapshot.surface_overlays).layers[index].enabled = enabled;
            }
        }
        synchronize_surface_images(&mut snapshot)?;
        snapshot.surface_overlays.validate()?;
        for layer in &snapshot.surface_overlays.layers {
            let target_exists = match &layer.scope {
                SurfaceScope::Definition { definition_id } => {
                    snapshot.sources.contains_key(definition_id)
                }
                SurfaceScope::Occurrence {
                    occurrence_id,
                    expected_definition_id,
                } => snapshot
                    .occurrences
                    .get(occurrence_id)
                    .is_some_and(|record| &record.definition_id == expected_definition_id),
            };
            if !target_exists || layer.layer_id.get() >= next_surface_layer {
                return Err(invalid("unknown surface target or invalid layer allocator"));
            }
        }
        snapshot.process_document.validate()?;
        for layer in &snapshot.process_document.layers {
            let valid = match &layer.scope {
                SurfaceScope::Definition { definition_id } => {
                    snapshot.sources.contains_key(definition_id)
                }
                SurfaceScope::Occurrence {
                    occurrence_id,
                    expected_definition_id,
                } => snapshot
                    .occurrences
                    .get(occurrence_id)
                    .is_some_and(|record| &record.definition_id == expected_definition_id),
            };
            if !valid
                || layer.layer_id.get() >= next_process_layer
                || layer
                    .operations
                    .iter()
                    .any(|operation| operation.operation_id.get() >= next_process_operation)
            {
                return Err(invalid(
                    "unknown process target or invalid process allocator",
                ));
            }
        }
        let skip = usize::from(self.undo.len() == MAX_PROJECT_HISTORY);
        validate_pins(
            std::iter::once(&snapshot)
                .chain(std::iter::once(self.current.as_ref()))
                .chain(self.undo.iter().skip(skip).map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref)),
        )?;
        let mut staged_process = self
            .staged_process
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "process staging lock poisoned"))?;
        staged_process.retain(|doc| doc.strong_count() != 0);
        let mut process_allocations = BTreeMap::new();
        for state in std::iter::once(&snapshot)
            .chain(std::iter::once(self.current.as_ref()))
            .chain(self.undo.iter().map(Arc::as_ref))
            .chain(self.redo.iter().map(Arc::as_ref))
        {
            process_allocations.insert(
                Arc::as_ptr(&state.process_document),
                state.process_document.allocated_bytes(),
            );
        }
        for doc in staged_process.iter().filter_map(Weak::upgrade) {
            process_allocations.insert(Arc::as_ptr(&doc), doc.allocated_bytes());
        }
        if process_allocations.values().sum::<usize>() > MAX_PROCESS_RETAINED_BYTES {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained/staged process allocation budget exceeded",
            ));
        }
        if !staged_process
            .iter()
            .any(|doc| doc.as_ptr() == Arc::as_ptr(&snapshot.process_document))
        {
            staged_process.push(Arc::downgrade(&snapshot.process_document));
        }
        let mut staged = self
            .staged_images
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "image staging lock poisoned"))?;
        staged.retain(|bytes| bytes.strong_count() != 0);
        let mut allocations = BTreeMap::new();
        for state in std::iter::once(&snapshot)
            .chain(std::iter::once(self.current.as_ref()))
            .chain(self.undo.iter().map(Arc::as_ref))
            .chain(self.redo.iter().map(Arc::as_ref))
        {
            for asset in state.surface_images.values() {
                allocations.insert(Arc::as_ptr(asset.bytes()), asset.bytes().capacity());
            }
        }
        for bytes in staged
            .iter()
            .filter_map(Weak::upgrade)
            .chain(incoming_image.iter().cloned())
        {
            allocations.insert(Arc::as_ptr(&bytes), bytes.capacity());
        }
        if allocations.values().sum::<usize>() > MAX_SURFACE_IMAGE_RETAINED_BYTES {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained/staged image allocation budget exceeded",
            ));
        }
        for asset in snapshot.surface_images.values() {
            if !staged
                .iter()
                .any(|bytes| bytes.as_ptr() == Arc::as_ptr(asset.bytes()))
            {
                staged.push(Arc::downgrade(asset.bytes()));
            }
        }
        if let Some(bytes) = incoming_image
            && !staged.iter().any(|old| old.as_ptr() == Arc::as_ptr(&bytes))
        {
            staged.push(Arc::downgrade(&bytes));
        }
        Ok(PreparedEdit {
            owner: self.owner.clone(),
            revision: self.revision,
            snapshot: Arc::new(snapshot),
            next_occurrence,
            next_surface_layer,
            next_process_layer,
            next_process_operation,
        })
    }
    pub fn commit(&mut self, edit: PreparedEdit) -> Result<(), ProjectError> {
        self.writable()?;
        if !Arc::ptr_eq(&self.owner, &edit.owner) || self.revision != edit.revision {
            return Err(error(
                ProjectErrorCode::StaleRevision,
                "prepared edit no longer belongs to the current project revision",
            ));
        }
        let revision = self.revision.next()?;
        if self.undo.len() == MAX_PROJECT_HISTORY {
            self.undo.pop_front();
        }
        self.undo.push_back(self.current.clone());
        self.redo.clear();
        self.current = edit.snapshot;
        self.next_occurrence = edit.next_occurrence;
        self.next_surface_layer = edit.next_surface_layer;
        self.next_process_layer = edit.next_process_layer;
        self.next_process_operation = edit.next_process_operation;
        self.revision = revision;
        Ok(())
    }
    pub fn undo(&mut self) -> Result<(), ProjectError> {
        self.writable()?;
        if self.undo.is_empty() {
            return Err(error(ProjectErrorCode::NoUndo, "no transaction to undo"));
        }
        let revision = self.revision.next()?;
        self.redo.push(self.current.clone());
        self.current = self.undo.pop_back().expect("checked history");
        self.revision = revision;
        Ok(())
    }
    pub fn redo(&mut self) -> Result<(), ProjectError> {
        self.writable()?;
        if self.redo.is_empty() {
            return Err(error(ProjectErrorCode::NoRedo, "no transaction to redo"));
        }
        let revision = self.revision.next()?;
        self.undo.push_back(self.current.clone());
        self.current = self.redo.pop().expect("checked history");
        self.revision = revision;
        Ok(())
    }
    pub fn capture(&self) -> SaveSnapshot {
        SaveSnapshot {
            manifest: ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: self.project_id.clone(),
                revision: self.revision,
                units: ProjectUnits::Millimetres,
                frame: ProjectFrame::RightHanded,
                next_occurrence: self.next_occurrence,
                next_surface_layer: self.next_surface_layer,
                surface_overlays: (*self.current.surface_overlays).clone(),
                process_document: (*self.current.process_document).clone(),
                next_process_layer: self.next_process_layer,
                next_process_operation: self.next_process_operation,
                definitions: self
                    .current
                    .sources
                    .values()
                    .map(|s| s.record.clone())
                    .collect(),
                occurrences: self.current.occurrences.values().cloned().collect(),
                manufacturing_intent: self.current.manufacturing_intent.clone(),
                manufacturing_artifact: self
                    .current
                    .manufacturing_artifact
                    .as_ref()
                    .map(|a| a.record.clone()),
            },
            snapshot: self.current.clone(),
            recovered_previous: self.recovered_previous,
        }
    }
    pub fn mark_saved(&mut self, receipt: SaveReceipt) -> Result<(), ProjectError> {
        self.writable()?;
        receipt.manifest.validate()?;
        if receipt.storage.read_only() {
            return Err(error(
                ProjectErrorCode::ReadOnly,
                "save receipt must own writer storage",
            ));
        }
        if receipt.manifest.project_id != self.project_id
            || receipt.manifest.revision > self.revision
            || receipt.manifest.next_occurrence > self.next_occurrence
            || receipt.manifest.next_surface_layer > self.next_surface_layer
            || receipt.manifest.next_process_layer > self.next_process_layer
            || receipt.manifest.next_process_operation > self.next_process_operation
        {
            return Err(error(
                ProjectErrorCode::StaleRevision,
                "save receipt does not belong to this project",
            ));
        }
        self.save_uncertain = receipt.durability_error.is_some();
        if !self.save_uncertain {
            self.saved = Some((
                receipt.manifest.revision,
                Content::from_manifest(&receipt.manifest),
            ));
            self.recovered_previous = false;
        } else if self
            .storage
            .as_ref()
            .is_none_or(|storage| storage.path() != receipt.storage.path())
        {
            self.saved = None;
        }
        self.storage = Some(receipt.storage);
        Ok(())
    }
}
fn add_occurrence(
    snapshot: &mut Snapshot,
    definition_id: DefinitionId,
    pose: RigidPoseMm,
    next: &mut u32,
) -> Result<(), ProjectError> {
    pose.validate().map_err(|e| invalid(e.message))?;
    if snapshot.occurrences.len() >= MAX_OCCURRENCES as usize {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "occurrence budget exceeded",
        ));
    }
    let id = OccurrenceId::new(*next).map_err(|e| invalid(e.message))?;
    *next = next.checked_add(1).ok_or_else(|| {
        error(
            ProjectErrorCode::ResourceLimit,
            "occurrence allocator exhausted",
        )
    })?;
    snapshot.occurrences.insert(
        id,
        OccurrenceRecord {
            occurrence_id: id,
            definition_id,
            pose,
        },
    );
    Ok(())
}
fn add_surface_layer(
    snapshot: &mut Snapshot,
    scope: SurfaceScope,
    face_id: FaceId,
    finish: SurfaceFinish,
    next: &mut u32,
) -> Result<(), ProjectError> {
    finish.validate()?;
    if let SurfaceFinish::ImageRelief { image_hash, .. } = &finish
        && !snapshot.surface_images.contains_key(image_hash)
    {
        return Err(invalid("image finish references unknown current image"));
    }
    if snapshot.surface_overlays.layers.len() == MAX_SURFACE_LAYERS {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "surface layer budget exceeded",
        ));
    }
    let layer_id = SurfaceLayerId::new(*next)?;
    *next = next.checked_add(1).ok_or_else(|| {
        error(
            ProjectErrorCode::ResourceLimit,
            "surface layer allocator exhausted",
        )
    })?;
    let doc = Arc::make_mut(&mut snapshot.surface_overlays);
    doc.layers.reserve_exact(1);
    doc.layers.push(SurfaceLayer {
        layer_id,
        enabled: true,
        scope,
        face_id,
        finish,
    });
    Ok(())
}
fn synchronize_surface_images(snapshot: &mut Snapshot) -> Result<(), ProjectError> {
    let layers = &snapshot.surface_overlays.layers;
    snapshot.surface_images.retain(|hash, _| {
        layers.iter().any(|layer| {
        matches!(&layer.finish, SurfaceFinish::ImageRelief { image_hash, .. } if image_hash == hash)
    })
    });
    validate_surface_image_budget(snapshot)?;
    if snapshot.surface_images.len() != snapshot.surface_overlays.images.len()
        || snapshot
            .surface_images
            .values()
            .zip(&snapshot.surface_overlays.images)
            .any(|(asset, record)| asset.record() != record)
    {
        let doc = Arc::make_mut(&mut snapshot.surface_overlays);
        doc.images
            .retain(|record| snapshot.surface_images.contains_key(&record.image_hash));
        for (hash, asset) in &snapshot.surface_images {
            if let Err(index) = doc
                .images
                .binary_search_by(|record| record.image_hash.cmp(hash))
            {
                doc.images.reserve_exact(1);
                doc.images.insert(index, asset.record.clone());
            }
        }
    }
    Ok(())
}
fn validate_surface_images(snapshot: &Snapshot) -> Result<(), ProjectError> {
    if snapshot.surface_images.len() != snapshot.surface_overlays.images.len() {
        return Err(error(
            ProjectErrorCode::CorruptAsset,
            "image metadata and captured assets disagree",
        ));
    }
    for ((hash, asset), record) in snapshot
        .surface_images
        .iter()
        .zip(&snapshot.surface_overlays.images)
    {
        if hash != &record.image_hash || asset.record() != record {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "image record differs from captured content",
            ));
        }
    }
    validate_surface_image_budget(snapshot)
}
fn validate_surface_image_budget(snapshot: &Snapshot) -> Result<(), ProjectError> {
    if snapshot.surface_images.len() > MAX_SURFACE_IMAGES
        || snapshot
            .surface_images
            .values()
            .map(|asset| asset.bytes().len())
            .sum::<usize>()
            > MAX_SURFACE_IMAGE_TOTAL_BYTES
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "live image budget exceeded",
        ));
    }
    Ok(())
}
fn validate_pins<'a>(snapshots: impl Iterator<Item = &'a Snapshot>) -> Result<(), ProjectError> {
    let mut sources: BTreeMap<&DefinitionId, &Arc<SourceAsset>> = BTreeMap::new();
    let mut manufacturing: BTreeMap<&SourceHash, &Arc<ManufacturingAsset>> = BTreeMap::new();
    let mut surface = BTreeMap::new();
    let mut images = BTreeMap::new();
    let mut process = BTreeMap::new();
    let mut image_allocations = BTreeMap::new();
    for snapshot in snapshots {
        validate_surface_images(snapshot)?;
        process
            .entry(Arc::as_ptr(&snapshot.process_document))
            .or_insert(snapshot.process_document.as_ref());
        for (hash, asset) in &snapshot.surface_images {
            if let Some(old) = images.insert(hash, asset)
                && old.record() != asset.record()
            {
                return Err(error(
                    ProjectErrorCode::CorruptAsset,
                    "conflicting retained image provenance",
                ));
            }
            image_allocations.insert(Arc::as_ptr(asset.bytes()), asset.bytes().capacity());
        }
        if let std::collections::btree_map::Entry::Vacant(entry) =
            surface.entry(Arc::as_ptr(&snapshot.surface_overlays))
        {
            snapshot.surface_overlays.validate()?;
            entry.insert(snapshot.surface_overlays.as_ref());
        }
        for (id, asset) in &snapshot.sources {
            if let Some(old) = sources.insert(id, asset)
                && !Arc::ptr_eq(old, asset)
                && (old.record != asset.record
                    || (!Arc::ptr_eq(&old.bytes, &asset.bytes)
                        && old.bytes.as_slice() != asset.bytes.as_slice()))
            {
                return Err(invalid("conflicting retained source identity"));
            }
        }
        if let Some(asset) = &snapshot.manufacturing_artifact
            && let Some(old) = manufacturing.insert(&asset.record.hash, asset)
            && !Arc::ptr_eq(old, asset)
            && (old.record != asset.record || old.bytes.as_slice() != asset.bytes.as_slice())
        {
            return Err(invalid("conflicting retained manufacturing identity"));
        }
    }
    if image_allocations.values().sum::<usize>() > MAX_SURFACE_IMAGE_RETAINED_BYTES {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained image allocation budget exceeded",
        ));
    }
    if sources.len() > MAX_DEFINITIONS as usize
        || sources.values().map(|s| s.bytes.len()).sum::<usize>() > MAX_RETAINED_SOURCE_BYTES
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained source budget exceeded",
        ));
    }
    if manufacturing
        .values()
        .map(|a| a.bytes.capacity())
        .sum::<usize>()
        > MAX_MANUFACTURING_RETAINED_BYTES as usize
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained manufacturing budget exceeded",
        ));
    }
    if surface
        .values()
        .map(|doc| doc.allocated_bytes())
        .sum::<usize>()
        > MAX_SURFACE_RETAINED_BYTES
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained surface document budget exceeded",
        ));
    }
    if process
        .values()
        .map(|doc| doc.allocated_bytes())
        .sum::<usize>()
        > MAX_PROCESS_RETAINED_BYTES
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained process document budget exceeded",
        ));
    }
    Ok(())
}
pub(crate) fn validate_snapshot(
    snapshot: &Snapshot,
    manifest: &ProjectManifest,
) -> Result<(), ProjectError> {
    manifest.validate()?;
    if !Content::from_manifest(manifest).matches(snapshot)
        || snapshot.sources.len() != manifest.definitions.len()
        || snapshot.occurrences.len() != manifest.occurrences.len()
    {
        return Err(invalid("captured snapshot does not match manifest"));
    }
    for (id, source) in &snapshot.sources {
        if *id != source.record.definition_id {
            return Err(invalid("source map key differs from identity"));
        }
        source.validate()?;
    }
    if let Some(asset) = &snapshot.manufacturing_artifact {
        let bundle = asset.validate()?;
        validate_manufacturing_input(&bundle, snapshot, &manifest.project_id, manifest.revision)?;
    }
    validate_pins(std::iter::once(snapshot))
}
fn validate_manufacturing_input(
    bundle: &ManufacturingBundle,
    snapshot: &Snapshot,
    project_id: &ProjectId,
    revision: ProjectRevision,
) -> Result<(), ProjectError> {
    let provenance = &bundle.provenance;
    let definitions: Vec<_> = snapshot
        .sources
        .values()
        .map(|a| a.record.clone())
        .collect();
    let occurrences: Vec<_> = snapshot.occurrences.values().cloned().collect();
    let intent = snapshot
        .manufacturing_intent
        .as_deref()
        .ok_or_else(|| invalid("manufacturing artifact has no current intent"))?;
    let input_hash = input_fingerprint(
        project_id,
        &definitions,
        &occurrences,
        intent,
        &snapshot.surface_overlays,
        &snapshot.process_document,
    )
    .map_err(manufacturing_error)?;
    if &provenance.project_id != project_id
        || provenance.input_revision > revision
        || provenance.input_hash != input_hash
        || &provenance.intent != intent
        || provenance.surface_overlays != *snapshot.surface_overlays
        || provenance.process_document != *snapshot.process_document
        || provenance.definitions.len() != definitions.len()
        || provenance.occurrences.len() != occurrences.len()
        || provenance.surface_images.len() != snapshot.surface_images.len()
        || provenance.surface_images.iter().any(|content| {
            snapshot
                .surface_images
                .get(&content.image_hash)
                .is_none_or(|asset| content.bytes.as_slice() != asset.bytes().as_slice())
        })
        || provenance.definitions.iter().any(|r| {
            snapshot
                .sources
                .get(&r.definition_id)
                .is_none_or(|a| a.record != *r)
        })
        || provenance
            .occurrences
            .iter()
            .any(|r| snapshot.occurrences.get(&r.occurrence_id) != Some(r))
    {
        return Err(error(
            ProjectErrorCode::CorruptAsset,
            "manufacturing provenance differs from current project input",
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests;
