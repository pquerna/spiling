// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Software-only native-section manufacturing, with mandatory independent replay.
mod backend;
#[cfg(test)]
mod clearance_screen;
pub mod delivery;
mod material;
mod selection;
mod timing;
pub use material::{
    create_default_intent, create_material_intent, create_requested_intent, filament_selection,
    generic_selection, resolve_material_profile, resolve_tool_material_profile, review_materials,
};
pub use spiling_contracts::manufacturing::{QualityPreset, SupportChoice};
pub mod planner;
mod realization;
mod routing;
mod stage_io;
mod surface_inspection;
pub mod verifier;
pub mod working_set;
pub use planner::preflight;

/// Re-execute a captured support stage without importing a source or running
/// any other manufacturing stage. Developer captures are not plan authority.
#[doc(hidden)]
pub fn replay_support_stage(
    blob: &std::path::Path,
    working_set: WorkingSet,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    planner::support_stage::replay(blob, working_set, cancel)
}
/// Replay the typed process realization boundary from a developer capture.
#[doc(hidden)]
pub fn replay_realization_stage(
    blob: &std::path::Path,
    working_set: WorkingSet,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    realization::stage::replay(blob, working_set, cancel)
}
pub use working_set::WorkingSet;

use spiling_contracts::geometry::{DefinitionId, KernelIdentity, OccurrenceRecord, SourceHash};
use spiling_contracts::manufacturing::*;
use spiling_contracts::project::{ProjectId, ProjectRevision, StoredDefinition};
use spiling_geometry::{KERNEL_NAME, KERNEL_REVISION, KERNEL_VERSION};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Binds native material authority and physical per-occurrence material events.
pub const PLANNER_VERSION: &str = "174";
pub const EMITTER_VERSION: &str = "23";
pub const VERIFIER_VERSION: &str = "63";

pub struct CompilerInput<'a> {
    pub project_id: &'a ProjectId,
    pub revision: ProjectRevision,
    pub definitions: &'a BTreeMap<DefinitionId, Arc<spiling_geometry::source::SourceDefinition>>,
    pub stored_definitions: &'a [StoredDefinition],
    pub occurrences: &'a [OccurrenceRecord],
    pub intent: &'a ManufacturingIntent,
    pub surface_overlays: &'a spiling_contracts::surface::SurfaceOverlays,
    pub process_document: &'a spiling_contracts::process::ProcessDocument,
    pub surface_images: &'a BTreeMap<
        spiling_contracts::geometry::SourceHash,
        Arc<spiling_surface_fields::HeightImage>,
    >,
    /// Host-selected live working-set admission for this operation.
    pub working_set: WorkingSet,
}

pub use spiling_chunks::ChunkSource;

/// A bundle paired with the exact emitted program bytes its descriptor names.
/// The bundle is a serializable document; the program is a separate artifact.
#[derive(Debug, Clone)]
pub struct ManufacturingResult {
    pub bundle: ManufacturingBundle,
    pub program: ChunkSource,
}
impl ManufacturingResult {
    /// Pair checked program bytes with a bundle, refusing a mismatched descriptor.
    pub fn new(
        bundle: ManufacturingBundle,
        program: ChunkSource,
    ) -> Result<Self, ManufacturingError> {
        check_program(&bundle.program, &program)?;
        Ok(Self { bundle, program })
    }
    /// Materialized program text for small diagnostics and tests.
    pub fn program_text(&self) -> Result<String, ManufacturingError> {
        let bytes = self
            .program
            .to_vec()
            .map_err(|e| error(ManufacturingErrorCode::ResourceLimit, e.to_string()))?;
        String::from_utf8(bytes).map_err(|_| {
            error(
                ManufacturingErrorCode::CorruptArtifact,
                "program bytes are not UTF-8",
            )
        })
    }
}
impl std::ops::Deref for ManufacturingResult {
    type Target = ManufacturingBundle;
    fn deref(&self) -> &ManufacturingBundle {
        &self.bundle
    }
}
impl std::ops::DerefMut for ManufacturingResult {
    fn deref_mut(&mut self) -> &mut ManufacturingBundle {
        &mut self.bundle
    }
}

