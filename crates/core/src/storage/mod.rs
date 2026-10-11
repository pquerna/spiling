// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete source-backed checkpoints; no automatic recovery or asset GC.
mod os;
use super::{
    MAX_RETAINED_SOURCE_BYTES, MAX_SOURCE_BYTES, ManufacturingAsset, SaveSnapshot, Snapshot,
    SourceAsset, error, invalid, validate_snapshot,
};
use os::Directory;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spiling_contracts::{
    geometry::{DefinitionId, SourceHash},
    manufacturing::{MAX_MANUFACTURING_BUNDLE_BYTES, MAX_MANUFACTURING_RETAINED_BYTES},
    project::*,
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const CURRENT: &str = "manifest.json";
const PREVIOUS: &str = "previous.json";
const LOCK: &str = "writer.lock";
const SOURCES: &str = "sources";
const MANUFACTURING: &str = "manufacturing";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStage {
    AssetsSynced,
    BeforeManifestReplace,
    AfterManifestReplace,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryRecord {
    manifest: ProjectManifest,
    revision_high_water: ProjectRevision,
    next_occurrence_high_water: u32,
}
#[derive(Debug)]
pub struct Storage {
    path: PathBuf,
    directory: Directory,
    sources: Directory,
    manufacturing: Directory,
    // Never unlink/recreate this inode. File's OS lock is released by RAII.
    _writer_lock: Option<File>,
    operation: Mutex<()>,
}
impl Storage {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn read_only(&self) -> bool {
        self._writer_lock.is_none()
    }
    fn validate_attachment(&self) -> Result<(), ProjectError> {
        self.directory.check_path().map_err(io)?;
        if !self
            .sources
            .same(&self.directory.child(SOURCES).map_err(io)?)
            .map_err(io)?
        {
            return Err(invalid("attached source directory was substituted"));
        }
        if !self
            .manufacturing
            .same(&self.directory.child(MANUFACTURING).map_err(io)?)
            .map_err(io)?
        {
            return Err(invalid("attached manufacturing directory was substituted"));
        }
        if let Some(lock) = &self._writer_lock
            && !self.directory.same_entry(LOCK, lock).map_err(io)?
        {
            return Err(invalid("writer lock inode was substituted"));
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct LoadedProject {
    pub manifest: ProjectManifest,
    pub saved_revision: ProjectRevision,
    pub sources: BTreeMap<DefinitionId, Arc<SourceAsset>>,
    pub manufacturing_artifact: Option<Arc<ManufacturingAsset>>,
    pub storage: Arc<Storage>,
    pub recovered_previous: bool,
}
#[derive(Debug)]
pub struct SaveReceipt {
    pub storage: Arc<Storage>,
    pub manifest: ProjectManifest,
    pub durability_error: Option<ProjectError>,
}
fn io(error_value: std::io::Error) -> ProjectError {
    error(
        if error_value.kind() == std::io::ErrorKind::FileTooLarge {
            ProjectErrorCode::ResourceLimit
        } else {
            ProjectErrorCode::Io
        },
        error_value.to_string(),
    )
}
fn cancelled(cancel: &AtomicBool) -> Result<(), ProjectError> {
    if cancel.load(Ordering::Acquire) {
        Err(error(
            ProjectErrorCode::Cancelled,
            "project operation cancelled before commit",
        ))
    } else {
        Ok(())
    }
}
fn bounded_read(
    directory: &Directory,
    name: &str,
    max: usize,
    missing: ProjectErrorCode,
) -> Result<Vec<u8>, ProjectError> {
    bounded_read_exact(directory, name, max, missing, None)
}
fn bounded_read_exact(
    directory: &Directory,
    name: &str,
    max: usize,
    missing: ProjectErrorCode,
    exact_size: Option<usize>,
) -> Result<Vec<u8>, ProjectError> {
    let mut file = directory.read(name).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            error(missing, format!("missing project file {name}"))
        } else {
            io(e)
        }
    })?;
    let size = file.metadata().map_err(io)?.len();
    if size == 0 {
        return Err(error(
            if missing == ProjectErrorCode::MissingAsset {
                ProjectErrorCode::CorruptAsset
            } else {
                ProjectErrorCode::InvalidProject
            },
            "project file is empty",
        ));
    }
    if size > max as u64 {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            format!("project file {name} exceeds size budget"),
        ));
    }
    if exact_size.is_some_and(|expected| size != expected as u64) {
        return Err(error(
            ProjectErrorCode::CorruptAsset,
            "immutable asset metadata length mismatch",
        ));
    }
    // Capture only the admitted allocation, even if another process grows the file.
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            error(
                ProjectErrorCode::CorruptAsset,
                "project file shrank during capture",
            )
        } else {
            io(e)
        }
    })?;
    if file.read(&mut [0u8; 1]).map_err(io)? != 0 {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "project file grew during capture",
        ));
    }
    Ok(bytes)
}
fn parse_manifest(bytes: &[u8]) -> Result<ProjectManifest, ProjectError> {
    let manifest: ProjectManifest =
        serde_json::from_slice(bytes).map_err(|e| invalid(format!("invalid manifest: {e}")))?;
    manifest.validate()?;
    Ok(manifest)
}
fn parse_recovery(bytes: &[u8]) -> Result<RecoveryRecord, ProjectError> {
    let record: RecoveryRecord = serde_json::from_slice(bytes)
        .map_err(|e| invalid(format!("invalid recovery checkpoint: {e}")))?;
    record.manifest.validate()?;
    if record.next_occurrence_high_water < record.manifest.next_occurrence
        || record.revision_high_water < record.manifest.revision
    {
        return Err(invalid(
            "recovery high-water is below checkpoint identity counters",
        ));
    }
    Ok(record)
}
fn durability_failure(e: std::io::Error) -> ProjectError {
    error(
        ProjectErrorCode::Io,
        format!(
            "Checkpoint was published, but durability confirmation failed: {e}. Crash durability is uncertain; inspect project status before retrying."
        ),
    )
}
fn load_sources(
    manifest: &ProjectManifest,
    sources: &Directory,
    source_budget: usize,
    cancel: &AtomicBool,
) -> Result<BTreeMap<DefinitionId, Arc<SourceAsset>>, ProjectError> {
    let source_budget = source_budget.min(MAX_RETAINED_SOURCE_BYTES);
    let mut result = BTreeMap::new();
    let mut total = 0usize;
    for record in &manifest.definitions {
        cancelled(cancel)?;
        let name = format!("{}.step", record.provenance.source_hash.as_str());
        let bytes = bounded_read(
            sources,
            &name,
            MAX_SOURCE_BYTES.min(source_budget - total),
            ProjectErrorCode::MissingAsset,
        )?;
        total += bytes.len(); // bounded_read admits at most the remaining budget.
        let source = Arc::new(SourceAsset::new(record.clone(), bytes)?);
        result.insert(record.definition_id.clone(), source);
    }
    Ok(result)
}
fn load_manufacturing(
    manifest: &ProjectManifest,
    directory: &Directory,
    budget: usize,
    cancel: &AtomicBool,
) -> Result<Option<Arc<ManufacturingAsset>>, ProjectError> {
    let Some(record) = &manifest.manufacturing_artifact else {
        return Ok(None);
    };
    cancelled(cancel)?;
    let bytes = bounded_read_exact(
        directory,
        &format!("{}.json", record.hash.as_str()),
        (MAX_MANUFACTURING_BUNDLE_BYTES as usize)
            .min(budget.min(MAX_MANUFACTURING_RETAINED_BYTES as usize)),
        ProjectErrorCode::MissingAsset,
        Some(record.byte_count as usize),
    )?;
    Ok(Some(Arc::new(ManufacturingAsset::new(
        record.clone(),
        bytes,
    )?)))
}
fn validate_stored_manufacturing(
    manifest: &ProjectManifest,
    directory: &Directory,
    cancel: &AtomicBool,
) -> Result<(), ProjectError> {
    if let Some(asset) = load_manufacturing(
        manifest,
        directory,
        MAX_MANUFACTURING_BUNDLE_BYTES as usize,
        cancel,
    )? {
        // Compare semantic input without capturing source bytes a second time.
        let bundle = asset.validate()?;
        let provenance = &bundle.provenance;
        if provenance.project_id != manifest.project_id
            || provenance.input_revision > manifest.revision
            || provenance.intent
                != *manifest
                    .manufacturing_intent
                    .as_deref()
                    .ok_or_else(|| invalid("artifact has no intent"))?
            || provenance.definitions.len() != manifest.definitions.len()
            || provenance.occurrences.len() != manifest.occurrences.len()
            || provenance
                .definitions
                .iter()
                .any(|r| !manifest.definitions.contains(r))
            || provenance
                .occurrences
                .iter()
                .any(|r| !manifest.occurrences.contains(r))
        {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "stored manufacturing provenance differs from checkpoint",
            ));
        }
    }
    Ok(())
}
fn validate_root(directory: &Directory) -> Result<(), ProjectError> {
    for name in directory.entries(32).map_err(io)? {
        match name.as_str() {
            CURRENT | PREVIOUS | LOCK => {
                directory.read(&name).map_err(io)?;
            }
            SOURCES | MANUFACTURING => {
                directory.child(&name).map_err(io)?;
            }
            _ if temporary_name(&name) => match directory.read(&name) {
                Ok(file) => {
                    if file.metadata().map_err(io)?.len() > MAX_PROJECT_MANIFEST_BYTES as u64 {
                        return Err(error(
                            ProjectErrorCode::ResourceLimit,
                            "temporary manifest exceeds 1 MiB",
                        ));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(io(e)),
            },
            _ => return Err(invalid("unexpected entry in project directory")),
        }
    }
    Ok(())
}
fn temporary_name(name: &str) -> bool {
    name.strip_prefix(".tmp-").is_some_and(|id| {
        uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id)
    })
}
fn asset_name(name: &str, suffix: &str) -> bool {
    name.strip_suffix(suffix).is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn asset_budget(
    sources: &Directory,
    suffix: &str,
    max_asset_bytes: usize,
) -> Result<(usize, u64, u64), ProjectError> {
    let mut count = 0;
    let mut total = 0u64;
    let mut scratch_count = 0;
    let mut scratch_bytes = 0u64;
    for name in sources.entries(MAX_PROJECT_STORED_ASSETS + 8).map_err(io)? {
        let temporary = temporary_name(&name);
        if !asset_name(&name, suffix) && !temporary {
            return Err(invalid("unexpected immutable asset entry"));
        }
        let file = match sources.read(&name) {
            Ok(file) => file,
            Err(e) if temporary && e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io(e)),
        };
        let size = file.metadata().map_err(io)?.len();
        if size == 0 && !temporary {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "immutable asset is empty",
            ));
        }
        if size > max_asset_bytes as u64 {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "stored asset exceeds byte limit",
            ));
        }
        if temporary {
            scratch_count += 1;
            scratch_bytes += size;
        } else {
            total += size;
            count += 1;
        }
    }
    if count > MAX_PROJECT_STORED_ASSETS
        || total > MAX_PROJECT_STORED_SOURCE_BYTES
        || scratch_count > 8
        || scratch_bytes > max_asset_bytes as u64
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "persisted asset/scratch budget exhausted",
        ));
    }
    Ok((count, total, scratch_bytes))
}
fn validate_stored_source(
    sources: &Directory,
    record: &StoredDefinition,
    expected: Option<&[u8]>,
    cancel: &AtomicBool,
) -> Result<(), ProjectError> {
    let name = format!("{}.step", record.provenance.source_hash.as_str());
    let mut file = sources.read(&name).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            error(ProjectErrorCode::MissingAsset, "missing immutable source")
        } else {
            io(e)
        }
    })?;
    let size = file.metadata().map_err(io)?.len();
    if size == 0 {
        return Err(error(
            ProjectErrorCode::CorruptAsset,
            "immutable source is empty",
        ));
    }
    if size > MAX_SOURCE_BYTES as u64 {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "immutable source exceeds 16 MiB",
        ));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut offset = 0usize;
    loop {
        cancelled(cancel)?;
        let count = file.read(&mut buffer).map_err(io)?;
        if count == 0 {
            break;
        }
        let end = offset
            .checked_add(count)
            .ok_or_else(|| invalid("source size overflow"))?;
        if end > MAX_SOURCE_BYTES {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "immutable source grew beyond 16 MiB",
            ));
        }
        if expected.is_some_and(|bytes| bytes.get(offset..end) != Some(&buffer[..count])) {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "immutable source differs from captured bytes",
            ));
        }
        if expected.is_none() {
            hash.update(&buffer[..count]);
        }
        offset = end;
    }
    if expected.is_some_and(|bytes| offset != bytes.len()) {
        return Err(error(
            ProjectErrorCode::CorruptAsset,
            "immutable source length mismatch",
        ));
    }
    if expected.is_none() {
        let digest: [u8; 32] = hash.finalize().into();
        if SourceHash::from_digest(&digest) != record.provenance.source_hash {
            return Err(error(
                ProjectErrorCode::CorruptAsset,
                "immutable source hash mismatch",
            ));
        }
    }
    Ok(())
}
fn writer_lock(directory: &Directory) -> Result<File, ProjectError> {
    // Opening an existing inode does not truncate it. Read-only never creates it.
    let file = directory.lock_file(LOCK).map_err(io)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(error(
            ProjectErrorCode::ProjectLocked,
            "project is held by another writer",
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(io(e)),
    }
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
        MAX_RETAINED_SOURCE_BYTES,
        MAX_MANUFACTURING_RETAINED_BYTES as usize,
    )
}
/// Same-path staged writer reopen retains the original lock until publication.
/// The incoming byte budget excludes allocations still retained by the caller.
pub fn open_reusing(
    path: &Path,
    read_only: bool,
    recover_previous: bool,
    cancel: &AtomicBool,
    existing: Option<Arc<Storage>>,
    source_budget: usize,
    manufacturing_budget: usize,
) -> Result<LoadedProject, ProjectError> {
    cancelled(cancel)?;
    let directory = Directory::open(path).map_err(io)?;
    let canonical = directory.path().to_path_buf();
    let storage = match existing {
        Some(storage) if !read_only && !storage.read_only() && storage.path == canonical => {
            if !storage.directory.same(&directory).map_err(io)? {
                return Err(invalid("attached project directory was substituted"));
            }
            storage
        }
        _ => {
            validate_root(&directory)?;
            let sources = directory.child(SOURCES).map_err(io)?;
            let manufacturing = directory.child(MANUFACTURING).map_err(io)?;
            directory.read(LOCK).map_err(io)?;
            let lock = if read_only {
                None
            } else {
                Some(writer_lock(&directory)?)
            };
            Arc::new(Storage {
                path: canonical,
                directory,
                sources,
                manufacturing,
                _writer_lock: lock,
                operation: Mutex::new(()),
            })
        }
    };
    let operation = storage
        .operation
        .lock()
        .map_err(|_| error(ProjectErrorCode::Io, "storage operation lock poisoned"))?;
    storage.validate_attachment()?;
    validate_root(&storage.directory)?;
    asset_budget(&storage.sources, ".step", MAX_SOURCE_BYTES)?;
    asset_budget(
        &storage.manufacturing,
        ".json",
        MAX_MANUFACTURING_BUNDLE_BYTES as usize,
    )?;
    let bytes = bounded_read(
        &storage.directory,
        if recover_previous { PREVIOUS } else { CURRENT },
        MAX_PROJECT_MANIFEST_BYTES as usize,
        ProjectErrorCode::InvalidProject,
    )?;
    let (manifest, saved_revision) = if recover_previous {
        let record = parse_recovery(&bytes)?;
        let saved_revision = record.manifest.revision;
        let mut manifest = record.manifest;
        manifest.revision = manifest.revision.max(record.revision_high_water).next()?;
        manifest.next_occurrence = manifest
            .next_occurrence
            .max(record.next_occurrence_high_water);
        (manifest, saved_revision)
    } else {
        let manifest = parse_manifest(&bytes)?;
        let saved_revision = manifest.revision;
        (manifest, saved_revision)
    };
    let sources = load_sources(&manifest, &storage.sources, source_budget, cancel)?;
    let manufacturing_artifact = load_manufacturing(
        &manifest,
        &storage.manufacturing,
        manufacturing_budget,
        cancel,
    )?;
    let snapshot = Snapshot {
        sources: sources.clone(),
        occurrences: manifest
            .occurrences
            .iter()
            .map(|r| (r.occurrence_id, r.clone()))
            .collect(),
        manufacturing_intent: manifest.manufacturing_intent.clone(),
        manufacturing_artifact: manufacturing_artifact.clone(),
    };
    validate_snapshot(&snapshot, &manifest)?;
    cancelled(cancel)?;
    manifest.validate()?;
    drop(operation);
    Ok(LoadedProject {
        manifest,
        saved_revision,
        sources,
        manufacturing_artifact,
        storage,
        recovered_previous: recover_previous,
    })
}
struct Temporary<'a> {
    directory: &'a Directory,
    name: String,
    file: File,
    published: bool,
}
impl<'a> Temporary<'a> {
    fn write(directory: &'a Directory, bytes: &[u8]) -> Result<Self, ProjectError> {
        let name = format!(".tmp-{}", uuid::Uuid::new_v4());
        let file = directory.create(&name).map_err(io)?;
        let mut temporary = Self {
            directory,
            name,
            file,
            published: false,
        };
        temporary.file.write_all(bytes).map_err(io)?;
        os::sync_file(&temporary.file).map_err(io)?;
        Ok(temporary)
    }
    fn replace(mut self, destination: &str) -> Result<(), ProjectError> {
        self.directory
            .replace(&self.name, destination)
            .map_err(io)?;
        self.published = true;
        Ok(())
    }
    fn publish_asset(mut self, destination: &str) -> Result<(), ProjectError> {
        self.directory
            .publish_file(&self.name, destination)
            .map_err(io)?;
        self.published = true;
        Ok(())
    }
}
impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = self.directory.remove_owned(&self.name, &self.file);
            self.directory.forget_owned(&self.name);
        }
    }
}
// Failure leaves at most an exclusively named sibling stage. Cleanup never
// recursively traverses it and never removes a substituted directory or entry.
struct Stage {
    directory: Directory,
    sources: Directory,
    manufacturing: Option<Directory>,
    parent: Directory,
    name: String,
    published: bool,
}
impl Drop for Stage {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        let _ = self.sources.clean_owned();
        if let Some(manufacturing) = &self.manufacturing {
            let _ = manufacturing.clean_owned();
            let _ = self
                .directory
                .remove_child_owned(MANUFACTURING, manufacturing);
        }
        let _ = self.directory.clean_owned();
        let _ = self.directory.remove_child_owned(SOURCES, &self.sources);
        let _ = self.parent.remove_child_owned(&self.name, &self.directory);
    }
}
pub fn save(
    snapshot: SaveSnapshot,
    existing: Option<Arc<Storage>>,
    target: Option<PathBuf>,
    cancel: &AtomicBool,
    mut observe: impl FnMut(SaveStage),
) -> Result<SaveReceipt, ProjectError> {
    cancelled(cancel)?;
    if existing.as_ref().is_some_and(|s| s.read_only()) {
        return Err(error(
            ProjectErrorCode::ReadOnly,
            "read-only storage cannot be saved",
        ));
    }
    validate_snapshot(&snapshot.snapshot, &snapshot.manifest)?;
    let bytes =
        serde_json::to_vec_pretty(&snapshot.manifest).map_err(|e| invalid(e.to_string()))?;
    if bytes.len() > MAX_PROJECT_MANIFEST_BYTES as usize {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "manifest exceeds 1 MiB",
        ));
    }
    let destination = target
        .or_else(|| existing.as_ref().map(|s| s.path.clone()))
        .ok_or_else(|| {
            error(
                ProjectErrorCode::NoSavedPath,
                "first save requires a new directory target",
            )
        })?;
    let same = existing
        .as_ref()
        .filter(|s| std::fs::canonicalize(&destination).is_ok_and(|p| p == s.path));
    if let Some(storage) = same {
        let storage = storage.clone();
        let operation = storage
            .operation
            .lock()
            .map_err(|_| error(ProjectErrorCode::Io, "storage operation lock poisoned"))?;
        validate_root(&storage.directory)?;
        storage.validate_attachment()?;
        let current = bounded_read(
            &storage.directory,
            CURRENT,
            MAX_PROJECT_MANIFEST_BYTES as usize,
            ProjectErrorCode::InvalidProject,
        )
        .and_then(|bytes| parse_manifest(&bytes));
        if current
            .as_ref()
            .is_ok_and(|manifest| manifest.project_id != snapshot.manifest.project_id)
        {
            return Err(invalid(
                "save cannot replace a different project's manifest",
            ));
        }
        let current = current.and_then(|manifest| {
            for record in &manifest.definitions {
                validate_stored_source(&storage.sources, record, None, cancel)?;
            }
            validate_stored_manufacturing(&manifest, &storage.manufacturing, cancel)?;
            Ok(manifest)
        });
        let old_manifest = match current {
            Ok(manifest) => manifest,
            Err(e) if snapshot.recovered_previous && e.code != ProjectErrorCode::Cancelled => {
                let bytes = bounded_read(
                    &storage.directory,
                    PREVIOUS,
                    MAX_PROJECT_MANIFEST_BYTES as usize,
                    ProjectErrorCode::InvalidProject,
                )?;
                let manifest = parse_recovery(&bytes)?.manifest;
                for record in &manifest.definitions {
                    validate_stored_source(&storage.sources, record, None, cancel)?;
                }
                validate_stored_manufacturing(&manifest, &storage.manufacturing, cancel)?;
                manifest
            }
            Err(e) => return Err(e),
        };
        if old_manifest.project_id != snapshot.manifest.project_id {
            return Err(invalid(
                "recovery cannot replace a different project's manifest",
            ));
        }
        let recovery = RecoveryRecord {
            revision_high_water: snapshot.manifest.revision.max(old_manifest.revision),
            next_occurrence_high_water: snapshot
                .manifest
                .next_occurrence
                .max(old_manifest.next_occurrence),
            manifest: old_manifest,
        };
        let previous = serde_json::to_vec_pretty(&recovery).map_err(|e| invalid(e.to_string()))?;
        if previous.len() > MAX_PROJECT_MANIFEST_BYTES as usize {
            return Err(error(
                ProjectErrorCode::ResourceLimit,
                "recovery checkpoint exceeds 1 MiB",
            ));
        }
        save_assets(&snapshot.snapshot, &storage.sources, cancel)?;
        save_manufacturing(&snapshot.snapshot, &storage.manufacturing, cancel)?;
        observe(SaveStage::AssetsSynced);
        cancelled(cancel)?;
        if storage.directory.exists(PREVIOUS).map_err(io)? {
            let prior = bounded_read(
                &storage.directory,
                PREVIOUS,
                MAX_PROJECT_MANIFEST_BYTES as usize,
                ProjectErrorCode::InvalidProject,
            )?;
            let prior = parse_recovery(&prior)?;
            if prior.manifest.project_id != snapshot.manifest.project_id {
                return Err(invalid(
                    "existing recovery checkpoint belongs to a different project",
                ));
            }
            for record in &prior.manifest.definitions {
                validate_stored_source(&storage.sources, record, None, cancel)?;
            }
            validate_stored_manufacturing(&prior.manifest, &storage.manufacturing, cancel)?;
        }
        let temporary = Temporary::write(&storage.directory, &bytes)?;
        Temporary::write(&storage.directory, &previous)?.replace(PREVIOUS)?;
        storage.directory.sync().map_err(io)?;
        observe(SaveStage::BeforeManifestReplace);
        storage.validate_attachment()?;
        cancelled(cancel)?;
        // Commit decision: cancellation is ignored from here through receipt.
        temporary.replace(CURRENT)?;
        observe(SaveStage::AfterManifestReplace);
        let durability_error = storage.directory.sync().err().map(durability_failure);
        drop(operation);
        Ok(SaveReceipt {
            storage,
            manifest: snapshot.manifest,
            durability_error,
        })
    } else {
        let name = destination
            .file_name()
            .ok_or_else(|| invalid("save target needs a directory basename"))?
            .to_owned();
        let parent_path = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent = Directory::open(parent_path).map_err(io)?;
        if parent.exists_native(&name).map_err(io)? {
            return Err(invalid("new save destination must not exist"));
        }
        let stage_name = format!(".spiling-stage-{}", uuid::Uuid::new_v4());
        let directory = parent.create_child(&stage_name).map_err(io)?;
        let sources = match directory.create_child(SOURCES) {
            Ok(sources) => sources,
            Err(e) => {
                let _ = parent.remove_child_owned(&stage_name, &directory);
                return Err(io(e));
            }
        };
        let mut stage = Stage {
            directory,
            sources,
            manufacturing: None,
            parent,
            name: stage_name,
            published: false,
        };
        stage.manufacturing = Some(stage.directory.create_child(MANUFACTURING).map_err(io)?);
        let lock = writer_lock(&stage.directory)?;
        let path = stage.parent.path().join(&name);
        let directory = stage.directory.relocated(path.clone()).map_err(io)?;
        let sources = stage.sources.relocated(path.join(SOURCES)).map_err(io)?;
        let staged_manufacturing = stage.manufacturing.as_ref().expect("created directory");
        let manufacturing = staged_manufacturing
            .relocated(path.join(MANUFACTURING))
            .map_err(io)?;
        save_assets(&snapshot.snapshot, &stage.sources, cancel)?;
        save_manufacturing(&snapshot.snapshot, staged_manufacturing, cancel)?;
        observe(SaveStage::AssetsSynced);
        cancelled(cancel)?;
        let temporary = Temporary::write(&stage.directory, &bytes)?;
        observe(SaveStage::BeforeManifestReplace);
        stage.parent.check_path().map_err(io)?;
        if !stage
            .parent
            .child(&stage.name)
            .map_err(io)?
            .same(&stage.directory)
            .map_err(io)?
            || !stage
                .directory
                .child(SOURCES)
                .map_err(io)?
                .same(&stage.sources)
                .map_err(io)?
            || !stage
                .directory
                .child(MANUFACTURING)
                .map_err(io)?
                .same(staged_manufacturing)
                .map_err(io)?
        {
            return Err(invalid(
                "owned first-save staging directory was substituted",
            ));
        }
        cancelled(cancel)?;
        temporary.replace(CURRENT)?;
        stage.directory.sync().map_err(io)?;
        stage
            .parent
            .publish_directory(&stage.name, &name)
            .map_err(io)?;
        stage.published = true;
        observe(SaveStage::AfterManifestReplace);
        let durability_error = stage.parent.sync().err().map(durability_failure);
        let storage = Arc::new(Storage {
            path,
            directory,
            sources,
            manufacturing,
            _writer_lock: Some(lock),
            operation: Mutex::new(()),
        });
        Ok(SaveReceipt {
            storage,
            manifest: snapshot.manifest,
            durability_error,
        })
    }
}
fn save_assets(
    snapshot: &Snapshot,
    sources: &Directory,
    cancel: &AtomicBool,
) -> Result<(), ProjectError> {
    let (mut count, mut total, scratch_bytes) = asset_budget(sources, ".step", MAX_SOURCE_BYTES)?;
    for asset in snapshot.sources.values() {
        cancelled(cancel)?;
        let name = format!("{}.step", asset.record.provenance.source_hash.as_str());
        if sources.exists(&name).map_err(io)? {
            validate_stored_source(sources, &asset.record, Some(asset.bytes.as_slice()), cancel)?;
        } else {
            count += 1;
            total += asset.bytes.len() as u64;
            if count > MAX_PROJECT_STORED_ASSETS
                || total > MAX_PROJECT_STORED_SOURCE_BYTES
                || scratch_bytes + asset.bytes.len() as u64 > MAX_SOURCE_BYTES as u64
            {
                return Err(error(
                    ProjectErrorCode::ResourceLimit,
                    "persisted asset/scratch budget exhausted",
                ));
            }
            Temporary::write(sources, asset.bytes.as_slice())?.publish_asset(&name)?;
        }
    }
    sources.sync().map_err(io)
}
fn save_manufacturing(
    snapshot: &Snapshot,
    directory: &Directory,
    cancel: &AtomicBool,
) -> Result<(), ProjectError> {
    let (count, total, scratch_bytes) =
        asset_budget(directory, ".json", MAX_MANUFACTURING_BUNDLE_BYTES as usize)?;
    if let Some(asset) = &snapshot.manufacturing_artifact {
        cancelled(cancel)?;
        let name = format!("{}.json", asset.record.hash.as_str());
        if directory.exists(&name).map_err(io)? {
            let bytes = bounded_read_exact(
                directory,
                &name,
                MAX_MANUFACTURING_BUNDLE_BYTES as usize,
                ProjectErrorCode::MissingAsset,
                Some(asset.record.byte_count as usize),
            )?;
            if bytes.as_slice() != asset.bytes.as_slice() {
                return Err(error(
                    ProjectErrorCode::CorruptAsset,
                    "immutable manufacturing bytes differ from capture",
                ));
            }
        } else {
            if count + 1 > MAX_PROJECT_STORED_ASSETS
                || total + asset.bytes.len() as u64 > MAX_PROJECT_STORED_SOURCE_BYTES
                || scratch_bytes + asset.bytes.len() as u64 > MAX_MANUFACTURING_BUNDLE_BYTES as u64
            {
                return Err(error(
                    ProjectErrorCode::ResourceLimit,
                    "persisted manufacturing asset/scratch budget exhausted",
                ));
            }
            Temporary::write(directory, asset.bytes.as_slice())?.publish_asset(&name)?;
        }
    }
    directory.sync().map_err(io)
}
#[cfg(test)]
mod tests;
