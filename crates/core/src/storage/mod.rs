// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Checked project meaning above the closed PTAC container representation.
use super::{
    MAX_APPEARANCE_RETAINED_BYTES, MAX_RETAINED_SOURCE_BYTES, MAX_SOURCE_BYTES, ManufacturingAsset,
    PaletteAsset, Project, SaveSnapshot, Snapshot, SourceAsset, SurfaceImageAsset, error, invalid,
    validate_snapshot,
};
use spiling_contracts::{
    appearance::{MAX_PALETTE_BYTES, PreparedPalette},
    geometry::{DefinitionId, SourceHash},
    manufacturing::{MAX_MANUFACTURING_BUNDLE_BYTES, MAX_MANUFACTURING_RETAINED_BYTES},
    project::*,
    surface::{MAX_SURFACE_IMAGE_BYTES, MAX_SURFACE_IMAGE_RETAINED_BYTES},
};
use spiling_core_ptac::{
    Capture, Container, Heads, Member, ObjectId, ObjectInput, ObjectRole, Selection, SnapshotId,
    SnapshotInput,
};
pub use spiling_core_ptac::{PublicationOutcome, StorageStats};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

const SOURCE: u16 = 1;
const MANUFACTURING: u16 = 2;
const IMAGE: u16 = 3;
const PALETTE_ORIGINAL: u16 = 4;
const PALETTE_CANONICAL: u16 = 5;
const MATERIALS: u16 = 6;
const ANALYSIS: u16 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStage {
    Staged,
    RowsInserted,
    BeforeCommit,
    Committed,
    BeforePublish,
    Published,
}

#[derive(Debug)]
pub struct Storage {
    container: Arc<Container>,
}
impl Storage {
    pub fn path(&self) -> &Path {
        self.container.path()
    }
    pub fn read_only(&self) -> bool {
        self.container.read_only()
    }
    pub fn stats(&self, cancel: &AtomicBool) -> Result<StorageStats, ProjectError> {
        self.container.stats(cancel).map_err(ptac_error)
    }
    pub fn collect_unreachable(
        &self,
        cancel: &AtomicBool,
        decide: impl FnMut() -> bool,
    ) -> Result<StorageStats, ProjectError> {
        self.container
            .collect_unreachable(cancel, decide)
            .map_err(ptac_error)?;
        self.stats(&AtomicBool::new(false))
    }
    pub fn compact_to_new(
        &self,
        target: &Path,
        cancel: &AtomicBool,
        mut observe: impl FnMut(SaveStage),
    ) -> Result<StorageCopyReceipt, ProjectError> {
        if self.read_only() {
            return Err(error(
                ProjectErrorCode::ReadOnly,
                "read-only storage cannot be compacted",
            ));
        }
        let publication = self
            .container
            .compact_to_new(target, cancel, |stage| {
                observe_stage(stage, true, &mut observe);
                stage_decision(stage, true, cancel)
            })
            .map_err(ptac_error)?;
        let outcome = publication.outcome;
        let durability_error = publication_error(outcome, publication.error);
        let storage = Arc::new(Self {
            container: publication.container,
        });
        let stats = if outcome == PublicationOutcome::PublishedConfirmed {
            storage.stats(&AtomicBool::new(false)).ok()
        } else {
            None
        };
        Ok(StorageCopyReceipt {
            storage,
            outcome,
            durability_error,
            stats,
        })
    }
}

#[derive(Debug)]
pub struct StorageCopyReceipt {
    pub storage: Arc<Storage>,
    pub outcome: PublicationOutcome,
    pub durability_error: Option<ProjectError>,
    pub stats: Option<StorageStats>,
}

#[derive(Debug, Clone)]
pub(crate) struct StorageBinding {
    container: Weak<Container>,
    heads: Heads,
    selected: SnapshotId,
    selection: Selection,
    guard: ProjectRecoveryGuard,
}