/// Descriptor naming exact program bytes.
pub fn program_descriptor(program: &ChunkSource) -> ProgramDescriptor {
    ProgramDescriptor {
        hash: spiling_contracts::geometry::SourceHash::from_digest(program.digest()),
        byte_count: program.len(),
    }
}

pub fn compile(
    input: CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<ManufacturingResult, ManufacturingError> {
    working_set::scope(input.working_set, || compile_scoped(input, cancel, None))
}

fn compile_scoped(
    input: CompilerInput<'_>,
    cancel: &AtomicBool,
    replay: Option<&mut Option<verifier::ReplaySink>>,
) -> Result<ManufacturingResult, ManufacturingError> {
    if matches!(&input.intent.printer.workflow, JobWorkflow::FixedDualMarlin { job } if job.has_bound_interior())
    {
        return selection::compile(input, cancel, replay);
    }
    let mut work = spiling_geometry::GeometryWorkBudget::compile();
    compile_exact_with_replay(input, cancel, &mut work, replay)
}

fn compile_exact(
    input: CompilerInput<'_>,
    cancel: &AtomicBool,
    geometry_work: &mut spiling_geometry::GeometryWorkBudget,
) -> Result<ManufacturingResult, ManufacturingError> {
    compile_exact_with_replay(input, cancel, geometry_work, None)
}

fn compile_exact_with_replay(
    input: CompilerInput<'_>,
    cancel: &AtomicBool,
    geometry_work: &mut spiling_geometry::GeometryWorkBudget,
    replay: Option<&mut Option<verifier::ReplaySink>>,
) -> Result<ManufacturingResult, ManufacturingError> {
    let mut timing = timing::Clock::new("manufacturing.compile");
    check_cancel(cancel)?;
    // Shared admission validates intent, native identity and the complete image
    // table before planning or provenance allocation. Do not hash/copy assets or
    // prepare the same native/field schedule through a second compile-only lane.
    let prepared = planner::plan(&input, cancel, geometry_work)?;
    timing.mark("planning");
    let mut definitions = input.stored_definitions.to_vec();
    definitions.sort_by(|a, b| a.definition_id.cmp(&b.definition_id));
    let occurrences = input.occurrences.to_vec();
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
    timing.mark("provenance");
    let (program, verification) = emit_and_replay(&prepared, input.intent, cancel, replay)?;
    let plan = prepared.into_plan();
    timing.mark("emission_and_independent_replay");
    let bundle = ManufacturingBundle {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
        provenance,
        plan,
        program: program_descriptor(&program),
        verification,
    };
    bundle.validate()?;
    timing.mark("bundle_validation");
    check_cancel(cancel)?;
    Ok(ManufacturingResult { bundle, program })
}

/// Emit the program and independently replay it at once: emission writes on
/// its own thread through a tapped writer while the replay decodes the same
/// bytes as each frame seals. The replay shares no emitter state; only the
/// program bytes pass between them.
fn emit_and_replay(
    prepared: &planner::PreparedPlan<'_>,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
    replay: Option<&mut Option<verifier::ReplaySink>>,
) -> Result<(ChunkSource, VerificationReport), ManufacturingError> {
    /// Sealed frames the emitter may run ahead of the replay.
    const AHEAD_FRAMES: usize = 16;
    let (writer, stream) =
        spiling_chunks::ChunkWriter::tapped(backend::program_writer_limit(), AHEAD_FRAMES);
    let working_set = working_set::current();
    let (program, report) = std::thread::scope(|scope| {
        let emitter = scope.spawn(move || {
            working_set::scope(working_set, || {
                backend::emit_prepared_into(prepared, writer, cancel)
            })
        });
        let (plan, _, _) = prepared.emission_inputs();
        let report = verifier::verify_streamed(
            std::io::BufReader::new(stream),
            plan,
            intent,
            cancel,
            replay.map(verifier::ReplayTarget::Capture),
        );
        let program = emitter
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (program, report)
    });
    // An emission failure is the cause of any replay failure it produced.
    let program = program?;
    let report = report?;
    let (plan, _, _) = prepared.emission_inputs();
    verifier::validate_selection(plan, program.digest(), &report)?;
    Ok((program, report))
}

fn verify_compiled(
    program: &ChunkSource,
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
    replay: Option<&mut Option<verifier::ReplaySink>>,
) -> Result<VerificationReport, ManufacturingError> {
    verifier::verify_actual(
        program,
        plan,
        intent,
        cancel,
        replay.map(verifier::ReplayTarget::Capture),
    )
}

/// Recompute replay; the persisted report is descriptive, never trusted authority.
/// Runs under the caller's [`working_set::scope`].
pub fn verify_bundle(
    result: &ManufacturingResult,
    cancel: &AtomicBool,
) -> Result<VerificationReport, ManufacturingError> {
    inspect_report(result, cancel)
}

pub struct EncodedReplayDisplay {
    pub info: ReplayDisplayInfo,
    pub bytes: spiling_chunks::ChunkSource,
    pub pages: Vec<ReplayPage>,
}

pub struct ManufacturingInspection {
    pub report: VerificationReport,
    pub provenance: ManufacturingInspectionProvenance,
    pub replay: Option<EncodedReplayDisplay>,
}

/// Exact serialized output of the compiler's independent replay.
pub struct CompiledArtifact {
    pub record: ManufacturingArtifactRecord,
    pub bundle: ChunkSource,
    pub program: ChunkSource,
    pub receipt: CompileVerification,
    pub inspection: Option<CapturedInspection>,
}

pub struct CapturedInspection {
    pub inspection: ManufacturingInspection,
    pub interior_selection: Option<Arc<InteriorSelection>>,
}

/// One ephemeral proof of an exact bundle/program pair. No bytes or parsed plan are retained.
pub struct CompileVerification {
    bundle_digest: [u8; 32],
    program_digest: [u8; 32],
}

impl CompileVerification {
    /// Inspect exact bytes covered at compile, never a caller-supplied decoded document.
    pub fn inspect_report(
        &self,
        encoded: &ChunkSource,
        program: &ChunkSource,
        cancel: &AtomicBool,
    ) -> Result<Option<(ManufacturingInspection, Vec<OccurrenceRecord>)>, ManufacturingError> {
        check_cancel(cancel)?;
        if encoded.digest() != &self.bundle_digest || program.digest() != &self.program_digest {
            return Ok(None);
        }
        let bundle = decode_bundle_from(encoded.len(), || encoded.reader())?;
        let report = bundle.verification.clone();
        let inspection = finish_inspection(
            &bundle,
            &SourceHash::from_digest(encoded.digest()),
            report,
            None,
            cancel,
        )?;
        Ok(Some((inspection, bundle.provenance.occurrences)))
    }
}

/// Serialize only a compiler-verified result, binding the receipt to the emitted immutable bytes.
pub fn compile_artifact(
    input: CompilerInput<'_>,
    byte_budget: u32,
    capture_motion: bool,
    cancel: &AtomicBool,
) -> Result<CompiledArtifact, ManufacturingError> {
    working_set::scope(input.working_set, || {
        let mut replay = None;
        let result = compile_scoped(input, cancel, capture_motion.then_some(&mut replay))?;
        seal_compiled_artifact(result, replay, byte_budget, cancel)
    })
}

fn seal_compiled_artifact(
    result: ManufacturingResult,
    replay: Option<verifier::ReplaySink>,
    byte_budget: u32,
    cancel: &AtomicBool,
) -> Result<CompiledArtifact, ManufacturingError> {
    let ManufacturingResult { bundle, program } = result;
    let exceeded = || {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "encoded manufacturing bundle exceeds remaining retained/staged budget",
        )
    };
    let mut writer = spiling_chunks::ChunkWriter::new(u64::from(MAX_MANUFACTURING_BUNDLE_BYTES));
    serde_json::to_writer(&mut writer, &bundle).map_err(|_| exceeded())?;
    let encoded = writer.finish().map_err(|_| exceeded())?;
    if (encoded.retained_bytes() + program.retained_bytes()) as u64 > u64::from(byte_budget) {
        return Err(exceeded());
    }
    check_cancel(cancel)?;
    let record = ManufacturingArtifactRecord {
        hash: SourceHash::from_digest(encoded.digest()),
        input_hash: bundle.provenance.input_hash.clone(),
        byte_count: encoded.len() as u32,
        summary: bundle.summary(),
        program: bundle.program.clone(),
    };
    let receipt = CompileVerification {
        bundle_digest: *encoded.digest(),
        program_digest: *program.digest(),
    };
    let inspection = replay
        .map(|sink| {
            let report = bundle.verification.clone();
            let inspection = finish_inspection(&bundle, &record.hash, report, Some(sink), cancel)?;
            Ok::<_, ManufacturingError>(CapturedInspection {
                inspection,
                interior_selection: bundle.plan.interior_selection.clone(),
            })
        })
        .transpose()?;
    Ok(CompiledArtifact {
        record,
        bundle: encoded,
        program,
        receipt,
        inspection,
    })
}

