// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Software-only native-section manufacturing, with mandatory independent replay.
pub mod backend;
pub mod planner;
mod realization;
pub mod verifier;
pub use planner::preflight;

use spiling_contracts::geometry::{DefinitionId, KernelIdentity, OccurrenceRecord};
use spiling_contracts::manufacturing::*;
use spiling_contracts::project::{ProjectId, ProjectRevision, StoredDefinition};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub const PLANNER_VERSION: &str = "4";
pub const EMITTER_VERSION: &str = "4";
pub const VERIFIER_VERSION: &str = "4";
const KERNEL_REVISION: &str = "5a4ddb7582c3d7f75a39e800a9fef345cdfe435e";

pub struct CompilerInput<'a> {
    pub project_id: &'a ProjectId,
    pub revision: ProjectRevision,
    pub definitions: &'a BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
    pub stored_definitions: &'a [StoredDefinition],
    pub occurrences: &'a [OccurrenceRecord],
    pub intent: &'a ManufacturingIntent,
    pub surface_overlays: &'a spiling_contracts::surface::SurfaceOverlays,
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
        input.surface_overlays,
    )?;
    let provenance = ManufacturingProvenance {
        project_id: input.project_id.clone(),
        input_revision: input.revision,
        input_hash,
        definitions,
        occurrences,
        intent: input.intent.clone(),
        surface_overlays: input.surface_overlays.clone(),
        kernel: KernelIdentity {
            name: "monstertruck".into(),
            version: "0.4.1".into(),
            revision: KERNEL_REVISION.into(),
        },
        planner_version: PLANNER_VERSION.into(),
        emitter_version: EMITTER_VERSION.into(),
        verifier_version: VERIFIER_VERSION.into(),
    };
    let mut plan = planner::plan(&input, cancel)?;
    realization::canonicalize(&mut plan, input.intent, cancel)?;
    let program = backend::emit(&plan, input.intent, cancel)?;
    let verification = verifier::verify(&program, &plan, input.intent, cancel)?;
    let bundle = ManufacturingBundle {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
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
    inspect_report(bundle, cancel)
}

pub struct EncodedReplayDisplay {
    pub info: ReplayDisplayInfo,
    pub bytes: Vec<u8>,
}

pub struct ManufacturingInspection {
    pub report: VerificationReport,
    pub provenance: ManufacturingInspectionProvenance,
    pub replay: Option<EncodedReplayDisplay>,
}

/// Fresh independent replay; display output is opt-in and bound to the caller's immutable bundle hash.
pub fn inspect_bundle(
    bundle: &ManufacturingBundle,
    bundle_hash: &spiling_contracts::geometry::SourceHash,
    output: VerificationOutput,
    cancel: &AtomicBool,
) -> Result<ManufacturingInspection, ManufacturingError> {
    validate_inspection_bundle(bundle, cancel)?;
    let mut sink = match output {
        VerificationOutput::ReportOnly => None,
        VerificationOutput::ReportAndMotion => Some(verifier::ReplaySink::new(
            &bundle.plan,
            &bundle.provenance.intent,
        )?),
    };
    let report = verifier::verify_actual(
        &bundle.program,
        &bundle.plan,
        &bundle.provenance.intent,
        cancel,
        sink.as_mut(),
    )?;
    check_cancel(cancel)?;
    let replay = if let Some(sink) = sink {
        let (bytes, bounds_mm) = sink.finish()?;
        let info = ReplayDisplayInfo {
            schema_version: REPLAY_SCHEMA_VERSION,
            bundle_hash: bundle_hash.clone(),
            input_hash: bundle.provenance.input_hash.clone(),
            program_hash: spiling_contracts::geometry::SourceHash::from_bytes(
                bundle.program.as_bytes(),
            ),
            verifier_version: VERIFIER_VERSION.into(),
            frame: bundle.provenance.intent.printer.coordinate_frame,
            event_count: report.deposition_segments
                + report.support_deposition_segments
                + report.travel_segments
                + report.retraction_count * 2
                + report.lifecycle_motion_count,
            deposition_count: report.deposition_segments + report.support_deposition_segments,
            travel_count: report.travel_segments,
            lifecycle_count: report.lifecycle_motion_count,
            process_count: report.retraction_count * 2,
            layer_count: bundle.plan.layers.len() as u32,
            bounds_mm,
        };
        info.validate()?;
        Some(EncodedReplayDisplay { info, bytes })
    } else {
        None
    };
    check_cancel(cancel)?;
    Ok(ManufacturingInspection {
        report,
        provenance: (&bundle.provenance).into(),
        replay,
    })
}

fn inspect_report(
    bundle: &ManufacturingBundle,
    cancel: &AtomicBool,
) -> Result<VerificationReport, ManufacturingError> {
    validate_inspection_bundle(bundle, cancel)?;
    verifier::verify(
        &bundle.program,
        &bundle.plan,
        &bundle.provenance.intent,
        cancel,
    )
}

fn validate_inspection_bundle(
    bundle: &ManufacturingBundle,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
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
    Ok(())
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
