// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod arcs;
pub(crate) mod boolean;
use boolean::budget as boolean_budget;
mod classification;
mod color;
mod joins;
mod neighborhood;
use neighborhood::SkinNeighborhoods;
mod bonds;
mod cathedral;
mod combined;
mod exposure;
mod native;
mod partition;
mod preflight;
mod region;
mod startup;
mod tolerant;
use startup::StartupWall;
pub(crate) mod support;
mod surface;
mod walls;
mod zones;
pub use preflight::layout_footprints;
pub use preflight::preflight;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;
use crate::{CompilerInput, check_cancel, error};
use geo::{Area, Intersects, MultiPolygon, Point};
use spiling_contracts::geometry::{GeometryErrorCode, RigidPoseMm, SECTION_SAMPLING_TOLERANCE_MM};
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_bead_height_mm, project_xyz6_coordinate_mm,
};
use spiling_contracts::manufacturing::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

pub const LAYER_QUANTIZATION_MM: f64 = 0.000001;
pub const OFFSET_TOLERANCE_MM: f64 = 0.005;

pub(crate) fn retained_coverage_bytes(coverage: &PlanarRegionCoverage) -> usize {
    bonds::coverage_bytes(coverage)
}

fn remaining_geometry_units(
    retained: usize,
    owned_bytes: usize,
) -> Result<usize, ManufacturingError> {
    let units = crate::working_set::units()
        .checked_sub(retained)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "retained geometry coordinate-unit budget exceeded",
            )
        })?;
    let bytes = (crate::working_set::bytes())
        .checked_sub(owned_bytes)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "retained geometry allocated-byte budget exceeded",
            )
        })?;
    Ok(units.min(bytes / std::mem::size_of::<geo::Coord<f64>>()))
}
// Compile-local immutable D span, required-C/exemption owner and exact XY width
// envelope. Actual physical Z remains on each materialized layer and skin key.
type WallCacheKey = (usize, Option<usize>, [u64; 3], Option<usize>);

/// Plan through ADR012 tolerant sections: the one slicing pipeline. Every
/// source, STEP or hybrid, is sectioned from its tessellation within the
/// recorded tolerance policy; no exact native lane remains.
pub(crate) fn plan(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
    geometry_work: &mut spiling_geometry::GeometryWorkBudget,
) -> Result<NormalizedPrintPlan, ManufacturingError> {
    let mut plan = plan_with(input, cancel, geometry_work)?;
    // Walls follow the BREP's own circles.
    let sources: Vec<_> = input
        .occurrences
        .iter()
        .filter_map(|occurrence| {
            input
                .definitions
                .get(&occurrence.definition_id)
                .map(|definition| definition.circles(&occurrence.pose))
        })
        .collect();
    arcs::attach(&mut plan, &sources, cancel)?;
    Ok(plan)
}

/// A fill, skin or bridge row whose emitted XYZ6 chord is shorter than the
/// minimum command deposits less than the profile tolerance; omit it, as other
/// slicers drop such fragments, rather than refuse the part. Walls are kept.
fn drop_unprintable_rows(layers: &mut [PrintLayer]) -> Result<(), ManufacturingError> {
    use spiling_contracts::manufacturing::program_coordinates::{
        MIN_DEPOSITION_COMMAND_MM, project_xyz6_point_mm,
    };
    for layer in layers {
        let mut failure = None;
        layer.paths.retain(|path| {
            if path.points_mm.len() != 2
                || matches!(
                    path.kind,
                    DepositionKind::Perimeter | DepositionKind::SeparatorWall
                )
            {
                return true;
            }
            match (
                project_xyz6_point_mm(path.points_mm[0]),
                project_xyz6_point_mm(path.points_mm[1]),
            ) {
                (Ok(a), Ok(b)) => (b[0] - a[0]).hypot(b[1] - a[1]) >= MIN_DEPOSITION_COMMAND_MM,
                (Err(e), _) | (_, Err(e)) => {
                    failure.get_or_insert(e);
                    true
                }
            }
        });
        if let Some(e) = failure {
            return Err(e);
        }
    }
    Ok(())
}

fn plan_with(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
    geometry_work: &mut spiling_geometry::GeometryWorkBudget,
) -> Result<NormalizedPrintPlan, ManufacturingError> {
    let mut admission = preflight::admit(input, cancel, geometry_work)?;

    match admission.report.disposition {
        PreflightDisposition::Admitted => {}
        disposition @ (PreflightDisposition::Refused | PreflightDisposition::Unresolved) => {
            let check = admission
                .report
                .checks
                .iter()
                .find(|check| {
                    check.required
                        && match disposition {
                            PreflightDisposition::Refused => matches!(
                                check.evidence,
                                PreflightEvidence::Unsupported | PreflightEvidence::Failed
                            ),
                            PreflightDisposition::Unresolved => matches!(
                                check.evidence,
                                PreflightEvidence::NotEvaluated | PreflightEvidence::Indeterminate
                            ),
                            PreflightDisposition::Admitted => false,
                        }
                })
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "preflight disposition has no matching required evidence",
                    )
                })?;
            let code = match check.code.as_str() {
                "resource_limit" | "event_limit" => ManufacturingErrorCode::ResourceLimit,
                "empty_project" => ManufacturingErrorCode::EmptyProject,
                "invalid_intent"
                | "source_identity"
                | "invalid_pose"
                | "missing_definition"
                | "invalid_surface_targets" => ManufacturingErrorCode::InvalidSpecification,
                "native_kernel_failure" => ManufacturingErrorCode::NativeKernelFailure,
                "native_conditioning_failure" => ManufacturingErrorCode::NativeConditioningFailure,
                "native_analysis_indeterminate"
                | "unresolved_native_event"
                | "unresolved_layer_schedule" => {
                    ManufacturingErrorCode::NativeAnalysisIndeterminate
                }
                "native_singular_geometry" => ManufacturingErrorCode::NativeSingularGeometry,
                _ => ManufacturingErrorCode::UnsupportedGeometry,
            };
            return Err(error(code, check.message.clone()));
        }
    }
    let mut surface_work = 0usize;

    preflight::resolve_native_events(input, &mut admission, &mut surface_work, cancel)?;
    let mut process_captures = Vec::new();
    process_captures
        .try_reserve_exact(input.occurrences.len())
        .map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "process capture allocation refused",
            )
        })?;
    for occurrence in input.occurrences {
        process_captures.push(zones::capture(
            input,
            &admission,
            occurrence.occurrence_id,
            &mut surface_work,
            cancel,
        )?);
    }
    // A proposal: support and mapped-field refinement split it further.
    let mut schedule = scheduled_layers(input, &mut admission, &process_captures, cancel)?;
    let mut support_work = support::SupportWork::default();
    let mut wall_memo = WallMemo::default();
    let mut void_memo = tolerant::VoidMemo::default();
    let mut survival_memo = neighborhood::SurvivalMemo::default();
    // Refinement grows the schedule in place; every retry and admission
    // charges its actual capacity against the working set.
    let outside = retry_outside_bytes(&admission, &schedule, &process_captures)?;
    remaining_geometry_units(0, geometry_bytes(&[outside])?)?;
    // Freeze the original startup pair's physical scope. Retrying by current
    // layer count would shrink that scope each time one interval is split.
    let probe_scope = match input.intent.recipe.supports {
        SupportPolicy::Disabled => schedule
            .get(1)
            .map(|layer| AnchorProbeScope::StartupPair { top_mm: layer.z }),
        SupportPolicy::Breakaway { .. } => Some(AnchorProbeScope::UnreachableSupport),
    };
    let native_prefix = loop {
        let Some(scope) = probe_scope else { break None };
        let Some(probe) = deposition_probe(
            input,
            &process_captures,
            &schedule,
            scope,
            &mut surface_work,
            cancel,
        )?
        else {
            break None;
        };
        let outcome = admit_schedule(
            input,
            cancel,
            &mut admission,
            &mut process_captures,
            &mut schedule,
            &mut surface_work,
            &mut support_work,
            Some(probe),
            None,
            &mut wall_memo,
            &mut void_memo,
            &mut survival_memo,
        );
        match outcome? {
            ScheduleAdmission::PrefixAdmitted(capture) => break Some(capture),
            ScheduleAdmission::RefineUnreachableLayer {
                index,
                failure,
                span_mm,
            } => {
                exhaust_or_refine(
                    &mut schedule,
                    index,
                    input,
                    failure,
                    span_mm,
                    &mut support_work,
                )?;
            }
            ScheduleAdmission::Complete(_) | ScheduleAdmission::RetryUndecided => {
                unreachable!("tentative prefix cannot publish a plan")
            }
        }
    };
    let mut prefix = native_prefix;
    // After a complete-pass refinement only the refined neighborhood changed.
    // Tolerant lanes first retry a truncated schedule through that
    // neighborhood: its layers, demands and columns there equal the complete
    // pass's, so a refinement it finds is the complete pass's first one.
    // Anything else falls through to the complete pass, which decides.
    let truncate_retries = matches!(
        input.intent.recipe.supports,
        SupportPolicy::Breakaway { .. }
    ) && !matches!(
        input.intent.recipe.sparse_infill,
        SparseInfillPolicy::WebRoof { .. }
    );
    let mut refined = None;
    loop {
        if let Some(index) = refined.take()
            && let Some(probe) = deposition_probe(
                input,
                &process_captures,
                &schedule,
                AnchorProbeScope::RefinedLayer {
                    end: index + RETRY_WINDOW_LAYERS,
                },
                &mut surface_work,
                cancel,
            )?
        {
            let outcome = admit_schedule(
                input,
                cancel,
                &mut admission,
                &mut process_captures,
                &mut schedule,
                &mut surface_work,
                &mut support_work,
                Some(probe),
                None,
                &mut wall_memo,
                &mut void_memo,
                &mut survival_memo,
            );
            match outcome {
                Ok(ScheduleAdmission::RefineUnreachableLayer {
                    index,
                    failure,
                    span_mm,
                }) => {
                    exhaust_or_refine(
                        &mut schedule,
                        index,
                        input,
                        failure,
                        span_mm,
                        &mut support_work,
                    )?;
                    refined = Some(index);
                    continue;
                }
                Err(failure) if failure.code == ManufacturingErrorCode::Cancelled => {
                    return Err(failure);
                }
                _ => {}
            }
        }
        let outcome = admit_schedule(
            input,
            cancel,
            &mut admission,
            &mut process_captures,
            &mut schedule,
            &mut surface_work,
            &mut support_work,
            None,
            prefix.take(),
            &mut wall_memo,
            &mut void_memo,
            &mut survival_memo,
        );
        match outcome? {
            ScheduleAdmission::Complete(plan) => return Ok(plan),
            ScheduleAdmission::RefineUnreachableLayer {
                index,
                failure,
                span_mm,
            } => {
                exhaust_or_refine(
                    &mut schedule,
                    index,
                    input,
                    failure,
                    span_mm,
                    &mut support_work,
                )?;
                refined = truncate_retries.then_some(index);
            }
            ScheduleAdmission::PrefixAdmitted(_) | ScheduleAdmission::RetryUndecided => {
                unreachable!("complete schedule must produce a plan")
            }
        }
    }
}

/// Output layers a truncated retry keeps above its refined index: the split
/// may move the next demand up by one layer.
const RETRY_WINDOW_LAYERS: usize = 3;

/// Bytes held outside one schedule admission across retries.
fn retry_outside_bytes(
    admission: &preflight::Admission,
    schedule: &Vec<LayerElevation>,
    process: &Vec<zones::Capture>,
) -> Result<usize, ManufacturingError> {
    geometry_bytes(&[
        admission.model_bytes(),
        schedule.capacity() * std::mem::size_of::<LayerElevation>(),
        process.capacity() * std::mem::size_of::<zones::Capture>(),
        process
            .iter()
            .map(|capture| {
                capture.operations.capacity()
                    * std::mem::size_of::<spiling_contracts::process::ProcessProvenance>()
            })
            .sum(),
    ])
}

#[expect(
    clippy::large_enum_variant,
    reason = "One admission result moves per schedule attempt; the prefix capture is not replicated"
)]
enum ScheduleAdmission {
    PrefixAdmitted(NativeScheduleCapture),
    RefineUnreachableLayer {
        index: usize,
        failure: ManufacturingError,
        span_mm: Option<f64>,
    },
    Complete(NormalizedPrintPlan),
    /// A truncated retry found no refinement the complete pass would make.
    RetryUndecided,
}

struct OrdinaryTemplate {
    phase: region::OrdinaryHatchPhase,
    top_axis: region::HatchAxis,
    paths: Vec<DepositionPath>,
    coverage: PlanarRegionCoverage,
    fits: Vec<SkinCommandFitDomain>,
}

// Successful tentative sections/D owners move into final admission unchanged.
struct NativeScheduleCapture {
    sections: Vec<SectionSpan>,
    native_surfaces: Vec<NativeSurfaceSpan>,
    retained_section_bytes: usize,
    retained_points: usize,
    startup_wall: Option<StartupWall>,
}

