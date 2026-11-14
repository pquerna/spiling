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

#[derive(Serialize)]
struct Context<'a> {
    domain: &'static str,
    layout_id: &'a spiling_contracts::project::PrintLayoutId,
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
    let (surfaces, process) = snapshot.scoped_documents();
    let context = Context {
        domain: "spiling.manufacturing-option-context.v1",
        layout_id: &snapshot.layout_id,
        definitions: snapshot
            .active_sources()
            .map(|source| &source.record)
            .collect(),
        occurrences: snapshot.occurrences.values().collect(),
        surface: &surfaces,
        process: &process,
        images: surfaces.images.iter().collect(),
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
    let definitions: Vec<_> = snapshot
        .active_sources()
        .map(|source| source.record.clone())
        .collect();
    let occurrences: Vec<_> = snapshot.occurrences.values().cloned().collect();
    let (surfaces, process) = snapshot.scoped_documents();
    surfaces.validate_targets(&definitions, &occurrences, u32::MAX)?;
    process.validate_targets(&definitions, &occurrences, u32::MAX, u32::MAX)?;
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

/// Distinct option allocations and shared intents, each charged once.
#[derive(Default)]
struct RetainedOptions {
    allocations: BTreeMap<*const ManufacturingOption, usize>,
    intents: BTreeMap<*const ManufacturingIntent, usize>,
}

impl RetainedOptions {
    /// Charge one options vector; returns the bytes it adds beyond those
    /// already charged.
    fn add(&mut self, options: &Vec<ManufacturingOption>) -> Result<usize, ProjectError> {
        let std::collections::btree_map::Entry::Vacant(entry) =
            self.allocations.entry(options.as_ptr())
        else {
            return Ok(0);
        };
        validate_manufacturing_options(options)?;
        let mut added = (options.capacity() * std::mem::size_of::<ManufacturingOption>())
            .saturating_add(
                options
                    .iter()
                    .map(|option| {
                        option.name.capacity()
                            + option.id.as_str().len()
                            + option.context_hash.as_str().len()
                    })
                    .sum::<usize>(),
            );
        entry.insert(added);
        for option in options.iter() {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                self.intents.entry(Arc::as_ptr(&option.intent))
            {
                let bytes = serde_json::to_vec(option.intent.as_ref())
                    .map_err(|_| invalid("manufacturing option intent cannot be encoded"))?;
                entry.insert(bytes.len());
                added = added.saturating_add(bytes.len());
            }
        }
        Ok(added)
    }

    fn add_snapshot(&mut self, snapshot: &Snapshot) -> Result<usize, ProjectError> {
        let mut added = 0usize;
        for layout in snapshot.layouts.iter() {
            added = added.saturating_add(self.add(layout.manufacturing_options.as_ref())?);
        }
        Ok(added)
    }

    fn total(&self) -> Result<usize, ProjectError> {
        self.allocations
            .values()
            .chain(self.intents.values())
            .try_fold(0usize, |total, bytes| {
                total.checked_add(*bytes).ok_or_else(|| {
                    error(
                        ProjectErrorCode::ResourceLimit,
                        "retained manufacturing option budget overflow",
                    )
                })
            })
    }
}

/// Validate one snapshot's options (each is bounded by the manifest).
pub(crate) fn validate_retained(snapshot: &Snapshot) -> Result<(), ProjectError> {
    RetainedOptions::default().add_snapshot(snapshot).map(drop)
}

/// Admit the option allocations a candidate adds beyond everything already
/// retained (current, history, live prepared and saved) against a fresh host
/// allowance. Unchanged options share their allocations and add nothing.
pub(crate) fn admit_growth<'a>(
    retained: impl Iterator<Item = &'a Snapshot>,
    saved: impl IntoIterator<Item = &'a Vec<ManufacturingOption>>,
    candidate: &Snapshot,
    allowance: crate::RetainedGrowthObserver,
) -> Result<(), ProjectError> {
    let mut census = RetainedOptions::default();
    for snapshot in retained {
        census.add_snapshot(snapshot)?;
    }
    for options in saved {
        census.add(options)?;
    }
    census.total()?;
    let growth = census.add_snapshot(candidate)?;
    if growth > 0 && growth > allowance() {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "retained manufacturing options exceed available memory",
        ));
    }
    census.total().map(drop)
}

#[cfg(test)]
pub(crate) fn retained_bytes<'a>(
    snapshots: impl Iterator<Item = &'a Snapshot>,
    saved: impl IntoIterator<Item = &'a Vec<ManufacturingOption>>,
) -> Result<usize, ProjectError> {
    let mut census = RetainedOptions::default();
    for snapshot in snapshots {
        census.add_snapshot(snapshot)?;
    }
    for options in saved {
        census.add(options)?;
    }
    census.total()
}

#[cfg(test)]
#[path = "manufacturing_options_tests.rs"]
mod tests;
