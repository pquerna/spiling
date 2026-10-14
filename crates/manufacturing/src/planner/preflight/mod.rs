// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{LAYER_QUANTIZATION_MM, forward, rotate, rotate_inverse, surface};
use crate::{CompilerInput, check_cancel, error};
use spiling_contracts::geometry::{
    AabbMm, FaceCarrier, MAX_DEFINITIONS, MAX_OCCURRENCES, OccurrenceId,
};
use spiling_contracts::manufacturing::*;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

pub(super) struct Admission {
    pub bounds: Option<AabbMm>,
    pub horizontal_events: Vec<f64>,
    pub reuse_slabs: bool,
    pub surface_fields: BTreeMap<OccurrenceId, Vec<surface::PreparedFinish>>,
    pub report: PreflightReport,
    pub schedule: Option<Vec<super::LayerElevation>>,
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
        reuse_slabs: true,
        surface_fields: BTreeMap::new(),
        schedule: None,
        report: PreflightReport {
            input_revision: input.revision,
            eligible: true,
            software_only: true,
            checks: Vec::new(),
        },
    };
    let mut pending_faces = Vec::new();
    let mut add = |code: &str, message: String, occurrence_id, face_id| {
        result.report.eligible = false;
        // Reserve two final checks and one truncation diagnostic. Never publish
        // an eligible report by discarding an error beyond the bounded display.
        if result.report.checks.len() < 253 {
            result.report.checks.push(PreflightCheck {
                code: code.into(),
                severity: PreflightSeverity::Error,
                message,
                occurrence_id,
                face_id,
            });
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
    for occurrence in input.occurrences.iter().take(MAX_OCCURRENCES as usize) {
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
        for face in definition.faces() {
            check_cancel(cancel)?;
            match &face.carrier {
                FaceCarrier::Plane { origin_mm, normal } => {
                    let n = rotate(&occurrence.pose, *normal);
                    if n[0].hypot(n[1]) <= 1e-10 {
                        let z = forward(&occurrence.pose, *origin_mm)[2];
                        if let Err(position) = result
                            .horizontal_events
                            .binary_search_by(|event| event.total_cmp(&z))
                        {
                            if result.horizontal_events.len() > MAX_PRINT_LAYERS as usize {
                                add("event_limit", "Distinct native horizontal events exceed the bounded layer schedule".into(), Some(occurrence.occurrence_id), Some(face.face_id.clone()));
                            } else {
                                result.horizontal_events.insert(position, z);
                            }
                        }
                        result.reuse_slabs &= n[0] == 0.0 && n[1] == 0.0;
                    } else {
                        result.reuse_slabs &= n[2] == 0.0;
                        if n[2] != 0.0 && pending_faces.len() < 16 {
                            pending_faces.push((occurrence.occurrence_id, face.face_id.clone()));
                        }
                    }
                }
                FaceCarrier::Cylinder { axis_direction, .. } => {
                    let axis = rotate(&occurrence.pose, *axis_direction);
                    result.reuse_slabs &= axis[0] == 0.0 && axis[1] == 0.0;
                    if axis[0].hypot(axis[1]) != 0.0 && pending_faces.len() < 16 {
                        pending_faces.push((occurrence.occurrence_id, face.face_id.clone()));
                    }
                }
            }
        }
        let mut placed = AabbMm {
            min: [0.0; 3],
            max: [0.0; 3],
        };
        for axis in 0..3 {
            let direction = rotate_inverse(
                &occurrence.pose,
                std::array::from_fn(|a| if a == axis { 1.0 } else { 0.0 }),
            );
            let extent = spiling_geometry::projected_extent(definition, direction, cancel)
                .map_err(|e| {
                    error(
                        if e.code == spiling_contracts::geometry::GeometryErrorCode::Cancelled {
                            ManufacturingErrorCode::Cancelled
                        } else {
                            ManufacturingErrorCode::UnsupportedGeometry
                        },
                        e.to_string(),
                    )
                })?;
            placed.min[axis] = extent[0] + occurrence.pose.translation_mm[axis];
            placed.max[axis] = extent[1] + occurrence.pose.translation_mm[axis];
        }
        // Even tilted bodies have a real analytic first/last material event.
        // Retain it so a floating body cannot first appear inside layer zero
        // and be falsely assigned bed anchoring by midpoint sampling.
        for z in [placed.min[2], placed.max[2]] {
            if let Err(position) = result
                .horizontal_events
                .binary_search_by(|event| event.total_cmp(&z))
            {
                if result.horizontal_events.len() > MAX_PRINT_LAYERS as usize {
                    add(
                        "event_limit",
                        "Native material extrema exceed the bounded layer schedule".into(),
                        Some(occurrence.occurrence_id),
                        None,
                    );
                } else {
                    result.horizontal_events.insert(position, z);
                }
            }
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
    if result.report.eligible {
        match super::scheduled_layers(input, &result, cancel) {
            Ok(schedule) => {
                let bed = envelope.min[2];
                let top = result.bounds.expect("eligible geometry has bounds").max[2];
                let mut retained_samples = 0usize;
                for fields in result.surface_fields.values() {
                    for field in fields {
                        let copies = if field.is_z_invariant(bed, top)? {
                            1
                        } else {
                            schedule.len()
                        };
                        // D, each wall center and fill erosion are retained once
                        // per realized span; one native carrier remains for witnesses.
                        let owners = copies
                            .checked_mul(input.intent.recipe.perimeter_count as usize + 2)
                            .and_then(|n| n.checked_add(1))
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "retained image geometry estimate overflow",
                                )
                            })?;
                        retained_samples = retained_samples
                            .checked_add(field.sample_points().checked_mul(owners).ok_or_else(
                                || {
                                    error(
                                        ManufacturingErrorCode::ResourceLimit,
                                        "retained image sample estimate overflow",
                                    )
                                },
                            )?)
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "retained image sample estimate overflow",
                                )
                            })?;
                    }
                }
                if retained_samples > MAX_DEPOSITION_SEGMENTS as usize {
                    result.report.eligible = false;
                    result.report.checks.push(PreflightCheck {
                        code: "resource_limit".into(),
                        severity: PreflightSeverity::Error,
                        message: "Refined surface sample estimate exceeds retained geometry budget"
                            .into(),
                        occurrence_id: None,
                        face_id: None,
                    });
                }
                result.schedule = Some(schedule);
            }
            Err(e) if e.code == ManufacturingErrorCode::Cancelled => return Err(e),
            Err(e) => {
                result.report.eligible = false;
                result.report.checks.push(PreflightCheck {
                    code: if e.code == ManufacturingErrorCode::ResourceLimit {
                        "resource_limit"
                    } else {
                        "unresolved_layer_schedule"
                    }
                    .into(),
                    severity: PreflightSeverity::Error,
                    message: e.message,
                    occurrence_id: None,
                    face_id: None,
                });
            }
        }
    }
    if result.report.checks.len() == 253 {
        result.report.checks.push(PreflightCheck {
            code: "additional_errors".into(),
            severity: PreflightSeverity::Error,
            message:
                "Additional metadata errors are omitted; correct these errors and repeat Preflight"
                    .into(),
            occurrence_id: None,
            face_id: None,
        });
    }
    for (occurrence_id, fields) in &result.surface_fields {
        for field in fields {
            if result.report.checks.len() >= 254 {
                break;
            }
            result.report.checks.push(PreflightCheck {
                code: "surface_construction_pending".into(), severity: PreflightSeverity::Info,
                message: format!("Native {} certificate and bounded feature metadata admitted; Compile must still prove native material/hole retention, realized offsets and actual deposited anchors. This is a manufacturing construction, not edited BREP or physical grip certification", field.certificate_description()),
                occurrence_id: Some(*occurrence_id), face_id: Some(field.face_id().clone()),
            });
        }
    }
    for (occurrence_id, face_id) in pending_faces {
        if result.report.checks.len() >= 254 {
            break;
        }
        result.report.checks.push(PreflightCheck { code: "oriented_native_gate_pending".into(), severity: PreflightSeverity::Info,
            message: "This sloped planar or tilted cylindrical native face requires per-layer native intersection, topology and deposited-anchor analysis during Compile; metadata eligibility does not pass that gate".into(),
            occurrence_id: Some(occurrence_id), face_id: Some(face_id) });
    }
    for (code, message) in [
        (
            "native_analysis_pending",
            "Compile must still perform native section, topology, offset, support and actual deposited-anchor bridge analysis. Only the admitted planar/cylindrical STEP profile is eligible; general/freeform STEP is not admitted.",
        ),
        (
            "physical_qualification_absent",
            "Software-only eligibility is not physical printability or machine qualification. No physical measurements, swept-bead collision certificate or printer qualification are available.",
        ),
    ] {
        result.report.checks.push(PreflightCheck {
            code: code.into(),
            severity: PreflightSeverity::Info,
            message: message.into(),
            occurrence_id: None,
            face_id: None,
        });
    }
    Ok(result)
}