// Tentative and final admission use the same native/D/wall/skin/anchor pipeline.
// Only the tentative physical terminal/output horizons differ. Nothing from a
// discarded attempt can authorize the final native ledger or emitted program.
#[expect(
    clippy::too_many_arguments,
    reason = "One admission orchestrator borrows independent native, surface, support and recycled-prefix owners"
)]
fn admit_schedule<'model>(
    input: &CompilerInput<'model>,
    cancel: &AtomicBool,
    admission: &mut preflight::Admission,
    process_captures: &mut [zones::Capture],
    schedule: &mut Vec<LayerElevation>,
    surface_work: &mut usize,
    support_work: &mut support::SupportWork,
    probe: Option<DepositionProbe>,
    native_prefix: Option<NativeScheduleCapture>,
    wall_memo: &mut WallMemo,
    void_memo: &mut tolerant::VoidMemo,
    survival_memo: &mut neighborhood::SurvivalMemo,
) -> Result<ScheduleAdmission, ManufacturingError> {
    let recipe = &input.intent.recipe;
    let truncated =
        probe.is_some_and(|probe| matches!(probe.scope, AnchorProbeScope::RefinedLayer { .. }));
    for capture in process_captures.iter_mut() {
        capture.applied = false;
    }
    let terminal_z = probe.map_or_else(
        || schedule.last().expect("nonempty schedule").z,
        |probe| probe.native_top_mm,
    );
    let mut count = schedule.partition_point(|layer| layer.z <= terminal_z) as u32;
    let mut occurrences = input.occurrences.iter().collect::<Vec<_>>();
    occurrences.sort_by_key(|o| o.occurrence_id);
    // M is sectioned once per certified native material slab. D has its own identity and
    // can change at every refined layer inside that same slab.
    let NativeScheduleCapture {
        mut sections,
        mut native_surfaces,
        mut retained_section_bytes,
        mut retained_points,
        mut startup_wall,
    } = match native_prefix {
        Some(capture) => capture,
        None => NativeScheduleCapture {
            sections: Vec::new(),
            native_surfaces: Vec::new(),
            retained_section_bytes: 0,
            retained_points: 0,
            startup_wall: None,
        },
    };
    if !StartupWall::permits(input)
        || startup_wall
            .as_ref()
            .is_some_and(|wall| !wall.matches(&sections, schedule))
    {
        startup_wall = None;
    }
    let mut index = sections.last().map_or(0, |section| section.end);
    let mut native_slab_end = index;
    let mut native_slab_start = 0u32;
    let mut native_material = None;
    let mut native_material_bytes = 0usize;
    let mut native_contributions = Vec::new();
    let mut native_occupied_shared: Option<Arc<ContinuousOccupied>> = None;
    let mut native_finished = Vec::new();
    let mut native_required = BTreeMap::new();
    let mut native_finished_bytes = 0usize;
    let mut native_source_area_mm2 = 0.0;
    let mut native_points = 0usize;
    let mut prefetched = None;
    while index < count {
        check_cancel(cancel)?;
        let mut sample = schedule[index as usize].sample;
        if index == native_slab_end {
            // The previous slab's query-local operands are obsolete. Release
            // them before admitting the next query; published shared buffers
            // remain owned and charged by sections/native_surfaces.
            drop(native_material.take());
            native_contributions.clear();
            drop(std::mem::take(&mut native_finished));
            native_required.clear();
            drop(native_occupied_shared.take());
            // A later split can move a layer's sample; mapped fields still
            // hold their vertical-error bound on every sliced layer.
            while let Some(cause) = native::surface_vertical_error(admission, schedule, index)? {
                check_cancel(cancel)?;
                native::refine_surface_layer(
                    schedule,
                    index,
                    input
                        .intent
                        .effective_deposition_limits()
                        .min_bead_height_mm,
                    &cause,
                )?;
            }
            // Mapped image layers keep their per-layer vertical-error check.
            let hold_end = if admission
                .surface_fields
                .values()
                .flatten()
                .any(|field| field.is_image())
            {
                index as usize + 1
            } else {
                count as usize
            };
            let captured = tolerant::collect_span(
                &occurrences,
                &admission.models,
                admission,
                &schedule[index as usize..hold_end],
                index,
                &mut prefetched,
                !matches!(input.intent.recipe.supports, SupportPolicy::Disabled),
                cancel,
            )?;
            count = schedule.partition_point(|layer| layer.z <= terminal_z) as u32;
            sample = schedule[index as usize].sample;
            native_slab_start = index;
            native_slab_end = captured.end;
            native_material = Some(captured.material);
            native_material_bytes = captured.material_bytes;
            native_contributions = captured.contributions;
            native_finished = captured.finished;
            native_finished_bytes = captured.finished_bytes;
            native_required.clear();
            native_points = captured.point_count;
            native_source_area_mm2 = captured.source_area_mm2;
            native_occupied_shared = Some(captured.occupied);
            let live_retained = retained_points;
            for (occurrence_id, native) in &native_finished {
                let fields = &admission.surface_fields[occurrence_id];
                if !fields.iter().any(|field| field.is_image()) {
                    continue;
                }
                let required = if fields.iter().any(|field| !field.is_image()) {
                    let staged = live_retained
                        .checked_add(native_points)
                        .filter(|n| {
                            n.checked_add(region::point_count(native))
                                .is_some_and(|total| total <= crate::working_set::units())
                        })
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "mandatory carrier realization staging point budget exceeded",
                            )
                        })?;
                    let mut required = surface::realize_required(
                        native.as_ref().clone(),
                        fields,
                        sample,
                        staged,
                        surface_work,
                        cancel,
                    )?;
                    native_points = native_points
                        .checked_add(region::point_count(&required))
                        .filter(|n| {
                            live_retained
                                .checked_add(*n)
                                .is_some_and(|total| total <= crate::working_set::units())
                        })
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "retained mandatory carrier point budget exceeded",
                            )
                        })?;
                    native_finished_bytes = native_finished_bytes
                        .saturating_add(native::material_bytes(&mut required))
                        .saturating_add(native::MATERIAL_ARC_BYTES);
                    Arc::new(required)
                } else {
                    Arc::clone(native)
                };
                native_required.insert(*occurrence_id, required);
            }
        }
        let live_retained = retained_points;
        let slab_start = schedule[index as usize].lower_z;
        let mut end = index + 1;
        while end < native_slab_end {
            check_cancel(cancel)?;
            let slab_end = schedule[end as usize].z;
            let mut invariant = true;
            for fields in admission.surface_fields.values() {
                for field in fields {
                    invariant &= field.is_z_invariant(slab_start, slab_end)?;
                }
            }
            invariant &= process_captures
                .iter()
                .filter_map(|capture| capture.zone)
                .all(|zone| zone.is_z_invariant(slab_start, slab_end));
            if !invariant {
                break;
            }
            end += 1;
        }
        let mut next = Arc::clone(native_material.as_ref().expect("captured native material"));
        // The ordinary recipe is a resolved process too. Preserve actual
        // original/finished D owners even when no modifier layer is authored.
        let mut process_domains = Vec::new();
        process_domains
            .try_reserve_exact(native_contributions.len() + native_finished.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "process contribution allocation refused",
                )
            })?;
        for (occurrence_id, material, bytes) in &native_contributions {
            process_domains.push(process_domain(
                input,
                process_captures
                    .iter()
                    .find(|capture| capture.occurrence_id == *occurrence_id)
                    .and_then(|capture| capture.zone),
                *occurrence_id,
                Arc::clone(material),
                *bytes,
                [slab_start, schedule[(end - 1) as usize].z],
            )?);
        }
        let mut next_bytes = native_material_bytes;
        for (occurrence_id, placed) in &native_finished {
            let fields = &admission.surface_fields[occurrence_id];
            live_retained
                .checked_add(native_points)
                .and_then(|n| n.checked_add(region::point_count(&next)))
                .and_then(|n| n.checked_add(region::point_count(placed)))
                .filter(|n| *n <= crate::working_set::units())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native carrier realization staging point budget exceeded",
                    )
                })?;
            let mut finished = surface::realize(
                placed.as_ref().clone(),
                fields,
                sample,
                live_retained + native_points + region::point_count(&next),
                surface_work,
                cancel,
            )?;
            let finished_bytes = native::material_bytes(&mut finished);
            let finished = Arc::new(finished);
            process_domains.push(process_domain(
                input,
                process_captures
                    .iter()
                    .find(|capture| capture.occurrence_id == *occurrence_id)
                    .and_then(|capture| capture.zone),
                *occurrence_id,
                Arc::clone(&finished),
                finished_bytes,
                [slab_start, schedule[(end - 1) as usize].z],
            )?);
            // Expanded occurrence separation is certified in preflight. Preserve
            // exact source vertices rather than resnapping them through a union.
            if next.0.is_empty() {
                next = Arc::clone(&finished);
                next_bytes = finished_bytes;
            } else {
                Arc::make_mut(&mut next)
                    .0
                    .extend(finished.0.iter().cloned());
                next_bytes =
                    native::material_bytes(Arc::get_mut(&mut next).expect("owned composed D"));
            }
        }
        validate_process_overlap(
            &process_domains,
            [slab_start, schedule[(end - 1) as usize].z],
            surface_work,
            cancel,
        )?;
        let source_area_mm2 = native_source_area_mm2;
        region::check_size(&next)?;
        if let Some(next) = Arc::get_mut(&mut next) {
            region::canonicalize(next);
        }
        let realized_area_mm2 = next.unsigned_area();
        if realized_area_mm2 < source_area_mm2 {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "surface construction lost source material",
            ));
        }
        // Source occupancy is independent of M/D invariance: chord-sampled
        // invariant contours can still under-enclose the true native carrier.
        let mut occupied = native_occupied_shared.as_ref().map(Arc::clone);
        // Finish construction boxes enclose the continuous requested field,
        // including XY/chord residual even for Z-invariant finishes. Invariance
        // alone does not turn a polygonal realization into outer containment.
        for (occurrence_id, fields) in &admission.surface_fields {
            let native_bounds = admission.native_bounds[occurrence_id];
            for field in fields {
                field.include_occupied_envelope(
                    occupied.as_mut().expect("owned finish occupied envelope"),
                    &surface::OccupiedHeightScope {
                        requested: [slab_start, schedule[(end - 1) as usize].z],
                        native: [native_bounds.min[2], native_bounds.max[2]],
                    },
                    schedule,
                    live_retained + native_points + region::point_count(&next),
                    surface_work,
                    cancel,
                )?;
            }
        }
        let occupied_points = occupied.as_ref().map_or(0, |outer| outer.point_count);
        // Empty native intervals are retained for genuine floating material.
        // Their support paths are generated after actual object-anchor analysis.
        retained_points = retained_points
            .checked_add(
                region::point_count(&next)
                    + occupied_points
                    + process_extra_points(&process_domains, &next, &sections),
            )
            .filter(|n| {
                n.checked_add(native_points)
                    .is_some_and(|total| total <= crate::working_set::units())
            })
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "retained native footprint point budget exceeded",
                )
            })?;
        if !sections
            .iter()
            .any(|section| Arc::ptr_eq(&section.material, &next))
        {
            retained_section_bytes = retained_section_bytes.saturating_add(next_bytes);
        }

        sections.push(SectionSpan {
            start: index,
            end,
            material: next,
            process_domains,
            occupied,
            source_area_mm2,
        });
        if end == native_slab_end && !native_finished.is_empty() {
            // Image coverage needs M for diagnostics and C for mandatory coverage.
            native_finished.retain_mut(|(occurrence_id, material)| {
                let retain = admission.surface_fields[occurrence_id]
                    .iter()
                    .any(|field| field.is_image());
                if !retain {
                    // Non-image M has no mandatory/shared published owner.
                    let material =
                        Arc::get_mut(material).expect("non-image source material is query-local");
                    native_finished_bytes -=
                        native::material_bytes(material) + native::MATERIAL_ARC_BYTES;
                }
                retain
            });
            if !native_finished.is_empty() {
                let mut image_islands = Vec::new();
                for (occurrence_id, native) in &native_finished {
                    let occurrence_start = image_islands.len();
                    for field in admission.surface_fields[occurrence_id]
                        .iter()
                        .filter(|field| field.is_image())
                    {
                        let mut owner = None;
                        for (polygon_index, polygon) in native.0.iter().enumerate() {
                            if field.owns_native_island(polygon, sample, surface_work, cancel)?
                                && owner.replace(polygon_index).is_some()
                            {
                                return Err(error(
                                    ManufacturingErrorCode::UnsupportedGeometry,
                                    "image field has multiple native exterior island owners",
                                ));
                            }
                        }
                        let owner = owner.ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::UnsupportedGeometry,
                                "image field has no native exterior island owner",
                            )
                        })?;
                        let identity = (*occurrence_id, owner);
                        if !image_islands[occurrence_start..].contains(&identity) {
                            image_islands.push(identity);
                        }
                    }
                }
                let points = native_finished
                    .iter()
                    .try_fold(0usize, |points, (id, material)| {
                        let required = &native_required[id];
                        points
                            .checked_add(region::point_count(material))
                            .and_then(|n| {
                                n.checked_add(if Arc::ptr_eq(material, required) {
                                    0
                                } else {
                                    region::point_count(required)
                                })
                            })
                    })
                    .and_then(|points| retained_points.checked_add(points))
                    .filter(|points| *points <= crate::working_set::units())
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "retained native and mandatory footprint budget exceeded",
                        )
                    })?;
                retained_points = points;
                native_surfaces.push(NativeSurfaceSpan {
                    start: native_slab_start,
                    end,
                    occurrences: std::mem::take(&mut native_finished),
                    image_islands,
                    required: std::mem::take(&mut native_required),
                    native_bytes: native_finished_bytes,
                });
            }
        }
        index = end;
    }
    let native_retained_points = retained_points;
    let retained_support_native_bytes = void_memo.retained_bytes();
    drop(native_finished);
    drop(native_required);
    drop(native_material);
    drop(native_occupied_shared);
    let count = if let Some(probe) = probe {
        probe
            .scope
            .end(schedule, recipe, surface_work, cancel)?
            .min(count as usize) as u32
    } else {
        count
    };
    let construction_owned_bytes = geometry_bytes(&[
        native::output_bytes(&sections, &native_surfaces, retained_section_bytes),
        retained_support_native_bytes,
        schedule.capacity() * std::mem::size_of::<LayerElevation>(),
        startup_wall.as_ref().map_or(0, StartupWall::layout_bytes),
    ])?;
    retained_points = retained_points
        .checked_add(
            startup_wall
                .as_ref()
                .map_or(0, |wall| wall.layout.point_count()),
        )
        .filter(|points| *points <= crate::working_set::units())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "startup wall retained-point overflow",
            )
        })?;
    if matches!(
        recipe.sparse_infill,
        SparseInfillPolicy::WebRoof {
            architecture: Some(_),
            ..
        }
    ) {
        let first = sections.first().ok_or_else(|| {
            error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "web + roof study architecture requires an upright constant-section part",
            )
        })?;
        if input.occurrences.len() != 1
            || sections
                .iter()
                .any(|span| !same_section(&first.material, &span.material))
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "web + roof study architecture requires one upright constant-section part",
            ));
        }
        let occurrence = &input.occurrences[0];
        let definition = &input.definitions[&occurrence.definition_id];
        let face = definition.faces().first().ok_or_else(|| {
            error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "web + roof lacks native carrier metadata",
            )
        })?;
        let bounds = admission.native_bounds[&occurrence.occurrence_id];
        // Eligibility uses all analytic carriers and the exact pose; equal
        // sampled sections alone never authorize a curved/sloped source.
        let mut cap_z = [f64::INFINITY, f64::NEG_INFINITY];
        for carrier in definition.faces() {
            if let Some(spiling_contracts::geometry::AnalyticSurfaceDescription::Plane {
                origin_mm,
                normal,
            }) = &carrier.geometry.analytic
                && normal[0] == 0.0
                && normal[1] == 0.0
                && normal[2] != 0.0
            {
                cap_z[0] = cap_z[0].min(origin_mm[2]);
                cap_z[1] = cap_z[1].max(origin_mm[2]);
            }
        }
        if !cap_z.iter().all(|z| z.is_finite()) || cap_z[0] >= cap_z[1] {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "web + roof requires finite separated native extrusion caps",
            ));
        }
        surface::admit_extrusion(
            input,
            occurrence,
            bounds,
            &face.face_id,
            cap_z[0],
            cap_z[1],
            cancel,
        )
        .map_err(|mut failure| {
            failure.message = format!("web + roof: {}", failure.message);
            failure
        })?;
        if let SparseInfillPolicy::WebRoof {
            architecture: Some(a),
            ..
        } = recipe.sparse_infill
            && let Some(o) = a.opening
        {
            let landing = bounds.min[2] + recipe.bottom_skin_thickness_mm + recipe.layer_height_mm;
            if o.bottom_z_mm < landing {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    format!("web opening must retain its bottom landing through {landing} mm"),
                ));
            }
        }
    }
    // Cathedral on any part: one lattice frame and each layer's ceilings.
    let mut web_roof = if matches!(recipe.sparse_infill, SparseInfillPolicy::WebRoof { .. }) {
        let covered = sections.last().map_or(0, |span| span.end as usize);
        let materials: Vec<&Arc<MultiPolygon<f64>>> = (0..schedule.len().min(covered))
            .map(|index| &sections[span_at(&sections, index as u32)].material)
            .collect();
        let tops: Vec<f64> = schedule[..materials.len()]
            .iter()
            .map(|layer| layer.z)
            .collect();
        Some(cathedral::field(
            &materials,
            &tops,
            recipe,
            input.intent.printer.build_envelope.min[2],
            cancel,
        )?)
    } else {
        None
    };
    let mut construction = joins::capture_or_color(
        input,
        &sections,
        schedule,
        &admission.native_bounds,
        retained_points,
        construction_owned_bytes,
        surface_work,
        admission.geometry_work,
        cancel,
    )?;
    retained_points += construction.points;
    let mut layers = Vec::with_capacity(count as usize);
    let mut retained_plan_bytes =
        layers.capacity() * std::mem::size_of::<PrintLayer>() + construction.bytes;
    let mut segments = 0usize;
    let skin_inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
    // Neighborhood leaves are courses: a refinement split changes only its
    // own course's leaf, never another range's aligned decomposition.
    let span_courses = sections
        .iter()
        .map(|span| schedule[span.start as usize].course)
        .collect::<Vec<_>>();
    let mut neighborhoods =
        SkinNeighborhoods::with_courses(&sections, &span_courses, skin_inset, *surface_work)?;
    drop(span_courses);
    // Tolerant sections are a function of their schedule entries alone.
    neighborhoods.attach_memo(
        survival_memo,
        schedule
            .iter()
            .map(|layer| {
                [
                    layer.z.to_bits(),
                    layer.lower_z.to_bits(),
                    layer.sample.to_bits(),
                    layer.height.to_bits(),
                ]
            })
            .collect(),
        probe.is_none(),
    );
    let outcome = (|| {
        let mut key = None;
        let mut classified = None;
        let mut classified_owned_bytes = 0usize;
        let mut layout_span = None;
        let mut wall_layout = None;
        let mut wall_layouts = (0..schedule.len()).map(|_| None).collect::<Vec<_>>();
        let mut wall_cache = BTreeMap::<WallCacheKey, Arc<walls::WallPlan>>::new();
        let material_twin = material_twins(&sections, cancel)?;
        prefill_walls(
            input,
            schedule,
            &sections,
            &native_surfaces,
            &construction,
            &material_twin,
            retained_points,
            wall_memo,
            neighborhoods.work_mut(),
            cancel,
        )?;
        let mut first_wall_key = None;
        let mut retained_wall_bytes = 0usize;
        // Same conservative sparse-BTree node convention as native::output_bytes:
        // a live entry covers all key/value/child slots of its possible node.
        let wall_index_entry_bytes = 16
            * (std::mem::size_of::<(WallCacheKey, Arc<walls::WallPlan>)>()
                + std::mem::size_of::<usize>());
        let wall_static_base_bytes =
            native::output_bytes(&sections, &native_surfaces, retained_section_bytes)
                .checked_add(retained_support_native_bytes)
                .and_then(|b| {
                    b.checked_add(schedule.capacity() * std::mem::size_of::<LayerElevation>())
                })
                .and_then(|b| {
                    b.checked_add(
                        wall_layouts.capacity()
                            * std::mem::size_of::<Option<Arc<walls::WallPlan>>>(),
                    )
                })
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "wall caller retained-byte overflow",
                    )
                })?;
        let phase_bytes = schedule
            .len()
            .checked_mul(std::mem::size_of::<region::OrdinaryHatchPhase>())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "ordinary phase metadata overflow",
                )
            })?;
        remaining_geometry_units(
            retained_points,
            geometry_bytes(&[
                wall_static_base_bytes,
                retained_plan_bytes,
                phase_bytes,
                startup_wall.as_ref().map_or(0, |wall| {
                    wall.layout_bytes().saturating_add(wall_index_entry_bytes)
                }),
            ])?,
        )?;
        if let Some(startup) = startup_wall.take() {
            retained_wall_bytes = startup.layout_bytes();
            wall_cache.insert(startup.key, startup.layout);
        }
        let mut phase_by_layer = Vec::new();
        phase_by_layer
            .try_reserve_exact(schedule.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "ordinary phase metadata allocation refused",
                )
            })?;
        phase_by_layer.resize(schedule.len(), region::OrdinaryHatchPhase::Centered);
        let phase_bytes = phase_by_layer
            .capacity()
            .checked_mul(std::mem::size_of::<region::OrdinaryHatchPhase>())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "ordinary phase capacity overflow",
                )
            })?;
        let wall_static_base_bytes = geometry_bytes(&[wall_static_base_bytes, phase_bytes])?;
        remaining_geometry_units(
            retained_points,
            geometry_bytes(&[
                wall_static_base_bytes,
                retained_plan_bytes,
                retained_wall_bytes,
                wall_cache.len() * wall_index_entry_bytes,
            ])?,
        )?;
        let mut phase_anchors = support::AnchorWorkspace::default();
        let mut templates: [Option<OrdinaryTemplate>; 2] = [None, None];
        let mut surface_resolution = Vec::new();
        let mut resolution_allocated_bytes = 0usize;
        let mut diagnostic_work = 0usize;
        for index in 0..count {
            check_cancel(cancel)?;
            let wall_static_bytes =
                geometry_bytes(&[wall_static_base_bytes, phase_anchors.retained_bytes()])?;
            let elevation = &schedule[index as usize];
            let z = elevation.z;
            let sample = elevation.sample;
            let height = elevation.height;
            let top_axis = region::HatchAxis::for_policy(
                recipe.top_skin_axis,
                hatch_key(schedule, index as usize),
            );
            let current = span_at(&sections, index);
            let below = physical_neighbor(schedule, index, recipe.bottom_skin_thickness_mm, false)
                .map(|i| span_at(&sections, i));
            let above = physical_neighbor(schedule, index, recipe.top_skin_thickness_mm, true)
                .map(|i| span_at(&sections, i));
            if sections[current].material.0.is_empty() {
                layers.push(PrintLayer {
                    skin_command_fits: Vec::new(),
                    index,
                    hatch_course: elevation.course,
                    z_mm: z,
                    lower_z_mm: elevation.lower_z,
                    height_mm: height,
                    section_z_mm: sample,
                    paths: Vec::new(),
                    region_coverage: empty_coverage(recipe.infill_fraction),
                });
                continue;
            }
            let native_index = native_surfaces.partition_point(|span| span.end <= index);
            let native_span = native_surfaces
                .get(native_index)
                .filter(|span| span.start <= index);
            // With no image exemptions C is exactly D. Otherwise preserve the
            // complete immutable C/source-island attribution, not XY equality.
            let required_owner = native_span
                .filter(|span| !span.image_islands.is_empty())
                .map(|_| native_index);
            let construction_phase = construction.domains.iter().position(|domain| {
                domain.reference.role == MaterialConstructionRole::FunctionalRegion
                    && domain.physical_z_mm[0] <= elevation.lower_z
                    && z <= domain.physical_z_mm[1]
            });
            let next_key = (
                current,
                below,
                above,
                (z - elevation.lower_z).to_bits(),
                required_owner,
                construction_phase,
                construction.functional_frame.as_ref().and_then(|frame| {
                    frame.solid_base_zones.iter().position(|zone| {
                        zone.physical_z_mm[0] <= elevation.lower_z && z <= zone.physical_z_mm[1]
                    })
                }),
            );
            if key != Some(next_key) {
                // Drop the prior classification before allocating its replacement.
                // The current wall layout survives skin-key changes, not span changes.
                drop(classified.take());
                templates = [None, None];
                let actual_height = z - elevation.lower_z;
                let limits = input.intent.effective_deposition_limits();
                let envelope = walls::width_envelope(&limits, actual_height)?;
                // Retain the exact unguarded stadium floor as well: adding the
                // guard must not merge adjacent height-dominated f64 floors.
                let envelope_key = [
                    envelope[0].to_bits(),
                    envelope[1].to_bits(),
                    limits.min_bead_width_mm.max(actual_height).to_bits(),
                ];
                // Identical material plans identical walls: key by the first span
                // with bit-equal material so repeated prismatic layers share one plan.
                let wall_span = if required_owner.is_none() && construction.domains.is_empty() {
                    material_twin[current]
                } else {
                    current
                };
                let layout_key = (wall_span, required_owner, envelope_key, construction_phase);
                if index == 0 {
                    first_wall_key = Some(layout_key);
                }
                if layout_span != Some(layout_key) {
                    drop(wall_layout.take());
                    if let Some(cached) = wall_cache.get(&layout_key) {
                        wall_layout = Some(Arc::clone(cached));
                    } else {
                        let required_material = required_material(
                            &sections[current].material,
                            native_span,
                            neighborhoods.work_mut(),
                            cancel,
                        )?;
                        let retained_wall_points =
                            skin_retained_points(retained_points, &neighborhoods, &[])?;
                        let wall_owned_bytes = wall_static_bytes
                            .checked_add(retained_plan_bytes)
                            .and_then(|b| b.checked_add(retained_wall_bytes))
                            .and_then(|b| b.checked_add(wall_cache.len() * wall_index_entry_bytes))
                            .and_then(|b| b.checked_add(resolution_allocated_bytes))
                            .and_then(|b| b.checked_add(neighborhoods.retained_bytes()))
                            .and_then(|b| {
                                b.checked_add(
                                    required_material.polygons.capacity()
                                        * std::mem::size_of::<&geo::Polygon<f64>>(),
                                )
                            })
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "wall caller live-byte overflow",
                                )
                            })?;

                        // Retries of the schedule (support refinement) replan every
                        // layer; identical material at the same envelope reuses its walls.
                        let memo_key = (required_owner.is_none()
                            && construction_phase.is_none()
                            && construction.domains.is_empty())
                        .then_some(envelope_key);
                        let remembered = memo_key
                            .and_then(|key| wall_memo.find(&sections[current].material, key));
                        let layout = match remembered {
                            Some(plan) => plan,
                            None => {
                                let plan = Arc::new(
                            construction.walls(
                        walls::WallRequest {
                            material: &sections[current].material,
                            required: &required_material,
                            recipe,
                            limits: &limits,
                            nozzle_diameter_mm: input.intent.printer.nozzle_diameter_mm,
                            lower_z: elevation.lower_z,
                            upper_z: z,
                            retained_points: retained_wall_points,
                            owned_bytes: wall_owned_bytes,
                        },
                        neighborhoods.work_mut(),
                        cancel,
                    ).map_err(|mut failure| {
                        failure.message = format!(
                            "layer {index} (section Z={sample} mm, deposition Z={z} mm, height={height} mm): {}",
                            failure.message
                        );
                        failure
                    })?,
                                );
                                if let Some(key) = memo_key {
                                    wall_memo.insert(&sections[current].material, key, &plan);
                                }
                                plan
                            }
                        };
                        let new_wall_bytes = layout
                            .retained_bytes
                            .checked_add(2 * std::mem::size_of::<usize>())
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "wall cache allocation-byte overflow",
                                )
                            })?;
                        if wall_owned_bytes
                            .checked_add(new_wall_bytes)
                            .and_then(|b| b.checked_add(wall_index_entry_bytes))
                            .is_none_or(|b| b as u64 > crate::working_set::bytes_u64())
                        {
                            return Err(error(
                                ManufacturingErrorCode::ResourceLimit,
                                "wall cache retention byte budget exceeded",
                            ));
                        }
                        retained_wall_bytes = retained_wall_bytes
                            .checked_add(new_wall_bytes)
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "wall cache retained-byte overflow",
                                )
                            })?;
                        retained_points += layout.point_count();
                        wall_cache.insert(layout_key, Arc::clone(&layout));
                        wall_layout = Some(layout);
                    }
                    layout_span = Some(layout_key);
                }
                let skin_owned_bytes = skin_external_owned_bytes(
                    wall_static_bytes,
                    retained_plan_bytes,
                    retained_wall_bytes,
                    wall_cache.len(),
                    wall_index_entry_bytes,
                    resolution_allocated_bytes,
                    &templates,
                )?;
                let lower = neighborhoods.survival(
                    current,
                    below,
                    retained_points,
                    skin_owned_bytes,
                    cancel,
                )?;
                let upper = neighborhoods.survival(
                    current,
                    above,
                    skin_query_retained_points(retained_points, &[&lower])?,
                    skin_query_retained_bytes(skin_owned_bytes, &[&lower])?,
                    cancel,
                )?;
                classified = Some(
                    classification::ClassifiedRegions::new(
                        Arc::clone(
                            wall_layout
                                .as_ref()
                                .expect("current wall layout initialized"),
                        ),
                        &lower,
                        &upper,
                        |scratch, scratch_bytes| {
                            admit_skin_scratch(
                                retained_points,
                                skin_owned_bytes,
                                &mut neighborhoods,
                                &[&lower, &upper],
                                scratch,
                                scratch_bytes,
                                cancel,
                            )
                        },
                        recipe,
                        cancel,
                    )?
                    .with_process(
                        &sections[current].process_domains,
                        input.intent,
                        classification::fixed_owned_budget(
                            skin_retained_points(
                                retained_points,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                            skin_retained_bytes(
                                skin_owned_bytes,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                        ),
                        neighborhoods.work_mut(),
                        cancel,
                    )?,
                );
                construction.classify(
                    classified.as_mut().expect("classified material regions"),
                    [elevation.lower_z, z],
                    classification::fixed_owned_budget(
                        skin_retained_points(retained_points, &neighborhoods, &[&lower, &upper])?,
                        skin_retained_bytes(skin_owned_bytes, &neighborhoods, &[&lower, &upper])?,
                    ),
                    neighborhoods.work_mut(),
                    cancel,
                )?;
                classified
                    .as_mut()
                    .expect("classified material")
                    .with_tool_widths(
                        input.intent,
                        classification::fixed_owned_budget(
                            skin_retained_points(
                                retained_points,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                            skin_retained_bytes(
                                skin_owned_bytes,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                        ),
                        neighborhoods.work_mut(),
                        cancel,
                    )?;
                classified_owned_bytes = classified
                    .as_mut()
                    .expect("classified process regions")
                    .retained_bytes();

                for plate in &classified
                    .as_ref()
                    .expect("classified process regions")
                    .coverage
                    .process_diaphragms
                {
                    let owned = &classified
                        .as_ref()
                        .expect("classified process regions")
                        .interiors[usize::from(plate.region_index)];
                    if !combined::inner_material_admits(
                        &owned.domain,
                        &owned.coverage,
                        plate.physical_z_mm,
                        recipe.bead_width_mm,
                        input.intent,
                        &admission.models,
                        cancel,
                    )? {
                        return Err(error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            "UnsupportedInterface: complete permanent plate lacks full-height inner material",
                        ));
                    }
                }
                key = Some(next_key);
                for &(occurrence_id, provenance) in &classified
                    .as_ref()
                    .expect("classified process regions")
                    .applied_operations
                {
                    if let Some(capture) = process_captures.iter_mut().find(|capture| {
                        capture.occurrence_id == occurrence_id
                            && capture
                                .zone
                                .is_some_and(|zone| zone.provenance == provenance)
                    }) {
                        capture.applied = true;
                    }
                }
            }
            // Only XY wall geometry is shared. Source-finish evidence is evaluated
            // at this actual sample, independently of the wall cache/envelope key.
            for fields in admission.surface_fields.values() {
                surface::certify_first_wall(
                    wall_layout
                        .as_ref()
                        .expect("current wall construction initialized"),
                    &sections[current].material,
                    fields,
                    sample,
                    cancel,
                )?;
            }
            if !admission.surface_fields.is_empty() {
                for (occurrence_id, fields) in &admission.surface_fields {
                    let Some(native) = native_span.and_then(|span| {
                        span.occurrences
                            .iter()
                            .find(|(id, _)| id == occurrence_id)
                            .map(|(_, material)| material)
                    }) else {
                        continue;
                    };
                    let records = surface::collect_resolution(
                        surface::ResolutionGeometry {
                            wall: wall_layout
                                .as_ref()
                                .expect("current wall construction initialized"),
                            material: &sections[current].material,
                            native,
                        },
                        fields,
                        sample,
                        index,
                        &mut diagnostic_work,
                        cancel,
                    )?;
                    let required = surface_resolution
                        .len()
                        .checked_add(records.len())
                        .filter(|n| *n <= MAX_SURFACE_RESOLUTION_RECORDS)
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "surface resolution record budget exceeded",
                            )
                        })?;
                    let added_bytes = records
                        .iter()
                        .try_fold(0usize, |sum, record| {
                            sum.checked_add(record.allocated_bytes())
                        })
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "surface resolution allocation overflow",
                            )
                        })?;
                    resolution_allocated_bytes = resolution_allocated_bytes
                        .checked_add(added_bytes)
                        .filter(|bytes| {
                            bytes
                                .checked_add(
                                    surface_resolution.capacity().saturating_sub(required)
                                        * std::mem::size_of::<SurfaceResolution>(),
                                )
                                .is_some_and(|total| {
                                    total <= MAX_MANUFACTURING_BUNDLE_BYTES as usize
                                })
                        })
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "surface resolution allocation budget exceeded",
                            )
                        })?;
                    surface_resolution
                        .try_reserve_exact(records.len())
                        .map_err(|_| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "surface resolution allocation refused",
                            )
                        })?;
                    surface_resolution.extend(records);
                }
            }
            wall_layouts[index as usize] = wall_layout.as_ref().map(Arc::clone);
            let axis = region::HatchAxis::for_policy(
                recipe.rectilinear_axis,
                hatch_key(schedule, index as usize),
            );
            let axis_slot = usize::from(axis == region::HatchAxis::Y);
            for template in &mut templates {
                if template
                    .as_ref()
                    .is_some_and(|template| template.top_axis != top_axis)
                {
                    *template = None;
                }
            }
            // Selection is an initial construction proposal, not a reusable anchor
            // receipt. It is recomputed against this layer's earlier initial paths,
            // then frozen for every reconstruction of these required normal rows.
            let guide_outside_bytes = skin_external_owned_bytes(
                wall_static_base_bytes,
                retained_plan_bytes,
                retained_wall_bytes,
                wall_cache.len(),
                wall_index_entry_bytes,
                resolution_allocated_bytes,
                &templates,
            )?;
            let hatch_layer = region::HatchLayer {
                roof: web_roof.as_ref().map(|field| field.roof(index as usize)),
                width_mm: recipe.bead_width_mm,
                z,
                lower_z: elevation.lower_z,
                height: z - elevation.lower_z,
                axis,
                top_axis,
                recipe,
            };
            let phase = classified
                .as_ref()
                .expect("region classification initialized")
                .choose_initial_ordinary_phase(
                    &hatch_layer,
                    layers
                        .last()
                        .into_iter()
                        .flat_map(|layer| layer.paths.iter()),
                    &mut phase_anchors,
                    &mut support_work.anchors,
                    classified_owned_bytes,
                    skin_retained_points(retained_points, &neighborhoods, &[])?,
                    skin_retained_bytes(guide_outside_bytes, &neighborhoods, &[])?,
                    |scratch, scratch_bytes, work| {
                        boolean::charge(neighborhoods.work_mut(), work)?;
                        admit_skin_scratch(
                            retained_points,
                            guide_outside_bytes,
                            &mut neighborhoods,
                            &[],
                            scratch,
                            scratch_bytes,
                            cancel,
                        )
                    },
                    cancel,
                )?;
            phase_by_layer[index as usize] = phase;
            if templates[axis_slot]
                .as_ref()
                .is_some_and(|template| template.phase != phase || template.top_axis != top_axis)
            {
                templates[axis_slot] = None;
            }
            let wall_static_bytes =
                geometry_bytes(&[wall_static_base_bytes, phase_anchors.retained_bytes()])?;
            let outside_skin_bytes = skin_external_owned_bytes(
                wall_static_bytes,
                retained_plan_bytes,
                retained_wall_bytes,
                wall_cache.len(),
                wall_index_entry_bytes,
                resolution_allocated_bytes,
                &templates,
            )?;
            if matches!(recipe.sparse_infill, SparseInfillPolicy::WebRoof { .. }) {
                // Roof branches depend on physical height, even when native D
                // and its wall/classification templates remain unchanged.
                templates[axis_slot] = None;
            }
            if templates[axis_slot].is_none() {
                let mut template_segments = segments;
                let classified = classified
                    .as_ref()
                    .expect("region classification initialized");
                let (paths, proposed, fits) = classified.paths(
                    z,
                    elevation.lower_z,
                    axis,
                    top_axis,
                    recipe,
                    web_roof.as_ref().map(|field| field.roof(index as usize)),
                    phase,
                    classified_owned_bytes,
                    &sections[current].process_domains,
                    &mut template_segments,
                    |scratch, scratch_bytes, work| {
                        boolean::charge(neighborhoods.work_mut(), work)?;
                        admit_skin_scratch(
                            retained_points,
                            outside_skin_bytes,
                            &mut neighborhoods,
                            &[],
                            scratch,
                            scratch_bytes,
                            cancel,
                        )
                    },
                    cancel,
                )?;
                let base = skin_retained_points(retained_points, &neighborhoods, &[])?
                    + classified.retained_points()
                    + paths.iter().map(|p| p.points_mm.capacity()).sum::<usize>()
                    + bonds::skin_command_fit_points(&fits);
                let live_bytes = geometry_bytes(&[
                    skin_retained_bytes(outside_skin_bytes, &neighborhoods, &[])?,
                    classified_owned_bytes,
                    bonds::path_bytes(&paths),
                    bonds::skin_command_fit_bytes(&fits),
                ])?;
                let mut admit = classification::fixed_owned_budget(base, live_bytes);
                admit(
                    bonds::coverage_owned_bytes(proposed)
                        .div_ceil(std::mem::size_of::<geo::Coord<f64>>()),
                    bonds::coverage_owned_bytes(proposed),
                )?;
                let coverage = proposed.clone();
                templates[axis_slot] = Some(OrdinaryTemplate {
                    phase,
                    top_axis,
                    paths,
                    coverage,
                    fits,
                });
            }
            let OrdinaryTemplate {
                paths: template,
                coverage,
                fits: template_fits,
                ..
            } = templates[axis_slot]
                .as_ref()
                .expect("current axis template initialized");
            let additional = template
                .iter()
                .map(|p| p.points_mm.len() - 1)
                .sum::<usize>();
            let ribs = coverage
                .process_interfaces
                .iter()
                .map(|i| i.centerlines_mm.len())
                .sum::<usize>();
            let rib_bytes = ribs
                .checked_mul(
                    std::mem::size_of::<DepositionPath>() + 2 * std::mem::size_of::<[f64; 3]>(),
                )
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "separator output accounting overflow",
                    )
                })?;
            let layer_bytes = bonds::path_bytes(template)
                .checked_add(bonds::coverage_owned_bytes(coverage))
                .and_then(|bytes| bytes.checked_add(bonds::skin_command_fit_bytes(template_fits)))
                .and_then(|bytes| bytes.checked_add(rib_bytes))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "layer output byte accounting overflow",
                    )
                })?;
            retained_plan_bytes =
                retained_plan_bytes
                    .checked_add(layer_bytes)
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "plan output byte accounting overflow",
                        )
                    })?;
            let template_bytes = templates
                .iter()
                .flatten()
                .map(
                    |OrdinaryTemplate {
                         paths,
                         coverage,
                         fits,
                         ..
                     }| {
                        bonds::path_bytes(paths)
                            + bonds::coverage_owned_bytes(coverage)
                            + bonds::skin_command_fit_bytes(fits)
                    },
                )
                .sum::<usize>();
            if retained_plan_bytes
                .checked_add(template_bytes)
                .and_then(|n| n.checked_add(resolution_allocated_bytes))
                .is_none_or(|n| n > MAX_MANUFACTURING_BUNDLE_BYTES as usize)
            {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "live plan output clones and interface metadata exceed the existing manufacturing byte budget",
                ));
            }
            let live_outside = skin_external_owned_bytes(
                wall_static_bytes,
                retained_plan_bytes,
                retained_wall_bytes,
                wall_cache.len(),
                wall_index_entry_bytes,
                resolution_allocated_bytes,
                &templates,
            )?;
            neighborhoods.admit_retained(
                retained_points,
                geometry_bytes(&[live_outside, classified_owned_bytes])?,
                cancel,
            )?;
            segments = segments
                .checked_add(additional)
                .filter(|n| *n <= crate::working_set::units())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "deposition segment limit exceeded",
                    )
                })?;
            let mut paths = Vec::with_capacity(template.len() + ribs);
            for path in template {
                check_cancel(cancel)?;
                paths.push(DepositionPath {
                    material_domain: path.material_domain,
                    kind: path.kind,
                    owner: path.owner,
                    width: path.width.clone(),
                    physical_z_mm: [elevation.lower_z, z],
                    points_mm: path.points_mm.iter().map(|p| [p[0], p[1], z]).collect(),
                    arcs: Vec::new(),
                    exposed: false,
                });
            }
            let mut coverage = coverage.clone();
            let mut skin_command_fits = template_fits.clone();
            for domain in &mut skin_command_fits {
                for fit in &mut domain.fits {
                    for point in fit.required_mm.iter_mut().chain(fit.fitted_mm.iter_mut()) {
                        point[2] = z;
                    }
                }
            }
            coverage.source_section_area_mm2 = sections[current].source_area_mm2;
            coverage.surface_added_area_mm2 =
                coverage.section_area_mm2 - coverage.source_section_area_mm2;
            if let Some(field) = &web_roof {
                field.stamp(index as usize, &mut coverage.web_roof_domains);
            }
            layers.push(PrintLayer {
                skin_command_fits,
                index,
                hatch_course: elevation.course,
                z_mm: z,
                lower_z_mm: elevation.lower_z,
                height_mm: height,
                section_z_mm: sample,
                paths,
                region_coverage: coverage,
            });
        }

        drop(classified);
        drop(wall_layout);
        drop(templates);
        drop(phase_anchors);
        if probe.is_none() {
            retained_points -= native_surfaces
                .iter()
                .map(|span| {
                    span.occurrences
                        .iter()
                        .map(|(id, material)| {
                            let required = &span.required[id];
                            region::point_count(material)
                                + if Arc::ptr_eq(material, required) {
                                    0
                                } else {
                                    region::point_count(required)
                                }
                        })
                        .sum::<usize>()
                })
                .sum::<usize>();
            drop(std::mem::take(&mut native_surfaces));
        }
        // Support reserves this derived-cache pool once; regeneration may evict and
        // rebuild within it, but cannot silently increase retained support staging.
        neighborhoods.freeze_retained_capacity();
        let support_native_points = skin_retained_points(retained_points, &neighborhoods, &[])?;
        // Surface-only captures/templates/classification are gone. Support owns the
        // changing layer buffers, so its callback reports those separately.
        let support_static_bytes = geometry_bytes(&[
            native::output_bytes(&sections, &native_surfaces, retained_section_bytes),
            retained_support_native_bytes,
            schedule.capacity() * std::mem::size_of::<LayerElevation>(),
            wall_layouts.capacity() * std::mem::size_of::<Option<Arc<walls::WallPlan>>>(),
            layers.capacity() * std::mem::size_of::<PrintLayer>(),
            phase_bytes,
        ])?;
        let support_outside_bytes = skin_external_owned_bytes(
            support_static_bytes,
            0,
            retained_wall_bytes,
            wall_cache.len(),
            wall_index_entry_bytes,
            resolution_allocated_bytes,
            &[None, None],
        )?;
        let mut support_native_owned_bytes =
            skin_retained_bytes(support_outside_bytes, &neighborhoods, &[])?;

        if let Some(field) = &mut web_roof {
            field.stabilize(
                &mut layers,
                recipe,
                &mut segments,
                support_native_owned_bytes,
                cancel,
            )?;
            support_native_owned_bytes =
                geometry_bytes(&[support_native_owned_bytes, field.column_bytes()])?;
        }

        support_work.planar = *neighborhoods.work_mut();
        let support_admission = support::realize(
            &mut layers,
            &support::SupportInput {
                sections: &sections,
                schedule,
                native_points: support_native_points,
                native_owned_bytes: support_native_owned_bytes,
                recipe,
                source_z_allowance_mm: admission
                    .models
                    .values()
                    .map(|model| model.support_margin())
                    .fold(0.0, f64::max),
                replay: web_roof.is_none() && construction.domains.is_empty(),
                phases: &phase_by_layer,
            },
            &mut segments,
            cancel,
            admission.geometry_work,
            probe.is_some() && !truncated,
            truncated,
            support_work,
            |index,
             axis,
             segments,
             retained_support_points,
             retained_support_bytes,
             anchor_owned_bytes,
             planar_work,
             recovery| {
                *neighborhoods.work_mut() = *planar_work;
                let outcome = (|| {
                    let live_retained_points = retained_points
                        .checked_add(retained_support_points)
                        .filter(|total| *total <= crate::working_set::units())
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "support raster retained point limit exceeded",
                            )
                        })?;
                    let outside_owned_bytes =
                        geometry_bytes(&[support_outside_bytes, retained_support_bytes])?;
                    let mut live_owned_bytes =
                        geometry_bytes(&[outside_owned_bytes, anchor_owned_bytes])?;
                    let anchor_capacity = std::cell::Cell::new(anchor_owned_bytes);
                    let elevation = &schedule[index];
                    let current = span_at(&sections, index as u32);
                    let below = physical_neighbor(
                        schedule,
                        index as u32,
                        recipe.bottom_skin_thickness_mm,
                        false,
                    )
                    .map(|i| span_at(&sections, i));
                    let above = physical_neighbor(
                        schedule,
                        index as u32,
                        recipe.top_skin_thickness_mm,
                        true,
                    )
                    .map(|i| span_at(&sections, i));
                    let lower = neighborhoods.survival(
                        current,
                        below,
                        live_retained_points,
                        live_owned_bytes,
                        cancel,
                    )?;
                    let upper = neighborhoods.survival(
                        current,
                        above,
                        skin_query_retained_points(live_retained_points, &[&lower])?,
                        skin_query_retained_bytes(live_owned_bytes, &[&lower])?,
                        cancel,
                    )?;
                    let wall = Arc::clone(
                        wall_layouts[index]
                            .as_ref()
                            .expect("physical-height wall construction already admitted"),
                    );
                    let mut classified = classification::ClassifiedRegions::new(
                        wall,
                        &lower,
                        &upper,
                        |scratch, scratch_bytes| {
                            admit_skin_scratch(
                                live_retained_points,
                                live_owned_bytes,
                                &mut neighborhoods,
                                &[&lower, &upper],
                                scratch,
                                scratch_bytes,
                                cancel,
                            )
                        },
                        recipe,
                        cancel,
                    )?
                    .with_process(
                        &sections[current].process_domains,
                        input.intent,
                        classification::fixed_owned_budget(
                            skin_retained_points(
                                live_retained_points,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                            skin_retained_bytes(
                                live_owned_bytes,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                        ),
                        neighborhoods.work_mut(),
                        cancel,
                    )?;
                    construction.classify(
                        &mut classified,
                        [elevation.lower_z, elevation.z],
                        classification::fixed_owned_budget(
                            skin_retained_points(
                                live_retained_points,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                            skin_retained_bytes(
                                live_owned_bytes,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                        ),
                        neighborhoods.work_mut(),
                        cancel,
                    )?;
                    classified.with_tool_widths(
                        input.intent,
                        classification::fixed_owned_budget(
                            skin_retained_points(
                                live_retained_points,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                            skin_retained_bytes(
                                live_owned_bytes,
                                &neighborhoods,
                                &[&lower, &upper],
                            )?,
                        ),
                        neighborhoods.work_mut(),
                        cancel,
                    )?;
                    let classified_bytes = classified.retained_bytes();
                    let hatch_layer = region::HatchLayer {
                        roof: web_roof.as_ref().map(|field| field.roof(index)),
                        width_mm: recipe.bead_width_mm,
                        z: elevation.z,
                        lower_z: elevation.lower_z,
                        height: elevation.z - elevation.lower_z,
                        axis,
                        top_axis: region::HatchAxis::for_policy(
                            recipe.top_skin_axis,
                            hatch_key(schedule, index),
                        ),
                        recipe,
                    };
                    let recovering_components = recovery.is_some();
                    let mut skin_bridge_domains = Vec::new();
                    let mut skin_command_fits = Vec::new();
                    let paths = if let Some(anchors) = recovery {
                        let query_points = skin_query_retained_points(0, &[&lower, &upper])?;
                        let query_owned_bytes = skin_query_retained_bytes(0, &[&lower, &upper])?;
                        let mut check_component =
                            |candidate: &mut support::RasterCandidate,
                             axis,
                             scratch: usize,
                             scratch_owned_bytes: usize| {
                                let live = scratch.checked_add(query_points).ok_or_else(|| {
                                    error(
                                        ManufacturingErrorCode::ResourceLimit,
                                        "component and query scratch overflow",
                                    )
                                })?;
                                let receipt = anchors(
                                    candidate,
                                    axis,
                                    live,
                                    geometry_bytes(&[scratch_owned_bytes, query_owned_bytes])?,
                                )?;
                                anchor_capacity.set(receipt.workspace_bytes);
                                Ok(receipt)
                            };
                        classified.paths_with_bridge_recovery(
                            &hatch_layer,
                            phase_by_layer[index],
                            &sections[current].process_domains,
                            classified_bytes,
                            anchor_owned_bytes,
                            segments,
                            |scratch, scratch_bytes, work| {
                                boolean::charge(neighborhoods.work_mut(), work)?;
                                admit_skin_scratch(
                                    live_retained_points,
                                    outside_owned_bytes,
                                    &mut neighborhoods,
                                    &[&lower, &upper],
                                    scratch,
                                    scratch_bytes,
                                    cancel,
                                )
                            },
                            &mut check_component,
                            &mut skin_bridge_domains,
                            &mut skin_command_fits,
                            cancel,
                        )?
                    } else {
                        let (paths, _, fits) = classified.paths(
                            elevation.z,
                            elevation.lower_z,
                            axis,
                            region::HatchAxis::for_policy(
                                recipe.top_skin_axis,
                                hatch_key(schedule, index),
                            ),
                            recipe,
                            web_roof.as_ref().map(|field| field.roof(index)),
                            phase_by_layer[index],
                            classified_bytes,
                            &sections[current].process_domains,
                            segments,
                            |scratch, scratch_bytes, work| {
                                boolean::charge(neighborhoods.work_mut(), work)?;
                                admit_skin_scratch(
                                    live_retained_points,
                                    live_owned_bytes,
                                    &mut neighborhoods,
                                    &[&lower, &upper],
                                    scratch,
                                    scratch_bytes,
                                    cancel,
                                )
                            },
                            cancel,
                        )?;
                        skin_command_fits = fits;
                        paths
                    };
                    // Recovery scratch carries the whole workspace receipt once.
                    // Refresh its capacity before cloning interface metadata below.
                    live_owned_bytes =
                        geometry_bytes(&[outside_owned_bytes, anchor_capacity.get()])?;
                    let base = skin_retained_points(
                        live_retained_points,
                        &neighborhoods,
                        &[&lower, &upper],
                    )? + classified.retained_points()
                        + paths.iter().map(|p| p.points_mm.capacity()).sum::<usize>()
                        + bonds::skin_bridge_points(&skin_bridge_domains)
                        + bonds::skin_command_fit_points(&skin_command_fits);
                    let live_bytes = geometry_bytes(&[
                        skin_retained_bytes(live_owned_bytes, &neighborhoods, &[&lower, &upper])?,
                        classified_bytes,
                        bonds::path_bytes(&paths),
                        bonds::skin_bridge_bytes(&skin_bridge_domains),
                        bonds::skin_command_fit_bytes(&skin_command_fits),
                    ])?;
                    let mut admit = classification::fixed_owned_budget(base, live_bytes);
                    admit(
                        bonds::interface_owned_bytes(&classified.coverage.process_interfaces)
                            .div_ceil(std::mem::size_of::<geo::Coord<f64>>()),
                        bonds::interface_owned_bytes(&classified.coverage.process_interfaces),
                    )?;
                    let process_interfaces = classified.coverage.process_interfaces.clone();
                    let whole_rows = if recovering_components {
                        // A bridge band and authored complement are disjoint owners.
                        // Global row extension must not re-enter the selected band.
                        Vec::new()
                    } else {
                        let outside_rows_bytes = geometry_bytes(&[
                            live_bytes,
                            bonds::interface_owned_bytes(&process_interfaces),
                        ])?;
                        let outside_rows_units = base
                            + bonds::interface_owned_bytes(&process_interfaces)
                                .div_ceil(std::mem::size_of::<geo::Coord<f64>>());
                        classified.whole_rows(
                            &paths,
                            &hatch_layer,
                            segments,
                            cancel,
                            classification::fixed_owned_budget(
                                outside_rows_units,
                                outside_rows_bytes,
                            ),
                        )?
                    };
                    Ok(support::RasterCandidate {
                        skin_bridge_domains,
                        skin_command_fits,
                        paths,
                        whole_rows,
                        process_interfaces,
                    })
                })();
                *planar_work = *neighborhoods.work_mut();
                outcome
            },
            |demand_bounds,
             retained_units,
             live_bytes,
             _native_points,
             _native_owned_bytes,
             _support_work,
             _native_work| {
                tolerant::project_protected_voids(
                    &admission.models,
                    void_memo,
                    demand_bounds,
                    truncated,
                    retained_units,
                    live_bytes,
                    cancel,
                )
            },
            |index, candidate, segments, work, outside_owned_bytes| {
                let current = span_at(&sections, index as u32);
                let base = support_native_points
                    + bonds::skin_bridge_points(&candidate.skin_bridge_domains)
                    + bonds::skin_command_fit_points(&candidate.skin_command_fits)
                    + candidate
                        .paths
                        .iter()
                        .map(|p| p.points_mm.capacity())
                        .sum::<usize>();
                let candidate_owned_bytes = geometry_bytes(&[
                    bonds::path_bytes(&candidate.paths),
                    bonds::path_bytes(&candidate.whole_rows),
                    bonds::skin_bridge_bytes(&candidate.skin_bridge_domains),
                    bonds::skin_command_fit_bytes(&candidate.skin_command_fits),
                ])?;
                bonds::construct(
                    &mut candidate.process_interfaces,
                    &sections[current].process_domains,
                    &mut candidate.paths,
                    schedule[index].z,
                    schedule[index].lower_z,
                    recipe,
                    segments,
                    classification::fixed_point_budget(
                        base,
                        geometry_bytes(&[outside_owned_bytes, candidate_owned_bytes])?,
                    ),
                    work,
                    cancel,
                )
            },
        );
        *neighborhoods.work_mut() = support_work.planar;
        let support_admission = support_admission?;
        match support_admission {
            support::SupportAdmission::Complete | support::SupportAdmission::Undecided
                if truncated =>
            {
                return Ok(ScheduleAdmission::RetryUndecided);
            }
            support::SupportAdmission::Undecided => {
                unreachable!("only truncated retries leave support undecided")
            }
            support::SupportAdmission::RefineUnreachableLayer {
                index,
                failure,
                span_mm,
            } => {
                return Ok(ScheduleAdmission::RefineUnreachableLayer {
                    index,
                    failure,
                    span_mm,
                });
            }
            support::SupportAdmission::Complete if probe.is_some() => {
                let startup_key = first_wall_key
                    .filter(|_| StartupWall::permits(input))
                    .filter(|_| {
                        sections
                            .last()
                            .is_some_and(|span| span.end as usize == schedule.len())
                    });
                let startup_layout = startup_key.and_then(|key| wall_cache.remove(&key));
                drop(wall_layouts);
                drop(wall_cache);
                // The handoff owns its layout alone; the memo keeps no copy.
                if let (Some(key), Some(layout)) = (startup_key, &startup_layout) {
                    wall_memo.forget(&sections[span_at(&sections, 0)].material, key.2, layout);
                }
                let startup_wall = startup_key
                    .zip(startup_layout)
                    .filter(|(_, layout)| Arc::strong_count(layout) == 1)
                    .map(|(key, layout)| StartupWall::new(key, layout, &schedule[0]));
                return Ok(ScheduleAdmission::PrefixAdmitted(NativeScheduleCapture {
                    sections: Vec::new(),
                    native_surfaces: Vec::new(),
                    retained_section_bytes,
                    retained_points: native_retained_points,
                    startup_wall,
                }));
            }
            support::SupportAdmission::Complete => {}
        }

        let coordinate_scratch = skin_retained_points(retained_points, &neighborhoods, &[])?;
        let coordinate_owned_bytes = skin_retained_bytes(
            geometry_bytes(&[support_outside_bytes, layer_payload_bytes(&layers)?])?,
            &neighborhoods,
            &[],
        )?;
        zones::diaphragm::construct_coordinates(
            &mut layers,
            classification::fixed_point_budget(coordinate_scratch, coordinate_owned_bytes),
            neighborhoods.work_mut(),
            cancel,
        )?;
        construction.attribute(&mut layers, neighborhoods.work_mut(), cancel)?;
        drop_unprintable_rows(&mut layers)?;
        exposure::mark_exposed_top(&mut layers, &sections, cancel)?;
        support::object_volumes(&mut layers, input.intent)?;
        for layer in &mut layers {
            bonds::refresh_witness_kinds(layer)?;
        }
        if process_captures
            .iter()
            .map(|capture| capture.operations.len())
            .sum::<usize>()
            > MAX_PROCESS_REGIONS
        {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "process resolution output limit exceeded",
            ));
        }
        let mut result = NormalizedPrintPlan {
            routing_tool_orders: if matches!(
                input.intent.printer.workflow,
                JobWorkflow::FixedDualMarlin { .. }
            ) {
                vec![[0, 1]; layers.len()]
            } else {
                Vec::new()
            },
            interior_selection: None,
            functional_material_frame: construction.functional_frame.take(),
            material_frames: std::mem::take(&mut construction.frames),
            material_domains: std::mem::take(&mut construction.stored),
            schema_version: MANUFACTURING_SCHEMA_VERSION,
            sparse_bands: Vec::new(),
            process_resolution: process_captures
                .iter()
                .flat_map(|capture| {
                    capture
                        .operations
                        .iter()
                        .map(move |&provenance| ProcessRegionResolution {
                            occurrence_id: capture.occurrence_id,
                            definition_id: input
                                .occurrences
                                .iter()
                                .find(|o| o.occurrence_id == capture.occurrence_id)
                                .expect("captured occurrence")
                                .definition_id
                                .clone(),
                            provenance,
                            status: if capture.applied
                                && capture
                                    .zone
                                    .is_some_and(|zone| zone.provenance == provenance)
                            {
                                if capture.zone.is_some_and(|zone| {
                                    zone.values[0].fraction == zone.values[1].fraction
                                }) {
                                    ProcessRegionStatus::NoDensityChange
                                } else {
                                    ProcessRegionStatus::Applied
                                }
                            } else {
                                ProcessRegionStatus::NoEligibleInterior
                            },
                        })
                })
                .collect(),
            native_geometry: NativeGeometryCoverageSummary {
                requested_shape_error_mm: recipe.native_shape_error_mm(),
                achieved_shape_error_mm: spiling_geometry::tolerant::TolerantPolicy::default()
                    .profile_mm
                    .min(recipe.native_shape_error_mm()),
                height_analysis_count: 0,
                slab_count: 0,
                certified_layer_count: count,
                event_count: admission.horizontal_events.len() as u32,
                geometry_work: admission.geometry_work.consumed(),
                height_merge_mm: input.intent.printer.motion_resolution.z_mm,
                evidence: PreflightEvidence::Tolerant,
                skin_coverage: SkinCoverageScope::ScheduledMaterialDistance,
                support_clearance: SupportClearanceScope::ContinuousTargetOuterOccupancy,
                source_feature_coverage: NativeFeatureCoverageSummary {
                    disjoint_source_layer_count: 0,
                    invariant_source_union_layer_count: 0,
                    native_interface_complex_layer_count: 0,
                    owned_critical_slab_count: 0,
                },
            },
            layers,
            surface_resolution,
            sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
            offset_tolerance_mm: OFFSET_TOLERANCE_MM,
            surface_tolerance_mm: surface::SURFACE_TOLERANCE_MM,
            layer_quantization_mm: LAYER_QUANTIZATION_MM,
        };
        if let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow {
            let job = job.router()?;
            for layer in &mut result.layers {
                let mut right = layer.paths.len();
                let mut left = 0;
                while left < right {
                    check_cancel(cancel)?;
                    if job.tool_for_path(&layer.paths[left])? == 0 {
                        left += 1;
                    } else {
                        right -= 1;
                        layer.paths.swap(left, right);
                    }
                }
            }
        }
        result.validate(input.intent)?;

        combined::combine(
            &mut result,
            input.intent,
            &sections,
            schedule,
            &wall_layouts,
            &mut neighborhoods,
            retained_points,
            support_outside_bytes,
            &admission.models,
            cancel,
        )?;
        result.native_geometry.geometry_work = admission.geometry_work.consumed();
        result.validate(input.intent)?;

        Ok(ScheduleAdmission::Complete(result))
    })();
    *surface_work = *neighborhoods.work_mut();
    drop(neighborhoods);
    match outcome {
        Ok(ScheduleAdmission::PrefixAdmitted(mut capture)) => {
            // Query caches borrow D; transfer the exact owners only after
            // those borrows end. No source/M/finish buffers are reconstructed.
            capture.sections = sections;
            capture.native_surfaces = native_surfaces;
            Ok(ScheduleAdmission::PrefixAdmitted(capture))
        }
        outcome => outcome,
    }
}

