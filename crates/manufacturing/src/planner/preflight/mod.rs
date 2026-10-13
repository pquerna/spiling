// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{LAYER_QUANTIZATION_MM, forward, rotate, rotate_inverse};
use crate::{CompilerInput, check_cancel, error};
use spiling_contracts::geometry::{AabbMm, FaceCarrier, MAX_DEFINITIONS, MAX_OCCURRENCES};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) struct Admission {
    pub bounds: Option<AabbMm>,
    pub horizontal_events: Vec<f64>,
    pub reuse_slabs: bool,
    pub report: PreflightReport,
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
        if (0..3).any(|a| {
            placed.min[a] < envelope.min[a] - LAYER_QUANTIZATION_MM
                || placed.max[a] > envelope.max[a] + LAYER_QUANTIZATION_MM
        }) {
            add("outside_envelope", "Placed analytic native extrema exceed the build envelope; change placement or printer".into(), Some(occurrence.occurrence_id), None);
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
    if let Some(bounds) = result.bounds
        && bounds.min[2] > envelope.min[2] + LAYER_QUANTIZATION_MM
        && matches!(input.intent.recipe.supports, SupportPolicy::Disabled)
    {
        add("floating_without_support", "Native material starts above the bed; enable breakaway supports or place it on the bed".into(), None, None);
    }
    drop(add);
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