/// Fresh independent replay; display output is opt-in and bound to the caller's immutable bundle hash.
/// Runs under the caller's [`working_set::scope`].
pub fn inspect_bundle(
    result: &ManufacturingResult,
    bundle_hash: &spiling_contracts::geometry::SourceHash,
    output: VerificationOutput,
    cancel: &AtomicBool,
) -> Result<ManufacturingInspection, ManufacturingError> {
    inspect_decoded_bundle(&result.bundle, &result.program, bundle_hash, output, cancel)
}

/// Freshly replay a previously decoded bundle whose envelope and hash binding
/// were checked by the caller. No saved verification report is reused.
pub fn inspect_decoded_bundle(
    bundle: &ManufacturingBundle,
    program: &ChunkSource,
    bundle_hash: &SourceHash,
    output: VerificationOutput,
    cancel: &AtomicBool,
) -> Result<ManufacturingInspection, ManufacturingError> {
    validate_inspection_bundle(bundle, program, cancel)?;
    let mut sink = None;
    let report = verifier::verify_actual(
        program,
        &bundle.plan,
        &bundle.provenance.intent,
        cancel,
        output
            .motion()
            .then_some(verifier::ReplayTarget::Capture(&mut sink)),
    )?;
    finish_inspection(bundle, bundle_hash, report, sink, cancel)
}

