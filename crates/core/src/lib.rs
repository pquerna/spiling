// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Runtime-independent authoring authority. Native caches are engine-owned.
mod analysis_results;
mod hardware;
pub use analysis_results::{AnalysisResultAsset, MAX_RETAINED_ANALYSIS_FIELD_BYTES};
mod print_layouts;
pub use print_layouts::PrintLayoutSnapshot;
mod placement;
mod routed_placement;
pub use placement::PlacementDraft;
mod interior_policy;
mod manufacturing_options;
pub use interior_policy::{interior_policy_opportunity, interior_policy_trial};
pub use manufacturing_options::{context_hash, trial_snapshot};
pub mod materials;
mod optical_construction;
mod process;
mod recipes;
pub use optical_construction::{OpticalConstructionCandidate, prepare_optical_construction};
pub mod library;
pub mod storage;
use process::apply_process_edit;
use sha2::{Digest, Sha256};
use spiling_contracts::{
    appearance::*, geometry::*, manufacturing::*, optical::*, process::*, project::*, surface::*,
};
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
pub const MAX_APPEARANCE_RETAINED_BYTES: usize = 64 * 1024 * 1024;

/// Immutable exact imported bytes and separately content-addressed canonical JSON.
#[derive(Debug)]
pub struct PaletteAsset {
    record: PaletteRecord,
    original: Arc<Vec<u8>>,
    canonical: Arc<Vec<u8>>,
    prepared: Arc<PreparedPalette>,
}
impl PaletteAsset {
    pub fn new(
        record: PaletteRecord,
        original: Vec<u8>,
        canonical: Vec<u8>,
    ) -> Result<Self, ProjectError> {
        record.validate_hashes(&original, &canonical)?;
        let remaining = MAX_APPEARANCE_RETAINED_BYTES
            .saturating_sub(original.capacity())
            .saturating_sub(canonical.capacity())
            .saturating_sub(record.allocated_bytes());
        let prepared = PreparedPalette::decode_bounded(&original, &canonical, remaining)?;
        Self::from_checked_prepared(record, original, canonical, prepared)
    }
    pub fn from_document(
        record: PaletteRecord,
        original: Vec<u8>,
        canonical: Vec<u8>,
        document: PaletteDocument,
    ) -> Result<Self, ProjectError> {
        record.validate_hashes(&original, &canonical)?;
        let prepared = PreparedPalette::new(document, &canonical)?;
        Self::from_checked_prepared(record, original, canonical, prepared)
    }
    fn from_checked_prepared(
        record: PaletteRecord,
        original: Vec<u8>,
        canonical: Vec<u8>,
        prepared: PreparedPalette,
    ) -> Result<Self, ProjectError> {
        record.validate_provenance(prepared.document())?;
        if prepared.canonical_hash() != &record.canonical_hash {
            return Err(invalid("palette prepared identity differs from record"));
        }
        Ok(Self {
            record,
            original: Arc::new(original),
            canonical: Arc::new(canonical),
            prepared: Arc::new(prepared),
        })
    }
    pub fn record(&self) -> &PaletteRecord {
        &self.record
    }
    pub fn original(&self) -> &Arc<Vec<u8>> {
        &self.original
    }
    pub fn canonical(&self) -> &Arc<Vec<u8>> {
        &self.canonical
    }
    pub fn document(&self) -> &PaletteDocument {
        self.prepared.document()
    }
    pub fn prepared(&self) -> &PreparedPalette {
        &self.prepared
    }
}

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
        Self::shared(record, Arc::new(bytes))
    }
    /// Another definition of the same source bytes, sharing their allocation.
    pub fn shared(record: StoredDefinition, bytes: Arc<Vec<u8>>) -> Result<Self, ProjectError> {
        let asset = Self { record, bytes };
        asset.validate()?;
        Ok(asset)
    }
    pub(crate) fn validate(&self) -> Result<(), ProjectError> {
        if self.bytes.is_empty() || self.bytes.len() > MAX_SOURCE_BYTES as usize {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "source asset is empty or exceeds the immutable object representation",
            ));
        }
        self.record
            .provenance
            .validate()
            .map_err(|e| invalid(e.message))?;
        let digest: [u8; 32] = Sha256::digest(self.bytes.as_slice()).into();
        if SourceHash::from_digest(&digest) != self.record.provenance.source_hash
            || DefinitionId::for_source(&digest, self.record.provenance.source_solid.as_ref())
                != self.record.definition_id
        {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "source SHA-256 or definition identity mismatch",
            ));
        }
        Ok(())
    }
}
/// Immutable exact JSON bundle bytes plus the exact emitted program they describe.
/// Persisted verification is metadata, not replay proof. The exact bundle
/// JSON and the program are both held as compressed frames and stored as
/// their own raw project objects.
#[derive(Debug, Clone)]
pub struct ManufacturingAsset {
    pub record: ManufacturingArtifactRecord,
    pub bundle: spiling_chunks::ChunkSource,
    pub program: spiling_chunks::ChunkSource,
}
impl ManufacturingAsset {
    pub fn new(
        record: ManufacturingArtifactRecord,
        bundle: spiling_chunks::ChunkSource,
        program: spiling_chunks::ChunkSource,
    ) -> Result<Self, ProjectError> {
        let asset = Self {
            record,
            bundle,
            program,
        };
        asset.validate()?;
        Ok(asset)
    }
    /// Frame bundle bytes; for callers holding a whole encoded bundle.
    pub fn from_bytes(
        record: ManufacturingArtifactRecord,
        bundle: &[u8],
        program: spiling_chunks::ChunkSource,
    ) -> Result<Self, ProjectError> {
        let bundle = spiling_chunks::ChunkSource::from_slice(
            bundle,
            u64::from(MAX_MANUFACTURING_BUNDLE_BYTES),
        )
        .map_err(|_| {
            error(
                ProjectErrorCode::ResourceLimit,
                "manufacturing bundle exceeds byte budget",
            )
        })?;
        Self::new(record, bundle, program)
    }
    /// Decode the exact bundle document from its frames.
    pub fn decode(&self) -> Result<ManufacturingBundle, ManufacturingError> {
        decode_bundle_from(self.bundle.len(), || self.bundle.reader())
    }
    /// Heap retained by this asset's compressed bundle and program frames.
    pub fn retained_bytes(&self) -> usize {
        self.bundle.retained_bytes() + self.program.retained_bytes()
    }
    /// Same immutable identity and exact content.
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        self.record == other.record && self.bundle == other.bundle && self.program == other.program
    }
    pub(crate) fn validate(&self) -> Result<ManufacturingBundle, ProjectError> {
        if self.bundle.is_empty() || self.bundle.len() > u64::from(MAX_MANUFACTURING_BUNDLE_BYTES) {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "manufacturing bundle exceeds byte budget",
            ));
        }
        self.record.validate().map_err(manufacturing_error)?;
        if SourceHash::from_digest(self.bundle.digest()) != self.record.hash
            || self.bundle.len() != u64::from(self.record.byte_count)
        {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "manufacturing hash or byte count mismatch",
            ));
        }
        if !self
            .record
            .program
            .matches(self.program.len(), self.program.digest())
        {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "manufacturing program hash or byte count mismatch",
            ));
        }
        let bundle = self.decode().map_err(manufacturing_error)?;
        if bundle.provenance.input_hash != self.record.input_hash
            || bundle.summary() != self.record.summary
            || bundle.program != self.record.program
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
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub focused_layout: PrintLayoutId,
    pub layouts: Vec<PrintLayoutSnapshot>,
    pub sources: BTreeMap<DefinitionId, Arc<SourceAsset>>,
    pub surface_overlays: Arc<SurfaceOverlays>,
    pub process_document: Arc<ProcessDocument>,
    pub surface_images: BTreeMap<SourceHash, Arc<SurfaceImageAsset>>,
    pub appearance_study: Arc<AppearanceStudy>,
    pub analysis: Arc<spiling_contracts::analysis::AnalysisDocument>,
    pub analysis_results: BTreeMap<SourceHash, Arc<AnalysisResultAsset>>,
    pub materials: Option<Arc<materials::MaterialAsset>>,
    pub appearance_palettes: BTreeMap<SourceHash, Arc<PaletteAsset>>,
}
#[derive(Debug, Clone)]
pub enum Edit {
    AttachAnalysisResult {
        asset: Arc<AnalysisResultAsset>,
    },
    Analysis {
        edit: spiling_contracts::analysis::AnalysisEdit,
    },
    PrintLayout {
        edit: PrintLayoutEdit,
    },
    SetHardware {
        capture: Option<Arc<spiling_contracts::hardware::ProjectHardwareCapture>>,
    },
    PutRecipe {
        transfer: spiling_contracts::library::RecipeTransfer,
        expected: Option<SourceHash>,
    },
    LoadRecipeStudy {
        reference: spiling_contracts::library::RecipeRef,
        context: spiling_contracts::library::RecipeStudyContext,
    },
    CaptureRecipe {
        id: spiling_contracts::library::RecipeId,
        name: String,
        ingredients: Vec<spiling_contracts::library::RecipeIngredient>,
        expected: Option<SourceHash>,
    },
    PutFilament {
        revision: spiling_contracts::library::FilamentRevision,
        origin: Option<spiling_contracts::library::LibraryOrigin>,
        expected: Option<SourceHash>,
    },
    ApplyOpticalConstruction {
        candidate: OpticalConstructionCandidate,
    },
    ImportAppearancePalette {
        asset: Arc<PaletteAsset>,
    },
    SetAppearanceTarget {
        target: AppearanceTarget,
        limits: SearchLimits,
    },
    AcceptAppearanceReport {
        report: Arc<AppearanceReport>,
        selection: Option<String>,
    },
    SetOpticalStudy {
        request: OpticalRequest,
    },
    AcceptOpticalReport {
        report: Arc<OpticalReport>,
        selection: Option<OpticalSelection>,
    },
    Process {
        edit: ProcessEdit,
    },
    Import {
        source: Arc<SourceAsset>,
        pose: RigidPoseMm,
    },
    /// Every placed solid of one source, as one transaction: each part is a
    /// definition and the pose of one occurrence of it.
    ImportParts {
        parts: Vec<(Arc<SourceAsset>, RigidPoseMm)>,
    },
    Add {
        definition_id: DefinitionId,
        pose: RigidPoseMm,
    },
    ApplyPlacement {
        placements: Vec<LayoutPlacement>,
        additional: Vec<Vec<LayoutPlacement>>,
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
    SetInteriorToolPolicy {
        edit: InteriorToolPolicyEdit,
    },
    RetainManufacturingOption {
        option: ManufacturingOption,
    },
    RemoveManufacturingOption {
        id: SourceHash,
    },
    ApplyManufacturingOption {
        id: SourceHash,
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
    pub(crate) storage_binding: Option<storage::StorageBinding>,
    pub(crate) retained: Vec<Arc<Snapshot>>,
    pub(crate) remaining_budgets: Result<storage::LoadBudgets, ProjectError>,
}
impl SaveSnapshot {
    /// Engine-selected growth for source objects missing from retained roots.
    pub fn limit_source_growth(&mut self, bytes: usize) -> Result<(), ProjectError> {
        let budgets = self
            .remaining_budgets
            .as_mut()
            .map_err(|error| error.clone())?;
        budgets.source_bytes = budgets.source_bytes.min(bytes);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Content {
    layouts: Vec<PrintLayoutRecord>,
    definitions: BTreeMap<DefinitionId, StoredDefinition>,
    surface_overlays: Arc<SurfaceOverlays>,
    process_document: Arc<ProcessDocument>,
    appearance_study: AppearanceStudyInfo,
    analysis: Arc<spiling_contracts::analysis::AnalysisDocument>,
    materials: Option<spiling_contracts::library::MaterialCollectionRecord>,
}
impl Content {
    fn matches(&self, snapshot: &Snapshot) -> bool {
        self.layouts == snapshot.layout_records()
            && self.materials.as_ref() == snapshot.materials.as_ref().map(|a| a.record())
            && self.surface_overlays == snapshot.surface_overlays
            && self.process_document == snapshot.process_document
            && self.analysis == snapshot.analysis
            && self.appearance_study.palette == snapshot.appearance_study.palette
            && self.appearance_study.target == snapshot.appearance_study.target
            && self.appearance_study.limits == snapshot.appearance_study.limits
            && self.appearance_study.report == snapshot.appearance_study.report_record
            && self.appearance_study.selection == snapshot.appearance_study.selection
            && self.appearance_study.optical
                == snapshot
                    .appearance_study
                    .optical
                    .as_ref()
                    .map(OpticalStudy::info)
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
            layouts: manifest.layouts.clone(),
            surface_overlays: Arc::new(manifest.surface_overlays.clone()),
            process_document: Arc::new(manifest.process_document.clone()),
            appearance_study: manifest.appearance_study.info(),
            analysis: Arc::new(manifest.analysis.clone()),
            materials: manifest.materials.clone(),
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
    staged_hardware: Mutex<Vec<Weak<spiling_contracts::hardware::ProjectHardwareCapture>>>,
    staged_process: Mutex<Vec<Weak<ProcessDocument>>>,
    staged_appearance: Mutex<Vec<Weak<Snapshot>>>,
    saved: Option<(ProjectRevision, Content)>,
    storage: Option<Arc<Storage>>,
    storage_binding: Option<storage::StorageBinding>,
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
            staged_hardware: Mutex::new(Vec::new()),
            staged_process: Mutex::new(Vec::new()),
            staged_appearance: Mutex::new(Vec::new()),
            saved: None,
            storage: None,
            storage_binding: None,
            recovered_previous: false,
            save_uncertain: false,
        }
    }
    pub fn from_loaded(loaded: LoadedProject) -> Result<Self, ProjectError> {
        loaded.manifest.validate()?;
        let mut snapshot = Snapshot {
            sources: loaded.sources,
            focused_layout: loaded.manifest.layouts[0].layout_id.clone(),
            layouts: loaded.layouts,
            surface_overlays: Arc::new(loaded.manifest.surface_overlays.clone()),
            process_document: Arc::new(loaded.manifest.process_document.clone()),
            surface_images: loaded.surface_images,
            appearance_study: Arc::new(loaded.manifest.appearance_study.clone()),
            analysis: Arc::new(loaded.manifest.analysis.clone()),
            analysis_results: loaded.analysis_results,
            appearance_palettes: loaded.appearance_palettes,
            materials: loaded.materials,
        };
        validate_snapshot(&snapshot, &loaded.manifest)?;
        hardware::share_capture(&mut snapshot);
        let mut project = Self::new();
        project.project_id = loaded.manifest.project_id.clone();
        project.revision = loaded.manifest.revision;
        project.next_occurrence = loaded.manifest.next_occurrence;
        project.next_surface_layer = loaded.manifest.next_surface_layer;
        project.next_process_layer = loaded.manifest.next_process_layer;
        project.next_process_operation = loaded.manifest.next_process_operation;
        project.saved = Some((
            loaded.saved_revision,
            Content {
                layouts: snapshot.layout_records(),
                ..Content::from_manifest(&loaded.manifest)
            },
        ));
        project.current = Arc::new(snapshot);
        project.storage = Some(loaded.storage);
        project.storage_binding = Some(loaded.binding);
        project.recovered_previous = loaded.recovered_previous;
        Ok(project)
    }
    pub fn info(&self) -> ProjectInfo {
        let read_only = self.storage.as_ref().is_some_and(|s| s.read_only());
        let dirty = self.save_uncertain
            || self.recovered_previous
            || self.saved.as_ref().map_or(
                self.current.layouts.len() != 1
                    || self.current.layouts[0].name != "Plate 1"
                    || self.current.layouts.iter().any(|l| {
                        !l.occurrences.is_empty()
                            || l.manufacturing_intent.is_some()
                            || !l.manufacturing_options.is_empty()
                            || l.hardware.is_some()
                    })
                    || !self.current.surface_overlays.layers.is_empty()
                    || !self.current.process_document.layers.is_empty()
                    || *self.current.appearance_study != AppearanceStudy::default()
                    || !self.current.analysis.cases.is_empty()
                    || self.current.materials.is_some()
                    || self.current.hardware.is_some(),
                |(_, content)| !content.matches(&self.current),
            );
        let (surface_overlays, process_document) = self.current.scoped_documents();
        ProjectInfo {
            project_id: self.project_id.clone(),
            revision: self.revision,
            layout_id: self.current.layout_id.clone(),
            layouts: self
                .current
                .layouts
                .iter()
                .map(PrintLayoutSnapshot::info)
                .collect(),
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
            surface_overlays,
            process_document,
            appearance_study: self.current.appearance_study.info(),
            analysis: (*self.current.analysis).clone(),
            materials: self.current.materials.as_ref().map(|a| a.record().clone()),
            hardware: self.current.hardware.clone(),
            manufacturing_options: self
                .current
                .manufacturing_options
                .iter()
                .map(ManufacturingOption::info)
                .collect(),
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
    pub fn remaining_analysis_field_bytes(&self) -> Result<usize, ProjectError> {
        Ok(storage::remaining_budgets(self)?.analysis_bytes)
    }
    pub fn manufacturing_option(
        &self,
        base_revision: ProjectRevision,
        id: &SourceHash,
    ) -> Result<ManufacturingOption, ProjectError> {
        if self.revision != base_revision {
            return Err(error(
                ProjectErrorCode::StaleRevision,
                "manufacturing option inspection revision is stale",
            ));
        }
        let option = self
            .current
            .manufacturing_options
            .iter()
            .find(|option| &option.id == id)
            .ok_or_else(|| invalid("unknown manufacturing option"))?;
        option.validate()?;
        Ok(option.clone())
    }
    pub fn appearance_study(&self) -> &AppearanceStudy {
        &self.current.appearance_study
    }
    /// Includes live prepared candidates, even after their revision becomes stale.
    pub fn retained_material_bytes(&self) -> Result<usize, ProjectError> {
        let staged = self
            .staged_appearance
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "material staging lock poisoned"))?;
        let live: Vec<_> = staged.iter().filter_map(Weak::upgrade).collect();
        Ok(materials::retained_bytes(
            std::iter::once(self.current.as_ref())
                .chain(self.undo.iter().map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref))
                .chain(live.iter().map(Arc::as_ref)),
        ))
    }
    pub fn retained_appearance_bytes(&self) -> Result<usize, ProjectError> {
        let staged = self
            .staged_appearance
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "appearance staging lock poisoned"))?;
        let staged: Vec<_> = staged.iter().filter_map(Weak::upgrade).collect();
        appearance_bytes_with_incoming(
            std::iter::once(self.current.as_ref())
                .chain(self.undo.iter().map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref))
                .chain(staged.iter().map(Arc::as_ref)),
            None,
            self.saved
                .as_ref()
                .and_then(|(_, content)| content.appearance_study.optical.as_ref())
                .map(|study| &study.request),
        )
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
    /// Unique source allocations across Current, history and prepared candidates.
    /// Host admission consumes this measurement; Core does not prescribe RAM.
    pub fn retained_source_bytes(&self) -> Result<usize, ProjectError> {
        storage::retained_source_allocations(self)?
            .values()
            .try_fold(0usize, |sum, bytes| {
                sum.checked_add(*bytes).ok_or_else(|| {
                    error(
                        ProjectErrorCode::ResourceLimit,
                        "retained source accounting overflow",
                    )
                })
            })
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
            for asset in snapshot
                .layouts
                .iter()
                .filter_map(|l| l.manufacturing_artifact.as_ref())
            {
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
            self.revision = if self.recovered_previous {
                self.revision.max(previous.revision.next()?)
            } else {
                self.revision.max(previous.revision)
            };
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
    fn install_palette(
        &self,
        snapshot: &mut Snapshot,
        mut asset: Arc<PaletteAsset>,
    ) -> Result<(), ProjectError> {
        let staged = self
            .staged_appearance
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "appearance staging lock poisoned"))?;
        let staged: Vec<_> = staged.iter().filter_map(Weak::upgrade).collect();
        if let Some(retained) = std::iter::once(self.current.as_ref())
            .chain(self.undo.iter().map(Arc::as_ref))
            .chain(self.redo.iter().map(Arc::as_ref))
            .chain(staged.iter().map(Arc::as_ref))
            .find_map(|state| state.appearance_palettes.get(&asset.record.canonical_hash))
        {
            if retained.canonical.as_slice() != asset.canonical.as_slice() {
                return Err(error(
                    ProjectErrorCode::CorruptAsset,
                    "conflicting canonical palette identity",
                ));
            }
            asset = retained.clone();
        }
        snapshot.appearance_palettes.clear();
        snapshot
            .appearance_palettes
            .insert(asset.record.canonical_hash.clone(), asset.clone());
        let study = Arc::make_mut(&mut snapshot.appearance_study);
        study.palette = Some(asset.record.clone());
        study.report = None;
        study.report_record = None;
        study.selection = None;
        if let Some(optical) = &mut study.optical {
            if optical.request.validate_palette(asset.prepared()).is_err() {
                study.optical = None;
            } else {
                optical.report = None;
                optical.report_record = None;
                optical.selection = None;
            }
        }
        Ok(())
    }
    /// Detached placement trials share immutable inputs and never reserve live IDs.
    pub fn placement_draft(&self) -> PlacementDraft {
        PlacementDraft::new(self)
    }
    fn import_part(
        &self,
        snapshot: &mut Snapshot,
        mut source: Arc<SourceAsset>,
        pose: RigidPoseMm,
        next_occurrence: &mut u32,
    ) -> Result<(), ProjectError> {
        source.validate()?;
        // Earlier parts of this same transaction are staged in `snapshot`.
        let staged = snapshot.sources.get(&source.record.definition_id).cloned();
        if let Some(retained) = staged.or_else(|| {
            std::iter::once(&self.current)
                .chain(self.undo.iter())
                .chain(self.redo.iter())
                .find_map(|snapshot| snapshot.sources.get(&source.record.definition_id))
                .cloned()
        }) {
            let old = &retained.record.provenance;
            let new = &source.record.provenance;
            if old.source_hash != new.source_hash
                || old.source_unit != new.source_unit
                || old.uncertainty_mm != new.uncertainty_mm
                || old.source_solid != new.source_solid
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
        add_occurrence(snapshot, id, pose, next_occurrence)
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
        let incoming_palette = match &edit {
            Edit::ImportAppearancePalette { asset } => Some(asset.clone()),
            _ => None,
        };
        match edit {
            Edit::PrintLayout { edit } => print_layouts::apply(
                &mut snapshot,
                edit,
                &mut next_occurrence,
                &mut next_surface_layer,
                &mut next_process_layer,
                &mut next_process_operation,
            )?,
            Edit::PutRecipe { transfer, expected } => {
                self.store_recipe(&mut snapshot, &transfer, expected.as_ref())?;
            }
            Edit::CaptureRecipe {
                id,
                name,
                ingredients,
                expected,
            } => self.capture_recipe(&mut snapshot, id, name, ingredients, expected)?,
            Edit::LoadRecipeStudy { reference, context } => {
                self.load_recipe_study(&mut snapshot, reference, context)?
            }
            Edit::PutFilament {
                revision,
                origin,
                expected,
            } => {
                use spiling_contracts::library::*;
                revision.validate().map_err(invalid)?;
                let mut document = snapshot
                    .materials
                    .as_ref()
                    .map(|a| a.document())
                    .transpose()?
                    .unwrap_or_default();
                let entry = document
                    .entries
                    .iter_mut()
                    .find(|e| e.current.id == revision.reference.id);
                match entry {
                    Some(entry) => {
                        if expected.as_ref() != Some(&entry.current.revision) {
                            return Err(error(
                                ProjectErrorCode::StaleRevision,
                                "project material changed; review the retained draft",
                            ));
                        }
                        entry.current = revision.reference.clone();
                        entry.origin = origin;
                    }
                    None => {
                        if expected.is_some() {
                            return Err(error(
                                ProjectErrorCode::StaleRevision,
                                "project material no longer exists",
                            ));
                        }
                        document.entries.push(FilamentEntry {
                            current: revision.reference.clone(),
                            archived: false,
                            origin,
                        });
                    }
                }
                if !document
                    .revisions
                    .iter()
                    .any(|r| r.reference == revision.reference)
                {
                    document.revisions.push(revision);
                }
                materials::prune_revisions(&mut document, []);
                snapshot.materials = Some(Arc::new(materials::MaterialAsset::new(&document)?));
            }
            Edit::ApplyOpticalConstruction { candidate } => {
                if candidate.captured_revision != self.revision
                    || !Arc::ptr_eq(&candidate.captured, &self.current)
                {
                    return Err(error(
                        ProjectErrorCode::StaleRevision,
                        "optical construction was prepared from a different project snapshot",
                    ));
                }
                snapshot.manufacturing_intent = Some(candidate.intent);
            }
            Edit::ImportAppearancePalette { asset } => {
                self.install_palette(&mut snapshot, asset)?
            }
            Edit::SetAppearanceTarget { target, limits } => {
                let study = Arc::make_mut(&mut snapshot.appearance_study);
                study.target = Some(target);
                study.limits = limits;
                study.report = None;
                study.report_record = None;
                study.selection = None;
            }
            Edit::AttachAnalysisResult { asset } => {
                asset.validate_binding(&snapshot, &self.project_id, self.revision)?;
                if asset.summary.captured_revision != self.revision
                    || !asset.matches_inputs(&snapshot)
                {
                    return Err(error(
                        ProjectErrorCode::StaleRevision,
                        "analysis completion no longer matches current captured inputs",
                    ));
                }
                let document = Arc::make_mut(&mut snapshot.analysis);
                document
                    .results
                    .retain(|record| record.case_id != asset.record.case_id);
                document.results.push(asset.record.clone());
                snapshot
                    .analysis_results
                    .insert(asset.record.hash.clone(), asset);
            }
            Edit::Analysis { edit } => {
                use spiling_contracts::analysis::{AnalysisCaseRecord, AnalysisEdit};
                edit.validate()?;
                if let AnalysisEdit::Put { case } = &edit {
                    let source_matches =
                        snapshot
                            .sources
                            .get(&case.definition_id)
                            .is_some_and(|source| {
                                source.record.provenance.source_hash == case.source_hash
                            });
                    let occurrence_matches = snapshot.layouts.iter().any(|layout| {
                        layout.layout_id == case.layout_id
                            && layout.occurrences.get(&case.occurrence_id).is_some_and(
                                |occurrence| occurrence.definition_id == case.definition_id,
                            )
                    });
                    if !source_matches || !occurrence_matches {
                        return Err(invalid(
                            "analysis case subject differs from the native project",
                        ));
                    }
                }
                let revision = self.revision.next()?;
                let document = Arc::make_mut(&mut snapshot.analysis);
                match edit {
                    AnalysisEdit::Put { case } => {
                        let record = AnalysisCaseRecord {
                            case,
                            edited_revision: revision,
                        };
                        if let Some(existing) = document
                            .cases
                            .iter_mut()
                            .find(|existing| existing.case.id == record.case.id)
                        {
                            if existing.case.occurrence_id != record.case.occurrence_id
                                || existing.case.definition_id != record.case.definition_id
                                || existing.case.source_hash != record.case.source_hash
                                || existing.case.layout_id != record.case.layout_id
                            {
                                return Err(invalid(
                                    "analysis case source binding is immutable; create a new case",
                                ));
                            }
                            if existing.case != record.case {
                                *existing = record;
                            }
                        } else {
                            document.cases.push(record);
                        }
                    }
                    AnalysisEdit::Remove { case_id } => {
                        let index = document
                            .cases
                            .iter()
                            .position(|record| record.case.id == case_id)
                            .ok_or_else(|| invalid("analysis case does not exist"))?;
                        document.cases.remove(index);
                        document.results.retain(|result| result.case_id != case_id);
                    }
                }
                document.validate()?;
            }
            Edit::AcceptAppearanceReport { report, selection } => {
                snapshot.appearance_study.matches_report(&report)?;
                if !report.is_complete() {
                    return Err(invalid("incomplete appearance report cannot be accepted"));
                }
                if selection
                    .as_ref()
                    .is_some_and(|key| !report.has_selection(key))
                {
                    return Err(invalid("appearance selection is not a report alternative"));
                }
                let study = Arc::make_mut(&mut snapshot.appearance_study);
                study.report_record = Some(report.canonical_record()?);
                study.report = Some(report);
                study.selection = selection;
            }
            Edit::SetOpticalStudy { request } => {
                let palette = snapshot
                    .appearance_study
                    .palette
                    .as_ref()
                    .ok_or_else(|| invalid("optical study requires an imported palette"))?;
                let asset = snapshot
                    .appearance_palettes
                    .get(&palette.canonical_hash)
                    .ok_or_else(|| {
                        error(ProjectErrorCode::MissingAsset, "optical palette is missing")
                    })?;
                request.validate_palette(asset.prepared())?;
                Arc::make_mut(&mut snapshot.appearance_study).optical =
                    Some(OpticalStudy::new(request));
            }
            Edit::AcceptOpticalReport { report, selection } => {
                let palette = snapshot
                    .appearance_study
                    .palette
                    .as_ref()
                    .ok_or_else(|| invalid("optical study requires an imported palette"))?;
                let optical = snapshot
                    .appearance_study
                    .optical
                    .as_ref()
                    .ok_or_else(|| invalid("optical study has no request"))?;
                optical.matches_report(palette, &report)?;
                if !report.is_complete() {
                    return Err(invalid("incomplete optical report cannot be accepted"));
                }
                if selection
                    .as_ref()
                    .is_some_and(|selection| !report.has_selection(selection))
                {
                    return Err(invalid(
                        "optical selection is not an assessed retained estimate",
                    ));
                }
                let record = report.canonical_record()?;
                let optical = Arc::make_mut(&mut snapshot.appearance_study)
                    .optical
                    .as_mut()
                    .ok_or_else(|| invalid("optical study has no request"))?;
                optical.report_record = Some(record);
                optical.report = Some(report);
                optical.selection = selection;
            }
            Edit::Process { edit } => {
                if let ProcessEdit::AddLayer { scope, .. }
                | ProcessEdit::SetSparseFraction { scope, .. }
                | ProcessEdit::SetBoxSparseFraction { scope, .. }
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
            Edit::Import { source, pose } => {
                self.import_part(&mut snapshot, source, pose, &mut next_occurrence)?;
            }
            Edit::ImportParts { parts } => {
                if parts.is_empty() {
                    return Err(invalid("import requires at least one part"));
                }
                for (source, pose) in parts {
                    self.import_part(&mut snapshot, source, pose, &mut next_occurrence)?;
                }
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
            Edit::ApplyPlacement {
                placements,
                additional,
            } => placement::apply_across_plates(
                &mut snapshot,
                &placements,
                &additional,
                [
                    &mut next_occurrence,
                    &mut next_surface_layer,
                    &mut next_process_layer,
                    &mut next_process_operation,
                ],
            )?,
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
                    .all_occurrences()
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
            Edit::SetHardware { capture } => {
                if let Some(c) = &capture {
                    c.validate().map_err(invalid)?;
                }
                if let Some(old) = &snapshot.manufacturing_intent {
                    let mut intent = (**old).clone();
                    intent.hardware = capture.clone();
                    if let Some(c) = &capture {
                        intent.printer = c.setup.printer.clone();
                        if let Some(profile) = &intent.material_profile
                            && let spiling_contracts::manufacturing::JobWorkflow::CompleteMarlin {
                                job,
                            } = &mut intent.printer.workflow
                        {
                            job.nozzle_temperature_c = profile.selection.nozzle_temperature_c();
                            job.bed_temperature_c = profile.selection.bed_temperature_c();
                        }
                    }
                    intent.validate().map_err(|e| invalid(e.message))?;
                    snapshot.manufacturing_intent = Some(Arc::new(intent));
                }
                snapshot.hardware = capture;
            }
            Edit::SetManufacturingIntent { intent } => {
                manufacturing_options::set_intent(&mut snapshot, intent)?;
            }
            Edit::SetInteriorToolPolicy { edit } => {
                interior_policy::apply(&mut snapshot, edit)?;
            }
            Edit::RetainManufacturingOption { option } => {
                manufacturing_options::retain_option(&mut snapshot, option)?;
            }
            Edit::RemoveManufacturingOption { id } => {
                let options = Arc::make_mut(&mut snapshot.manufacturing_options);
                let index = options
                    .iter()
                    .position(|option| option.id == id)
                    .ok_or_else(|| invalid("unknown manufacturing option"))?;
                options.remove(index);
            }
            Edit::ApplyManufacturingOption { id } => {
                let option = snapshot
                    .manufacturing_options
                    .iter()
                    .find(|option| option.id == id)
                    .ok_or_else(|| invalid("unknown manufacturing option"))?;
                manufacturing_options::validate_option_context(&snapshot, option)?;
                let intent = option.intent.clone();
                manufacturing_options::set_intent(&mut snapshot, intent)?;
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
                    .flat_map(|s| {
                        s.layouts
                            .iter()
                            .filter_map(|l| l.manufacturing_artifact.as_ref())
                    })
                    .find(|old| old.record.hash == asset.record.hash)
                {
                    if !retained.same_content(&asset) {
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
        validate_appearance(&snapshot)?;
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
                } => snapshot.all_occurrences().any(|record| {
                    &record.occurrence_id == occurrence_id
                        && &record.definition_id == expected_definition_id
                }),
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
                } => snapshot.all_occurrences().any(|record| {
                    &record.occurrence_id == occurrence_id
                        && &record.definition_id == expected_definition_id
                }),
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
        snapshot.invalidate_changed_layout_results(&self.project_id)?;
        let subjects: BTreeMap<_, _> = snapshot
            .layouts
            .iter()
            .flat_map(|layout| {
                layout.occurrences.values().map(|occurrence| {
                    (
                        (occurrence.occurrence_id, occurrence.definition_id.clone()),
                        layout.layout_id.clone(),
                    )
                })
            })
            .collect();
        if snapshot.analysis.cases.iter().any(|record| {
            subjects.get(&(record.case.occurrence_id, record.case.definition_id.clone()))
                != Some(&record.case.layout_id)
        }) {
            let document = Arc::make_mut(&mut snapshot.analysis);
            document.cases.retain_mut(|record| {
                let Some(layout_id) =
                    subjects.get(&(record.case.occurrence_id, record.case.definition_id.clone()))
                else {
                    return false;
                };
                if &record.case.layout_id != layout_id {
                    record.case.layout_id = layout_id.clone();
                    record.edited_revision = self.revision.next().expect("revision admitted above");
                }
                true
            });
        }
        if snapshot.analysis.results.iter().any(|result| {
            !snapshot
                .analysis
                .cases
                .iter()
                .any(|case| case.case.id == result.case_id)
        }) {
            let document = Arc::make_mut(&mut snapshot.analysis);
            document.results.retain(|result| {
                document
                    .cases
                    .iter()
                    .any(|case| case.case.id == result.case_id)
            });
        }
        snapshot.analysis_results.retain(|hash, _| {
            snapshot
                .analysis
                .results
                .iter()
                .any(|result| &result.hash == hash)
        });
        let skip = usize::from(self.undo.len() == MAX_PROJECT_HISTORY);
        validate_pins(
            std::iter::once(&snapshot)
                .chain(std::iter::once(self.current.as_ref()))
                .chain(self.undo.iter().skip(skip).map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref)),
        )?;
        let mut staged_hardware = self
            .staged_hardware
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "hardware staging lock poisoned"))?;
        staged_hardware.retain(|h| h.strong_count() != 0);
        if hardware::bytes(
            std::iter::once(&snapshot)
                .chain(std::iter::once(self.current.as_ref()))
                .chain(self.undo.iter().map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref)),
            staged_hardware.iter().filter_map(Weak::upgrade).chain(
                self.saved.as_ref().into_iter().flat_map(|(_, content)| {
                    content.layouts.iter().filter_map(|l| l.hardware.clone())
                }),
            ),
        )? > hardware::MAX_RETAINED_BYTES
        {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained and prepared hardware capture budget exceeded",
            ));
        }
        if let Some(c) = &snapshot.hardware
            && !staged_hardware.iter().any(|h| h.as_ptr() == Arc::as_ptr(c))
        {
            staged_hardware.push(Arc::downgrade(c));
        }
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
        let mut candidate = self.manifest();
        candidate.materials = snapshot.materials.as_ref().map(|a| a.record().clone());
        candidate.revision = self.revision.next()?;
        candidate.next_occurrence = next_occurrence;
        candidate.next_surface_layer = next_surface_layer;
        candidate.next_process_layer = next_process_layer;
        candidate.next_process_operation = next_process_operation;
        candidate.definitions = snapshot
            .sources
            .values()
            .map(|asset| asset.record.clone())
            .collect();
        candidate.layouts = snapshot.layout_records();
        candidate.surface_overlays = (*snapshot.surface_overlays).clone();
        candidate.process_document = (*snapshot.process_document).clone();
        candidate.appearance_study = (*snapshot.appearance_study).clone();
        candidate.analysis = (*snapshot.analysis).clone();
        candidate.analysis.validate_subjects(
            &candidate.definitions,
            &candidate.layouts,
            candidate.revision,
        )?;
        validate_manifest_size(&candidate)?;
        let snapshot = Arc::new(snapshot);
        let mut staged = self
            .staged_appearance
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "appearance staging lock poisoned"))?;
        staged.retain(|state| state.strong_count() != 0);
        let live: Vec<_> = staged.iter().filter_map(Weak::upgrade).collect();
        let mut analysis_allocations = BTreeMap::new();
        if analysis_results::retained_bytes(
            std::iter::once(snapshot.as_ref())
                .chain(std::iter::once(self.current.as_ref()))
                .chain(self.undo.iter().map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref))
                .chain(live.iter().map(Arc::as_ref)),
        ) > MAX_RETAINED_ANALYSIS_FIELD_BYTES
        {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained/staged analysis fields exceed 32 MiB",
            ));
        }
        for retained in std::iter::once(snapshot.as_ref())
            .chain(std::iter::once(self.current.as_ref()))
            .chain(self.undo.iter().map(Arc::as_ref))
            .chain(self.redo.iter().map(Arc::as_ref))
            .chain(live.iter().map(Arc::as_ref))
        {
            analysis_allocations.insert(
                Arc::as_ptr(&retained.analysis),
                retained.analysis.allocated_bytes(),
            );
        }
        if let Some((_, saved)) = &self.saved {
            analysis_allocations.insert(
                Arc::as_ptr(&saved.analysis),
                saved.analysis.allocated_bytes(),
            );
        }
        if analysis_allocations.values().sum::<usize>()
            > spiling_contracts::analysis::MAX_ANALYSIS_RETAINED_BYTES
        {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained/staged analysis exceeds 4 MiB",
            ));
        }
        if manufacturing_options::retained_bytes(
            std::iter::once(snapshot.as_ref())
                .chain(std::iter::once(self.current.as_ref()))
                .chain(self.undo.iter().map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref))
                .chain(live.iter().map(Arc::as_ref)),
            self.saved
                .as_ref()
                .into_iter()
                .flat_map(|(_, content)| content.layouts.iter().map(|l| &l.manufacturing_options)),
        )? > manufacturing_options::MAX_RETAINED_OPTION_BYTES
        {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained/staged manufacturing options exceed 16 MiB",
            ));
        }
        if hardware::bytes(
            std::iter::once(snapshot.as_ref())
                .chain(std::iter::once(self.current.as_ref()))
                .chain(self.undo.iter().map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref))
                .chain(live.iter().map(Arc::as_ref)),
            self.saved.iter().flat_map(|(_, content)| {
                content.layouts.iter().flat_map(|l| {
                    l.manufacturing_options
                        .iter()
                        .filter_map(|option| option.intent.hardware.clone())
                })
            }),
        )? > hardware::MAX_RETAINED_BYTES
        {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained/staged option hardware exceeds 2 MiB",
            ));
        }
        if materials::retained_bytes(
            std::iter::once(snapshot.as_ref())
                .chain(std::iter::once(self.current.as_ref()))
                .chain(self.undo.iter().map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref))
                .chain(live.iter().map(Arc::as_ref)),
        ) > materials::MAX_RETAINED_MATERIAL_BYTES
        {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained/staged project materials exceed 64 MiB",
            ));
        }
        let retained = appearance_bytes_with_incoming(
            std::iter::once(snapshot.as_ref())
                .chain(std::iter::once(self.current.as_ref()))
                .chain(self.undo.iter().map(Arc::as_ref))
                .chain(self.redo.iter().map(Arc::as_ref))
                .chain(live.iter().map(Arc::as_ref)),
            incoming_palette.as_deref(),
            self.saved
                .as_ref()
                .and_then(|(_, content)| content.appearance_study.optical.as_ref())
                .map(|study| &study.request),
        )?;
        if retained > MAX_APPEARANCE_RETAINED_BYTES {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "retained/staged appearance allocation budget exceeded",
            ));
        }
        staged.push(Arc::downgrade(&snapshot));
        Ok(PreparedEdit {
            owner: self.owner.clone(),
            revision: self.revision,
            snapshot,
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
    fn manifest(&self) -> ProjectManifest {
        ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id: self.project_id.clone(),
            revision: self.revision,
            units: ProjectUnits::Millimetres,
            frame: ProjectFrame::RightHanded,
            next_occurrence: self.next_occurrence,
            next_surface_layer: self.next_surface_layer,
            surface_overlays: (*self.current.surface_overlays).clone(),
            process_document: (*self.current.process_document).clone(),
            appearance_study: (*self.current.appearance_study).clone(),
            analysis: (*self.current.analysis).clone(),
            materials: self.current.materials.as_ref().map(|a| a.record().clone()),
            next_process_layer: self.next_process_layer,
            next_process_operation: self.next_process_operation,
            definitions: self
                .current
                .sources
                .values()
                .map(|s| s.record.clone())
                .collect(),
            layouts: self.current.layout_records(),
        }
    }
    pub fn capture(&self) -> SaveSnapshot {
        SaveSnapshot {
            manifest: self.manifest(),
            snapshot: self.current.clone(),
            storage_binding: self.storage_binding.clone(),
            retained: std::iter::once(&self.current)
                .chain(self.undo.iter())
                .chain(self.redo.iter())
                .cloned()
                .collect(),
            remaining_budgets: storage::remaining_budgets(self),
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
        self.save_uncertain = receipt.outcome != storage::PublicationOutcome::PublishedConfirmed
            || receipt.durability_error.is_some();
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
        self.storage_binding = receipt.binding;
        Ok(())
    }
}

fn validate_appearance(snapshot: &Snapshot) -> Result<(), ProjectError> {
    snapshot.appearance_study.validate()?;
    match &snapshot.appearance_study.palette {
        Some(record) => {
            let asset = snapshot
                .appearance_palettes
                .get(&record.canonical_hash)
                .ok_or_else(|| {
                    error(
                        ProjectErrorCode::MissingAsset,
                        "appearance palette is missing",
                    )
                })?;
            if asset.record() != record || snapshot.appearance_palettes.len() != 1 {
                return Err(error(
                    ProjectErrorCode::CorruptAsset,
                    "appearance palette metadata differs from asset",
                ));
            }
            if let Some(report) = &snapshot.appearance_study.report
                && (report.total_rows as usize != asset.document().rows.len()
                    || report.visits.iter().any(|visit| {
                        asset
                            .document()
                            .rows
                            .binary_search_by(|row| row.key.cmp(&visit.key))
                            .ok()
                            .is_none_or(|index| {
                                asset.prepared().evidence_hashes()[index] != visit.evidence_hash
                            })
                    })
                    || report.alternatives.iter().any(|alternative| {
                        asset
                            .document()
                            .rows
                            .binary_search_by(|row| row.key.cmp(&alternative.key))
                            .ok()
                            .is_none_or(|index| {
                                asset.document().rows[index].name != alternative.name
                            })
                    }))
            {
                return Err(error(
                    ProjectErrorCode::CorruptAsset,
                    "appearance report evidence differs from retained palette",
                ));
            }
            if let Some(optical) = &snapshot.appearance_study.optical {
                optical.request.validate_palette(asset.prepared())?;
                if let Some(report) = &optical.report {
                    for (key, evidence_hash) in optical
                        .request
                        .materials
                        .iter()
                        .zip(&report.material_evidence_hashes)
                    {
                        let index = asset
                            .document()
                            .rows
                            .binary_search_by(|row| row.key.cmp(key))
                            .map_err(|_| {
                                error(
                                    ProjectErrorCode::CorruptAsset,
                                    "optical material is absent from retained palette",
                                )
                            })?;
                        if &asset.prepared().evidence_hashes()[index] != evidence_hash {
                            return Err(error(
                                ProjectErrorCode::CorruptAsset,
                                "optical report evidence differs from retained palette",
                            ));
                        }
                    }
                }
            }
        }
        None if !snapshot.appearance_palettes.is_empty() => {
            return Err(invalid("unreferenced appearance palette"));
        }
        None => {}
    }
    Ok(())
}

fn appearance_bytes<'a>(
    snapshots: impl Iterator<Item = &'a Snapshot>,
) -> Result<usize, ProjectError> {
    appearance_bytes_with_incoming(snapshots, None, None)
}

