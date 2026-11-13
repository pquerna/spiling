// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::surface;
use crate::{CompilerInput, check_cancel, error};
use spiling_contracts::geometry::{
    AabbMm, GeometryErrorCode, MAX_DEFINITIONS, MAX_OCCURRENCES, OccurrenceId,
};
use spiling_contracts::manufacturing::*;
use spiling_geometry::tolerant::{TolerantModel, TolerantPolicy};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

pub(super) struct Admission<'work> {
    pub bounds: Option<AabbMm>,
    pub horizontal_events: Vec<f64>,
    pub native_bounds: BTreeMap<OccurrenceId, AabbMm>,
    pub surface_fields: BTreeMap<OccurrenceId, Vec<surface::PreparedFinish>>,
    pub report: PreflightReport,
    pub geometry_work: &'work mut spiling_geometry::GeometryWorkBudget,
    /// ADR012: each occurrence's tessellation placed in world coordinates.
    pub models: BTreeMap<OccurrenceId, std::sync::Arc<TolerantModel>>,
}

impl Admission<'_> {
    pub(super) fn model_bytes(&self) -> usize {
        self.models
            .values()
            .map(|model| model.retained_bytes())
            .sum::<usize>()
    }
}

/// The selected printer's motion step on one world axis.
fn axis_resolution(input: &CompilerInput<'_>, axis: usize) -> f64 {
    let resolution = input.intent.printer.motion_resolution;
    if axis == 2 {
        resolution.z_mm
    } else {
        resolution.xy_mm
    }
}

pub fn preflight(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<PreflightReport, ManufacturingError> {
    let mut work = spiling_geometry::GeometryWorkBudget::compile();
    Ok(admit(input, cancel, &mut work)?.report)
}

/// Native/finish bounds for packing heuristics. Complete candidate compilation and
/// independent replay remain mandatory; these bounds do not certify supports.
pub fn layout_footprints(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<Vec<(OccurrenceId, AabbMm)>, ManufacturingError> {
    if matches!(&input.intent.printer.workflow, JobWorkflow::FixedDualMarlin { job } if !job.uniform_without_construction())
    {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "Fixed-dual arrangement currently requires uniform parts without material construction",
        ));
    }
    let mut work = spiling_geometry::GeometryWorkBudget::compile();
    let admission = admit(input, cancel, &mut work)?;
    for check in &admission.report.checks {
        if check.required
            && matches!(
                check.evidence,
                PreflightEvidence::Unsupported
                    | PreflightEvidence::Failed
                    | PreflightEvidence::Indeterminate
            )
            && !matches!(
                check.code.as_str(),
                "outside_envelope"
                    | "hardware_placement"
                    | "surface_component_exclusion"
                    | "surface_prime_exclusion"
                    | "surface_occurrence_exclusion"
            )
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                &check.message,
            ));
        }
    }
    input
        .occurrences
        .iter()
        .map(|o| {
            let b = *admission
                .native_bounds
                .get(&o.occurrence_id)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "Layout source has no admitted native bounds",
                    )
                })?;
            Ok((
                o.occurrence_id,
                admission
                    .surface_fields
                    .get(&o.occurrence_id)
                    .map_or(b, |f| surface::expanded(b, f)),
            ))
        })
        .collect()
}