struct SectionSpan {
    start: u32,
    end: u32,
    material: Arc<MultiPolygon<f64>>,
    process_domains: Vec<ProcessDomain>,
    // None borrows exact all-Z invariant D. Otherwise this separately owned
    // outer envelope is only corridor exclusion, never deposited-anchor evidence.
    occupied: Option<Arc<ContinuousOccupied>>,
    source_area_mm2: f64,
}

struct ProcessDomain {
    material: Arc<MultiPolygon<f64>>,
    coverage: ProcessRegionCoverage,
    bytes: usize,
    zone: Option<zones::Zone>,
    physical_z_mm: [f64; 2],
}

fn process_domain(
    input: &CompilerInput<'_>,
    zone: Option<zones::Zone>,
    occurrence_id: spiling_contracts::geometry::OccurrenceId,
    material: Arc<MultiPolygon<f64>>,
    bytes: usize,
    physical_z_mm: [f64; 2],
) -> Result<ProcessDomain, ManufacturingError> {
    let occurrence = input
        .occurrences
        .iter()
        .find(|o| o.occurrence_id == occurrence_id)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::InvalidSpecification,
                "process contribution lost captured occurrence",
            )
        })?;
    let value = input.process_document.resolve(
        &occurrence.definition_id,
        occurrence_id,
        input.intent.recipe.infill_fraction,
    );
    Ok(ProcessDomain {
        material,
        coverage: ProcessRegionCoverage {
            functional_solid_zone: None,
            material_domain: None,
            area_mm2: 0.0,
            fraction: value.fraction,
            contributions: vec![ProcessRegionContribution {
                occurrence_id,
                definition_id: occurrence.definition_id.clone(),
                provenance: value.provenance,
            }],
        },
        bytes,
        zone,
        physical_z_mm,
    })
}