fn appearance_bytes_with_incoming<'a>(
    snapshots: impl Iterator<Item = &'a Snapshot>,
    incoming: Option<&PaletteAsset>,
    saved_request: Option<&Arc<OpticalRequest>>,
) -> Result<usize, ProjectError> {
    let mut bytes = BTreeMap::new();
    let mut studies = BTreeMap::new();
    let mut reports = BTreeMap::new();
    let mut optical_requests = BTreeMap::new();
    let mut optical_reports = BTreeMap::new();
    if let Some(request) = saved_request {
        optical_requests.insert(Arc::as_ptr(request), request.allocated_bytes());
    }
    let mut documents = BTreeMap::new();
    let mut assets = BTreeMap::new();
    for snapshot in snapshots {
        studies.insert(
            Arc::as_ptr(&snapshot.appearance_study),
            snapshot.appearance_study.allocated_bytes(),
        );
        if let Some(report) = &snapshot.appearance_study.report {
            reports.insert(Arc::as_ptr(report), report.allocated_bytes());
        }
        if let Some(optical) = &snapshot.appearance_study.optical {
            optical_requests.insert(
                Arc::as_ptr(&optical.request),
                optical.request.allocated_bytes(),
            );
            if let Some(report) = &optical.report {
                optical_reports.insert(Arc::as_ptr(report), report.allocated_bytes());
                optical_requests.insert(
                    Arc::as_ptr(&report.request),
                    report.request.allocated_bytes(),
                );
            }
        }
        for asset in snapshot.appearance_palettes.values() {
            bytes.insert(Arc::as_ptr(asset.original()), asset.original().capacity());
            bytes.insert(Arc::as_ptr(asset.canonical()), asset.canonical().capacity());
            documents.insert(
                Arc::as_ptr(&asset.prepared),
                asset.prepared.allocated_bytes(),
            );
            assets.insert(Arc::as_ptr(asset), asset.record.allocated_bytes());
        }
    }
    if let Some(asset) = incoming {
        bytes.insert(Arc::as_ptr(asset.original()), asset.original().capacity());
        bytes.insert(Arc::as_ptr(asset.canonical()), asset.canonical().capacity());
        documents.insert(
            Arc::as_ptr(&asset.prepared),
            asset.prepared.allocated_bytes(),
        );
        assets.insert(asset as *const PaletteAsset, asset.record.allocated_bytes());
    }
    bytes
        .values()
        .chain(assets.values())
        .chain(documents.values())
        .chain(studies.values())
        .chain(reports.values())
        .chain(optical_requests.values())
        .chain(optical_reports.values())
        .try_fold(0usize, |total, size| {
            total.checked_add(*size).ok_or_else(|| {
                error(
                    ProjectErrorCode::ResourceLimit,
                    "appearance allocation budget overflow",
                )
            })
        })
}

