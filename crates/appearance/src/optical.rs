// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Finite upright BUILD-frame patches under a uniform normal-view model. Width
//! cancels from area fractions, but still binds physical context and identity.
//! Layer order/phase affects finite exposure, not a fabricated sequence correction:
//! identical fractions produce the same reference color. Nothing here routes a
//! recipe, validates a printer/process, or establishes physical color accuracy.
use crate::prusa_v7::{self, BaseColor, Part};
use crate::{ciede2000, color_to_d65_lab, d65_lab_to_xyz_y1, xyz_y1_to_srgb};
use spiling_contracts::appearance::{
    AppearanceColor, AppearanceEvidence, AppearanceIlluminant, AppearanceObserver,
    AppearanceOutcome, AppearanceStatus, PreparedPalette,
};
use spiling_contracts::optical::*;
use spiling_contracts::project::{ProjectError, ProjectErrorCode};
use std::sync::Arc;

fn invalid(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::InvalidProject, message)
}

/// O(slots), regardless of layer count. Full periods contribute their actual
/// per-material lengths; at most two clipped periodic intervals cover the tail.
/// Remainder and quotient are paired, avoiding rounded h/period reporting an
/// extra period when h lies just below an exactly representable boundary.
pub fn resolve_cycle(
    cycle: &LayerCycle,
    patch: &OpticalPatch,
) -> Result<CycleExposure, ProjectError> {
    cycle.validate(3, patch)?;
    resolve_slots(
        &cycle.slots,
        cycle.phase_slot as usize,
        cycle.origin_z_mm,
        patch,
    )
}
fn resolve_slots(
    slots: &[OpticalSlot],
    phase_slot: usize,
    origin_z_mm: f64,
    patch: &OpticalPatch,
) -> Result<CycleExposure, ProjectError> {
    let mut per_material = [0.0; 3];
    let mut period = 0.0;
    for i in 0..slots.len() {
        let slot = slots[(phase_slot + i) % slots.len()];
        period += slot.height_mm;
        per_material[slot.material as usize] += slot.height_mm;
    }
    let offset = patch.origin_mm[2] - origin_z_mm;
    let phase = offset.rem_euclid(period);
    let remainder = patch.height_mm % period;
    let count = ((patch.height_mm - remainder) / period).round();
    if !count.is_finite() || count < 0.0 || count > 4_503_599_627_370_496.0 {
        return Err(invalid(
            "optical complete-period count is not exactly representable",
        ));
    }
    let mut exposed = per_material.map(|length| length * count);
    // Split without forming phase+height: adding an extremely small remainder
    // to a large phase can otherwise erase a physically positive interval.
    let first = remainder.min(period - phase);
    let second = remainder - first;
    let mut start = 0.0;
    for i in 0..slots.len() {
        let slot = slots[(phase_slot + i) % slots.len()];
        let end = start + slot.height_mm;
        let clipped = |window_start: f64, length: f64| {
            let relative_start = (start - window_start).max(0.0);
            let relative_end = (end - window_start).max(0.0);
            relative_end.min(length) - relative_start.min(length)
        };
        exposed[slot.material as usize] += clipped(phase, first) + clipped(0.0, second);
        start = end;
    }
    let fractions = exposed.map(|height| height / patch.height_mm);
    let sum: f64 = fractions.iter().sum();
    if exposed
        .iter()
        .chain(fractions.iter())
        .any(|v| !v.is_finite() || *v < 0.0)
        || (sum - 1.0).abs() > 128.0 * f64::EPSILON
    {
        return Err(invalid(
            "optical exposure lost representable finite-window coverage",
        ));
    }
    Ok(CycleExposure {
        period_mm: period,
        phase_at_patch_mm: phase,
        complete_periods: count as u64,
        exposed_height_mm: exposed,
        fractions,
    })
}

