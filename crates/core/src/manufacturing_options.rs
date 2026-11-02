// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{Snapshot, error, invalid, validate_pins};
use serde::Serialize;
use sha2::{Digest, Sha256};
use spiling_contracts::{
    geometry::{DefinitionId, MAX_DEFINITIONS, MAX_OCCURRENCES, OccurrenceRecord, SourceHash},
    manufacturing::{ManufacturingErrorCode, ManufacturingIntent},
    project::{
        ManufacturingOption, ProjectError, ProjectErrorCode, StoredDefinition,
        validate_manufacturing_options,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(crate) const MAX_RETAINED_OPTION_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
struct Context<'a> {
    domain: &'static str,
    definitions: Vec<&'a StoredDefinition>,
    occurrences: Vec<&'a OccurrenceRecord>,
    surface: &'a spiling_contracts::surface::SurfaceOverlays,
    process: &'a spiling_contracts::process::ProcessDocument,
    images: Vec<&'a spiling_contracts::surface::SurfaceImageRecord>,
    materials: Option<&'a spiling_contracts::library::MaterialCollectionRecord>,
    hardware: Option<&'a spiling_contracts::hardware::ProjectHardwareCapture>,
}

pub fn context_hash(snapshot: &Snapshot) -> Result<SourceHash, ProjectError> {
    validate_context(snapshot)?;
    let context = Context {
        domain: "spiling.manufacturing-option-context.v1",
        definitions: snapshot
            .sources
            .values()
            .map(|source| &source.record)
            .collect(),
        occurrences: snapshot.occurrences.values().collect(),
        surface: &snapshot.surface_overlays,
        process: &snapshot.process_document,
        images: snapshot
            .surface_images
            .values()
            .map(|image| image.record())
            .collect(),
        materials: snapshot.materials.as_ref().map(|asset| asset.record()),
        hardware: snapshot.hardware.as_deref(),
    };
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, &context)
        .map_err(|_| invalid("manufacturing option context cannot be encoded"))?;
    Ok(SourceHash::from_digest(&writer.0.finalize().into()))
}

fn validate_context(snapshot: &Snapshot) -> Result<(), ProjectError> {
    if snapshot.sources.len() > MAX_DEFINITIONS as usize
        || snapshot.occurrences.len() > MAX_OCCURRENCES as usize
    {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "manufacturing option context record limit exceeded",
        ));
    }
    for (id, source) in &snapshot.sources {
        if id != &source.record.definition_id {
            return Err(invalid("manufacturing option source map identity mismatch"));
        }
        source.validate()?;
    }
    let mut used = BTreeSet::<&DefinitionId>::new();
    for (id, occurrence) in &snapshot.occurrences {
        if id != &occurrence.occurrence_id
            || !snapshot.sources.contains_key(&occurrence.definition_id)
        {
            return Err(invalid(
                "manufacturing option occurrence identity or definition mismatch",
            ));
        }
        occurrence
            .pose
            .validate()
            .map_err(|error| invalid(error.message))?;
        used.insert(&occurrence.definition_id);
    }
    if used.len() != snapshot.sources.len() {
        return Err(invalid(
            "manufacturing option context has unreferenced definitions",
        ));
    }
    let definitions: Vec<_> = snapshot
        .sources
        .values()
        .map(|source| source.record.clone())
        .collect();
    let occurrences: Vec<_> = snapshot.occurrences.values().cloned().collect();
    snapshot
        .surface_overlays
        .validate_targets(&definitions, &occurrences, u32::MAX)?;
    snapshot
        .process_document
        .validate_targets(&definitions, &occurrences, u32::MAX, u32::MAX)?;
    for asset in snapshot.surface_images.values() {
        asset.record().validate_content(asset.bytes())?;
    }
    if let Some(materials) = &snapshot.materials {
        materials.record().validate().map_err(invalid)?;
        if materials.record().hash != SourceHash::from_bytes(materials.bytes())
            || materials.record().byte_count != materials.bytes().len() as u64
        {
            return Err(invalid(
                "manufacturing option material capture identity mismatch",
            ));
        }
    }
    if let Some(hardware) = &snapshot.hardware {
        hardware.validate().map_err(invalid)?;
    }
    validate_pins(std::iter::once(snapshot))
}