fn process_extra_points(
    domains: &[ProcessDomain],
    material: &Arc<MultiPolygon<f64>>,
    prior: &[SectionSpan],
) -> usize {
    domains
        .iter()
        .enumerate()
        .filter(|(index, domain)| {
            !Arc::ptr_eq(&domain.material, material)
                && !domains[..*index]
                    .iter()
                    .any(|other| Arc::ptr_eq(&domain.material, &other.material))
                && !prior.iter().any(|span| {
                    Arc::ptr_eq(&domain.material, &span.material)
                        || span
                            .process_domains
                            .iter()
                            .any(|other| Arc::ptr_eq(&domain.material, &other.material))
                })
        })
        .map(|(_, domain)| region::point_count(&domain.material))
        .sum()
}

fn validate_process_overlap(
    domains: &[ProcessDomain],
    interval: [f64; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    use geo::{BoundingRect, Intersects};
    for (index, a) in domains.iter().enumerate() {
        for b in &domains[..index] {
            check_cancel(cancel)?;
            let spatial = a
                .zone
                .is_some_and(|zone| zone.values[0].fraction != zone.values[1].fraction)
                || b.zone
                    .is_some_and(|zone| zone.values[0].fraction != zone.values[1].fraction);
            if !spatial && a.coverage.fraction == b.coverage.fraction {
                continue;
            }
            let (Some(a_bounds), Some(b_bounds)) =
                (a.material.bounding_rect(), b.material.bounding_rect())
            else {
                continue;
            };
            if !a_bounds.intersects(&b_bounds) {
                continue;
            }
            let overlap =
                classification::intersection_accumulated(&a.material, &b.material, work, cancel)?;
            if overlap.unsigned_area() > 0.0 {
                return Err(error(
                    ManufacturingErrorCode::InvalidSpecification,
                    format!(
                        "overlapping process profiles for occurrences {} and {} in final-D interval [{},{}] mm require unsupported spatial composition (including interface neighbors/extent/construction)",
                        a.coverage.contributions[0].occurrence_id.get(),
                        b.coverage.contributions[0].occurrence_id.get(),
                        interval[0],
                        interval[1],
                    ),
                ));
            }
        }
    }
    Ok(())
}

impl SectionSpan {
    fn visit_occupied(
        &self,
        cancel: &AtomicBool,
        visit: impl FnMut(&geo::Polygon<f64>, Option<[f64; 2]>) -> Result<bool, ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        self.visit_occupied_screened(cancel, |_, _| Ok(true), visit)
    }

    /// `screen` sees each piece's XY bounds and height scope before the piece
    /// is constructed; `Ok(false)` skips it. Only a consumer that provably
    /// ignores the piece may decline it; proofs must not.
    fn visit_occupied_screened(
        &self,
        cancel: &AtomicBool,
        mut screen: impl FnMut([[f64; 2]; 2], Option<[f64; 2]>) -> Result<bool, ManufacturingError>,
        mut visit: impl FnMut(&geo::Polygon<f64>, Option<[f64; 2]>) -> Result<bool, ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        use geo::BoundingRect;
        use std::ops::ControlFlow;
        let rect = |polygon: &geo::Polygon<f64>| {
            polygon.bounding_rect().map_or([[f64::NAN; 2]; 2], |b| {
                [[b.min().x, b.min().y], [b.max().x, b.max().y]]
            })
        };
        if let Some(outer) = &self.occupied {
            for piece in &outer.pieces {
                check_cancel(cancel)?;
                if !screen(rect(&piece.polygon), Some(piece.height_mm))? {
                    continue;
                }
                if !visit(&piece.polygon, Some(piece.height_mm))? {
                    return Ok(());
                }
            }
            for source in &outer.tolerant {
                let result = source
                    .model
                    .visit_support_projection_screened(
                        source.interval,
                        cancel,
                        |bounds, height| match screen(bounds, Some(height)) {
                            Ok(keep) => ControlFlow::Continue(keep),
                            Err(error) => ControlFlow::Break(Err(error)),
                        },
                        |piece| match visit(&tolerant::polygon(piece.points), Some(piece.height_mm))
                        {
                            Ok(true) => ControlFlow::Continue(()),
                            Ok(false) => ControlFlow::Break(Ok(())),
                            Err(error) => ControlFlow::Break(Err(error)),
                        },
                    )
                    .map_err(|e| native_error(e, "tolerant continuous support enclosure"))?;
                if let ControlFlow::Break(result) = result {
                    return result;
                }
            }
        } else {
            for polygon in &self.material.0 {
                check_cancel(cancel)?;
                if !screen(rect(polygon), None)? {
                    continue;
                }
                if !visit(polygon, None)? {
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Independent raw conservative operands retain their own certified height
/// scope. In particular an endpoint sheet is not extruded through an otherwise
/// empty slab merely because its XY projection is nonempty.
#[derive(Clone, Default)]
struct ContinuousOccupied {
    pieces: Vec<OccupiedPiece>,
    tolerant: Vec<TolerantOccupied>,
    point_count: usize,
    payload_bytes: usize,
}

// Borrow prepared source geometry across layers; expanded facet arrays are
// query scratch, never a retained copy of the source for each scheduled slab.
#[derive(Clone)]
struct TolerantOccupied {
    model: Arc<spiling_geometry::tolerant::TolerantModel>,
    interval: [f64; 2],
}

#[derive(Clone)]
struct OccupiedPiece {
    polygon: geo::Polygon<f64>,
    height_mm: [f64; 2],
}

impl ContinuousOccupied {
    fn push(
        &mut self,
        mut polygon: geo::Polygon<f64>,
        height_mm: [f64; 2],
    ) -> Result<(), ManufacturingError> {
        let points = polygon.exterior().0.len()
            + polygon
                .interiors()
                .iter()
                .map(|ring| ring.0.len())
                .sum::<usize>();
        self.point_count = self
            .point_count
            .checked_add(points)
            .filter(|n| *n <= crate::working_set::units())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "continuous occupied piece point budget exceeded",
                )
            })?;
        self.pieces.try_reserve(1).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "continuous occupied piece allocation refused",
            )
        })?;
        self.payload_bytes = self
            .payload_bytes
            .saturating_add(native::polygon_bytes(&mut polygon));
        self.pieces.push(OccupiedPiece { polygon, height_mm });
        Ok(())
    }
}