fn finish_inspection(
    bundle: &ManufacturingBundle,
    bundle_hash: &SourceHash,
    report: VerificationReport,
    sink: Option<verifier::ReplaySink>,
    cancel: &AtomicBool,
) -> Result<ManufacturingInspection, ManufacturingError> {
    check_cancel(cancel)?;
    // Motion too large to display is omitted, never a verification failure.
    let replay = if let Some(sink) = sink.filter(|sink| !sink.omitted()) {
        let tool_epoch_count = report
            .routing
            .as_ref()
            .map_or(0, |routing| routing.selection_count);
        let waste_count = report.routing.as_ref().map_or(0, |routing| {
            routing
                .tools
                .iter()
                .map(|tool| tool.waste_deposition_segments)
                .sum()
        });
        let process_count =
            report.retraction_count * 2 - if report.routing.is_some() { 2 } else { 0 };
        let (bytes, bounds_mm) = sink.finish()?;
        let info = ReplayDisplayInfo {
            schema_version: REPLAY_SCHEMA_VERSION,
            bundle_hash: bundle_hash.clone(),
            verifier_version: VERIFIER_VERSION.into(),
            frame: bundle.provenance.intent.printer.coordinate_frame,
            event_count: report.deposition_segments
                + report.support_deposition_segments
                + report.adhesion_deposition_segments
                + report.travel_segments
                + process_count
                + report.lifecycle_motion_count
                + tool_epoch_count
                + waste_count,
            deposition_count: report.deposition_segments
                + report.support_deposition_segments
                + report.adhesion_deposition_segments
                + waste_count,
            travel_count: report.travel_segments,
            lifecycle_count: report.lifecycle_motion_count,
            process_count,
            tool_epoch_count,
            waste_count,
            layer_count: bundle.plan.layers.len() as u32,
            bounds_mm,
        };
        info.validate()?;
        let pages = index_replay(bytes.reader(), &info)?;
        Some(EncodedReplayDisplay { info, bytes, pages })
    } else {
        None
    };
    check_cancel(cancel)?;
    let mut provenance: ManufacturingInspectionProvenance = (&bundle.provenance).into();
    provenance.surface_color = surface_inspection::inspect(bundle, cancel)?;
    provenance.validate()?;
    Ok(ManufacturingInspection {
        report,
        provenance,
        replay,
    })
}