#[derive(Debug)]
pub struct LoadedProject {
    pub analysis_results: BTreeMap<SourceHash, Arc<super::AnalysisResultAsset>>,
    pub manifest: ProjectManifest,
    pub saved_revision: ProjectRevision,
    pub sources: BTreeMap<DefinitionId, Arc<SourceAsset>>,
    pub layouts: Vec<super::PrintLayoutSnapshot>,
    pub surface_images: BTreeMap<SourceHash, Arc<SurfaceImageAsset>>,
    pub appearance_palettes: BTreeMap<SourceHash, Arc<PaletteAsset>>,
    pub materials: Option<Arc<super::materials::MaterialAsset>>,
    pub storage: Arc<Storage>,
    pub recovered_previous: bool,
    pub(crate) binding: StorageBinding,
}

#[derive(Debug)]
pub struct SaveReceipt {
    pub storage: Arc<Storage>,
    pub manifest: ProjectManifest,
    pub outcome: PublicationOutcome,
    pub durability_error: Option<ProjectError>,
    pub(crate) binding: Option<StorageBinding>,
}

#[derive(Debug, Clone, Copy)]
pub struct LoadBudgets {
    pub analysis_bytes: usize,
    pub source_bytes: usize,
    pub manufacturing_bytes: usize,
    pub image_bytes: usize,
    pub appearance_bytes: usize,
    pub material_bytes: usize,
}
impl LoadBudgets {
    pub const MAXIMUM: Self = Self {
        analysis_bytes: super::MAX_RETAINED_ANALYSIS_FIELD_BYTES,
        source_bytes: MAX_RETAINED_SOURCE_BYTES,
        manufacturing_bytes: MAX_MANUFACTURING_RETAINED_BYTES as usize,
        image_bytes: MAX_SURFACE_IMAGE_RETAINED_BYTES,
        appearance_bytes: MAX_APPEARANCE_RETAINED_BYTES,
        material_bytes: super::materials::MAX_RETAINED_MATERIAL_BYTES,
    };
}

fn ptac_error(value: spiling_core_ptac::Error) -> ProjectError {
    use spiling_core_ptac::Error;
    let code = match &value {
        Error::Unsupported(_) => ProjectErrorCode::UnsupportedFormat,
        Error::Invalid(_) | Error::NoPrevious => ProjectErrorCode::InvalidProject,
        Error::ResourceLimit(_) => ProjectErrorCode::ResourceLimit,
        Error::Locked => ProjectErrorCode::ProjectLocked,
        Error::ReadOnly => ProjectErrorCode::ReadOnly,
        Error::Conflict => ProjectErrorCode::StaleRevision,
        Error::Cancelled => ProjectErrorCode::Cancelled,
        Error::Io(_) | Error::Sqlite(_) => ProjectErrorCode::Io,
    };
    error(code, value.to_string())
}

fn cancelled(cancel: &AtomicBool) -> Result<(), ProjectError> {
    if cancel.load(Ordering::Acquire) {
        Err(error(
            ProjectErrorCode::Cancelled,
            "project operation cancelled before publication",
        ))
    } else {
        Ok(())
    }
}

fn publication_error(
    outcome: PublicationOutcome,
    value: Option<spiling_core_ptac::Error>,
) -> Option<ProjectError> {
    if outcome == PublicationOutcome::PublishedConfirmed {
        None
    } else {
        Some(error(
            ProjectErrorCode::Io,
            format!(
                "save publication is {outcome:?}; inspect/reopen before retrying: {}",
                value.map_or_else(
                    || "publication could not be confirmed".to_owned(),
                    |value| value.to_string()
                )
            ),
        ))
    }
}

fn observe_stage(
    stage: spiling_core_ptac::SaveStage,
    is_new: bool,
    observe: &mut impl FnMut(SaveStage),
) {
    use spiling_core_ptac::SaveStage as Stage;
    match stage {
        Stage::Staged => observe(SaveStage::Staged),
        Stage::BeforeCommit => {
            observe(SaveStage::RowsInserted);
            if !is_new {
                observe(SaveStage::BeforeCommit);
            }
        }
        Stage::Committed => observe(SaveStage::Committed),
        Stage::BeforePublish => {
            observe(SaveStage::BeforePublish);
            if is_new {
                observe(SaveStage::BeforeCommit);
            }
        }
        Stage::Published => observe(SaveStage::Published),
    }
}