fn native_error(
    e: spiling_contracts::geometry::GeometryError,
    context: &str,
) -> ManufacturingError {
    let code = match e.code {
        GeometryErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
        GeometryErrorCode::ResourceLimit => ManufacturingErrorCode::ResourceLimit,
        GeometryErrorCode::KernelFailure => ManufacturingErrorCode::NativeKernelFailure,
        GeometryErrorCode::ConditioningFailure => ManufacturingErrorCode::NativeConditioningFailure,
        GeometryErrorCode::AnalysisIndeterminate => {
            ManufacturingErrorCode::NativeAnalysisIndeterminate
        }
        GeometryErrorCode::SingularGeometry => ManufacturingErrorCode::NativeSingularGeometry,
        GeometryErrorCode::ShapeErrorExceeded => ManufacturingErrorCode::NativeShapeErrorExceeded,
        _ => ManufacturingErrorCode::UnsupportedGeometry,
    };
    // Keep the native diagnostic category in the bounded downstream message.
    error(code, format!("{context}: {e}"))
}

#[cfg_attr(test, derive(Debug, PartialEq))]
struct LayerElevation {
    z: f64,
    lower_z: f64,
    sample: f64,
    height: f64,
    /// Nominal layer this interval belongs to. The base schedule numbers its
    /// layers bottom-up; a later split divides one course into thinner
    /// layers that keep it, so alternating hatch axes are a function of
    /// physical position, never of a shifting output index.
    course: u32,
}