/// Write the firmware delivery package around a freshly verified program
/// (`report` from [`inspect_bundle`]), then re-read it independently; only a
/// package that re-reads is returned. `None` when the printer takes a plain
/// program. `objects` are the plate's posed display meshes the package
/// carries for slicer viewers; the program alone is the printer's authority.
pub fn delivery_package(
    result: &ManufacturingResult,
    bundle_hash: &spiling_contracts::geometry::SourceHash,
    report: &VerificationReport,
    objects: &[delivery::PackageObject],
    cancel: &AtomicBool,
) -> Result<Option<delivery::EncodedDeliveryPackage>, ManufacturingError> {
    let Some(format) = delivery::package_format(&result.bundle.provenance.intent) else {
        return Ok(None);
    };
    match format {
        DeliveryPackageFormat::BambuGcode3mfV1 => {
            let (bytes, members, md5) = delivery::write_bambu_package(
                &result.bundle,
                &result.program,
                report,
                objects,
                cancel,
            )?;
            let info = verifier::package::verify_bambu_package(
                &bytes,
                &result.program,
                &result.bundle,
                bundle_hash,
                report,
                objects.len(),
                cancel,
            )?;
            if info.members != members || info.program_md5 != md5 {
                return Err(error(
                    ManufacturingErrorCode::VerificationFailed,
                    "delivery package re-read disagrees with what was written",
                ));
            }
            info.validate()?;
            Ok(Some(delivery::EncodedDeliveryPackage { info, bytes }))
        }
    }
}

fn inspect_report(
    result: &ManufacturingResult,
    cancel: &AtomicBool,
) -> Result<VerificationReport, ManufacturingError> {
    validate_inspection_bundle(&result.bundle, &result.program, cancel)?;
    verifier::verify(
        &result.program,
        &result.plan,
        &result.provenance.intent,
        cancel,
    )
}

fn validate_inspection_bundle(
    bundle: &ManufacturingBundle,
    program: &ChunkSource,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    // Decoding binds the bundle envelope; inspection binds provenance and
    // the current program, and replay independently validates plan semantics.
    bundle.provenance.validate()?;
    check_program(&bundle.program, program)?;
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

fn check_program(
    descriptor: &ProgramDescriptor,
    program: &ChunkSource,
) -> Result<(), ManufacturingError> {
    descriptor.validate()?;
    if !descriptor.matches(program.len(), program.digest()) {
        return Err(error(
            ManufacturingErrorCode::CorruptArtifact,
            "program bytes differ from the bundle's program descriptor",
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

pub(crate) fn compute_error(cause: spiling_compute::ComputeError) -> ManufacturingError {
    let code = match &cause {
        spiling_compute::ComputeError::Cancelled => ManufacturingErrorCode::Cancelled,
        spiling_compute::ComputeError::Allocation => ManufacturingErrorCode::ResourceLimit,
        _ => ManufacturingErrorCode::UnsupportedCapability,
    };
    error(code, cause.to_string())
}

#[cfg(test)]
mod tests;