fn stage_decision(stage: spiling_core_ptac::SaveStage, is_new: bool, cancel: &AtomicBool) -> bool {
    stage == spiling_core_ptac::SaveStage::Published
        || (!is_new && stage == spiling_core_ptac::SaveStage::Committed)
        || !cancel.load(Ordering::Acquire)
}

fn parse_manifest(bytes: &[u8]) -> Result<ProjectManifest, ProjectError> {
    if bytes.is_empty() || bytes.len() > MAX_PROJECT_MANIFEST_BYTES as usize {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "project root exceeds its byte envelope",
        ));
    }
    #[derive(serde::Deserialize)]
    struct Format {
        format_version: u32,
    }
    let format: Format = serde_json::from_slice(bytes)
        .map_err(|value| invalid(format!("invalid project root version: {value}")))?;
    if format.format_version != PROJECT_FORMAT_VERSION {
        return Err(error(
            ProjectErrorCode::UnsupportedFormat,
            "unsupported project root format",
        ));
    }
    let mut manifest: ProjectManifest = serde_json::from_slice(bytes)
        .map_err(|value| invalid(format!("invalid project root: {value}")))?;
    manifest.validate()?;
    if let Some(optical) = &mut manifest.appearance_study.optical
        && let Some(report) = &mut optical.report
        && let Some(report) = Arc::get_mut(report)
    {
        report.request = optical.request.clone();
    }
    Ok(manifest)
}

fn parse_guard(bytes: &[u8]) -> Result<ProjectRecoveryGuard, ProjectError> {
    if bytes.is_empty() || bytes.len() > spiling_core_ptac::MAX_GUARD_BYTES {
        return Err(invalid("recovery guard exceeds its byte envelope"));
    }
    let guard: ProjectRecoveryGuard = serde_json::from_slice(bytes)
        .map_err(|value| invalid(format!("invalid recovery guard: {value}")))?;
    guard.validate()?;
    Ok(guard)
}

fn object_id(hash: &SourceHash) -> ObjectId {
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hash.as_str()[index * 2..index * 2 + 2], 16)
            .expect("SourceHash is checked lowercase SHA-256");
    }
    ObjectId::from_bytes(bytes)
}

fn member(role: u16, hash: &SourceHash) -> Member {
    Member {
        role: ObjectRole::new(role).expect("closed nonzero project role"),
        object: object_id(hash),
    }
}

fn expected_members(manifest: &ProjectManifest) -> BTreeSet<Member> {
    let mut result = BTreeSet::new();
    for record in &manifest.analysis.results {
        result.insert(member(ANALYSIS, &record.hash));
    }
    if let Some(record) = &manifest.materials {
        result.insert(member(MATERIALS, &record.hash));
    }
    for definition in &manifest.definitions {
        result.insert(member(SOURCE, &definition.provenance.source_hash));
    }
    for layout in &manifest.layouts {
        if let Some(artifact) = &layout.manufacturing_artifact {
            result.insert(member(MANUFACTURING, &artifact.hash));
        }
    }
    for image in &manifest.surface_overlays.images {
        result.insert(member(IMAGE, &image.image_hash));
    }
    if let Some(palette) = &manifest.appearance_study.palette {
        result.insert(member(PALETTE_ORIGINAL, &palette.original_hash));
        result.insert(member(PALETTE_CANONICAL, &palette.canonical_hash));
    }
    result
}

fn validate_closure(manifest: &ProjectManifest, capture: &Capture<'_>) -> Result<(), ProjectError> {
    let expected = expected_members(manifest);
    if capture.members.len() != expected.len()
        || capture.members.iter().copied().collect::<BTreeSet<_>>() != expected
    {
        return Err(invalid(
            "project snapshot membership is not the exact declared role closure",
        ));
    }
    Ok(())
}