fn base(evidence: &AppearanceEvidence) -> Result<BaseColor, OpticalUnknownReason> {
    let rgb = match evidence {
        AppearanceEvidence::CatalogEstimate { srgb } => *srgb,
        AppearanceEvidence::Unknown { .. } => return Err(OpticalUnknownReason::MissingEvidence),
        AppearanceEvidence::Measured {
            color,
            conditions,
            material_lot,
        } => {
            if material_lot.is_none() || !conditions.is_measured() {
                return Err(OpticalUnknownReason::MissingEvidence);
            }
            if !conditions.is_supported() {
                return Err(OpticalUnknownReason::UnsupportedConditions);
            }
            // Existing unclipped CSS colorimetry is the admission authority. Do
            // not force a measured out-of-domain basis into reference RGB.
            let converted = match *color {
                AppearanceColor::Srgb { channels } => Ok(channels),
                AppearanceColor::XyzY1 { channels } => xyz_y1_to_srgb(channels),
                AppearanceColor::XyzY100 { channels } => {
                    xyz_y1_to_srgb(channels.map(|v| v / 100.0))
                }
                AppearanceColor::D65Lab { channels } => {
                    d65_lab_to_xyz_y1(channels).and_then(xyz_y1_to_srgb)
                }
            };
            converted.map_err(|_| OpticalUnknownReason::UnsupportedColorDomain)?
        }
    };
    if rgb
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err(OpticalUnknownReason::UnsupportedColorDomain);
    }
    Ok(BaseColor::new(rgb.map(|v| (v * 255.0).round() as u8)))
}
fn assess(
    exposure: CycleExposure,
    bases: &[Result<BaseColor, OpticalUnknownReason>; 3],
    request: &OpticalRequest,
    target_lab: [f64; 3],
) -> Result<OpticalAssessment, ProjectError> {
    if request.observation.illuminant != AppearanceIlluminant::D65
        || request.observation.observer != AppearanceObserver::Cie1931TwoDegree
    {
        return Ok(OpticalAssessment::Unassessed {
            reason: OpticalUnknownReason::UnsupportedConditions,
            exposure,
        });
    }
    let mut first = None;
    for (i, &fraction) in exposure.fractions.iter().enumerate() {
        if fraction > 0.0 {
            match &bases[i] {
                Ok(color) => {
                    first = Some(color);
                }
                Err(reason) => {
                    return Ok(OpticalAssessment::Unassessed {
                        reason: *reason,
                        exposure,
                    });
                }
            }
        }
    }
    let first = first.ok_or_else(|| invalid("optical exposure has no positive component"))?;
    let mut parts = [Part {
        color: first,
        ratio: 0.0,
    }; 3];
    let mut count = 0;
    for (i, &fraction) in exposure.fractions.iter().enumerate() {
        if fraction > 0.0 {
            let color = bases[i]
                .as_ref()
                .map_err(|_| invalid("unassessed optical basis reached the reference evaluator"))?;
            parts[count] = Part {
                color,
                ratio: fraction,
            };
            count += 1;
        }
    }
    let prediction = prusa_v7::predict(&parts[..count]);
    let delta_e00 = ciede2000(prediction.lab, target_lab)?;
    let delta_lightness = (prediction.lab[0] - target_lab[0]).abs();
    if !delta_lightness.is_finite() {
        return Err(invalid("optical lightness difference is nonfinite"));
    }
    Ok(OpticalAssessment::Estimate {
        exposure,
        predicted_lab: prediction.lab,
        display_srgb8: prediction.rgb,
        delta_e00,
        delta_lightness,
        within_tolerance: delta_e00 <= request.target.delta_e00_tolerance
            && request
                .target
                .lightness_tolerance
                .is_none_or(|max| delta_lightness <= max),
    })
}