// No actual scheduled support top can fit beneath these object floors. This
// derives the probe from the declared physical gap and XYZ6 bead intervals,
// rather than a layer count, nozzle fraction or arbitrary near-bed distance.
fn unreachable_support_end(
    schedule: &[LayerElevation],
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let SupportPolicy::Breakaway { z_gap_mm, .. } = recipe.supports else {
        return Ok(0);
    };
    let Some(first) = schedule.first() else {
        return Ok(0);
    };
    let earliest_top = project_xyz6_bead_height_mm(first.z, first.z - first.lower_z)?[1];
    for (index, layer) in schedule.iter().enumerate().skip(1) {
        preflight::charge_capture_proposal_work(work, 1, cancel)?;
        let floor = project_xyz6_bead_height_mm(layer.z, layer.z - layer.lower_z)?[0];
        if program_coordinates::enclosed_vertical_gap_mm(floor, earliest_top)? >= z_gap_mm {
            return Ok(index);
        }
    }
    Ok(schedule.len())
}

#[derive(Clone, Copy)]
enum AnchorProbeScope {
    UnreachableSupport,
    StartupPair {
        top_mm: f64,
    },
    /// Output layers below `end` after a complete-pass refinement. Support
    /// keeps complete-pass semantics; only a refinement it finds is a decision.
    RefinedLayer {
        end: usize,
    },
}

impl AnchorProbeScope {
    fn end(
        self,
        schedule: &[LayerElevation],
        recipe: &PlanarPrintRecipe,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        match self {
            Self::UnreachableSupport => unreachable_support_end(schedule, recipe, work, cancel),
            Self::StartupPair { top_mm } => {
                preflight::charge_capture_proposal_work(work, 1, cancel)?;
                Ok(schedule.partition_point(|layer| layer.lower_z < top_mm))
            }
            Self::RefinedLayer { end } => Ok(end.min(schedule.len())),
        }
    }
}

#[derive(Clone, Copy)]
struct DepositionProbe {
    native_top_mm: f64,
    scope: AnchorProbeScope,
}

fn deposition_probe(
    input: &CompilerInput<'_>,
    process: &[zones::Capture],
    schedule: &[LayerElevation],
    scope: AnchorProbeScope,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<DepositionProbe>, ManufacturingError> {
    let end = scope.end(schedule, &input.intent.recipe, work, cancel)?;
    if end <= 1 {
        return Ok(None);
    }
    // Preserve the complete existing upper scheduled-D skin horizon. Lower
    // horizons begin at the bed; finishes use their unchanged captured fields.
    let mut after = physical_neighbor(
        schedule,
        (end - 1) as u32,
        input.intent.recipe.top_skin_thickness_mm,
        true,
    )
    .map_or(schedule.len(), |index| index as usize + 1);
    let output_top = schedule[end - 1].z;
    for capture in process {
        preflight::charge_capture_proposal_work(work, 1, cancel)?;
        if let Some(plate) = capture.zone.and_then(|zone| zone.diaphragm)
            && plate.physical_z_mm[0] < output_top
        {
            after =
                after.max(schedule.partition_point(|layer| layer.lower_z < plate.physical_z_mm[1]));
        }
    }
    Ok(Some(DepositionProbe {
        native_top_mm: schedule[after - 1].z,
        scope,
    }))
}

/// Alternating-axis key of `schedule[index]`. A layer outside refinement
/// uses its course. Within a maximal run of layers whose courses were split,
/// the key counts down from the run's top layer, which keeps its course axis:
/// every layer inside the run crosses its neighbors, the course above still
/// crosses the run's top, and a split changes no layer above its run. Plan
/// validation and replay derive the same key independently from the
/// published courses.
fn hatch_key(schedule: &[LayerElevation], index: usize) -> u32 {
    let refined = |layer: usize| {
        let course = schedule[layer].course;
        layer
            .checked_sub(1)
            .is_some_and(|below| schedule[below].course == course)
            || schedule
                .get(layer + 1)
                .is_some_and(|above| above.course == course)
    };
    if !refined(index) {
        return schedule[index].course;
    }
    let mut top = index;
    while top + 1 < schedule.len() && refined(top + 1) {
        top += 1;
    }
    schedule[top].course + (top - index) as u32
}

/// Refine the schedule at an unreachable layer. When it can be refined no
/// further and supports are enabled, mark the layer exhausted so the retry
/// prints its overhang as authored; a second request for it still refuses.
///
/// A split can anchor a stroke only by moving the section below it closer in
/// Z, which shortens the stagger it bridges. When an uncertified corridor
/// returns inside an interval already split for one and its unanchored span
/// is no shorter by more than the profile tolerance, the split moved nothing
/// the stroke needed: the first layer above a flat ceiling, or a stroke
/// across a sparse gap below, stays unanchored however thin the layers get.
/// Such a layer is exhausted at once rather than halved toward the minimum
/// bead height.
fn exhaust_or_refine(
    schedule: &mut Vec<LayerElevation>,
    index: usize,
    input: &CompilerInput<'_>,
    failure: ManufacturingError,
    span_mm: Option<f64>,
    support_work: &mut support::SupportWork,
) -> Result<(), ManufacturingError> {
    let z = schedule.get(index).map(|layer| layer.z.to_bits());
    let futile = span_mm.is_some_and(|span| {
        schedule.get(index).is_some_and(|layer| {
            support_work
                .anchor_refinements
                .iter()
                .rev()
                .find(|split| split.interval_mm[0] < layer.z && layer.z <= split.interval_mm[1])
                .is_some_and(|split| span > split.span_mm - ANCHOR_REFINEMENT_PROGRESS_MM)
        })
    });
    let interval = index
        .checked_sub(1)
        .filter(|_| index < schedule.len())
        .map(|below| [schedule[below].lower_z, schedule[index].z]);
    let refined = if futile {
        Err(failure)
    } else {
        refine_deposition_pair(schedule, index, input, failure)
    };
    match refined {
        Ok(()) => {
            if let (Some(span_mm), Some(interval_mm)) = (span_mm, interval) {
                support_work
                    .anchor_refinements
                    .try_reserve(1)
                    .map_err(|_| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "actual-anchor refinement record allocation refused",
                        )
                    })?;
                support_work
                    .anchor_refinements
                    .push(support::AnchorRefinement {
                        interval_mm,
                        span_mm,
                    });
            }
            Ok(())
        }
        Err(failure)
            if failure.code != ManufacturingErrorCode::ResourceLimit
                && failure.code != ManufacturingErrorCode::Cancelled
                && !matches!(input.intent.recipe.supports, SupportPolicy::Disabled) =>
        {
            match z {
                Some(z) if support_work.exhausted.insert(z) => Ok(()),
                _ => Err(failure),
            }
        }
        Err(failure) => Err(failure),
    }
}

/// Section changes below the ADR012 profile tolerance are not geometric
/// progress for an actual-anchor split.
const ANCHOR_REFINEMENT_PROGRESS_MM: f64 = 0.01;

fn refine_deposition_pair(
    schedule: &mut Vec<LayerElevation>,
    index: usize,
    input: &CompilerInput<'_>,
    failure: ManufacturingError,
) -> Result<(), ManufacturingError> {
    if index == 0 || index >= schedule.len() {
        return Err(failure);
    }
    let eligible = |layer: &LayerElevation| {
        let midpoint = layer.lower_z + (layer.z - layer.lower_z) * 0.5;
        [midpoint - layer.lower_z, layer.z - midpoint]
            .into_iter()
            .all(|height| {
                height > 2.0 * spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM
                    && input
                        .intent
                        .admits_deposition(input.intent.recipe.bead_width_mm, height)
            })
    };
    let lower = eligible(&schedule[index - 1]);
    let upper = eligible(&schedule[index]);
    if !lower && !upper {
        return Err(failure);
    }
    if schedule.len() >= crate::working_set::records::<LayerElevation>() {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "actual-anchor schedule refinement exceeds the manufacturing working set",
        ));
    }
    // Add only one candidate boundary. Splitting the thicker eligible interval
    // reduces the stagger between actual midpoints most per added layer; ties
    // preserve the already-admitted lower layer. This proposes, never proves,
    // eligibility, and preserves every existing physical/event endpoint.
    let lower_height = schedule[index - 1].z - schedule[index - 1].lower_z;
    let upper_height = schedule[index].z - schedule[index].lower_z;
    let target = if lower && (!upper || lower_height > upper_height) {
        index - 1
    } else {
        index
    };
    native::split_layer(
        schedule,
        target as u32,
        "actual deposited-anchor eligibility",
    )?;
    Ok(())
}

fn image_support_contact_floor(
    event: f64,
    z_gap_mm: f64,
    nominal_height_mm: f64,
) -> Result<f64, ManufacturingError> {
    let scale = event.abs().max(nominal_height_mm).max(z_gap_mm).max(1.0);
    let arithmetic = (8.0 * f64::EPSILON * scale).next_up();
    let half_quantum = (0.5 / 1_000_000.0f64).next_up();
    let lower = (((event + half_quantum).next_up() + arithmetic).next_up() + z_gap_mm).next_up();
    let mut floor = project_xyz6_coordinate_mm(lower + half_quantum)?;
    if project_xyz6_bead_height_mm(floor, 0.0)?[0] < lower {
        floor = project_xyz6_coordinate_mm(floor + 1.0 / 1_000_000.0)?;
    }
    if !floor.is_finite() || project_xyz6_bead_height_mm(floor, 0.0)?[0] < lower {
        return Err(error(
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
            "image contact floor cannot enclose the emitter's coordinate representation",
        ));
    }
    Ok(floor)
}

fn scheduled_layers(
    input: &CompilerInput<'_>,
    admission: &mut preflight::Admission,
    process: &[zones::Capture],
    cancel: &AtomicBool,
) -> Result<Vec<LayerElevation>, ManufacturingError> {
    let bed = input.intent.printer.build_envelope.min[2];
    // Upper bracket endpoints are only representable schedule bookkeeping.
    // Every accepted actual slab, including its assigned critical neighborhood,
    // must certify source/prism metric and incidence before any paths are built.
    let top = *admission.horizontal_events.last().ok_or_else(|| {
        error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "actual native terminal height has not been certified",
        )
    })?;
    let recipe = &input.intent.recipe;
    let mut events = admission.horizontal_events.clone();
    joins::append_z_events(input, admission, &mut events, cancel)?;
    events.sort_by(f64::total_cmp);
    events.dedup_by(|a, b| *a == *b);
    let mut row_events = Vec::new();
    let mut image_contact_floors = Vec::new();
    for fields in admission.surface_fields.values() {
        for field in fields {
            row_events.clear();
            field.append_z_events(
                bed,
                top,
                &mut row_events,
                crate::working_set::records::<LayerElevation>(),
                cancel,
            )?;
            // Equal row mappings on repeated occurrences must not consume the
            // event budget repeatedly. Reuse the native event insertion pattern.
            for event in row_events.drain(..) {
                check_cancel(cancel)?;
                if let SupportPolicy::Breakaway { z_gap_mm, .. } = recipe.supports {
                    let floor = image_support_contact_floor(
                        event,
                        z_gap_mm,
                        input.intent.select_layer_height()?.height_mm,
                    )?;
                    if floor > bed
                        && floor < top
                        && let Err(position) =
                            image_contact_floors.binary_search_by(|old: &f64| old.total_cmp(&floor))
                    {
                        if events.len() + image_contact_floors.len()
                            >= crate::working_set::records::<f64>()
                        {
                            return Err(error(
                                ManufacturingErrorCode::ResourceLimit,
                                "combined native/image/contact events exceed the manufacturing working set",
                            ));
                        }
                        image_contact_floors.insert(position, floor);
                    }
                }
                if let Err(position) = events.binary_search_by(|old| old.total_cmp(&event)) {
                    if events.len() >= crate::working_set::records::<f64>() {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "combined native/image events exceed the manufacturing working set",
                        ));
                    }
                    events.insert(position, event);
                }
            }
        }
    }
    if let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow {
        for material in job
            .assignments
            .iter()
            .map(|a| &a.material)
            .chain(job.surface_constructions.iter().map(|c| &c.shell))
        {
            let mut previous = bed;
            let mut visited = 0usize;
            while let Some(boundary) = material.next_boundary_after(previous)? {
                check_cancel(cancel)?;
                let identical_to_top = (boundary - top).abs()
                    <= 4.0 * f64::EPSILON * boundary.abs().max(top.abs()).max(1.0);
                if boundary >= top || identical_to_top {
                    break;
                }
                if boundary <= previous {
                    return Err(error(
                        ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
                        "physical material-cycle boundary cannot advance the native schedule",
                    ));
                }
                visited += 1;
                if visited > crate::working_set::records::<f64>() {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "material-cycle event work exceeds the manufacturing working set",
                    ));
                }
                if let Err(position) = events.binary_search_by(|old| old.total_cmp(&boundary)) {
                    if events.len() >= crate::working_set::records::<f64>() {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "combined native/field/material events exceed the manufacturing working set",
                        ));
                    }
                    events.insert(position, boundary);
                }
                previous = boundary;
            }
        }
    }
    if let SupportPolicy::Breakaway {
        z_gap_mm,
        interface_thickness_mm,
        ..
    } = recipe.supports
    {
        // A nominal floor-gap event can round UP past the physical minimum.
        // Reserve the emitter projection and height-construction arithmetic,
        // then put the support stop on the actual XYZ6 lattice. This is an
        // output-representation enclosure, not an extra geometry tolerance.
        // Source approximation increases the contact-selection minimum, not
        // the complete source/event-derived schedule. Contact selection can
        // choose an earlier admissible top without introducing short source
        // intervals that change otherwise supported object construction.
        row_events.clear();
        for &event in &events {
            check_cancel(cancel)?;
            // Material that only ends facing up (a top face, the part's top)
            // carries nothing above it that support could hold.
            if admission
                .upward_only_events
                .binary_search_by(|z| z.total_cmp(&event))
                .is_ok()
            {
                continue;
            }
            let scale = event
                .abs()
                .max(input.intent.select_layer_height()?.height_mm)
                .max(z_gap_mm)
                .max(1.0);
            let arithmetic = (8.0 * f64::EPSILON * scale).next_up();
            let half_quantum = (0.5 / 1_000_000.0f64).next_up();
            let upper = (((event - half_quantum).next_down() - arithmetic).next_down() - z_gap_mm)
                .next_down();
            if upper <= bed {
                continue;
            }
            let mut stop = project_xyz6_coordinate_mm(upper - half_quantum)?;
            if project_xyz6_bead_height_mm(stop, 0.0)?[1] > upper {
                stop = project_xyz6_coordinate_mm(stop - 1.0 / 1_000_000.0)?;
            }
            if !stop.is_finite() || project_xyz6_bead_height_mm(stop, 0.0)?[1] > upper {
                return Err(error(
                    ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
                    "support stop cannot enclose the emitter's coordinate representation",
                ));
            }
            for boundary in [stop, stop - interface_thickness_mm] {
                if boundary > bed && boundary < top {
                    if row_events.len() + events.len() >= crate::working_set::records::<f64>() {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "combined native/field/support events exceed the manufacturing working set",
                        ));
                    }
                    row_events.push(boundary);
                }
            }
        }
        if row_events.len() + image_contact_floors.len() + events.len()
            > crate::working_set::records::<f64>()
        {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "combined native/field/support/contact events exceed the manufacturing working set",
            ));
        }
        row_events.append(&mut image_contact_floors);
        row_events.sort_by(f64::total_cmp);
        row_events.dedup_by(|a, b| *a == *b);
        for stop in row_events.drain(..) {
            check_cancel(cancel)?;
            let Err(position) = events.binary_search_by(|old| old.total_cmp(&stop)) else {
                continue;
            };
            let below = if position == 0 {
                bed
            } else {
                events[position - 1]
            };
            let above = if position == events.len() {
                top
            } else {
                events[position]
            };
            let minimum = 2.0 * spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM;
            // Never move a true native/field event or manufacture a sub-process
            // layer to obtain a closer support contact. Retain the existing
            // partition and select its latest provably safe top instead; the
            // declared Z gap is a minimum, so the actual gap can be larger.
            if stop - below <= minimum || above - stop <= minimum {
                continue;
            }
            events.insert(position, stop);
        }
    }
    let geometry_event_count = events.len();
    for zone in process.iter().filter_map(|capture| capture.zone) {
        if zone.axis == 2 {
            events.extend(zone.z_events());
        }
    }
    let proposal = goal_layer_schedule(input.intent, bed, top, &events)
        .and_then(|schedule| refine_surface_schedule(schedule, &admission.surface_fields, cancel))
        .and_then(|schedule| {
            if schedule.iter().any(|layer| {
                !input
                    .intent
                    .admits_deposition(recipe.bead_width_mm, layer.z - layer.lower_z)
            }) {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "actual schedule intervals exceed the declared deposition height limits",
                ));
            }
            Ok(schedule)
        });
    match proposal {
        Err(failure)
            if failure.code == ManufacturingErrorCode::UnsupportedGeometry
                && process
                    .iter()
                    .any(|capture| capture.zone.is_some_and(|zone| zone.diaphragm.is_some())) =>
        {
            Err(zones::diaphragm::schedule_diagnostic(
                input,
                admission,
                process,
                &events[..geometry_event_count],
                failure,
                cancel,
            )?)
        }
        result => result,
    }
}

