// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Immutable portable material bytes, shared by current, history and prepared snapshots.
use crate::{Snapshot, invalid};
use spiling_contracts::{geometry::SourceHash, library::*, project::ProjectError};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
pub const MAX_RETAINED_MATERIAL_BYTES: usize = 64 * 1024 * 1024;
#[derive(Debug)]
pub struct MaterialAsset {
    bytes: Arc<Vec<u8>>,
    record: MaterialCollectionRecord,
}
impl MaterialAsset {
    pub fn new(collection: &MaterialCollection) -> Result<Self, ProjectError> {
        let bytes = collection.canonical_bytes().map_err(invalid)?;
        let record = MaterialCollectionRecord {
            hash: SourceHash::from_bytes(&bytes),
            byte_count: bytes.len() as u64,
            filament_count: collection.entries.len() as u32,
            recipe_count: collection.recipes.len() as u32,
        };
        Ok(Self {
            bytes: Arc::new(bytes.into_boxed_slice().into_vec()),
            record,
        })
    }
    pub fn decode(record: &MaterialCollectionRecord, bytes: Vec<u8>) -> Result<Self, ProjectError> {
        record.validate().map_err(invalid)?;
        let document = MaterialCollection::decode(&bytes).map_err(invalid)?;
        let asset = Self::new(&document)?;
        if asset.record != *record || asset.bytes.as_slice() != bytes.as_slice() {
            return Err(invalid(
                "material collection differs from its exact descriptor",
            ));
        }
        Ok(asset)
    }
    pub fn record(&self) -> &MaterialCollectionRecord {
        &self.record
    }
    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }
    pub fn document(&self) -> Result<MaterialCollection, ProjectError> {
        MaterialCollection::decode(&self.bytes).map_err(invalid)
    }
}
/// Project entries/recipes and personal spools share one exact-revision reachability census.
pub(crate) fn prune_revisions(
    collection: &mut MaterialCollection,
    external: impl IntoIterator<Item = FilamentRef>,
) {
    let needed: BTreeSet<_> = collection
        .entries
        .iter()
        .map(|e| e.current.clone())
        .chain(
            collection
                .recipes
                .iter()
                .flat_map(|r| r.definition.ingredients.iter().map(|i| i.filament.clone())),
        )
        .chain(external)
        .collect();
    collection
        .revisions
        .retain(|r| needed.contains(&r.reference));
}
pub(crate) fn retained_bytes<'a>(snapshots: impl Iterator<Item = &'a Snapshot>) -> usize {
    let allocations: BTreeMap<_, _> = snapshots
        .filter_map(|s| s.materials.as_ref())
        .map(|a| (Arc::as_ptr(a.bytes()), a.bytes().capacity()))
        .collect();
    allocations.values().sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edit, Project, storage};
    use std::sync::atomic::AtomicBool;
    #[test]
    fn portable_collection_survives_save_history_and_read_only_open_without_library() {
        let root = std::env::temp_dir().join(format!("spiling-materials-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("Project.spil");
        let definition = FilamentDefinition {
            name: "Owned PLA".into(),
            material: None,
            color_name: None,
            appearance: None,
            profiles: vec![],
            source: DefinitionSource::Operator,
        };
        let revision = FilamentRevision {
            reference: FilamentRef {
                id: FilamentId(uuid::Uuid::new_v4().to_string()),
                revision: definition.content_hash().unwrap(),
            },
            definition,
        };
        let origin = LibraryOrigin {
            library_id: LibraryId(uuid::Uuid::new_v4().to_string()),
            filament: revision.reference.clone(),
        };
        let mut project = Project::new();
        let base = project.info().revision;
        let edit = project
            .prepare(Edit::PutFilament {
                revision: revision.clone(),
                origin: Some(origin.clone()),
                expected: None,
            })
            .unwrap();
        project.commit(edit).unwrap();
        assert!(project.info().dirty);
        assert!(project.info().revision > base);
        assert!(project.info().materials.is_some());
        project.undo().unwrap();
        assert!(project.info().materials.is_none());
        project.redo().unwrap();
        let receipt = storage::save(
            project.capture(),
            None,
            Some(path.clone()),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        project.mark_saved(receipt).unwrap();
        assert!(!project.info().dirty);
        drop(project);
        let loaded = storage::open(&path, true, false, &AtomicBool::new(false)).unwrap();
        let project = Project::from_loaded(loaded).unwrap();
        let collection = project
            .snapshot()
            .materials
            .as_ref()
            .unwrap()
            .document()
            .unwrap();
        assert_eq!(collection.entries[0].origin.as_ref(), Some(&origin));
        assert_eq!(
            collection.resolve(&revision.reference),
            Some(&revision.definition)
        );
        assert_eq!(
            project
                .prepare(Edit::PutFilament {
                    revision,
                    origin: None,
                    expected: None
                })
                .unwrap_err()
                .code,
            spiling_contracts::project::ProjectErrorCode::ReadOnly
        );
        drop(project);
        std::fs::remove_dir_all(root).unwrap();
    }
}