fn validate_manifest_size(manifest: &ProjectManifest) -> Result<(), ProjectError> {
    struct BoundedWriter(usize);
    impl std::io::Write for BoundedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("manifest size overflow"))?;
            if self.0 > MAX_PROJECT_MANIFEST_BYTES as usize {
                return Err(std::io::Error::other("manifest exceeds 1 MiB"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // Five persistent revision/allocator u32 values can each grow by at most
    // nine decimal digits. Reserve this without cloning project documents.
    const MAX_COUNTER_GROWTH: usize = 5 * 9;
    serde_json::to_writer_pretty(BoundedWriter(MAX_COUNTER_GROWTH), manifest)
        .map_err(|e| error(ProjectErrorCode::ResourceLimit, e.to_string()))
}
fn add_occurrence(
    snapshot: &mut Snapshot,
    definition_id: DefinitionId,
    pose: RigidPoseMm,
    next: &mut u32,
) -> Result<(), ProjectError> {
    pose.validate().map_err(|e| invalid(e.message))?;
    if snapshot.all_occurrences().count() >= MAX_OCCURRENCES as usize {
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
    let snapshots: Vec<_> = snapshots.collect();
    if analysis_results::retained_bytes(snapshots.iter().copied())
        > MAX_RETAINED_ANALYSIS_FIELD_BYTES
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained analysis fields exceed 32 MiB",
        ));
    }
    if manufacturing_options::retained_bytes(snapshots.iter().copied(), None)?
        > manufacturing_options::MAX_RETAINED_OPTION_BYTES
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained manufacturing options exceed 16 MiB",
        ));
    }
    if hardware::bytes(snapshots.iter().copied(), std::iter::empty())?
        > hardware::MAX_RETAINED_BYTES
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained hardware capture budget exceeded",
        ));
    }
    if materials::retained_bytes(snapshots.iter().copied()) > materials::MAX_RETAINED_MATERIAL_BYTES
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained project material bytes exceed 64 MiB",
        ));
    }
    if appearance_bytes(snapshots.iter().copied())? > MAX_APPEARANCE_RETAINED_BYTES {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained appearance allocation budget exceeded",
        ));
    }
    let mut sources: BTreeMap<&DefinitionId, &Arc<SourceAsset>> = BTreeMap::new();
    let mut manufacturing: BTreeMap<&SourceHash, &Arc<ManufacturingAsset>> = BTreeMap::new();
    let mut surface = BTreeMap::new();
    let mut images = BTreeMap::new();
    let mut process = BTreeMap::new();
    let mut image_allocations = BTreeMap::new();
    for snapshot in snapshots {
        validate_surface_images(snapshot)?;
        validate_appearance(snapshot)?;
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
        for asset in snapshot
            .layouts
            .iter()
            .filter_map(|l| l.manufacturing_artifact.as_ref())
        {
            if let Some(old) = manufacturing.insert(&asset.record.hash, asset)
                && !Arc::ptr_eq(old, asset)
                && !old.same_content(asset)
            {
                return Err(invalid("conflicting retained manufacturing identity"));
            }
        }
    }
    if image_allocations.values().sum::<usize>() > MAX_SURFACE_IMAGE_RETAINED_BYTES {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained image allocation budget exceeded",
        ));
    }
    if sources.len() > MAX_DEFINITIONS as usize {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained definition inventory exceeded",
        ));
    }
    if manufacturing
        .values()
        .map(|a| a.retained_bytes())
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
    if manifest.materials.as_ref() != snapshot.materials.as_ref().map(|a| a.record()) {
        return Err(invalid(
            "project material descriptor and captured asset disagree",
        ));
    }

    manifest.validate()?;
    if snapshot.analysis_results.len() != manifest.analysis.results.len() {
        return Err(invalid(
            "retained analysis object closure differs from manifest",
        ));
    }
    for record in &manifest.analysis.results {
        let asset = snapshot
            .analysis_results
            .get(&record.hash)
            .filter(|asset| &asset.record == record)
            .ok_or_else(|| invalid("retained analysis object is missing or misbound"))?;
        asset.validate_binding(snapshot, &manifest.project_id, manifest.revision)?;
    }
    if !Content::from_manifest(manifest).matches(snapshot)
        || snapshot.sources.len() != manifest.definitions.len()
        || snapshot.all_occurrences().count()
            != manifest
                .layouts
                .iter()
                .map(|l| l.occurrences.len())
                .sum::<usize>()
    {
        return Err(invalid("captured snapshot does not match manifest"));
    }
    for (id, source) in &snapshot.sources {
        if *id != source.record.definition_id {
            return Err(invalid("source map key differs from identity"));
        }
        source.validate()?;
    }
    for l in &snapshot.layouts {
        if let Some(asset) = &l.manufacturing_artifact {
            let bundle = asset.validate()?;
            let mut scoped = snapshot.clone();
            scoped.focused_layout = l.layout_id.clone();
            validate_manufacturing_input(
                &bundle,
                &scoped,
                &manifest.project_id,
                manifest.revision,
            )?;
        }
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
        .active_sources()
        .map(|a| a.record.clone())
        .collect();
    let occurrences: Vec<_> = snapshot.occurrences.values().cloned().collect();
    let intent = snapshot
        .manufacturing_intent
        .as_deref()
        .ok_or_else(|| invalid("manufacturing artifact has no current intent"))?;
    let (surfaces, process) = snapshot.scoped_documents();
    let input_hash = input_fingerprint(
        project_id,
        &definitions,
        &occurrences,
        intent,
        &surfaces,
        &process,
    )
    .map_err(manufacturing_error)?;
    if &provenance.project_id != project_id
        || provenance.input_revision > revision
        || provenance.input_hash != input_hash
        || &provenance.intent != intent
        || provenance.surface_overlays != surfaces
        || provenance.process_document != process
        || provenance.definitions.len() != definitions.len()
        || provenance.occurrences.len() != occurrences.len()
        || provenance.surface_images.len() != surfaces.images.len()
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
mod analysis_tests;
#[cfg(test)]
mod optical_tests;
#[cfg(test)]
mod tests;