fn read_asset(
    capture: &Capture<'_>,
    hash: &SourceHash,
    budget: usize,
    exact: Option<usize>,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, ProjectError> {
    cancelled(cancel)?;
    let id = object_id(hash);
    if exact.is_some_and(|size| capture.object_length(id).is_ok_and(|actual| actual != size)) {
        return Err(error(
            ProjectErrorCode::CorruptAsset,
            "asset length differs from project metadata",
        ));
    }
    let bytes = capture
        .read_object(id, budget, cancel)
        .map_err(ptac_error)?;
    if bytes.capacity() > budget {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "captured asset allocation exceeds remaining budget",
        ));
    }
    Ok(bytes)
}

fn load_assets(
    manifest: &ProjectManifest,
    capture: &Capture<'_>,
    budgets: LoadBudgets,
    cancel: &AtomicBool,
) -> Result<Snapshot, ProjectError> {
    validate_closure(manifest, capture)?;
    let mut snapshot = Snapshot {
        focused_layout: manifest.layouts[0].layout_id.clone(),
        layouts: manifest
            .layouts
            .iter()
            .map(|l| super::PrintLayoutSnapshot {
                layout_id: l.layout_id.clone(),
                name: l.name.clone(),
                occurrences: l
                    .occurrences
                    .iter()
                    .map(|o| (o.occurrence_id, o.clone()))
                    .collect(),
                manufacturing_intent: l.manufacturing_intent.clone(),
                manufacturing_options: Arc::new(l.manufacturing_options.clone()),
                hardware: l.hardware.clone(),
                manufacturing_artifact: None,
            })
            .collect(),
        surface_overlays: Arc::new(manifest.surface_overlays.clone()),
        process_document: Arc::new(manifest.process_document.clone()),
        appearance_study: Arc::new(manifest.appearance_study.clone()),
        analysis: Arc::new(manifest.analysis.clone()),
        ..Snapshot::default()
    };
    super::hardware::share_capture(&mut snapshot);
    let mut analysis_budget = budgets
        .analysis_bytes
        .min(super::MAX_RETAINED_ANALYSIS_FIELD_BYTES);
    for record in &manifest.analysis.results {
        let bytes = read_asset(
            capture,
            &record.hash,
            analysis_budget.min(spiling_contracts::analysis::mechanics::MAX_MECHANICAL_FIELD_BYTES),
            Some(record.byte_count as usize),
            cancel,
        )?;
        analysis_budget -= bytes.capacity();
        snapshot.analysis_results.insert(
            record.hash.clone(),
            Arc::new(super::AnalysisResultAsset::new(record.clone(), bytes)?),
        );
    }
    if let Some(record) = &manifest.materials {
        let bytes = read_asset(
            capture,
            &record.hash,
            budgets
                .material_bytes
                .min(spiling_contracts::library::MAX_MATERIAL_COLLECTION_BYTES),
            None,
            cancel,
        )?;
        snapshot.materials = Some(Arc::new(super::materials::MaterialAsset::decode(
            record, bytes,
        )?));
    }
    let mut source_budget = budgets.source_bytes.min(MAX_RETAINED_SOURCE_BYTES);
    for record in &manifest.definitions {
        let bytes = read_asset(
            capture,
            &record.provenance.source_hash,
            source_budget.min(MAX_SOURCE_BYTES),
            None,
            cancel,
        )?;
        source_budget -= bytes.capacity();
        snapshot.sources.insert(
            record.definition_id.clone(),
            Arc::new(SourceAsset::new(record.clone(), bytes)?),
        );
    }
    let mut image_budget = budgets.image_bytes.min(MAX_SURFACE_IMAGE_RETAINED_BYTES);
    for record in &manifest.surface_overlays.images {
        let bytes = read_asset(
            capture,
            &record.image_hash,
            image_budget.min(MAX_SURFACE_IMAGE_BYTES),
            Some(record.byte_count as usize),
            cancel,
        )?;
        image_budget -= bytes.capacity();
        snapshot.surface_images.insert(
            record.image_hash.clone(),
            Arc::new(SurfaceImageAsset::new(record.clone(), bytes)?),
        );
    }
    let mut remaining_manufacturing = budgets
        .manufacturing_bytes
        .min(MAX_MANUFACTURING_RETAINED_BYTES as usize);
    let mut loaded_manufacturing = BTreeMap::<SourceHash, Arc<ManufacturingAsset>>::new();
    for (layout, record) in snapshot.layouts.iter_mut().zip(&manifest.layouts) {
        if let Some(record) = &record.manufacturing_artifact {
            let asset = if let Some(asset) = loaded_manufacturing.get(&record.hash) {
                if asset.record != *record {
                    return Err(invalid("shared layout artifact metadata differs"));
                }
                asset.clone()
            } else {
                let bytes = read_asset(
                    capture,
                    &record.hash,
                    remaining_manufacturing.min(MAX_MANUFACTURING_BUNDLE_BYTES as usize),
                    Some(record.byte_count as usize),
                    cancel,
                )?;
                remaining_manufacturing -= bytes.capacity();
                let asset = Arc::new(ManufacturingAsset::new(record.clone(), bytes)?);
                loaded_manufacturing.insert(record.hash.clone(), asset.clone());
                asset
            };
            layout.manufacturing_artifact = Some(asset);
        }
    }
    let metadata = super::appearance_bytes_with_incoming(std::iter::once(&snapshot), None, None)?;
    let mut appearance_budget = budgets
        .appearance_bytes
        .min(MAX_APPEARANCE_RETAINED_BYTES)
        .checked_sub(metadata)
        .ok_or_else(|| {
            error(
                ProjectErrorCode::ResourceLimit,
                "incoming appearance metadata exceeds budget",
            )
        })?;
    if let Some(record) = &manifest.appearance_study.palette {
        appearance_budget = appearance_budget
            .checked_sub(record.allocated_bytes())
            .ok_or_else(|| {
                error(
                    ProjectErrorCode::ResourceLimit,
                    "incoming palette metadata exceeds budget",
                )
            })?;
        let original = read_asset(
            capture,
            &record.original_hash,
            appearance_budget.min(MAX_PALETTE_BYTES),
            Some(record.original_bytes as usize),
            cancel,
        )?;
        appearance_budget -= original.capacity();
        let canonical = read_asset(
            capture,
            &record.canonical_hash,
            appearance_budget.min(MAX_PALETTE_BYTES),
            Some(record.canonical_bytes as usize),
            cancel,
        )?;
        appearance_budget -= canonical.capacity();
        record.validate_hashes(&original, &canonical)?;
        let prepared = PreparedPalette::decode_bounded(&original, &canonical, appearance_budget)?;
        let asset =
            PaletteAsset::from_checked_prepared(record.clone(), original, canonical, prepared)?;
        snapshot
            .appearance_palettes
            .insert(record.canonical_hash.clone(), Arc::new(asset));
    }
    validate_snapshot(&snapshot, manifest)?;
    Ok(snapshot)
}

