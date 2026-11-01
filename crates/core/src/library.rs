// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Single-writer personal-library publication and immutable material dependency closure.
use serde::{Deserialize, Serialize};
use spiling_contracts::{geometry::SourceHash, library::*};
use spiling_core_ptac::{
    Container, DocumentProfile, Heads, ObjectInput, ObjectRole, PublicationOutcome, Selection,
    SnapshotInput,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

const MATERIALS_ROLE: u16 = 1;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
type Result<T> = std::result::Result<T, LibraryError>;
fn invalid(message: impl Into<String>) -> LibraryError {
    LibraryError::new(LibraryErrorCode::InvalidData, message)
}
fn conflict() -> LibraryError {
    LibraryError::new(
        LibraryErrorCode::StaleRevision,
        "library or item changed; refresh and review the retained draft",
    )
}
fn ptac(error: spiling_core_ptac::Error) -> LibraryError {
    use spiling_core_ptac::Error as E;
    let code = match &error {
        E::Locked => LibraryErrorCode::Locked,
        E::ReadOnly => LibraryErrorCode::ReadOnly,
        E::Conflict => LibraryErrorCode::StaleRevision,
        E::ResourceLimit(_) => LibraryErrorCode::ResourceLimit,
        E::Cancelled => LibraryErrorCode::Cancelled,
        E::Io(e) if e.kind() == std::io::ErrorKind::NotFound => LibraryErrorCode::NotFound,
        E::Io(_) | E::Sqlite(_) => LibraryErrorCode::Io,
        _ => LibraryErrorCode::InvalidData,
    };
    LibraryError::new(code, error.to_string())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionRecord {
    sha256: SourceHash,
    byte_count: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    id: LibraryId,
    revision: u64,
    materials: CollectionRecord,
    hardware: spiling_contracts::hardware::HardwareLibrary,
    spools: Vec<SpoolRecord>,
    receipts: Vec<LibraryReceipt>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Guard {
    schema: u32,
    id: LibraryId,
    revision_floor: u64,
}
struct Loaded {
    document: LibraryDocument,
    container: Arc<Container>,
    heads: Heads,
    floor: u64,
}
#[derive(Debug, Clone)]
pub struct LibraryStore {
    path: PathBuf,
}
#[derive(Debug)]
pub struct LibraryCommit {
    pub document: LibraryDocument,
    pub outcome: PublicationOutcome,
    pub replayed: bool,
    pub publication_error: Option<String>,
}
impl LibraryCommit {
    pub fn receipt(self, request_id: String) -> LibraryWriteReceipt {
        LibraryWriteReceipt {
            identity: LibraryIdentity {
                id: self.document.id,
                revision: self.document.revision,
            },
            request_id,
            publication: match self.outcome {
                PublicationOutcome::PublishedConfirmed => LibraryPublication::Confirmed,
                PublicationOutcome::PublishedUnconfirmed => LibraryPublication::Unconfirmed,
                PublicationOutcome::OutcomeUnknown => LibraryPublication::Unknown,
            },
            replayed: self.replayed,
            message: self.publication_error,
        }
    }
}
impl LibraryStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn hardware(
        &self,
        cancel: &AtomicBool,
    ) -> Result<spiling_contracts::hardware::LibraryHardware> {
        let document = self.read(cancel)?;
        Ok(spiling_contracts::hardware::LibraryHardware {
            identity: document.as_ref().map(identity),
            hardware: document.map(|d| d.hardware).unwrap_or_default(),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn read(&self, cancel: &AtomicBool) -> Result<Option<LibraryDocument>> {
        if !self.exists()? {
            return Ok(None);
        }
        Ok(Some(self.load(true, Selection::Current, cancel)?.document))
    }
    fn exists(&self) -> Result<bool> {
        match std::fs::symlink_metadata(&self.path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(LibraryError::new(LibraryErrorCode::Io, e.to_string())),
        }
    }
    fn load(&self, read_only: bool, selection: Selection, cancel: &AtomicBool) -> Result<Loaded> {
        let container = Container::open_existing_for_profile(
            &self.path,
            read_only,
            DocumentProfile::UserLibrary,
        )
        .map_err(ptac)?;
        let capture = container.capture(selection, cancel).map_err(ptac)?;
        let manifest: Manifest =
            serde_json::from_slice(&capture.root).map_err(|e| invalid(e.to_string()))?;
        let guard: Guard =
            serde_json::from_slice(&capture.guard).map_err(|e| invalid(e.to_string()))?;
        if manifest.revision == 0
            || guard.revision_floor == 0
            || manifest.schema != LIBRARY_SCHEMA
            || guard.schema != LIBRARY_SCHEMA
            || manifest.id != guard.id
            || manifest.revision > guard.revision_floor
            || guard.revision_floor > i64::MAX as u64
            || (selection == Selection::Current && manifest.revision != guard.revision_floor)
        {
            return Err(invalid(
                "library root and independent revision guard disagree",
            ));
        }
        if manifest.materials.byte_count > MAX_MATERIAL_COLLECTION_BYTES {
            return Err(LibraryError::new(
                LibraryErrorCode::ResourceLimit,
                "material collection exceeds 16 MiB",
            ));
        }
        if capture.members.len() != 1 || capture.members[0].role.get() != MATERIALS_ROLE {
            return Err(invalid("unexpected library object-role closure"));
        }
        let bytes = capture
            .read_object(
                capture.members[0].object,
                MAX_MATERIAL_COLLECTION_BYTES,
                cancel,
            )
            .map_err(ptac)?;
        if bytes.len() != manifest.materials.byte_count
            || !manifest.materials.sha256.matches_bytes(&bytes)
        {
            return Err(invalid("library material object differs from its manifest"));
        }
        let materials = MaterialCollection::decode(&bytes).map_err(invalid)?;
        let document = LibraryDocument {
            schema: manifest.schema,
            id: manifest.id,
            revision: manifest.revision,
            materials,
            hardware: manifest.hardware,
            spools: manifest.spools,
            receipts: manifest.receipts,
        };
        document.validate().map_err(invalid)?;
        let heads = capture.heads;
        let floor = guard.revision_floor;
        drop(capture);
        Ok(Loaded {
            document,
            container,
            heads,
            floor,
        })
    }
    pub fn status(&self, cancel: &AtomicBool) -> Result<LibraryStatus> {
        let document = self.read(cancel)?;
        let value = LibraryStatus {
            location: spiling_contracts::geometry::NativePath::from_os_str(self.path.as_os_str())
                .map_err(|e| invalid(e.to_string()))?,
            identity: document.as_ref().map(identity),
            filament_count: document
                .as_ref()
                .map_or(0, |d| d.materials.entries.len() as u32),
            spool_count: document.as_ref().map_or(0, |d| d.spools.len() as u32),
            recipe_count: document
                .as_ref()
                .map_or(0, |d| d.materials.recipes.len() as u32),
        };
        value.validate().map_err(invalid)?;
        Ok(value)
    }
    pub fn list_recipes(
        &self,
        query: &LibraryRecipeQuery,
        cancel: &AtomicBool,
    ) -> Result<LibraryRecipePage> {
        query.validate().map_err(invalid)?;
        let document = self.read(cancel)?;
        check_expected(query.expected.as_ref(), document.as_ref())?;
        let needle = query.query.to_lowercase();
        let mut entries: Vec<_> = document
            .as_ref()
            .into_iter()
            .flat_map(|d| &d.materials.recipes)
            .filter(|r| r.definition.name.to_lowercase().contains(&needle))
            .map(RecipeSummary::from)
            .collect();
        entries.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then(a.reference.id.cmp(&b.reference.id))
        });
        let total_matches = entries.len() as u32;
        if query.offset > total_matches {
            return Err(invalid("recipe page offset exceeds matching entries"));
        }
        let end = (query.offset as usize + MAX_LIBRARY_PAGE as usize).min(entries.len());
        Ok(LibraryRecipePage {
            identity: document.as_ref().map(identity),
            entries: entries[query.offset as usize..end].to_vec(),
            total_matches,
            next_offset: (end < total_matches as usize).then_some(end as u32),
        })
    }
    pub fn recipe(
        &self,
        library: &LibraryId,
        reference: &RecipeRef,
        cancel: &AtomicBool,
    ) -> Result<RecipeTransfer> {
        let document = self
            .read(cancel)?
            .ok_or_else(|| invalid("library is empty"))?;
        if &document.id != library {
            return Err(conflict());
        }
        document
            .materials
            .recipe_transfer(reference)
            .map_err(invalid)
    }
    pub fn list_filaments(
        &self,
        query: &LibraryFilamentQuery,
        cancel: &AtomicBool,
    ) -> Result<LibraryFilamentPage> {
        query.validate().map_err(invalid)?;
        let document = self.read(cancel)?;
        check_expected(query.expected.as_ref(), document.as_ref())?;
        let needle = query.query.to_lowercase();
        let mut matching = Vec::new();
        if let Some(document) = &document {
            for entry in &document.materials.entries {
                if entry.archived && !query.include_archived {
                    continue;
                }
                let definition = document
                    .materials
                    .resolve(&entry.current)
                    .ok_or_else(|| invalid("unresolved current filament"))?;
                let on_hand_spools = document
                    .spools
                    .iter()
                    .filter(|spool| {
                        spool.filament.id == entry.current.id
                            && spool.state == StockState::OnHand
                            && !spool
                                .quantity
                                .as_ref()
                                .is_some_and(|q| q.remaining_g == 0.0)
                    })
                    .count() as u32;
                if query.on_hand_only && on_hand_spools == 0 {
                    continue;
                }
                let material = definition.material.as_ref();
                let text = format!(
                    "{} {} {} {} {}",
                    definition.name,
                    definition.color_name.as_deref().unwrap_or(""),
                    material.map_or("", |m| m.manufacturer.as_str()),
                    material.map_or("", |m| m.product.as_str()),
                    material.map_or("", |m| m.polymer_family.as_str())
                );
                if !text.to_lowercase().contains(&needle) {
                    continue;
                }
                let earlier_spools = document
                    .spools
                    .iter()
                    .filter(|spool| {
                        spool.filament.id == entry.current.id
                            && spool.filament != entry.current
                            && spool.state == StockState::OnHand
                    })
                    .count() as u32;
                matching.push(LibraryFilamentSummary {
                    entry: entry.clone(),
                    name: definition.name.clone(),
                    material: definition.material.clone(),
                    color_name: definition.color_name.clone(),
                    appearance: definition.appearance.clone(),
                    profile_count: definition.profiles.len() as u32,
                    on_hand_spools,
                    earlier_spools,
                });
            }
        }
        if query.offset as usize > matching.len() {
            return Err(invalid("filament page offset outside captured search"));
        }
        let total_matches = matching.len() as u32;
        let entries: Vec<_> = matching
            .into_iter()
            .skip(query.offset as usize)
            .take(query.limit())
            .collect();
        let end = query.offset + entries.len() as u32;
        let page = LibraryFilamentPage {
            identity: document.as_ref().map(identity),
            entries,
            total_matches,
            next_offset: (end < total_matches).then_some(end),
        };
        page.validate().map_err(invalid)?;
        Ok(page)
    }
    pub fn get_filament(
        &self,
        library: &LibraryId,
        reference: &FilamentRef,
        cancel: &AtomicBool,
    ) -> Result<FilamentRevision> {
        validate_library_uuid(&library.0).map_err(invalid)?;
        reference.validate().map_err(invalid)?;
        let document = self.read(cancel)?.ok_or_else(|| {
            LibraryError::new(
                LibraryErrorCode::NotFound,
                "personal library has not been created",
            )
        })?;
        if &document.id != library {
            return Err(conflict());
        }
        document
            .materials
            .revisions
            .into_iter()
            .find(|revision| &revision.reference == reference)
            .ok_or_else(|| {
                LibraryError::new(
                    LibraryErrorCode::NotFound,
                    "filament revision no longer exists; refresh the library",
                )
            })
    }
    pub fn list_spools(
        &self,
        query: &LibrarySpoolQuery,
        cancel: &AtomicBool,
    ) -> Result<LibrarySpoolPage> {
        query.validate().map_err(invalid)?;
        let document = self.read(cancel)?;
        check_expected(query.expected.as_ref(), document.as_ref())?;
        let matching: Vec<_> = document
            .as_ref()
            .into_iter()
            .flat_map(|d| d.spools.iter())
            .filter(|spool| {
                query
                    .filament
                    .as_ref()
                    .is_none_or(|f| f.id == spool.filament.id)
            })
            .cloned()
            .collect();
        if query.offset as usize > matching.len() {
            return Err(invalid("spool page offset outside captured stock"));
        }
        let total_matches = matching.len() as u32;
        let spools: Vec<_> = matching
            .into_iter()
            .skip(query.offset as usize)
            .take(query.limit())
            .collect();
        let end = query.offset + spools.len() as u32;
        let page = LibrarySpoolPage {
            identity: document.as_ref().map(identity),
            spools,
            total_matches,
            next_offset: (end < total_matches).then_some(end),
        };
        page.validate().map_err(invalid)?;
        Ok(page)
    }
    pub fn write(&self, request: &LibraryWrite, cancel: &AtomicBool) -> Result<LibraryCommit> {
        self.write_observed(request, cancel, |_| true)
    }
    pub fn write_observed(
        &self,
        request: &LibraryWrite,
        cancel: &AtomicBool,
        mut observe: impl FnMut(spiling_core_ptac::SaveStage) -> bool,
    ) -> Result<LibraryCommit> {
        request.validate().map_err(invalid)?;
        let digest = request.digest().map_err(invalid)?;
        let loaded = if self.exists()? {
            Some(self.load(false, Selection::Current, cancel)?)
        } else {
            None
        };
        let mut document = loaded
            .as_ref()
            .map(|loaded| loaded.document.clone())
            .unwrap_or_else(|| LibraryDocument {
                schema: LIBRARY_SCHEMA,
                id: LibraryId(uuid::Uuid::new_v4().to_string()),
                revision: 0,
                materials: MaterialCollection::default(),
                hardware: Default::default(),
                spools: Vec::new(),
                receipts: Vec::new(),
            });
        if let Some(receipt) = document
            .receipts
            .iter()
            .find(|receipt| receipt.request_id == request.request_id)
        {
            if receipt.request_hash != digest {
                return Err(invalid(
                    "request identity reused with different library content",
                ));
            }
            // Observing a receipt does not repair an earlier directory-sync failure.
            // Confirm the unchanged current root without applying the mutation again.
            let mut committed = self.publish(
                document,
                loaded.as_ref(),
                Selection::Current,
                cancel,
                &mut observe,
            )?;
            committed.replayed = true;
            return Ok(committed);
        }
        if document.revision != request.base_revision
            || (loaded.is_some() && request.library_id.as_ref() != Some(&document.id))
            || (loaded.is_none() && request.library_id.is_some())
        {
            return Err(conflict());
        }
        apply(&mut document, &request.mutation)?;
        document.revision = document
            .revision
            .checked_add(1)
            .filter(|r| *r <= i64::MAX as u64)
            .ok_or_else(|| {
                LibraryError::new(
                    LibraryErrorCode::ResourceLimit,
                    "library revision exhausted",
                )
            })?;
        if document.receipts.len() == MAX_LIBRARY_RECEIPTS {
            document.receipts.remove(0);
        }
        document.receipts.push(LibraryReceipt {
            request_id: request.request_id.clone(),
            request_hash: digest,
            revision: document.revision,
        });
        prune(&mut document);
        document.validate().map_err(invalid)?;
        self.publish(
            document,
            loaded.as_ref(),
            Selection::Current,
            cancel,
            &mut observe,
        )
    }
    fn publish(
        &self,
        document: LibraryDocument,
        loaded: Option<&Loaded>,
        retain: Selection,
        cancel: &AtomicBool,
        observe: &mut impl FnMut(spiling_core_ptac::SaveStage) -> bool,
    ) -> Result<LibraryCommit> {
        let materials = document.materials.canonical_bytes().map_err(invalid)?;
        let manifest = Manifest {
            schema: document.schema,
            id: document.id.clone(),
            revision: document.revision,
            materials: CollectionRecord {
                sha256: SourceHash::from_bytes(&materials),
                byte_count: materials.len(),
            },
            hardware: document.hardware.clone(),
            spools: document.spools.clone(),
            receipts: document.receipts.clone(),
        };
        let root = serde_json::to_vec(&manifest).map_err(|e| invalid(e.to_string()))?;
        if root.len() > MAX_MANIFEST_BYTES {
            return Err(LibraryError::new(
                LibraryErrorCode::ResourceLimit,
                "library manifest exceeds 1 MiB",
            ));
        }
        let guard = serde_json::to_vec(&Guard {
            schema: LIBRARY_SCHEMA,
            id: document.id.clone(),
            revision_floor: document.revision,
        })
        .map_err(|e| invalid(e.to_string()))?;
        let objects = [ObjectInput {
            role: ObjectRole::new(MATERIALS_ROLE).map_err(ptac)?,
            bytes: &materials,
        }];
        let input = SnapshotInput {
            root: &root,
            objects: &objects,
            guard: &guard,
        };
        let published = match loaded {
            Some(loaded) => loaded
                .container
                .publish(loaded.heads, retain, input, cancel, observe),
            None => Container::create_new_for_profile(
                &self.path,
                DocumentProfile::UserLibrary,
                input,
                cancel,
                observe,
            ),
        }
        .map_err(ptac)?;
        Ok(LibraryCommit {
            document,
            outcome: published.outcome,
            replayed: false,
            publication_error: published
                .error
                .map(|e| LibraryError::new(LibraryErrorCode::Io, e.to_string()).message),
        })
    }
    /// Explicit previous-root recovery never uses a corrupt current payload as retained authority.
    pub fn recover_previous(
        &self,
        expected_library: &LibraryId,
        expected_floor: u64,
        cancel: &AtomicBool,
    ) -> Result<LibraryCommit> {
        let loaded = self.load(false, Selection::Previous, cancel)?;
        if &loaded.document.id != expected_library || loaded.floor != expected_floor {
            return Err(conflict());
        }
        let mut document = loaded.document.clone();
        document.revision = loaded
            .floor
            .checked_add(1)
            .filter(|r| *r <= i64::MAX as u64)
            .ok_or_else(|| {
                LibraryError::new(
                    LibraryErrorCode::ResourceLimit,
                    "library revision exhausted",
                )
            })?;
        self.publish(
            document,
            Some(&loaded),
            Selection::Previous,
            cancel,
            &mut |_| true,
        )
    }
}

fn identity(document: &LibraryDocument) -> LibraryIdentity {
    LibraryIdentity {
        id: document.id.clone(),
        revision: document.revision,
    }
}
fn check_expected(
    expected: Option<&LibraryIdentity>,
    document: Option<&LibraryDocument>,
) -> Result<()> {
    if expected.is_some_and(|expected| document.is_none_or(|d| identity(d) != *expected)) {
        return Err(conflict());
    }
    Ok(())
}
fn apply(document: &mut LibraryDocument, mutation: &LibraryMutation) -> Result<()> {
    match mutation {
        LibraryMutation::ReplaceHardware { hardware } => {
            hardware.validate().map_err(invalid)?;
            let mut replacement = hardware.clone();
            for m in &mut replacement.machines {
                if let Some(old) = document.hardware.machines.iter().find(|o| o.id == m.id) {
                    let next = old
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| invalid("machine generation exhausted"))?;
                    if m.generation != old.generation && m.generation != next {
                        return Err(invalid(
                            "machine generation must retain or advance one installation",
                        ));
                    }
                    if m.setup != old.setup {
                        m.generation = next;
                    }
                } else if m.generation != 1 {
                    return Err(invalid("new machine generation must be one"));
                }
            }
            for r in &mut replacement.routes {
                if let Some(old) = document.hardware.routes.iter().find(|o| o.id == r.id) {
                    let next = old
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| invalid("connection generation exhausted"))?;
                    if r.generation != old.generation && r.generation != next {
                        return Err(invalid("connection generation must retain or advance once"));
                    }
                    if r.machine_id != old.machine_id
                        || r.family != old.family
                        || r.endpoint != old.endpoint
                        || r.expected_identity != old.expected_identity
                        || r.credential != old.credential
                    {
                        r.generation = next;
                    }
                    if (r.endpoint != old.endpoint
                        || r.expected_identity != old.expected_identity
                        || r.family != old.family
                        || r.machine_id != old.machine_id)
                        && r.credential.is_some()
                        && r.credential == old.credential
                    {
                        return Err(invalid(
                            "changed authenticated peer requires a fresh credential scope",
                        ));
                    }
                } else if r.generation != 1 {
                    return Err(invalid("new connection generation must be one"));
                }
            }
            document.hardware = replacement;
        }
        LibraryMutation::PutFilament {
            id,
            expected,
            definition,
        } => {
            let current = document
                .materials
                .entries
                .iter()
                .find(|entry| &entry.current.id == id);
            if current.map(|entry| &entry.current.revision) != expected.as_ref() {
                return Err(conflict());
            }
            if let Some(current) = current
                && document.spools.iter().any(|s| &s.filament.id == id)
                && !document
                    .materials
                    .resolve(&current.current)
                    .expect("validated current definition")
                    .same_stock_variant(definition)
            {
                return Err(invalid(
                    "owned spools retain their physical variant; create a new variant or explicitly reassign the spools before changing material, diameter or color",
                ));
            }
            let reference = FilamentRef {
                id: id.clone(),
                revision: definition.content_hash().map_err(invalid)?,
            };
            if document.materials.resolve(&reference).is_none() {
                document.materials.revisions.push(FilamentRevision {
                    reference: reference.clone(),
                    definition: definition.clone(),
                });
            }
            match document
                .materials
                .entries
                .iter_mut()
                .find(|entry| &entry.current.id == id)
            {
                Some(entry) => entry.current = reference,
                None => document.materials.entries.push(FilamentEntry {
                    current: reference,
                    archived: false,
                    origin: None,
                }),
            }
        }
        LibraryMutation::ArchiveFilament { filament, archived } => {
            let entry = document
                .materials
                .entries
                .iter_mut()
                .find(|entry| entry.current.id == filament.id)
                .ok_or_else(|| invalid("filament entry not found"))?;
            if &entry.current != filament {
                return Err(conflict());
            }
            entry.archived = *archived;
        }
        LibraryMutation::PutSpool { spool } => {
            if document.materials.resolve(&spool.filament).is_none() {
                return Err(invalid("spool refers to an uncaptured filament revision"));
            }
            match document
                .spools
                .iter_mut()
                .find(|entry| entry.id == spool.id)
            {
                Some(entry) => *entry = spool.clone(),
                None => document.spools.push(spool.clone()),
            }
        }
        LibraryMutation::RemoveSpool { id } => {
            let index = document
                .spools
                .iter()
                .position(|spool| &spool.id == id)
                .ok_or_else(|| invalid("spool not found"))?;
            document.spools.remove(index);
        }
        LibraryMutation::PutRecipe { transfer, expected } => {
            document.materials =
                std::mem::take(&mut document.materials).with_recipe(transfer, expected.as_ref())?;
        }
        LibraryMutation::RemoveRecipe { id, expected } => {
            let index = document
                .materials
                .recipes
                .iter()
                .position(|recipe| &recipe.id == id)
                .ok_or_else(|| invalid("recipe not found"))?;
            if &document.materials.recipes[index].revision != expected {
                return Err(conflict());
            }
            document.materials.recipes.remove(index);
        }
    }
    document
        .materials
        .entries
        .sort_by(|a, b| a.current.id.cmp(&b.current.id));
    document
        .materials
        .revisions
        .sort_by(|a, b| a.reference.cmp(&b.reference));
    document.materials.recipes.sort_by(|a, b| a.id.cmp(&b.id));
    document.spools.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(())
}
fn prune(document: &mut LibraryDocument) {
    crate::materials::prune_revisions(
        &mut document.materials,
        document.spools.iter().map(|s| s.filament.clone()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("spiling-library-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> LibraryStore {
            LibraryStore::new(self.0.join("My library.spil"))
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn definition(name: &str) -> FilamentDefinition {
        FilamentDefinition {
            name: name.into(),
            material: None,
            color_name: None,
            appearance: None,
            profiles: vec![],
            source: DefinitionSource::Operator,
        }
    }
    fn write(
        store: &LibraryStore,
        document: Option<&LibraryDocument>,
        mutation: LibraryMutation,
    ) -> LibraryCommit {
        store
            .write(
                &LibraryWrite {
                    library_id: document.map(|d| d.id.clone()),
                    base_revision: document.map_or(0, |d| d.revision),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    mutation,
                },
                &AtomicBool::new(false),
            )
            .unwrap()
    }
    #[test]
    fn hardware_generation_and_endpoint_credentials_are_native_cas_invariants() {
        use spiling_contracts::hardware::*;
        let root = Temporary::new();
        let store = root.store();
        let catalog = builtin_catalog().unwrap();
        let mut h = HardwareLibrary::default();
        let machine = uuid::Uuid::new_v4().to_string();
        h.machines.push(OwnedMachine {
            id: machine.clone(),
            nickname: "Workshop".into(),
            serial: None,
            location: None,
            setup: catalog.setups[0].clone(),
            generation: 1,
        });
        h.routes.push(EndpointRoute {
            id: uuid::Uuid::new_v4().to_string(),
            machine_id: machine,
            family: ConnectionFamily::OctoPrint,
            endpoint: "http://printer-a.local".into(),
            expected_identity: "configured-device-a".into(),
            generation: 1,
            credential: Some(CredentialRef {
                slot_id: uuid::Uuid::new_v4().to_string(),
                role: CredentialRole::ApiKey,
                backend: CredentialBackend::PlaintextFile,
            }),
        });
        let first = write(
            &store,
            None,
            LibraryMutation::ReplaceHardware {
                hardware: h.clone(),
            },
        );
        h.machines[0].nickname = "Renamed workshop".into();
        let second = write(
            &store,
            Some(&first.document),
            LibraryMutation::ReplaceHardware {
                hardware: h.clone(),
            },
        );
        assert_eq!(second.document.hardware.machines[0].generation, 1);
        h.machines[0].setup.printer.build_envelope.max[2] = 200.;
        let third = write(
            &store,
            Some(&second.document),
            LibraryMutation::ReplaceHardware {
                hardware: h.clone(),
            },
        );
        assert_eq!(third.document.hardware.machines[0].generation, 2);
        let mut retargeted = third.document.hardware.clone();
        retargeted.routes[0].endpoint = "http://printer-b.local".into();
        let request = LibraryWrite {
            library_id: Some(third.document.id.clone()),
            base_revision: third.document.revision,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::ReplaceHardware {
                hardware: retargeted.clone(),
            },
        };
        assert_eq!(
            store
                .write(&request, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            LibraryErrorCode::InvalidData
        );
        retargeted.routes[0].credential = None;
        let fourth = write(
            &store,
            Some(&third.document),
            LibraryMutation::ReplaceHardware {
                hardware: retargeted,
            },
        );
        assert_eq!(fourth.document.hardware.routes[0].generation, 2);
        let stale = LibraryWrite {
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::ReplaceHardware { hardware: h },
            ..request
        };
        assert_eq!(
            store
                .write(&stale, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            LibraryErrorCode::StaleRevision
        );
    }
    #[test]
    fn twenty_filaments_duplicate_spools_and_exact_old_revisions_reopen_without_inputs() {
        let temporary = Temporary::new();
        let store = temporary.store();
        let cancel = AtomicBool::new(false);
        assert!(store.read(&cancel).unwrap().is_none());
        assert!(!store.path().exists());
        let mut state = None;
        for i in 0..20 {
            let commit = write(
                &store,
                state.as_ref(),
                LibraryMutation::PutFilament {
                    id: FilamentId(uuid::Uuid::new_v4().to_string()),
                    expected: None,
                    definition: definition(&format!("Filament {i}")),
                },
            );
            assert_eq!(commit.outcome, PublicationOutcome::PublishedConfirmed);
            state = Some(commit.document);
        }
        let mut document = state.unwrap();
        let first = document.materials.entries[0].current.clone();
        for i in 0..2 {
            let commit = write(
                &store,
                Some(&document),
                LibraryMutation::PutSpool {
                    spool: SpoolRecord {
                        id: SpoolId(uuid::Uuid::new_v4().to_string()),
                        filament: first.clone(),
                        name: format!("Spool {i}"),
                        state: StockState::OnHand,
                        quantity: if i == 0 {
                            None
                        } else {
                            Some(StockQuantity {
                                remaining_g: 500.0,
                                source: QuantitySource::Weighed,
                            })
                        },
                        lot: None,
                        location: None,
                    },
                },
            );
            document = commit.document;
        }
        let commit = write(
            &store,
            Some(&document),
            LibraryMutation::PutFilament {
                id: first.id.clone(),
                expected: Some(first.revision.clone()),
                definition: definition("Revised filament"),
            },
        );
        let reopened = store.read(&cancel).unwrap().unwrap();
        assert_eq!(reopened, commit.document);
        assert_eq!(reopened.materials.entries.len(), 20);
        assert_eq!(reopened.spools.len(), 2);
        assert_eq!(reopened.materials.revisions.len(), 21);
        assert_eq!(reopened.spools[0].filament, first);
        assert_eq!(reopened.spools[1].filament, first);
        assert!(reopened.materials.resolve(&first).is_some());
        assert_eq!(
            reopened
                .spools
                .iter()
                .filter(|s| s.quantity.is_none())
                .count(),
            1
        );
        assert!(
            Container::open_existing(store.path(), true).is_err(),
            "personal file is not a project"
        );
    }
    #[test]
    fn repeated_requests_are_idempotent_and_stale_or_foreign_library_drafts_do_not_write() {
        let temporary = Temporary::new();
        let store = temporary.store();
        let cancel = AtomicBool::new(false);
        let request = LibraryWrite {
            library_id: None,
            base_revision: 0,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::PutFilament {
                id: FilamentId(uuid::Uuid::new_v4().to_string()),
                expected: None,
                definition: definition("PLA"),
            },
        };
        let first = store.write(&request, &cancel).unwrap();
        let repeated = store.write(&request, &cancel).unwrap();
        assert!(repeated.replayed);
        assert_eq!(repeated.document, first.document);
        let mut changed = request.clone();
        if let LibraryMutation::PutFilament { definition, .. } = &mut changed.mutation {
            definition.name = "Different".into();
        }
        assert_eq!(
            store.write(&changed, &cancel).unwrap_err().code,
            LibraryErrorCode::InvalidData
        );
        changed.request_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            store.write(&changed, &cancel).unwrap_err().code,
            LibraryErrorCode::StaleRevision
        );
        changed.base_revision = first.document.revision;
        changed.library_id = Some(LibraryId(uuid::Uuid::new_v4().to_string()));
        assert_eq!(
            store.write(&changed, &cancel).unwrap_err().code,
            LibraryErrorCode::StaleRevision
        );
        assert_eq!(store.read(&cancel).unwrap().unwrap(), first.document);
    }
    #[test]
    fn owned_stock_is_not_reinterpreted_as_a_different_color() {
        let temporary = Temporary::new();
        let store = temporary.store();
        let cancel = AtomicBool::new(false);
        let mut red = definition("Red PLA");
        red.appearance = Some(DeclaredAppearance {
            color: spiling_contracts::appearance::AppearanceColor::Srgb {
                channels: [1.0, 0.0, 0.0],
            },
            source: DefinitionSource::Operator,
        });
        let initial = write(
            &store,
            None,
            LibraryMutation::PutFilament {
                id: FilamentId(uuid::Uuid::new_v4().to_string()),
                expected: None,
                definition: red.clone(),
            },
        );
        let reference = initial.document.materials.entries[0].current.clone();
        let stocked = write(
            &store,
            Some(&initial.document),
            LibraryMutation::PutSpool {
                spool: SpoolRecord {
                    id: SpoolId(uuid::Uuid::new_v4().to_string()),
                    filament: reference.clone(),
                    name: "Physical spool".into(),
                    state: StockState::OnHand,
                    quantity: None,
                    lot: None,
                    location: None,
                },
            },
        );
        let mut blue = red;
        blue.appearance.as_mut().unwrap().color =
            spiling_contracts::appearance::AppearanceColor::Srgb {
                channels: [0.0, 0.0, 1.0],
            };
        let request = LibraryWrite {
            library_id: Some(stocked.document.id.clone()),
            base_revision: stocked.document.revision,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::PutFilament {
                id: reference.id,
                expected: Some(reference.revision),
                definition: blue,
            },
        };
        let rejected = store.write(&request, &cancel).unwrap_err();
        assert_eq!(rejected.code, LibraryErrorCode::InvalidData);
        assert_eq!(store.read(&cancel).unwrap().unwrap(), stocked.document);
    }
    #[test]
    fn uncertain_publication_resolves_original_request_without_duplicate_or_logical_edit() {
        let temporary = Temporary::new();
        let store = temporary.store();
        let cancel = AtomicBool::new(false);
        let request = LibraryWrite {
            library_id: None,
            base_revision: 0,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::PutFilament {
                id: FilamentId(uuid::Uuid::new_v4().to_string()),
                expected: None,
                definition: definition("PLA"),
            },
        };
        let first = store
            .write_observed(&request, &cancel, |stage| {
                stage != spiling_core_ptac::SaveStage::Published
            })
            .unwrap();
        assert_eq!(first.outcome, PublicationOutcome::PublishedUnconfirmed);
        assert_eq!(store.read(&cancel).unwrap().unwrap(), first.document);
        let resolved = store.write(&request, &cancel).unwrap();
        assert!(resolved.replayed);
        assert_eq!(resolved.outcome, PublicationOutcome::PublishedConfirmed);
        assert_eq!(resolved.document, first.document);
        assert_eq!(resolved.document.revision, 1);
        assert_eq!(resolved.document.materials.entries.len(), 1);
    }
    #[test]
    fn invalid_first_write_cancellation_and_lifetime_writer_contention_preserve_data() {
        let temporary = Temporary::new();
        let store = temporary.store();
        let cancel = AtomicBool::new(false);
        let mut request = LibraryWrite {
            library_id: None,
            base_revision: 0,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::PutFilament {
                id: FilamentId(uuid::Uuid::new_v4().to_string()),
                expected: None,
                definition: definition(" "),
            },
        };
        assert!(store.write(&request, &cancel).is_err());
        assert!(!store.path().exists());
        if let LibraryMutation::PutFilament { definition, .. } = &mut request.mutation {
            definition.name = "PLA".into();
        }
        assert_eq!(
            store
                .write(&request, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            LibraryErrorCode::Cancelled
        );
        assert!(!store.path().exists());
        let first = store.write(&request, &cancel).unwrap();
        let writer =
            Container::open_existing_for_profile(store.path(), false, DocumentProfile::UserLibrary)
                .unwrap();
        assert_eq!(
            store.write(&request, &cancel).unwrap_err().code,
            LibraryErrorCode::Locked
        );
        assert_eq!(store.read(&cancel).unwrap().unwrap(), first.document);
        drop(writer);
        assert!(store.write(&request, &cancel).unwrap().replayed);
    }
    #[test]
    fn recipe_publication_captures_dependencies_and_replays_without_duplicate_entries_or_stock() {
        let temporary = Temporary::new();
        let store = temporary.store();
        let cancel = AtomicBool::new(false);
        let filaments = (0..2)
            .map(|i| {
                let definition = definition(&format!("Ingredient {i}"));
                FilamentRevision {
                    reference: FilamentRef {
                        id: FilamentId(uuid::Uuid::new_v4().to_string()),
                        revision: definition.content_hash().unwrap(),
                    },
                    definition,
                }
            })
            .collect();
        let transfer = RecipeTransfer::from_declarations(
            RecipeId(uuid::Uuid::new_v4().to_string()),
            "Library recipe".into(),
            filaments,
            spiling_contracts::appearance::AppearanceColor::Srgb { channels: [0.4; 3] },
            vec![0.2; 2],
        )
        .unwrap();
        let stock_id = FilamentId(uuid::Uuid::new_v4().to_string());
        let first_stock = write(
            &store,
            None,
            LibraryMutation::PutFilament {
                id: stock_id.clone(),
                expected: None,
                definition: definition("Stock revision one"),
            },
        );
        let stock_ref = first_stock.document.materials.entries[0].current.clone();
        let with_spool = write(
            &store,
            Some(&first_stock.document),
            LibraryMutation::PutSpool {
                spool: SpoolRecord {
                    id: SpoolId(uuid::Uuid::new_v4().to_string()),
                    filament: stock_ref.clone(),
                    name: "Owned physical spool".into(),
                    state: StockState::OnHand,
                    quantity: None,
                    lot: None,
                    location: None,
                },
            },
        );
        let updated_stock = write(
            &store,
            Some(&with_spool.document),
            LibraryMutation::PutFilament {
                id: stock_id,
                expected: Some(stock_ref.revision.clone()),
                definition: definition("Stock revision two"),
            },
        );
        let request = LibraryWrite {
            library_id: Some(updated_stock.document.id.clone()),
            base_revision: updated_stock.document.revision,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::PutRecipe {
                transfer: transfer.clone(),
                expected: None,
            },
        };
        let first = store.write(&request, &cancel).unwrap();
        let replay = store.write(&request, &cancel).unwrap();
        assert!(replay.replayed);
        assert_eq!(first.document, replay.document);
        assert_eq!(first.document.materials.entries.len(), 3);
        assert!(first.document.materials.resolve(&stock_ref).is_some());
        assert_eq!(first.document.spools, updated_stock.document.spools);
        assert_eq!(
            store
                .recipe(&first.document.id, &transfer.recipe.reference(), &cancel)
                .unwrap(),
            transfer
        );
        let stale = LibraryWrite {
            library_id: Some(first.document.id.clone()),
            base_revision: first.document.revision,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: request.mutation,
        };
        assert_eq!(
            store.write(&stale, &cancel).unwrap_err().code,
            LibraryErrorCode::StaleRevision
        );
        assert_eq!(store.read(&cancel).unwrap().unwrap(), first.document);
        let page = store
            .list_recipes(
                &LibraryRecipeQuery {
                    query: "LIBRARY".into(),
                    ..Default::default()
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(page.total_matches, 1);
        let removed = write(
            &store,
            Some(&first.document),
            LibraryMutation::RemoveRecipe {
                id: transfer.recipe.id,
                expected: transfer.recipe.revision,
            },
        );
        assert!(removed.document.materials.recipes.is_empty());
        assert_eq!(removed.document.materials.entries.len(), 3);
        assert_eq!(removed.document.spools, updated_stock.document.spools);
    }
}
