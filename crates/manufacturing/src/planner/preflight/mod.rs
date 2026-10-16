// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{LAYER_QUANTIZATION_MM, surface};
use crate::{CompilerInput, check_cancel, error};
use spiling_contracts::geometry::{
    AabbMm, GeometryErrorCode, MAX_DEFINITIONS, MAX_OCCURRENCES, OccurrenceId,
};
use spiling_contracts::manufacturing::*;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

pub(super) struct Admission {
    pub bounds: Option<AabbMm>,
    pub horizontal_events: Vec<f64>,
    pub native_bounds: BTreeMap<OccurrenceId, AabbMm>,
    pub native_event_bands: Vec<[f64; 2]>,
    pub material_domains: BTreeMap<OccurrenceId, [[f64; 2]; 2]>,
    pub reuse_slabs: bool,
    pub surface_fields: BTreeMap<OccurrenceId, Vec<surface::PreparedFinish>>,
    pub report: PreflightReport,
    pub geometry_work: spiling_geometry::GeometryWorkBudget,
}

pub fn preflight(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<PreflightReport, ManufacturingError> {
    Ok(admit(input, cancel)?.report)
}

pub(super) fn admit(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<Admission, ManufacturingError> {
    check_cancel(cancel)?;
    let mut result = Admission {
        bounds: None,
        horizontal_events: Vec::new(),
        native_bounds: BTreeMap::new(),
        native_event_bands: Vec::new(),
        material_domains: BTreeMap::new(),
        reuse_slabs: true,
        surface_fields: BTreeMap::new(),
        geometry_work: spiling_geometry::GeometryWorkBudget::new(super::MAX_BOOLEAN_PAIRS as u64),
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
        let mut placed = AabbMm {
            min: [0.0; 3],
            max: [0.0; 3],
        };
        for axis in 0..3 {
            let direction = std::array::from_fn(|a| if a == axis { 1.0 } else { 0.0 });
            let extent = match spiling_geometry::placed_bounds(
                definition,
                occurrence.pose,
                direction,
                spiling_geometry::BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: LAYER_QUANTIZATION_MM,
                },
                &mut result.geometry_work,
                cancel,
            ) {
                Ok(extent) => extent,
                Err(e)
                    if matches!(
                        e.code,
                        GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                    ) =>
                {
                    return Err(super::native_error(e, "native projected bounds"));
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
            };
            placed.min[axis] = extent.minimum[0];
            placed.max[axis] = extent.maximum[1];
        }
        result
            .native_bounds
            .insert(occurrence.occurrence_id, placed);
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
        occurrence_bounds.insert(occurrence.occurrence_id, placed);
        if (0..3).any(|a| {
            placed.min[a] < envelope.min[a] - LAYER_QUANTIZATION_MM
                || placed.max[a] > envelope.max[a] + LAYER_QUANTIZATION_MM
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
    let mut exclusion_work = 0usize;
    for occurrence_id in result.surface_fields.keys() {
        for (other, bounds) in &occurrence_bounds {
            check_cancel(cancel)?;
            if occurrence_id == other {
                continue;
            }
            exclusion_work += 1;
            if exclusion_work > super::MAX_BOOLEAN_PAIRS {
                add(
                    "resource_limit",
                    "Surface occurrence exclusion work budget exceeded".into(),
                    Some(*occurrence_id),
                    None,
                );
                break;
            }
            if surface::intersects(occurrence_bounds[occurrence_id], *bounds) {
                add("surface_occurrence_exclusion", "Finished occurrence conservative expanded bounds intersect another occurrence; overlapping finishes are not admitted".into(), Some(*occurrence_id), None);
            }
        }
    }
    if let Some(bounds) = result.bounds
        && bounds.min[2] > envelope.min[2] + LAYER_QUANTIZATION_MM
        && matches!(input.intent.recipe.supports, SupportPolicy::Disabled)
    {
        add("floating_without_support", "Native material starts above the bed; enable breakaway supports or place it on the bed".into(), None, None);
    }
    drop(add);
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
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let bed = input.intent.printer.build_envelope.min[2];
    let policy = spiling_geometry::AnalysisPolicy {
        maximum_height_uncertainty_mm: LAYER_QUANTIZATION_MM,
        maximum_shape_error_mm: input.intent.recipe.native_shape_error_mm,
    };
    let mut completed_bounds: Option<AabbMm> = None;
    for occurrence in input.occurrences {
        check_cancel(cancel)?;
        let mut bounds = admission.native_bounds[&occurrence.occurrence_id];
        let analysis = spiling_geometry::analyze_placed_height_domain(
            &input.definitions[&occurrence.definition_id],
            occurrence.pose,
            [bounds.min[2], bounds.max[2]],
            policy,
            &mut admission.geometry_work,
            cancel,
        )
        .map_err(|e| super::native_error(e, "complete native height analysis"))?;
        if analysis.definition_id() != &occurrence.definition_id
            || analysis.pose() != Some(occurrence.pose)
            || analysis.height_interval_mm() != [bounds.min[2], bounds.max[2]]
            || analysis.event_brackets_mm().is_empty()
        {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                format!(
                    "occurrence {} has no captured complete terminal-event coverage",
                    occurrence.occurrence_id.get()
                ),
            ));
        }
        let invariant = analysis.invariance().is_some();
        let mut bands = analysis.event_brackets_mm().to_vec();
        bands.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        bands.dedup_by(|a, b| *a == *b);
        let domain = [
            bands[0],
            *bands
                .iter()
                .max_by(|a, b| a[1].total_cmp(&b[1]))
                .expect("nonempty native event bands"),
        ];
        admission
            .material_domains
            .insert(occurrence.occurrence_id, domain);
        admission.reuse_slabs &= invariant;
        for bracket in bands {
            if !bracket.iter().all(|z| z.is_finite())
                || bracket[0] > bracket[1]
                || bracket[1] - bracket[0] > policy.maximum_height_uncertainty_mm
                || bracket[1] - bracket[0] > policy.maximum_shape_error_mm
            {
                return Err(error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    format!(
                        "occurrence {} has an unresolved native event band {bracket:?}",
                        occurrence.occurrence_id.get()
                    ),
                ));
            }
            let band_position = admission.native_event_bands.binary_search_by(|old| {
                old[0]
                    .total_cmp(&bracket[0])
                    .then(old[1].total_cmp(&bracket[1]))
            });
            if let Err(position) = band_position {
                if admission.native_event_bands.len() >= MAX_PRINT_LAYERS as usize {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native event-band coverage exceeds the layer schedule budget",
                    ));
                }
                admission.native_event_bands.insert(position, bracket);
            }
            // Exact representable strata stay exact. An isolated numerical band
            // uses its upper endpoint, never a fictitious exact midpoint. The
            // bed can own a bounded contact neighborhood without a sliver; the
            // complete band remains captured for actual closed-slab proof.
            let z = if bracket[0] <= bed
                && bracket[1] >= bed
                && (bracket[0] - bed).abs().max((bracket[1] - bed).abs())
                    <= policy.maximum_shape_error_mm
            {
                bed
            } else {
                bracket[1]
            };
            if let Err(position) = admission
                .horizontal_events
                .binary_search_by(|old| old.total_cmp(&z))
            {
                if admission.horizontal_events.len() >= MAX_PRINT_LAYERS as usize {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native material events exceed the layer schedule budget",
                    ));
                }
                admission.horizontal_events.insert(position, z);
            }
        }
        // This complete query covered the original certified support domain.
        // Native event candidates include every source extremum and no injected
        // query endpoints. Intersect their full enclosure with initial bounds;
        // do not append the old conservative support-pad as an empty top layer.
        bounds.min[2] = bounds.min[2].max(domain[0][0]);
        bounds.max[2] = bounds.max[2].min(domain[1][1]);
        admission
            .native_bounds
            .insert(occurrence.occurrence_id, bounds);
        let placed = admission
            .surface_fields
            .get(&occurrence.occurrence_id)
            .map_or(bounds, |fields| surface::expanded(bounds, fields));
        if let Some(combined) = &mut completed_bounds {
            combined
                .include(placed.min)
                .and_then(|_| combined.include(placed.max))
                .map_err(|e| {
                    error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        e.to_string(),
                    )
                })?;
        } else {
            completed_bounds = Some(placed);
        }
    }
    admission.bounds = completed_bounds;
    Ok(())
}

fn native_check_code(code: GeometryErrorCode) -> &'static str {
    match code {
        GeometryErrorCode::ConditioningFailure => "native_conditioning_failure",
        GeometryErrorCode::AnalysisIndeterminate => "native_analysis_indeterminate",
        GeometryErrorCode::UnsupportedGeometry => "native_unsupported_geometry",
        GeometryErrorCode::UnsupportedConversion => "native_unsupported_conversion",
        GeometryErrorCode::KernelFailure => "native_kernel_failure",
        GeometryErrorCode::SingularGeometry => "native_singular_geometry",
        _ => "native_invalid_geometry",
    }
}