pub fn open(
    path: &Path,
    read_only: bool,
    recover_previous: bool,
    cancel: &AtomicBool,
) -> Result<LoadedProject, ProjectError> {
    open_reusing(
        path,
        read_only,
        recover_previous,
        cancel,
        None,
        LoadBudgets::MAXIMUM,
    )
}

pub fn open_reusing(
    path: &Path,
    read_only: bool,
    recover_previous: bool,
    cancel: &AtomicBool,
    existing: Option<Arc<Storage>>,
    budgets: LoadBudgets,
) -> Result<LoadedProject, ProjectError> {
    cancelled(cancel)?;
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(error(
            ProjectErrorCode::UnsupportedFormat,
            "projects must be PTAC .spil files; directory projects are unsupported",
        ));
    }
    let storage = match existing {
        Some(storage)
            if !read_only
                && !storage.read_only()
                && storage.container.matches_path(path).map_err(ptac_error)? =>
        {
            storage.container.reopen().map_err(ptac_error)?;
            storage
        }
        _ => Arc::new(Storage {
            container: Container::open_existing(path, read_only).map_err(ptac_error)?,
        }),
    };
    let selection = if recover_previous {
        Selection::Previous
    } else {
        Selection::Current
    };
    let capture = storage
        .container
        .capture(selection, cancel)
        .map_err(ptac_error)?;
    let mut manifest = parse_manifest(&capture.root)?;
    let guard = parse_guard(&capture.guard)?;
    guard.dominates(&manifest)?;
    let snapshot = load_assets(&manifest, &capture, budgets, cancel)?;
    let binding = StorageBinding {
        container: Arc::downgrade(&storage.container),
        heads: capture.heads,
        selected: capture.selected,
        selection,
        guard: guard.clone(),
    };
    drop(capture);
    validate_other_root(&storage.container, &binding, cancel)?;
    let saved_revision = manifest.revision;
    guard.apply_to(&mut manifest)?;
    if recover_previous {
        manifest.revision = manifest.revision.next()?;
    }
    manifest.validate()?;
    cancelled(cancel)?;
    Ok(LoadedProject {
        analysis_results: snapshot.analysis_results,
        manifest,
        saved_revision,
        sources: snapshot.sources,
        layouts: snapshot.layouts,
        surface_images: snapshot.surface_images,
        appearance_palettes: snapshot.appearance_palettes,
        materials: snapshot.materials,
        storage,
        recovered_previous: recover_previous,
        binding,
    })
}