pub fn trial_snapshot(
    snapshot: &Snapshot,
    intent: Arc<ManufacturingIntent>,
) -> Result<Arc<Snapshot>, ProjectError> {
    validate_context(snapshot)?;
    validate_current_hardware(snapshot, &intent)?;
    let mut trial = snapshot.clone();
    trial.manufacturing_artifact = None;
    set_intent(&mut trial, intent)?;
    validate_pins(std::iter::once(&trial))?;
    Ok(Arc::new(trial))
}

fn validate_current_hardware(
    snapshot: &Snapshot,
    intent: &ManufacturingIntent,
) -> Result<(), ProjectError> {
    if intent.hardware != snapshot.hardware {
        return Err(invalid(
            "manufacturing option hardware capture differs from current project",
        ));
    }
    intent.validate().map_err(|error| {
        crate::error(
            if error.code == ManufacturingErrorCode::ResourceLimit {
                ProjectErrorCode::ResourceLimit
            } else {
                ProjectErrorCode::InvalidProject
            },
            error.message,
        )
    })
}

pub(crate) fn set_intent(
    snapshot: &mut Snapshot,
    mut intent: Arc<ManufacturingIntent>,
) -> Result<(), ProjectError> {
    if intent.hardware.is_none()
        && let Some(hardware) = &snapshot.hardware
    {
        Arc::make_mut(&mut intent).hardware = Some(hardware.clone());
    }
    intent.validate().map_err(|error| {
        crate::error(
            if error.code == ManufacturingErrorCode::ResourceLimit {
                ProjectErrorCode::ResourceLimit
            } else {
                ProjectErrorCode::InvalidProject
            },
            error.message,
        )
    })?;
    snapshot.hardware = intent.hardware.clone();
    snapshot.manufacturing_intent = Some(intent);
    Ok(())
}

pub(crate) fn validate_option_context(
    snapshot: &Snapshot,
    option: &ManufacturingOption,
) -> Result<(), ProjectError> {
    option.validate()?;
    if option.context_hash != context_hash(snapshot)? {
        return Err(invalid(
            "manufacturing option belongs to a different source or manufacturing context",
        ));
    }
    validate_current_hardware(snapshot, &option.intent)
}

pub(crate) fn retain_option(
    snapshot: &mut Snapshot,
    option: ManufacturingOption,
) -> Result<(), ProjectError> {
    validate_option_context(snapshot, &option)?;
    let options = Arc::make_mut(&mut snapshot.manufacturing_options);
    options.push(option);
    validate_manufacturing_options(options)
}

pub(crate) fn retained_bytes<'a>(
    snapshots: impl Iterator<Item = &'a Snapshot>,
    saved: Option<&Arc<Vec<ManufacturingOption>>>,
) -> Result<usize, ProjectError> {
    let mut allocations = BTreeMap::new();
    let mut intents = BTreeMap::new();
    for options in snapshots
        .map(|snapshot| &snapshot.manufacturing_options)
        .chain(saved)
    {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            allocations.entry(Arc::as_ptr(options))
        {
            validate_manufacturing_options(options)?;
            entry.insert(
                (options.capacity() * std::mem::size_of::<ManufacturingOption>()).saturating_add(
                    options
                        .iter()
                        .map(|option| {
                            option.name.capacity()
                                + option.id.as_str().len()
                                + option.context_hash.as_str().len()
                        })
                        .sum::<usize>(),
                ),
            );
            for option in options.iter() {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    intents.entry(Arc::as_ptr(&option.intent))
                {
                    let bytes = serde_json::to_vec(option.intent.as_ref())
                        .map_err(|_| invalid("manufacturing option intent cannot be encoded"))?;
                    entry.insert(bytes.len());
                }
            }
        }
    }
    allocations
        .values()
        .chain(intents.values())
        .try_fold(0usize, |total, bytes| {
            total.checked_add(*bytes).ok_or_else(|| {
                error(
                    ProjectErrorCode::ResourceLimit,
                    "retained manufacturing option budget overflow",
                )
            })
        })
}

#[cfg(test)]
#[path = "manufacturing_options_tests.rs"]
mod tests;
