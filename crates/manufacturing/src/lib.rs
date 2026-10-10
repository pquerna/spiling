// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Software-only native-section manufacturing, with mandatory independent replay.
pub mod backend;
pub mod planner;
pub mod verifier;

use spiling_contracts::geometry::{DefinitionId, KernelIdentity, OccurrenceRecord};
use spiling_contracts::manufacturing::*;
use spiling_contracts::project::{ProjectId, ProjectRevision, StoredDefinition};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub const PLANNER_VERSION: &str = "1";
pub const EMITTER_VERSION: &str = "1";
pub const VERIFIER_VERSION: &str = "1";
const KERNEL_REVISION: &str = "d87b4d9ced1f3baf31aa771ac0e7c663efb1c001";

pub struct CompilerInput<'a> {
    pub project_id: &'a ProjectId,
    pub revision: ProjectRevision,
    pub definitions: &'a BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
    pub stored_definitions: &'a [StoredDefinition],
    pub occurrences: &'a [OccurrenceRecord],
    pub intent: &'a ManufacturingIntent,
}

pub fn compile(
    input: CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<ManufacturingBundle, ManufacturingError> {
    check_cancel(cancel)?;
    input.intent.validate()?;
    if input.definitions.len() > spiling_contracts::geometry::MAX_DEFINITIONS as usize
        || input.occurrences.len() > spiling_contracts::geometry::MAX_OCCURRENCES as usize
    {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "compiler native input record budget exceeded",
        ));
    }
    if input.definitions.is_empty() || input.occurrences.is_empty() {
        return Err(error(
            ManufacturingErrorCode::EmptyProject,
            "compilation requires native definitions and occurrences",
        ));
    }
    if input.definitions.len() != input.stored_definitions.len() {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "stored/native definition sets differ",
        ));
    }
    for stored in input.stored_definitions {
        check_cancel(cancel)?;
        let definition = input
            .definitions
            .get(&stored.definition_id)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::InvalidSpecification,
                    "stored definition has no native definition",
                )
            })?;
        let native = definition.provenance();
        if definition.id() != &stored.definition_id
            || native.source_hash != stored.provenance.source_hash
            || native.source_unit != stored.provenance.source_unit
            || native.uncertainty_mm != stored.provenance.uncertainty_mm
        {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "stored/native source identity or units mismatch",
            ));
        }
    }
    let mut definitions = input.stored_definitions.to_vec();
    definitions.sort_by(|a, b| a.definition_id.cmp(&b.definition_id));
    let mut occurrences = input.occurrences.to_vec();
    occurrences.sort_by_key(|o| o.occurrence_id);
    let input_hash = input_fingerprint(
        input.project_id,
        &definitions,
        input.occurrences,
        input.intent,
    )?;
    let provenance = ManufacturingProvenance {
        project_id: input.project_id.clone(),
        input_revision: input.revision,
        input_hash,
        definitions,
        occurrences,
        intent: input.intent.clone(),
        kernel: KernelIdentity {
            name: "monstertruck".into(),
            version: "0.4.1".into(),
            revision: KERNEL_REVISION.into(),
        },
        planner_version: PLANNER_VERSION.into(),
        emitter_version: EMITTER_VERSION.into(),
        verifier_version: VERIFIER_VERSION.into(),
    };
    let plan = planner::plan(&input, cancel)?;
    let program = backend::emit(&plan, input.intent, cancel)?;
    let verification = verifier::verify(&program, &plan, input.intent, cancel)?;
    let bundle = ManufacturingBundle {
        schema_version: 1,
        provenance,
        plan,
        program,
        verification,
    };
    bundle.validate()?;
    check_cancel(cancel)?;
    Ok(bundle)
}

/// Recompute replay; the persisted report is descriptive, never trusted authority.
pub fn verify_bundle(
    bundle: &ManufacturingBundle,
    cancel: &AtomicBool,
) -> Result<VerificationReport, ManufacturingError> {
    check_cancel(cancel)?;
    bundle.validate()?;
    let p = &bundle.provenance;
    if p.planner_version != PLANNER_VERSION
        || p.emitter_version != EMITTER_VERSION
        || p.verifier_version != VERIFIER_VERSION
        || p.kernel.name != "monstertruck"
        || p.kernel.version != "0.4.1"
        || p.kernel.revision != KERNEL_REVISION
    {
        return Err(error(
            ManufacturingErrorCode::UnsupportedCapability,
            "bundle algorithm/kernel version is not supported",
        ));
    }
    verifier::verify(&bundle.program, &bundle.plan, &p.intent, cancel)
}

pub(crate) fn error(code: ManufacturingErrorCode, message: impl AsRef<str>) -> ManufacturingError {
    ManufacturingError::new(code, message.as_ref())
}
pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    if cancel.load(Ordering::Relaxed) {
        Err(error(
            ManufacturingErrorCode::Cancelled,
            "manufacturing operation cancelled",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