fn validate_other_root(
    container: &Container,
    binding: &StorageBinding,
    cancel: &AtomicBool,
) -> Result<(), ProjectError> {
    let other = match binding.selection {
        Selection::Current if binding.heads.previous().is_some() => Selection::Previous,
        Selection::Current => return Ok(()),
        Selection::Previous => Selection::Current,
    };
    cancelled(cancel)?;
    if let Ok(capture) = container.capture(other, cancel) {
        if capture.heads != binding.heads {
            return Err(error(
                ProjectErrorCode::StaleRevision,
                "container heads changed during project admission",
            ));
        }
        if let Ok(manifest) = parse_manifest(&capture.root) {
            binding.guard.dominates(&manifest)?;
        }
    }
    cancelled(cancel)
}

fn snapshot_objects(snapshot: &Snapshot) -> BTreeMap<Member, Arc<Vec<u8>>> {
    let mut objects = BTreeMap::new();
    for asset in snapshot.analysis_results.values() {
        objects.insert(member(ANALYSIS, &asset.record.hash), asset.bytes.clone());
    }
    if let Some(asset) = &snapshot.materials {
        objects.insert(
            member(MATERIALS, &asset.record().hash),
            asset.bytes().clone(),
        );
    }
    for asset in snapshot.sources.values() {
        objects.insert(
            member(SOURCE, &asset.record.provenance.source_hash),
            asset.bytes.clone(),
        );
    }
    for asset in snapshot
        .layouts
        .iter()
        .filter_map(|l| l.manufacturing_artifact.as_ref())
    {
        objects.insert(
            member(MANUFACTURING, &asset.record.hash),
            asset.bytes.clone(),
        );
    }
    for asset in snapshot.surface_images.values() {
        objects.insert(
            member(IMAGE, &asset.record().image_hash),
            asset.bytes().clone(),
        );
    }
    for asset in snapshot.appearance_palettes.values() {
        objects.insert(
            member(PALETTE_ORIGINAL, &asset.record().original_hash),
            asset.original().clone(),
        );
        objects.insert(
            member(PALETTE_CANONICAL, &asset.record().canonical_hash),
            asset.canonical().clone(),
        );
    }
    objects
}

fn inputs(objects: &BTreeMap<Member, Arc<Vec<u8>>>) -> Vec<ObjectInput<'_>> {
    objects
        .iter()
        .map(|(member, bytes)| ObjectInput {
            role: member.role,
            bytes: bytes.as_slice(),
        })
        .collect()
}

