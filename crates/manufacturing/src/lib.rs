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
use spiling_geometry::{KERNEL_NAME, KERNEL_REVISION, KERNEL_VERSION};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Binds source-only realization, including constrained PCurve endpoint selection.
pub const PLANNER_VERSION: &str = "21";
pub const EMITTER_VERSION: &str = "7";
pub const VERIFIER_VERSION: &str = "9";

pub struct CompilerInput<'a> {
    pub project_id: &'a ProjectId,
    pub revision: ProjectRevision,
    pub definitions: &'a BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
    pub stored_definitions: &'a [StoredDefinition],
    pub occurrences: &'a [OccurrenceRecord],
    pub intent: &'a ManufacturingIntent,
    pub surface_overlays: &'a spiling_contracts::surface::SurfaceOverlays,
    pub process_document: &'a spiling_contracts::process::ProcessDocument,
    pub surface_images: &'a BTreeMap<
        spiling_contracts::geometry::SourceHash,
        Arc<spiling_surface_fields::HeightImage>,
    >,
}

pub fn compile(
    input: CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<ManufacturingBundle, ManufacturingError> {
    check_cancel(cancel)?;
    // Shared admission validates intent, native identity and the complete image
    // table before planning or provenance allocation. Do not hash/copy assets or
    // prepare the same native/field schedule through a second compile-only lane.
    let mut plan = planner::plan(&input, cancel)?;
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
        input.process_document,
    )?;
    let provenance = ManufacturingProvenance {
        project_id: input.project_id.clone(),
        input_revision: input.revision,
        input_hash,
        definitions,
        occurrences,
        intent: input.intent.clone(),
        surface_overlays: input.surface_overlays.clone(),
        process_document: input.process_document.clone(),
        surface_images: input
            .surface_images
            .iter()
            .map(
                |(image_hash, image)| spiling_contracts::surface::SurfaceImageContent {
                    image_hash: image_hash.clone(),
                    bytes: Arc::clone(image.bytes()),
                },
            )
            .collect(),
        kernel: KernelIdentity {
            name: KERNEL_NAME.into(),
            version: KERNEL_VERSION.into(),
            revision: KERNEL_REVISION.into(),
        },
        planner_version: PLANNER_VERSION.into(),
        emitter_version: EMITTER_VERSION.into(),
        verifier_version: VERIFIER_VERSION.into(),
    };
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
        || p.kernel.name != KERNEL_NAME
        || p.kernel.version != KERNEL_VERSION
        || p.kernel.revision != KERNEL_REVISION
    {
        return Err(error(
            ManufacturingErrorCode::UnsupportedCapability,
            "bundle algorithm/kernel version is not supported",
        ));
    }
    Ok(())
}

/// HeightImage construction validates immutable content once; admission binds
/// those checked allocations to the sole metadata table, including disabled layers.
pub(crate) fn validate_surface_images(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    input
        .process_document
        .validate_targets(
            input.stored_definitions,
            input.occurrences,
            u32::MAX,
            u32::MAX,
        )
        .map_err(|e| error(ManufacturingErrorCode::InvalidSpecification, e.to_string()))?;
    use spiling_contracts::surface::{
        MAX_SURFACE_IMAGE_BYTES, MAX_SURFACE_IMAGE_TOTAL_BYTES, MAX_SURFACE_IMAGES,
    };
    input.surface_overlays.validate().map_err(|e| {
        let code = if e.code == spiling_contracts::project::ProjectErrorCode::ResourceLimit {
            ManufacturingErrorCode::ResourceLimit
        } else {
            ManufacturingErrorCode::InvalidSpecification
        };
        error(code, e.to_string())
    })?;
    if input.surface_images.len() > MAX_SURFACE_IMAGES {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "captured surface image count budget exceeded",
        ));
    }
    if input.surface_images.len() != input.surface_overlays.images.len() {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "captured surface image table differs from overlay metadata",
        ));
    }
    let mut bytes = 0usize;
    for record in &input.surface_overlays.images {
        check_cancel(cancel)?;
        let image = input
            .surface_images
            .get(&record.image_hash)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::InvalidSpecification,
                    "missing captured surface image",
                )
            })?;
        if image.record() != record {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "captured surface image provenance differs from overlay metadata",
            ));
        }
        if image.bytes().capacity() > MAX_SURFACE_IMAGE_BYTES {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "captured surface image allocation budget exceeded",
            ));
        }
        bytes = bytes
            .checked_add(image.bytes().capacity())
            .filter(|n| *n <= MAX_SURFACE_IMAGE_TOTAL_BYTES)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "captured surface image byte budget exceeded",
                )
            })?;
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