fn goal_layer_schedule(
    intent: &ManufacturingIntent,
    bed: f64,
    top: f64,
    events: &[f64],
) -> Result<Vec<LayerElevation>, ManufacturingError> {
    let nominal = intent.select_layer_height()?.height_mm;
    let limits = intent.effective_deposition_limits();
    let (minimum, maximum) = match intent.recipe.layer_height_policy {
        LayerHeightPolicy::Preferred => (
            limits.min_bead_height_mm,
            limits.max_bead_height_mm.min(intent.recipe.bead_width_mm),
        ),
        LayerHeightPolicy::ProtectedMaximum => (0.0, nominal),
    };
    let merge_mm = intent.printer.motion_resolution.z_mm;
    let first = intent.recipe.first_layer_height_mm;
    let schedule =
        layer_schedule_with_bounds(bed, top, nominal, minimum, maximum, events, merge_mm, first)?;
    let command_admitted = |layer: &LayerElevation| -> Result<bool, ManufacturingError> {
        Ok(
            intent.admits_deposition(intent.recipe.bead_width_mm, layer.z - layer.lower_z)
                && intent.admits_deposition(
                    intent.recipe.bead_width_mm,
                    project_xyz6_coordinate_mm(layer.z)?
                        - project_xyz6_coordinate_mm(layer.lower_z)?,
                ),
        )
    };
    for layer in &schedule {
        if !command_admitted(layer)? {
            // A protected nominal cap tolerates only reporting roundoff. The
            // true interval and XYZ6 command still obey the captured physical
            // envelope. Rebalance between the same protected events using the
            // existing neighboring-allocation check; never widen either limit.
            return layer_schedule_with_bounds(
                bed,
                top,
                nominal,
                limits.min_bead_height_mm,
                maximum
                    .min(limits.max_bead_height_mm)
                    .min(intent.recipe.bead_width_mm),
                events,
                merge_mm,
                first,
            );
        }
    }
    Ok(schedule)
}

/// The software reference printers' declared Z step.
#[cfg(test)]
const REFERENCE_Z_STEP_MM: f64 = 0.0025;
#[cfg(test)]
fn layer_schedule(
    bed: f64,
    top: f64,
    nominal: f64,
    events: &[f64],
) -> Result<Vec<LayerElevation>, ManufacturingError> {
    layer_schedule_with_bounds(
        bed,
        top,
        nominal,
        0.0,
        nominal,
        events,
        REFERENCE_Z_STEP_MM,
        None,
    )
}

/// `first`, when set, is the first layer's height above the bed: one
/// boundary there, and that interval is one layer (a native event inside it
/// still splits it).
#[expect(
    clippy::too_many_arguments,
    reason = "one schedule over explicit captured bounds; no hidden policy struct"
)]
fn layer_schedule_with_bounds(
    bed: f64,
    top: f64,
    nominal: f64,
    minimum: f64,
    maximum: f64,
    events: &[f64],
    merge_mm: f64,
    first: Option<f64>,
) -> Result<Vec<LayerElevation>, ManufacturingError> {
    // ADR012: heights closer than the printer's Z step are one commanded
    // height. The bed and top anchor; otherwise the lowest of a cluster stays,
    // so every scheduled layer is at least one Z step and the result does not
    // depend on event order. Arithmetic identity remains a subset of this.
    let roundoff = 4.0 * f64::EPSILON * bed.abs().max(top.abs()).max(1.0);
    let merge_mm = merge_mm.max(roundoff);
    if !merge_mm.is_finite() || top - bed <= merge_mm {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "part is not taller than one Z step of the selected printer",
        ));
    }
    let mut sorted = Vec::with_capacity(events.len().min(crate::working_set::records::<f64>()));
    for &event in events {
        if sorted.len() >= crate::working_set::records::<f64>() {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "horizontal native events exceed the layer budget",
            ));
        }
        if event.is_finite() && event - bed > merge_mm && top - event > merge_mm {
            sorted.push(event);
        }
    }
    sorted.sort_by(f64::total_cmp);
    // Admit genuine boundaries before growing either buffer. No nominal grid
    // participates in event identity or interval construction.
    let mut boundaries = Vec::with_capacity(
        sorted
            .len()
            .min(crate::working_set::records::<LayerElevation>() - 1)
            + 2,
    );
    boundaries.extend([bed, top]);
    let first_end = first
        .map(|height| bed + height)
        .filter(|z| top - *z > merge_mm);
    for event in first_end.into_iter().chain(sorted) {
        let position = boundaries.partition_point(|boundary| *boundary < event);
        if event - boundaries[position - 1] <= merge_mm || boundaries[position] - event <= merge_mm
        {
            continue;
        }
        if boundaries.len() > crate::working_set::records::<LayerElevation>() {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "horizontal native events exceed the layer budget",
            ));
        }
        boundaries.insert(position, event);
    }
    let subdivisions = |start: f64, end: f64| -> Result<usize, ManufacturingError> {
        let width = end - start;
        if start == bed && Some(end) == first_end && width <= maximum {
            return Ok(1);
        }
        let ratio = (width / nominal).floor().max(1.0);
        if !width.is_finite()
            || width <= 0.0
            || !nominal.is_finite()
            || nominal <= 0.0
            || !ratio.is_finite()
            || ratio > crate::working_set::records::<LayerElevation>() as f64
        {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "layer schedule exceeds the manufacturing working set",
            ));
        }
        let mut count = (ratio as usize).max(1);
        // ceil(width / nominal) can add an entire layer at an integral
        // arithmetic ratio. Check the actual candidate height instead, using
        // only bounded roundoff at the interval's construction scale.
        let roundoff = 4.0 * f64::EPSILON * start.abs().max(end.abs()).max(width.abs()).max(1.0);
        let candidate_height = width / count as f64;
        if candidate_height > nominal && candidate_height - nominal > roundoff {
            count += 1;
        }
        if minimum > 0.0 && width / (count as f64) < minimum {
            let possible = (width / minimum).floor();
            if possible < 1.0 || !possible.is_finite() {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "protected geometric boundaries leave no process-admissible layer",
                ));
            }
            count = count.min(possible as usize);
        }
        if minimum > 0.0 {
            // Test the intervals we will actually construct and project, not
            // merely width/count. Prefer an adjacent admissible allocation to
            // weakening the captured lower/upper process boundaries.
            let fits = |candidate: usize| -> Result<bool, ManufacturingError> {
                let mut previous = start;
                for index in 1..=candidate {
                    let z = if index == candidate {
                        end
                    } else {
                        start + width * (index as f64 / candidate as f64)
                    };
                    let height = z - previous;
                    let command_height =
                        project_xyz6_coordinate_mm(z)? - project_xyz6_coordinate_mm(previous)?;
                    if height < minimum
                        || height > maximum
                        || command_height < minimum
                        || command_height > maximum
                    {
                        return Ok(false);
                    }
                    previous = z;
                }
                Ok(true)
            };
            if !fits(count)? {
                if count > 1 && fits(count - 1)? {
                    count -= 1;
                } else if count < crate::working_set::records::<LayerElevation>()
                    && fits(count + 1)?
                {
                    count += 1;
                } else {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        "protected geometric boundaries have no neighboring command-admissible layer allocation",
                    ));
                }
            }
        }
        if width / count as f64 > maximum + roundoff {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "protected geometric boundaries cannot fit the captured layer-height envelope",
            ));
        }
        Ok(count)
    };
    let mut layer_count = 0;
    for interval in boundaries.windows(2) {
        layer_count += subdivisions(interval[0], interval[1])?;
        if layer_count > crate::working_set::records::<LayerElevation>() {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "layer schedule exceeds the manufacturing working set",
            ));
        }
    }
    let mut schedule = Vec::with_capacity(layer_count);
    for interval in boundaries.windows(2) {
        let start = interval[0];
        let end = interval[1];
        let count = subdivisions(start, end)?;
        // Interpolated endpoints carry the scale of the original interval,
        // even when a shifted-bed interval crosses Z=0. Local endpoint
        // magnitudes alone lose that arithmetic provenance near zero.
        let construction_roundoff =
            4.0 * f64::EPSILON * start.abs().max(end.abs()).max((end - start).abs()).max(1.0);
        let mut previous = start;
        // Balanced subdivisions cannot introduce a tiny remainder next to a
        // genuine event; the last endpoint remains exactly that event.
        for index in 1..=count {
            let z = if index == count {
                end
            } else {
                start + (end - start) * (index as f64 / count as f64)
            };
            let interval_height = z - previous;
            // Publish nominal height only for an upward subtraction excess
            // within the endpoint-construction roundoff bound. Keep exact Z
            // and the true midpoint; the strict published cap is unchanged.
            let height = if interval_height > nominal
                && interval_height - nominal <= construction_roundoff
            {
                nominal
            } else {
                interval_height
            };
            // Events closer than the feature tolerance were merged above, so a
            // layer this thin can only come from arithmetic, not geometry.
            if height <= 2.0 * spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    format!(
                        "required layer event at Z {z:.17e} above {previous:.17e} creates unresolved layer thickness {height:.17e} mm"
                    ),
                ));
            }
            let course = schedule.len() as u32;
            schedule.push(LayerElevation {
                z,
                lower_z: previous,
                sample: previous + interval_height * 0.5,
                height,
                course,
            });
            previous = z;
        }
    }
    Ok(schedule)
}

fn refine_surface_schedule(
    schedule: Vec<LayerElevation>,
    fields: &BTreeMap<spiling_contracts::geometry::OccurrenceId, Vec<surface::PreparedFinish>>,
    cancel: &AtomicBool,
) -> Result<Vec<LayerElevation>, ManufacturingError> {
    if fields.is_empty() {
        return Ok(schedule);
    }
    let mut layer_count = schedule.len();
    let mut pending = schedule;
    pending.reverse();
    let mut refined = Vec::new();
    while let Some(layer) = pending.pop() {
        check_cancel(cancel)?;
        let start = layer.lower_z;
        let mut split = false;
        for occurrence_fields in fields.values() {
            for field in occurrence_fields {
                if let Some(allowed) = field.allowed_vertical_error() {
                    split |= field.vertical_error_bound(start, layer.z, layer.sample)? > allowed;
                }
            }
        }
        if !split {
            // Finish-driven refinement belongs to the base schedule: each
            // resolved interval is its own course.
            refined.push(LayerElevation {
                course: refined.len() as u32,
                ..layer
            });
            continue;
        }
        let midpoint = layer.sample;
        let lower_height = midpoint - start;
        let upper_height = layer.z - midpoint;
        if lower_height.min(upper_height)
            <= 2.0 * spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                format!(
                    "image vertical error cannot be resolved in slab [{start:.9}, {:.9}] mm without unresolved layer thickness",
                    layer.z
                ),
            ));
        }
        layer_count += 1;
        if layer_count > crate::working_set::records::<LayerElevation>() {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "image vertical error refinement exceeds the manufacturing working set",
            ));
        }
        pending.push(LayerElevation {
            z: layer.z,
            lower_z: midpoint,
            sample: midpoint + upper_height * 0.5,
            height: upper_height,
            course: layer.course,
        });
        pending.push(LayerElevation {
            z: midpoint,
            lower_z: start,
            sample: start + lower_height * 0.5,
            height: lower_height,
            course: layer.course,
        });
    }
    Ok(refined)
}

fn physical_neighbor(
    schedule: &[LayerElevation],
    index: u32,
    thickness: f64,
    above: bool,
) -> Option<u32> {
    let i = index as usize;
    if above {
        let target = schedule[i].z + thickness;
        let j = schedule
            .partition_point(|layer| layer.z < target - LAYER_QUANTIZATION_MM)
            .max(i + 1);
        (j < schedule.len()).then_some(j as u32)
    } else {
        let bottom = schedule[i].lower_z;
        let target = bottom - thickness;
        if target < schedule[0].lower_z - LAYER_QUANTIZATION_MM {
            return None;
        }
        let j = schedule.partition_point(|layer| layer.z <= target + LAYER_QUANTIZATION_MM);
        Some(j as u32)
    }
}

#[cfg(feature = "roof-research")]
pub struct CathedralResearchColumns {
    pub roots: [Vec<f64>; 2],
    pub joints: Vec<(usize, f64, f64, f64)>,
}