/// ADR012 preflight: bounds and material extents come from each
/// occurrence's placed tessellation.
pub(super) fn admit<'work>(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
    geometry_work: &'work mut spiling_geometry::GeometryWorkBudget,
) -> Result<Admission<'work>, ManufacturingError> {
    check_cancel(cancel)?;
    let mut tolerant_models: BTreeMap<spiling_contracts::geometry::DefinitionId, TolerantModel> =
        BTreeMap::new();
    let mut result = Admission {
        bounds: None,
        horizontal_events: Vec::new(),
        native_bounds: BTreeMap::new(),
        surface_fields: BTreeMap::new(),
        geometry_work,
        models: BTreeMap::new(),
        report: PreflightReport {
            input_revision: input.revision,
            disposition: PreflightDisposition::Admitted,
            software_only: true,
            checks: Vec::new(),
        },
    };
    let mut truncated = false;
    let mut pending_faces = Vec::new();
    let mut add = |code: &str, message: String, occurrence_id, face_id| {
        let evidence = match code {
            "native_analysis_indeterminate"
            | "native_conditioning_failure"
            | "unresolved_native_event" => PreflightEvidence::Indeterminate,
            "native_unsupported_geometry"
            | "native_unsupported_conversion"
            | "unsupported_surface" => PreflightEvidence::Unsupported,
            _ => PreflightEvidence::Failed,
        };
        if result.report.disposition != PreflightDisposition::Refused {
            result.report.disposition = if evidence == PreflightEvidence::Indeterminate {
                PreflightDisposition::Unresolved
            } else {
                PreflightDisposition::Refused
            };
        }
        // Truncating required failures never restores admission.
        if result.report.checks.len() < 252 {
            result.report.checks.push(PreflightCheck {
                code: code.into(),
                severity: PreflightSeverity::Error,
                evidence,
                required: true,
                message,
                occurrence_id,
                face_id,
            });
        } else {
            truncated = true;
        }
    };
    if let Err(e) = input.intent.validate() {
        add("invalid_intent", e.to_string(), None, None);
    }
    if matches!(
        input.intent.recipe.sparse_infill,
        SparseInfillPolicy::Grid | SparseInfillPolicy::WebRoof { .. }
    ) && (input
        .process_document
        .layers
        .iter()
        .any(|layer| layer.enabled && !layer.operations.is_empty())
        || input
            .surface_overlays
            .layers
            .iter()
            .any(|layer| layer.enabled))
    {
        add(
            "infill_scope",
            "Two-axis infill currently requires no enabled process or finish layers".into(),
            None,
            None,
        );
    }
    if let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow {
        if let Some(construction) = &job.functional_construction {
            let source = input
                .occurrences
                .iter()
                .find(|occurrence| {
                    occurrence.occurrence_id == construction.occurrence_id
                        && occurrence.definition_id == construction.definition_id
                })
                .and_then(|occurrence| input.definitions.get(&occurrence.definition_id));
            if source.is_none_or(|native| {
                native.provenance().source_hash != construction.source_hash
                    || !native
                        .faces()
                        .iter()
                        .any(|face| face.face_id == construction.face_id)
            }) {
                add("invalid_functional_target", "Functional material construction does not bind the immutable native source and face".into(), Some(construction.occurrence_id), Some(construction.face_id.clone()));
            }
        }
        for construction in &job.surface_constructions {
            let source = input
                .occurrences
                .iter()
                .find(|o| {
                    o.occurrence_id == construction.occurrence_id
                        && o.definition_id == construction.definition_id
                })
                .and_then(|o| input.definitions.get(&o.definition_id));
            if source.is_none_or(|native| {
                native.provenance().source_hash != construction.source_hash
                    || construction.faces.iter().any(|f| {
                        !native
                            .faces()
                            .iter()
                            .any(|actual| actual.face_id == f.face_id)
                    })
            }) {
                add("invalid_surface_targets", "Surface material construction does not bind the immutable native source/face table".into(), Some(construction.occurrence_id), None);
            }
        }
        if input.occurrences.len() != job.assignments.len()
            || job.assignments.iter().any(|assignment| {
                input
                    .occurrences
                    .iter()
                    .filter(|occurrence| occurrence.occurrence_id == assignment.occurrence_id)
                    .count()
                    != 1
            })
        {
            add(
                "fixed_dual_sources",
                "Fixed-dual routing requires exactly the explicitly assigned native occurrences"
                    .into(),
                None,
                None,
            );
        }
        if !matches!(input.intent.recipe.supports, SupportPolicy::Disabled)
            || (!matches!(input.intent.recipe.sparse_infill, SparseInfillPolicy::Fine)
                && !job.has_bound_interior())
            || !matches!(
                input.intent.recipe.process.retraction,
                RetractionPolicy::Disabled
            )
            || input
                .process_document
                .layers
                .iter()
                .any(|layer| layer.enabled && !layer.operations.is_empty())
        {
            add("fixed_dual_policy", "Fixed-dual routing requires disabled body retraction/support/process constructions; combined interiors require an explicit same-formulation tool binding".into(), None, None);
        }
    }
    if input.definitions.len() > MAX_DEFINITIONS as usize
        || input.occurrences.len() > MAX_OCCURRENCES as usize
    {
        add(
            "resource_limit",
            "Native input record budget exceeded".into(),
            None,
            None,
        );
    }
    if input.definitions.is_empty() || input.occurrences.is_empty() {
        add(
            "empty_project",
            "Place at least one admitted native definition before Compile".into(),
            None,
            None,
        );
    }
    if input.stored_definitions.len() != input.definitions.len() {
        add(
            "source_identity",
            "Stored/native definition sets differ".into(),
            None,
            None,
        );
    }
    for stored in input
        .stored_definitions
        .iter()
        .take(MAX_DEFINITIONS as usize)
    {
        check_cancel(cancel)?;
        match input.definitions.get(&stored.definition_id) {
            Some(native)
                if native.id() == &stored.definition_id
                    && native.provenance().source_hash == stored.provenance.source_hash
                    && native.provenance().source_unit == stored.provenance.source_unit
                    && native.provenance().uncertainty_mm == stored.provenance.uncertainty_mm => {}
            _ => add(
                "source_identity",
                "Stored/native source identity or units mismatch".into(),
                None,
                None,
            ),
        }
    }
    let mut surface_valid = match crate::validate_surface_images(input, cancel)
        .and_then(|()| surface::validate_targets(input, cancel))
    {
        Ok(()) => true,
        Err(e) if e.code == ManufacturingErrorCode::Cancelled => return Err(e),
        Err(e) => {
            add(
                if e.code == ManufacturingErrorCode::ResourceLimit {
                    "resource_limit"
                } else {
                    "invalid_surface_targets"
                },
                e.message,
                None,
                None,
            );
            false
        }
    };
    let mut occurrence_bounds = BTreeMap::new();
    let mut surface_samples = 0usize;
    let envelope = input.intent.printer.build_envelope;
    'occurrences: for occurrence in input.occurrences.iter().take(MAX_OCCURRENCES as usize) {
        check_cancel(cancel)?;
        if let Err(e) = occurrence.pose.validate() {
            add(
                "invalid_pose",
                e.to_string(),
                Some(occurrence.occurrence_id),
                None,
            );
            continue;
        }
        let Some(definition) = input.definitions.get(&occurrence.definition_id) else {
            add(
                "missing_definition",
                "Occurrence definition is missing".into(),
                Some(occurrence.occurrence_id),
                None,
            );
            continue;
        };
        if let JobWorkflow::FixedDualMarlin { .. } = &input.intent.printer.workflow {
            let rotation = occurrence.pose.rotation_xyzw;
            if rotation[0] != 0.0 || rotation[1] != 0.0 {
                add(
                    "fixed_dual_upright",
                    "Fixed-dual routing requires an exactly upright source frame".into(),
                    Some(occurrence.occurrence_id),
                    None,
                );
            }
            if definition.faces().iter().any(|face| {
                input
                    .surface_overlays
                    .effective_layer(
                        &occurrence.definition_id,
                        occurrence.occurrence_id,
                        &face.face_id,
                    )
                    .is_some_and(|layer| {
                        !matches!(
                            layer.finish,
                            spiling_contracts::surface::SurfaceFinish::Smooth
                        )
                    })
            }) {
                add(
                    "fixed_dual_finish",
                    "Fixed-dual routing does not admit effective source finishes".into(),
                    Some(occurrence.occurrence_id),
                    None,
                );
            }
        }
        if !tolerant_models.contains_key(&occurrence.definition_id) {
            match definition.prepare(TolerantPolicy::default(), cancel) {
                Ok(model) if model.failed_faces == 0 => {
                    tolerant_models.insert(occurrence.definition_id.clone(), model);
                }
                Ok(model) => {
                    add(
                        "native_unsupported_geometry",
                        format!(
                            "{} source faces could not be tessellated within the profile tolerance",
                            model.failed_faces
                        ),
                        Some(occurrence.occurrence_id),
                        None,
                    );
                    continue 'occurrences;
                }
                Err(e) if e.code == GeometryErrorCode::Cancelled => {
                    return Err(super::native_error(e, "tolerant tessellation"));
                }
                Err(e) => {
                    add(
                        native_check_code(e.code),
                        e.to_string(),
                        Some(occurrence.occurrence_id),
                        None,
                    );
                    continue 'occurrences;
                }
            }
        }
        let model = tolerant_models[&occurrence.definition_id].placed(&occurrence.pose);
        let margin = model.policy().profile_mm;
        let (min, max) = model.bounds();
        // Tessellation vertices lie on the surface; chords can cut inside
        // curved walls by at most the profile tolerance laterally. Z stays
        // at the vertex extremes so bed contact remains exactly at the bed.
        let placed = AabbMm {
            min: [min[0] - margin, min[1] - margin, min[2]],
            max: [max[0] + margin, max[1] + margin, max[2]],
        };
        result
            .models
            .insert(occurrence.occurrence_id, std::sync::Arc::new(model));
        let mut placed = placed;
        result
            .native_bounds
            .insert(occurrence.occurrence_id, placed);
        if let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow {
            match job.assignments.iter().find(|assignment| assignment.occurrence_id == occurrence.occurrence_id) {
                Some(assignment) if (0..3).all(|axis| {
                    placed.min[axis] >= assignment.work_envelope_mm.min[axis]
                        && placed.max[axis] <= assignment.work_envelope_mm.max[axis]
                }) => {}
                _ => add("fixed_dual_source_reservation", "Complete conservative native source bounds must lie within its explicit routed work reservation".into(), Some(occurrence.occurrence_id), None),
            }
        }
        for face in definition
            .faces()
            .iter()
            .take(16usize.saturating_sub(pending_faces.len()))
        {
            pending_faces.push((occurrence.occurrence_id, face.face_id.clone()));
        }
        if surface_valid {
            match surface::metadata(
                input,
                occurrence,
                placed,
                (MAX_DEPOSITION_SEGMENTS as usize / 4).saturating_sub(surface_samples),
                result.geometry_work,
                cancel,
            ) {
                Ok(fields) => {
                    let additional = fields
                        .iter()
                        .map(surface::PreparedFinish::sample_points)
                        .sum::<usize>();
                    surface_samples = surface_samples.saturating_add(additional);
                    placed = surface::expanded(placed, &fields);
                    if !fields.is_empty() {
                        for component in &input.intent.printer.components {
                            if surface::intersects(placed, surface::component_bounds(component)) {
                                add(
                                    "surface_component_exclusion",
                                    format!(
                                        "Finished material conservative bounds intersect fixed component {}; this lane does not certify swept-tool clearance",
                                        component.id
                                    ),
                                    Some(occurrence.occurrence_id),
                                    None,
                                );
                            }
                        }
                        if let JobWorkflow::CompleteMarlin { job } = &input.intent.printer.workflow
                            && surface::prime_intersects(
                                placed,
                                job,
                                input.intent.recipe.bead_width_mm,
                            )
                        {
                            add("surface_prime_exclusion", "Finished material expanded XY envelope intersects the declared priming segment".into(), Some(occurrence.occurrence_id), None);
                        }
                        result
                            .surface_fields
                            .insert(occurrence.occurrence_id, fields);
                    }
                }
                Err(e) if e.code == ManufacturingErrorCode::Cancelled => return Err(e),
                Err(e) => {
                    // A resource refusal is final; do not spend the remaining
                    // occurrence budget sampling more finishes after exhaustion.
                    if e.code == ManufacturingErrorCode::ResourceLimit {
                        surface_valid = false;
                    }
                    add(
                        if e.code == ManufacturingErrorCode::ResourceLimit {
                            "resource_limit"
                        } else {
                            "unsupported_surface"
                        },
                        e.message,
                        Some(occurrence.occurrence_id),
                        None,
                    );
                }
            }
        }
        if let Some(hardware) = &input.intent.hardware
            && let Err(reason) = hardware.admits_bounds(&placed)
        {
            add(
                "hardware_placement",
                reason,
                Some(occurrence.occurrence_id),
                None,
            );
        }
        occurrence_bounds.insert(occurrence.occurrence_id, placed);
        // ADR012: extents within one motion step of the envelope are the same
        // commanded position; tessellated extents may sit that far outside.
        if (0..3).any(|a| {
            placed.min[a] < envelope.min[a] - axis_resolution(input, a)
                || placed.max[a] > envelope.max[a] + axis_resolution(input, a)
        }) {
            add("outside_envelope", "Placed native extrema plus admitted surface-finish expansion exceed the build envelope; change placement, finish or printer".into(), Some(occurrence.occurrence_id), None);
        }
        if let Some(bounds) = &mut result.bounds {
            bounds
                .include(placed.min)
                .map_err(|e| error(ManufacturingErrorCode::UnsupportedGeometry, e.to_string()))?;
            bounds
                .include(placed.max)
                .map_err(|e| error(ManufacturingErrorCode::UnsupportedGeometry, e.to_string()))?;
        } else {
            result.bounds = Some(placed);
        }
    }
    for occurrence_id in result.surface_fields.keys() {
        for (other, bounds) in &occurrence_bounds {
            check_cancel(cancel)?;
            if occurrence_id == other {
                continue;
            }
            if surface::intersects(occurrence_bounds[occurrence_id], *bounds) {
                add("surface_occurrence_exclusion", "Finished occurrence conservative expanded bounds intersect another occurrence; overlapping finishes are not admitted".into(), Some(*occurrence_id), None);
            }
        }
    }
    if let Some(bounds) = result.bounds
        && bounds.min[2] > envelope.min[2] + axis_resolution(input, 2)
        && matches!(input.intent.recipe.supports, SupportPolicy::Disabled)
    {
        add("floating_without_support", "Native material starts above the bed; enable breakaway supports or place it on the bed".into(), None, None);
    }
    drop(add);
    if let Ok(selected) = input.intent.select_layer_height()
        && selected.reason != LayerHeightReason::Requested
    {
        result.report.checks.push(PreflightCheck {
            code: "layer_height_goal_adjusted".into(),
            severity: PreflightSeverity::Warning,
            evidence: PreflightEvidence::Certified,
            required: false,
            message: format!("Preferred layer height {} mm selects {} mm under captured process limits; actual intervals and geometry remain Compile obligations", input.intent.recipe.layer_height_mm, selected.height_mm),
            occurrence_id: None, face_id: None,
        });
    }
    if truncated {
        result.report.checks.push(PreflightCheck {
            code: "additional_errors".into(),
            severity: PreflightSeverity::Error,
            evidence: if result.report.disposition == PreflightDisposition::Unresolved {
                PreflightEvidence::Indeterminate
            } else {
                PreflightEvidence::Failed
            },
            required: true,
            message:
                "Additional metadata errors are omitted; correct these errors and repeat Preflight"
                    .into(),
            occurrence_id: None,
            face_id: None,
        });
    }
    if result.report.disposition == PreflightDisposition::Admitted {
        result.report.checks.push(PreflightCheck {
            code: "native_metadata_admitted".into(),
            severity: PreflightSeverity::Info,
            evidence: PreflightEvidence::Certified,
            required: true,
            message: "Captured intent, source identity, pose and bounded native placement admitted. Complete physical height events, actual schedule and native metric/feature incidence remain Compile obligations".into(),
            occurrence_id: None, face_id: None,
        });
    }
    for (occurrence_id, fields) in &result.surface_fields {
        for field in fields {
            if result.report.checks.len() >= 254 {
                break;
            }
            result.report.checks.push(PreflightCheck { code: "surface_construction_pending".into(), severity: PreflightSeverity::Info, evidence: PreflightEvidence::NotEvaluated, required: false, message: format!("Native {} certificate and bounded feature metadata admitted; Compile must still prove native material/hole retention, realized offsets and actual deposited anchors. This is a manufacturing construction, not edited BREP or physical grip certification", field.certificate_description()),
            occurrence_id: Some(*occurrence_id), face_id: Some(field.face_id().clone()), });
        }
    }
    for (occurrence_id, face_id) in pending_faces {
        if result.report.checks.len() >= 254 {
            break;
        }
        result.report.checks.push(PreflightCheck {
            code: "oriented_native_gate_pending".into(),
            severity: PreflightSeverity::Info,
            evidence: PreflightEvidence::NotEvaluated,
            required: false,
            message: "This native face requires complete physical events, certified slab shape/feature incidence, continuous occupied envelopes and actual deposited-anchor analysis during Compile".into(),
            occurrence_id: Some(occurrence_id), face_id: Some(face_id),
        });
    }
    for (code, message) in [
        (
            "native_analysis_pending",
            "Complete physical event/branch coverage, ordered uncertainty bands and every actual closed slab are Compile-only obligations. Compile must certify metric/feature incidence and continuous occupied envelopes, then section topology, offsets, supports and actual deposited anchors. Admitted input is not authorization to manufacture.",
        ),
        (
            "physical_qualification_absent",
            "Software-only admission is not physical printability or machine qualification. No physical measurements, swept-bead collision certificate or printer qualification are available.",
        ),
    ] {
        result.report.checks.push(PreflightCheck {
            code: code.into(),
            severity: PreflightSeverity::Info,
            evidence: PreflightEvidence::NotEvaluated,
            required: false,
            message: message.into(),
            occurrence_id: None,
            face_id: None,
        });
    }
    result.report.validate()?;
    Ok(result)
}