/// Explicit bounded grid traversal. Invalid pattern-specific phases consume a
/// visit; top-K output limits never terminate enumeration. Palette rows and
/// evidence hashes are borrowed once and reference bases precomputed once.
pub fn compare(
    palette: &PreparedPalette,
    request: Arc<OpticalRequest>,
    cancelled: &dyn Fn() -> bool,
) -> Result<OpticalReport, ProjectError> {
    request.validate_palette(palette)?;
    let rows = &palette.document().rows;
    let mut bases = [Err(OpticalUnknownReason::MissingEvidence); 3];
    let mut hashes = Vec::with_capacity(request.materials.len());
    for (i, key) in request.materials.iter().enumerate() {
        let index = rows
            .binary_search_by(|row| row.key.cmp(key))
            .map_err(|_| invalid("validated optical pool lost a prepared palette key"))?;
        bases[i] = base(&rows[index].evidence);
        hashes.push(palette.evidence_hashes()[index].clone());
    }
    let target_lab = color_to_d65_lab(request.target.color)?;
    let requested = assess(
        resolve_cycle(&request.recipe, &request.patch)?,
        &bases,
        &request,
        target_lab,
    )?;
    let total = request.candidate_count()?;
    let limits = request.limits;
    let mut report = OpticalReport {
        producer: OPTICAL_PRODUCER.to_owned(),
        input_hash: optical_input_hash(palette.canonical_hash(), &request)?,
        palette_hash: palette.canonical_hash().clone(),
        request,
        material_evidence_hashes: hashes,
        evidence: OpticalEvidence::ReferenceEstimate,
        execution: OpticalExecution::StudyOnlyNotRouted,
        requested,
        total_candidates: total,
        visits: Vec::with_capacity((total as usize).min(limits.max_visited as usize)),
        alternatives: Vec::new(),
        alternatives_truncated: false,
        status: AppearanceStatus::Cancelled,
    };
    report.alternatives =
        Vec::with_capacity((total as usize).min(limits.max_alternatives as usize));
    let mut assessed_count = 0;
    let mut any_match = false;
    let mut unknown = false;
    let mut slots = [OpticalSlot {
        material: 0,
        height_mm: 0.0,
    }; 64];
    let phases = report.request.search.phase_slots.len();
    let heights = report.request.search.slot_heights_mm.len();
    for index in 0..total {
        if cancelled() {
            report.status = AppearanceStatus::Cancelled;
            break;
        }
        if report.visits.len() == limits.max_visited as usize {
            report.status = AppearanceStatus::WorkLimit;
            break;
        }
        let pattern = &report.request.search.patterns[index as usize / phases / heights].materials;
        let height = report.request.search.slot_heights_mm[index as usize / phases % heights];
        let phase = report.request.search.phase_slots[index as usize % phases] as usize;
        let assessment = if phase >= pattern.len() {
            OpticalAssessment::FilteredPhase
        } else {
            for (slot, &material) in slots.iter_mut().zip(pattern) {
                *slot = OpticalSlot {
                    material,
                    height_mm: height,
                };
            }
            assess(
                resolve_slots(
                    &slots[..pattern.len()],
                    phase,
                    report.request.search.origin_z_mm,
                    &report.request.patch,
                )?,
                &bases,
                &report.request,
                target_lab,
            )?
        };
        match assessment {
            OpticalAssessment::Estimate {
                delta_e00,
                within_tolerance,
                ..
            } => {
                assessed_count += 1;
                any_match |= within_tolerance;
                let position = report
                    .alternatives
                    .binary_search_by(|&previous| {
                        let OpticalAssessment::Estimate {
                            delta_e00: previous_error,
                            ..
                        } = report.visits[previous as usize].assessment
                        else {
                            unreachable!("ranked optical visits are estimates")
                        };
                        previous_error
                            .total_cmp(&delta_e00)
                            .then_with(|| previous.cmp(&index))
                    })
                    .unwrap_or_else(|p| p);
                if position < limits.max_alternatives as usize {
                    if report.alternatives.len() == limits.max_alternatives as usize {
                        report.alternatives.pop();
                    }
                    report.alternatives.insert(position, index);
                }
            }
            OpticalAssessment::Unassessed { .. } => unknown = true,
            OpticalAssessment::FilteredPhase => {}
        }
        report.visits.push(OpticalVisit { index, assessment });
    }
    if report.visits.len() == total as usize {
        report.status = AppearanceStatus::Complete {
            outcome: if any_match {
                AppearanceOutcome::AssessedMatch
            } else if assessed_count == 0 || unknown {
                AppearanceOutcome::NoAssessedMatch
            } else {
                AppearanceOutcome::AssessedMismatch
            },
        };
    }
    report.alternatives_truncated = assessed_count > report.alternatives.len();
    report.validate()?;
    Ok(report)
}