#[cfg(feature = "roof-research")]
pub fn cathedral_research_columns(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<Option<CathedralResearchColumns>, ManufacturingError> {
    region::web_roof::synthesize(
        &plan.layers,
        recipe,
        layer_payload_bytes(&plan.layers)?,
        cancel,
    )
    .map(|columns| {
        columns.map(|columns| CathedralResearchColumns {
            roots: columns.roots,
            joints: columns.joints,
        })
    })
}

fn empty_coverage(_fraction: f64) -> PlanarRegionCoverage {
    PlanarRegionCoverage {
        web_roof_domains: Vec::new(),
        skin_bridge_domains: Vec::new(),
        process_diaphragms: Vec::new(),
        diaphragm_area_mm2: 0.0,
        section_area_mm2: 0.0,
        source_section_area_mm2: 0.0,
        surface_added_area_mm2: 0.0,
        separator_area_mm2: 0.0,
        process_interfaces: Vec::new(),
        wall_area_mm2: 0.0,
        bottom_skin_area_mm2: 0.0,
        top_skin_area_mm2: 0.0,
        solid_interior_area_mm2: 0.0,
        sparse_interior_area_mm2: 0.0,
        intentional_void_area_mm2: 0.0,
        protected_hole_area_mm2: 0.0,
        nominal_deposition_volume_mm3: 0.0,
        process_regions: Vec::new(),
        support_area_mm2: 0.0,
        support_nominal_deposition_volume_mm3: 0.0,
        unsupported_overhang_mm: 0.0,
        support_omitted_wall_mm: 0.0,
        thin_widened_mm: 0.0,
        thin_omitted_area_mm2: 0.0,
    }
}

/// For each span, the first span whose material is bit-identical (itself when
/// none earlier is). Hashing only buckets candidates; equality decides.
/// Walls are a pure function of material and width envelope (only their XY
/// geometry is shared; layer Z is applied later). Plan every distinct pair
/// the schedule will need, in parallel, so the ordered pass finds them in the
/// memo. A pair whose planning refuses is left for the ordered pass, which
/// reports it at its layer.
#[allow(clippy::too_many_arguments)]
fn prefill_walls(
    input: &CompilerInput<'_>,
    schedule: &[LayerElevation],
    sections: &[SectionSpan],
    native_surfaces: &[NativeSurfaceSpan],
    construction: &color::Construction,
    material_twin: &[usize],
    retained_points: usize,
    wall_memo: &mut WallMemo,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    use rayon::prelude::*;
    if !construction.domains.is_empty() {
        return Ok(());
    }
    let limits = input.intent.effective_deposition_limits();
    let mut seen = std::collections::HashSet::new();
    let mut wanted = Vec::new();
    // A probe pass sections only a prefix of the schedule.
    let covered = sections.last().map_or(0, |span| span.end as usize);
    for (index, elevation) in schedule.iter().enumerate().take(covered) {
        check_cancel(cancel)?;
        let index = index as u32;
        let current = span_at(sections, index);
        if sections[current].material.0.is_empty() {
            continue;
        }
        let native_index = native_surfaces.partition_point(|span| span.end <= index);
        let native_span = native_surfaces
            .get(native_index)
            .filter(|span| span.start <= index);
        if native_span.is_some_and(|span| !span.image_islands.is_empty()) {
            continue;
        }
        let actual_height = elevation.z - elevation.lower_z;
        let envelope = walls::width_envelope(&limits, actual_height)?;
        let key = [
            envelope[0].to_bits(),
            envelope[1].to_bits(),
            limits.min_bead_width_mm.max(actual_height).to_bits(),
        ];
        if wall_memo.find(&sections[current].material, key).is_some()
            || !seen.insert((material_twin[current], key))
        {
            continue;
        }
        wanted.push((index, current, native_index, key));
    }
    // Rayon workers do not inherit the operation's working-set scope, and
    // concurrent tasks are live together: each admits an equal share of the
    // growth beyond the already-retained geometry. A task refused for its share
    // only misses this memo; the serial layer pass retries with the whole set.
    let working_set = {
        let unit = std::mem::size_of::<geo::Coord<f64>>();
        let retained_bytes = retained_points.saturating_mul(unit);
        let growth = crate::working_set::bytes().saturating_sub(retained_bytes)
            / rayon::current_num_threads().max(1);
        crate::working_set::WorkingSet::new(retained_bytes.saturating_add(growth).max(1))?
    };
    let planned: Vec<_> = wanted
        .par_iter()
        .map(|&(index, current, native_index, key)| {
            crate::working_set::scope(working_set, || {
                let elevation = &schedule[index as usize];
                let native_span = native_surfaces
                    .get(native_index)
                    .filter(|span| span.start <= index);
                let mut work = 0usize;
                let plan =
                    required_material(&sections[current].material, native_span, &mut work, cancel)
                        .and_then(|required| {
                            construction.walls(
                                walls::WallRequest {
                                    material: &sections[current].material,
                                    required: &required,
                                    recipe: &input.intent.recipe,
                                    limits: &limits,
                                    nozzle_diameter_mm: input.intent.printer.nozzle_diameter_mm,
                                    lower_z: elevation.lower_z,
                                    upper_z: elevation.z,
                                    retained_points,
                                    owned_bytes: 0,
                                },
                                &mut work,
                                cancel,
                            )
                        });
                (current, key, plan.ok(), work)
            })
        })
        .collect();
    for (current, key, plan, spent) in planned {
        *work = work.saturating_add(spent);
        if let Some(plan) = plan {
            wall_memo.insert(&sections[current].material, key, &Arc::new(plan));
        }
    }
    check_cancel(cancel)
}

/// Two sections are the same shape when they differ by less than the
/// recorded profile tolerance along their boundary: sliced sections of one
/// prism can differ in ring start and redundant collinear vertices.
fn same_section(a: &MultiPolygon<f64>, b: &MultiPolygon<f64>) -> bool {
    use geo::{Area, BooleanOps, Euclidean, Length};
    if a == b {
        return true;
    }
    let perimeter: f64 =
        a.0.iter()
            .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
            .map(|ring| Euclidean.length(ring))
            .sum();
    let profile = spiling_geometry::tolerant::TolerantPolicy::default().profile_mm;
    a.0.len() == b.0.len() && a.xor(b).unsigned_area() <= profile * perimeter
}

fn material_hash(material: &MultiPolygon<f64>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for polygon in &material.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            ring.0.len().hash(&mut hasher);
            for c in &ring.0 {
                c.x.to_bits().hash(&mut hasher);
                c.y.to_bits().hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

type MemoEntry = (Arc<MultiPolygon<f64>>, [u64; 3], Arc<walls::WallPlan>);

/// Wall plans by bit-identical material and width envelope, kept across
/// schedule retries of one lane. Hashing only buckets; equality decides.
#[derive(Default)]
pub(super) struct WallMemo {
    plans: std::collections::HashMap<u64, Vec<MemoEntry>>,
}

impl WallMemo {
    fn find(
        &self,
        material: &Arc<MultiPolygon<f64>>,
        key: [u64; 3],
    ) -> Option<Arc<walls::WallPlan>> {
        self.plans
            .get(&material_hash(material))?
            .iter()
            .find(|(m, k, _)| *k == key && (Arc::ptr_eq(m, material) || m.0 == material.0))
            .map(|(_, _, plan)| Arc::clone(plan))
    }
    fn forget(
        &mut self,
        material: &Arc<MultiPolygon<f64>>,
        key: [u64; 3],
        plan: &Arc<walls::WallPlan>,
    ) {
        if let Some(bucket) = self.plans.get_mut(&material_hash(material)) {
            bucket.retain(|(_, k, p)| !(*k == key && Arc::ptr_eq(p, plan)));
        }
    }
    fn insert(
        &mut self,
        material: &Arc<MultiPolygon<f64>>,
        key: [u64; 3],
        plan: &Arc<walls::WallPlan>,
    ) {
        self.plans
            .entry(material_hash(material))
            .or_default()
            .push((Arc::clone(material), key, Arc::clone(plan)));
    }
}

fn material_twins(
    sections: &[SectionSpan],
    cancel: &AtomicBool,
) -> Result<Vec<usize>, ManufacturingError> {
    use std::hash::{Hash, Hasher};
    let mut first = std::collections::HashMap::<u64, Vec<usize>>::new();
    let mut twins = Vec::with_capacity(sections.len());
    for (index, span) in sections.iter().enumerate() {
        check_cancel(cancel)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for polygon in &span.material.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                ring.0.len().hash(&mut hasher);
                for c in &ring.0 {
                    c.x.to_bits().hash(&mut hasher);
                    c.y.to_bits().hash(&mut hasher);
                }
            }
        }
        let bucket = first.entry(hasher.finish()).or_default();
        let twin = bucket
            .iter()
            .copied()
            .find(|&j| sections[j].material.0 == span.material.0)
            .unwrap_or(index);
        if twin == index {
            bucket.push(index);
        }
        twins.push(twin);
    }
    Ok(twins)
}

/// Consecutive spans whose material agrees within the ADR012 profile
/// tolerance, as one source prism: the first span of each run and the end
/// layer of its last. Tolerant spans are one layer each; a material
/// construction binds one domain per prism, not per layer, as the source's
/// own vertical faces do. Straight edges give bit-identical sections; curved
/// walls are densified on their true curve from per-layer mesh crossings, so
/// their rings agree only within the tolerance.
fn material_runs(sections: &[SectionSpan]) -> impl Iterator<Item = (&SectionSpan, u32)> {
    let tolerance = spiling_geometry::tolerant::TolerantPolicy::default().profile_mm;
    let mut index = 0;
    std::iter::from_fn(move || {
        let first = sections.get(index)?;
        index += 1;
        while sections.get(index).is_some_and(|next| {
            next.start == sections[index - 1].end
                && (Arc::ptr_eq(&next.material, &first.material)
                    || next.material.0 == first.material.0
                    || same_within(&first.material, &next.material, tolerance))
        }) {
            index += 1;
        }
        Some((first, sections[index - 1].end))
    })
}

/// Same polygons and rings, every vertex of each within `tolerance` of the
/// corresponding ring of the other. Canonical rings start at their least
/// vertex and run the same way, so one cursor walks the other ring forward:
/// linear work, and a ring that does not follow fails rather than searching.
fn same_within(a: &MultiPolygon<f64>, b: &MultiPolygon<f64>, tolerance: f64) -> bool {
    a.0.len() == b.0.len()
        && a.0.iter().zip(&b.0).all(|(p, q)| {
            p.interiors().len() == q.interiors().len()
                && std::iter::once(p.exterior())
                    .chain(p.interiors())
                    .zip(std::iter::once(q.exterior()).chain(q.interiors()))
                    .all(|(r, s)| follows(r, s, tolerance) && follows(s, r, tolerance))
        })
}

fn follows(ring: &geo::LineString<f64>, other: &geo::LineString<f64>, tolerance: f64) -> bool {
    use geo::{Distance, Euclidean};
    let segments = other.0.len().saturating_sub(1);
    if segments == 0 || ring.0.len() < 2 {
        return false;
    }
    let near = |c: geo::Coord<f64>, j: usize| {
        Euclidean.distance(&Point(c), &geo::Line::new(other.0[j], other.0[j + 1])) <= tolerance
    };
    let Some(mut cursor) = (0..segments).find(|&j| near(ring.0[0], j)) else {
        return false;
    };
    let mut advanced = 0usize;
    for &c in &ring.0[1..] {
        while !near(c, cursor) {
            cursor = (cursor + 1) % segments;
            advanced += 1;
            if advanced > 2 * segments {
                return false;
            }
        }
    }
    true
}

/// Spans that print as one source prism: the same material and process
/// domains.
fn same_prism(a: &SectionSpan, b: &SectionSpan) -> bool {
    let same =
        |x: &Arc<MultiPolygon<f64>>, y: &Arc<MultiPolygon<f64>>| Arc::ptr_eq(x, y) || x.0 == y.0;
    same(&a.material, &b.material)
        && a.process_domains.len() == b.process_domains.len()
        && a.process_domains
            .iter()
            .zip(&b.process_domains)
            .all(|(x, y)| x.coverage == y.coverage && same(&x.material, &y.material))
}

type TolerantModels = BTreeMap<
    spiling_contracts::geometry::OccurrenceId,
    Arc<spiling_geometry::tolerant::TolerantModel>,
>;

fn span_at(sections: &[SectionSpan], index: u32) -> usize {
    let position = sections.partition_point(|s| s.end <= index);
    debug_assert!(sections[position].start <= index);
    position
}

/// Query-owned combinations are live staging geometry; borrowed D leaves and
/// cached Arc nodes are already charged by their respective owners.
fn skin_query_retained_points(
    retained_points: usize,
    materials: &[&neighborhood::Material<'_>],
) -> Result<usize, ManufacturingError> {
    materials
        .iter()
        .try_fold(retained_points, |points, material| {
            points
                .checked_add(material.temporary_points())
                .filter(|points| *points <= crate::working_set::units())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "retained skin query coordinate-unit budget exceeded",
                    )
                })
        })
}

fn skin_retained_points(
    retained_points: usize,
    neighborhoods: &SkinNeighborhoods<'_>,
    materials: &[&neighborhood::Material<'_>],
) -> Result<usize, ManufacturingError> {
    let retained = retained_points
        .checked_add(neighborhoods.retained_points())
        .filter(|points| *points <= crate::working_set::units())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "retained skin cache coordinate-unit budget exceeded",
            )
        })?;
    skin_query_retained_points(retained, materials)
}

/// Exact known outside ownership. Borrowed leaves/cache handles contribute no
/// extra payload here; the neighborhood admits its own cache separately.
fn skin_query_retained_bytes(
    owned_bytes: usize,
    materials: &[&neighborhood::Material<'_>],
) -> Result<usize, ManufacturingError> {
    materials.iter().try_fold(owned_bytes, |bytes, material| {
        bytes
            .checked_add(material.temporary_bytes())
            .filter(|bytes| *bytes as u64 <= crate::working_set::bytes_u64())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "retained skin query allocated-byte budget exceeded",
                )
            })
    })
}

fn skin_retained_bytes(
    owned_bytes: usize,
    neighborhoods: &SkinNeighborhoods<'_>,
    materials: &[&neighborhood::Material<'_>],
) -> Result<usize, ManufacturingError> {
    let bytes = owned_bytes
        .checked_add(neighborhoods.retained_bytes())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "skin retained byte overflow",
            )
        })?;
    skin_query_retained_bytes(bytes, materials)
}

fn skin_external_owned_bytes(
    static_bytes: usize,
    plan_bytes: usize,
    wall_bytes: usize,
    cache_len: usize,
    cache_entry_bytes: usize,
    resolution_bytes: usize,
    templates: &[Option<OrdinaryTemplate>; 2],
) -> Result<usize, ManufacturingError> {
    let index_bytes = cache_len.checked_mul(cache_entry_bytes).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "wall index byte overflow",
        )
    })?;
    let mut bytes = [
        static_bytes,
        plan_bytes,
        wall_bytes,
        index_bytes,
        resolution_bytes,
    ]
    .into_iter()
    .try_fold(0usize, |total, part| total.checked_add(part))
    .ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "skin outside byte overflow",
        )
    })?;
    for OrdinaryTemplate {
        paths,
        coverage,
        fits,
        ..
    } in templates.iter().flatten()
    {
        bytes = bytes
            .checked_add(bonds::path_bytes(paths))
            .and_then(|b| b.checked_add(bonds::coverage_owned_bytes(coverage)))
            .and_then(|b| b.checked_add(bonds::skin_command_fit_bytes(fits)))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "skin template byte overflow",
                )
            })?;
    }
    if bytes as u64 > crate::working_set::bytes_u64() {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "skin outside retained-byte budget exceeded",
        ));
    }
    Ok(bytes)
}

fn geometry_bytes(parts: &[usize]) -> Result<usize, ManufacturingError> {
    let bytes = parts
        .iter()
        .try_fold(0usize, |total, part| total.checked_add(*part))
        .filter(|bytes| *bytes as u64 <= crate::working_set::bytes_u64())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "owned geometry byte allowance exceeded",
            )
        })?;
    Ok(bytes)
}

fn layer_payload_bytes(layers: &[PrintLayer]) -> Result<usize, ManufacturingError> {
    // The stable PrintLayer Vec allocation belongs to the fixed outside pool.
    let mut bytes = 0usize;
    for layer in layers {
        bytes = geometry_bytes(&[
            bytes,
            bonds::path_bytes(&layer.paths),
            bonds::coverage_owned_bytes(&layer.region_coverage),
            bonds::skin_command_fit_bytes(&layer.skin_command_fits),
        ])?;
    }
    Ok(bytes)
}

pub(super) fn rotate(pose: &RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = pose.rotation_xyzw;
    let t = [
        2.0 * (y * p[2] - z * p[1]),
        2.0 * (z * p[0] - x * p[2]),
        2.0 * (x * p[1] - y * p[0]),
    ];
    [
        p[0] + w * t[0] + y * t[2] - z * t[1],
        p[1] + w * t[1] + z * t[0] - x * t[2],
        p[2] + w * t[2] + x * t[1] - y * t[0],
    ]
}
pub(super) fn forward(pose: &RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let r = rotate(pose, p);
    std::array::from_fn(|a| r[a] + pose.translation_mm[a])
}
fn rotate_inverse(pose: &RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = pose.rotation_xyzw;
    rotate(
        &RigidPoseMm {
            translation_mm: [0.0; 3],
            rotation_xyzw: [-x, -y, -z, w],
        },
        p,
    )
}
fn inverse(pose: &RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    rotate_inverse(pose, std::array::from_fn(|a| p[a] - pose.translation_mm[a]))
}

struct NativeSurfaceSpan {
    start: u32,
    end: u32,
    occurrences: Vec<(
        spiling_contracts::geometry::OccurrenceId,
        Arc<MultiPolygon<f64>>,
    )>,
    image_islands: Vec<(spiling_contracts::geometry::OccurrenceId, usize)>,
    required: BTreeMap<spiling_contracts::geometry::OccurrenceId, Arc<MultiPolygon<f64>>>,
    // Captured while M/C were owned, before sharing their immutable buffers.
    native_bytes: usize,
}

/// Assemble the mandatory coverage operand without copying polygon geometry.
/// Image-owned islands require C (native M plus non-image finishes); others require D.
fn required_material<'a>(
    requested: &'a MultiPolygon<f64>,
    native_span: Option<&'a NativeSurfaceSpan>,
    attribution_work: &mut usize,
    cancel: &AtomicBool,
) -> Result<region::RequiredMaterial<'a>, ManufacturingError> {
    let requested_points = region::point_count(requested);
    let mut source_owners = Vec::new();
    if let Some(span) = native_span {
        for (occurrence_id, polygon_index) in &span.image_islands {
            let occurrence = span
                .occurrences
                .binary_search_by_key(occurrence_id, |(id, _)| *id)
                .expect("certified native occurrence remains retained");
            let source = &span.occurrences[occurrence].1.0[*polygon_index];
            check_cancel(cancel)?;
            let required = &span.required[occurrence_id];
            *attribution_work = attribution_work
                .checked_add(requested_points)
                .and_then(|work| work.checked_add(requested.0.len()))
                .and_then(|work| work.checked_add(region::point_count(required)))
                .and_then(|work| work.checked_add(required.0.len()))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "required material attribution work counter overflow",
                    )
                })?;
            let witness = Point(source.exterior().0[0]);
            let mut owner = None;
            for (index, polygon) in requested.0.iter().enumerate() {
                check_cancel(cancel)?;
                if polygon.intersects(&witness) && owner.replace(index).is_some() {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        "required native material has multiple realized island owners",
                    ));
                }
            }
            let owner = owner.ok_or_else(|| {
                error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "required native material has no realized island owner",
                )
            })?;
            let mut required_owner = None;
            for polygon in &required.0 {
                check_cancel(cancel)?;
                if polygon.intersects(&witness) && required_owner.replace(polygon).is_some() {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        "required native material has multiple mandatory island owners",
                    ));
                }
            }
            let required_owner = required_owner.ok_or_else(|| {
                error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "required native material has no mandatory island owner",
                )
            })?;
            source_owners.push((owner, required_owner));
        }
    }
    if source_owners.is_empty() {
        return Ok(region::RequiredMaterial::from_section(requested));
    }
    let capacity = requested
        .0
        .len()
        .checked_add(source_owners.len())
        .filter(|count| *count <= crate::working_set::units())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "required material reference budget exceeded",
            )
        })?;
    let mut polygons = Vec::with_capacity(capacity);
    for (index, polygon) in requested.0.iter().enumerate() {
        check_cancel(cancel)?;
        let before = polygons.len();
        polygons.extend(
            source_owners
                .iter()
                .filter_map(|(owner, source)| (*owner == index).then_some(*source)),
        );
        if polygons.len() == before {
            polygons.push(polygon);
        }
    }
    Ok(region::RequiredMaterial { polygons })
}

/// Classification owns its scratch requirement; the planner owns reclaimable
/// cached ranges and all concurrently live source/query/support geometry.
fn admit_skin_scratch(
    retained: usize,
    owned_bytes: usize,
    neighborhoods: &mut neighborhood::SkinNeighborhoods<'_>,
    queries: &[&neighborhood::Material<'_>],
    scratch: usize,
    scratch_owned_bytes: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let external = skin_query_retained_points(retained, queries)?
        .checked_add(scratch)
        .filter(|total| *total <= crate::working_set::units())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "classified external coordinate-unit budget exceeded",
            )
        })?;
    let external_bytes = skin_query_retained_bytes(owned_bytes, queries)?
        .checked_add(scratch_owned_bytes)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "classified external byte overflow",
            )
        })?;
    neighborhoods.admit_retained(external, external_bytes, cancel)
}