pub(crate) fn remaining_budgets(project: &Project) -> Result<LoadBudgets, ProjectError> {
    let staged: Vec<_> = {
        let staged = project.staged_appearance.lock().map_err(|_| {
            error(
                ProjectErrorCode::Io,
                "prepared snapshot accounting lock poisoned",
            )
        })?;
        staged.iter().filter_map(Weak::upgrade).collect()
    };
    let mut sources = BTreeMap::new();
    let mut manufacturing = BTreeMap::new();
    for snapshot in std::iter::once(&project.current)
        .chain(project.undo.iter())
        .chain(project.redo.iter())
        .chain(staged.iter())
    {
        for asset in snapshot.sources.values() {
            sources.insert(Arc::as_ptr(&asset.bytes), asset.bytes.capacity());
        }
        for asset in snapshot
            .layouts
            .iter()
            .filter_map(|l| l.manufacturing_artifact.as_ref())
        {
            manufacturing.insert(Arc::as_ptr(&asset.bytes), asset.bytes.capacity());
        }
    }
    Ok(LoadBudgets {
        analysis_bytes: super::MAX_RETAINED_ANALYSIS_FIELD_BYTES.saturating_sub(
            super::analysis_results::retained_bytes(
                std::iter::once(project.current.as_ref())
                    .chain(project.undo.iter().map(Arc::as_ref))
                    .chain(project.redo.iter().map(Arc::as_ref))
                    .chain(staged.iter().map(Arc::as_ref)),
            ),
        ),
        material_bytes: super::materials::MAX_RETAINED_MATERIAL_BYTES.saturating_sub(
            super::materials::retained_bytes(
                std::iter::once(project.current.as_ref())
                    .chain(project.undo.iter().map(Arc::as_ref))
                    .chain(project.redo.iter().map(Arc::as_ref))
                    .chain(staged.iter().map(Arc::as_ref)),
            ),
        ),
        source_bytes: MAX_RETAINED_SOURCE_BYTES
            .saturating_sub(allocation_sum(sources.values().copied())?),
        manufacturing_bytes: (MAX_MANUFACTURING_RETAINED_BYTES as usize)
            .saturating_sub(allocation_sum(manufacturing.values().copied())?),
        image_bytes: MAX_SURFACE_IMAGE_RETAINED_BYTES
            .saturating_sub(project.retained_surface_image_bytes()?),
        appearance_bytes: MAX_APPEARANCE_RETAINED_BYTES
            .saturating_sub(project.retained_appearance_bytes()?),
    })
}

fn allocation_sum(mut values: impl Iterator<Item = usize>) -> Result<usize, ProjectError> {
    values.try_fold(0usize, |total, value| {
        total.checked_add(value).ok_or_else(|| {
            error(
                ProjectErrorCode::ResourceLimit,
                "retained allocation arithmetic overflow",
            )
        })
    })
}