/// Deep Compile-only discovery. An uncertainty endpoint remains bookkeeping
/// until every affected actual slab has metric and event-incidence evidence.
pub(super) fn resolve_native_events(
    input: &CompilerInput<'_>,
    admission: &mut Admission,
    proposal_work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let bed = input.intent.printer.build_envelope.min[2];
    // ADR012 feature-first heights: horizontal faces and extents of each
    // placed tessellation, merged within the height tolerance. The bed is
    // the first boundary; nothing here is a native event band.
    // ADR012: heights closer than the printer's Z step are one commanded
    // height, e.g. assembled bodies meant to sit flush.
    let merge_mm = input.intent.printer.motion_resolution.z_mm;
    let mut heights = Vec::new();
    let mut combined: Option<AabbMm> = None;
    for occurrence in input.occurrences {
        check_cancel(cancel)?;
        let model = &admission.models[&occurrence.occurrence_id];
        heights.extend(model.feature_heights(merge_mm));
        let bounds = admission.native_bounds[&occurrence.occurrence_id];
        let placed = admission
            .surface_fields
            .get(&occurrence.occurrence_id)
            .map_or(bounds, |fields| surface::expanded(bounds, fields));
        match &mut combined {
            Some(all) => {
                all.include(placed.min)
                    .and_then(|_| all.include(placed.max))
                    .map_err(|e| {
                        error(ManufacturingErrorCode::InvalidSpecification, e.to_string())
                    })?;
            }
            None => combined = Some(placed),
        }
    }
    heights.sort_by(f64::total_cmp);
    for z in heights {
        let z = if (z - bed).abs() <= merge_mm { bed } else { z };
        if z < bed {
            continue;
        }
        if admission
            .horizontal_events
            .last()
            .is_none_or(|last| z - last > merge_mm)
        {
            admission.horizontal_events.push(z);
        }
    }
    admission.bounds = combined;
    *proposal_work = proposal_work.saturating_add(admission.horizontal_events.len());
    Ok(())
}

pub(super) fn charge_capture_proposal_work(
    work: &mut usize,
    amount: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work.checked_add(amount).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "cumulative native capture proposal work counter overflow",
        )
    })?;
    Ok(())
}

fn native_check_code(code: GeometryErrorCode) -> &'static str {
    match code {
        GeometryErrorCode::ConditioningFailure => "native_conditioning_failure",
        GeometryErrorCode::AnalysisIndeterminate
        | GeometryErrorCode::ParameterCorrespondenceUnavailable => "native_analysis_indeterminate",
        GeometryErrorCode::UnsupportedGeometry => "native_unsupported_geometry",
        GeometryErrorCode::UnsupportedConversion => "native_unsupported_conversion",
        GeometryErrorCode::KernelFailure => "native_kernel_failure",
        GeometryErrorCode::SingularGeometry => "native_singular_geometry",
        _ => "native_invalid_geometry",
    }
}