pub fn save(
    snapshot: SaveSnapshot,
    existing: Option<Arc<Storage>>,
    target: Option<PathBuf>,
    cancel: &AtomicBool,
    mut observe: impl FnMut(SaveStage),
) -> Result<SaveReceipt, ProjectError> {
    cancelled(cancel)?;
    if existing.as_ref().is_some_and(|storage| storage.read_only()) {
        return Err(error(
            ProjectErrorCode::ReadOnly,
            "read-only storage cannot be saved",
        ));
    }
    validate_snapshot(&snapshot.snapshot, &snapshot.manifest)?;
    let root = serde_json::to_vec_pretty(&snapshot.manifest)
        .map_err(|value| invalid(value.to_string()))?;
    if root.len() > MAX_PROJECT_MANIFEST_BYTES as usize {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "manifest exceeds 1 MiB",
        ));
    }
    let destination = target
        .or_else(|| {
            existing
                .as_ref()
                .map(|storage| storage.path().to_path_buf())
        })
        .ok_or_else(|| {
            error(
                ProjectErrorCode::NoSavedPath,
                "first save requires a new .spil target",
            )
        })?;
    let same = match &existing {
        Some(storage)
            if storage
                .container
                .matches_path(&destination)
                .map_err(ptac_error)? =>
        {
            Some(storage)
        }
        _ => None,
    };
    let mut guard = ProjectRecoveryGuard::from_manifest(&snapshot.manifest);
    let objects = snapshot_objects(&snapshot.snapshot);
    if objects.keys().copied().collect::<BTreeSet<_>>() != expected_members(&snapshot.manifest) {
        return Err(invalid("captured assets differ from declared role closure"));
    }
    let mut previous_root = None;
    let mut previous_objects = BTreeMap::new();
    let mut base = None;
    if let Some(storage) = &existing {
        let binding = snapshot.storage_binding.as_ref().ok_or_else(|| {
            error(
                ProjectErrorCode::Busy,
                "save attachment requires explicit inspect/reopen after an uncertain outcome",
            )
        })?;
        if !binding
            .container
            .upgrade()
            .is_some_and(|container| Arc::ptr_eq(&storage.container, &container))
        {
            return Err(invalid(
                "save snapshot belongs to another storage attachment",
            ));
        }
        let capture = storage
            .container
            .capture(binding.selection, cancel)
            .map_err(ptac_error)?;
        if capture.heads != binding.heads || capture.selected != binding.selected {
            return Err(error(
                ProjectErrorCode::StaleRevision,
                "save/recovery publication binding is stale",
            ));
        }
        let durable_guard = parse_guard(&capture.guard)?;
        if durable_guard != binding.guard {
            return Err(invalid(
                "durable guard changed without its bound publication",
            ));
        }
        capture.verify_all(cancel).map_err(ptac_error)?;
        guard.merge(&durable_guard)?;
        base = Some((binding.heads, binding.selection));
        if same.is_none() {
            let mut available = BTreeMap::new();
            for state in &snapshot.retained {
                available.extend(snapshot_objects(state));
            }
            let mut budgets = snapshot.remaining_budgets.clone()?;
            for member in &capture.members {
                let bytes = if let Some(bytes) = available.get(member) {
                    bytes.clone()
                } else {
                    let budget = match member.role.get() {
                        SOURCE => &mut budgets.source_bytes,
                        MANUFACTURING => &mut budgets.manufacturing_bytes,
                        IMAGE => &mut budgets.image_bytes,
                        MATERIALS => &mut budgets.material_bytes,
                        ANALYSIS => &mut budgets.analysis_bytes,
                        PALETTE_ORIGINAL | PALETTE_CANONICAL => &mut budgets.appearance_bytes,
                        _ => return Err(invalid("unknown retained project object role")),
                    };
                    let bytes = capture
                        .read_object(member.object, *budget, cancel)
                        .map_err(ptac_error)?;
                    *budget = budget.checked_sub(bytes.capacity()).ok_or_else(|| {
                        error(
                            ProjectErrorCode::ResourceLimit,
                            "Save As retained asset budget exceeded",
                        )
                    })?;
                    Arc::new(bytes)
                };
                previous_objects.insert(*member, bytes);
            }
            previous_root = Some(capture.root.clone());
        }
    } else if snapshot.storage_binding.is_some() {
        return Err(invalid(
            "attached save snapshot requires its storage attachment",
        ));
    }
    let guard_bytes = serde_json::to_vec(&guard).map_err(|value| invalid(value.to_string()))?;
    let object_inputs = inputs(&objects);
    let input = SnapshotInput {
        root: &root,
        objects: &object_inputs,
        guard: &guard_bytes,
    };
    let is_new = same.is_none();
    let mut observer = |stage| {
        observe_stage(stage, is_new, &mut observe);
        stage_decision(stage, is_new, cancel)
    };
    let publication = if let Some(storage) = same {
        let (heads, selection) = base.expect("attached save has a validated base");
        storage
            .container
            .publish(heads, selection, input, cancel, &mut observer)
    } else if let Some(previous_root) = &previous_root {
        let previous_inputs = inputs(&previous_objects);
        let previous = SnapshotInput {
            root: previous_root,
            objects: &previous_inputs,
            guard: &guard_bytes,
        };
        Container::create_new_retaining(&destination, input, previous, cancel, &mut observer)
    } else {
        Container::create_new(&destination, input, cancel, &mut observer)
    }
    .map_err(ptac_error)?;
    let outcome = publication.outcome;
    let binding = if outcome == PublicationOutcome::OutcomeUnknown {
        None
    } else {
        Some(StorageBinding {
            container: Arc::downgrade(&publication.container),
            heads: publication.heads,
            selected: publication.heads.current(),
            selection: Selection::Current,
            guard,
        })
    };
    let durability_error = publication_error(outcome, publication.error);
    Ok(SaveReceipt {
        storage: Arc::new(Storage {
            container: publication.container,
        }),
        manifest: snapshot.manifest,
        outcome,
        durability_error,
        binding,
    })
}

#[cfg(test)]
mod tests;
