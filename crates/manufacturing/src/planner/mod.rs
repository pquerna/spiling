// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod boolean;
use boolean::budget as boolean_budget;
mod classification;
mod neighborhood;
use neighborhood::SkinNeighborhoods;
mod native;
mod preflight;
mod region;
mod support;
mod surface;
pub use preflight::preflight;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;
use crate::{CompilerInput, check_cancel, error};
use geo::{Area, Intersects, MultiPolygon, Point};
use spiling_contracts::geometry::{
    DefinitionId, GeometryErrorCode, RigidPoseMm, SECTION_SAMPLING_TOLERANCE_MM,
};
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
const MAX_BOOLEAN_PAIRS: usize = 128_000_000;
// Native interval operations and polygon Boolean pairs are independent resources.
const MAX_NATIVE_WORK_UNITS: u64 = 128_000_000;

pub(crate) fn plan(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<NormalizedPrintPlan, ManufacturingError> {
    let (mut admission, geometry_sources) = preflight::admit(input, cancel)?;
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
    let recipe = &input.intent.recipe;
    let mut surface_work = 0usize;
    preflight::resolve_native_events(
        input,
        &mut admission,
        &geometry_sources,
        &mut surface_work,
        cancel,
    )?;
    let prepared_sources = prepare_sources(
        geometry_sources,
        admission.native_capture_proposal_bytes(),
        &mut admission.geometry_work,
        cancel,
    )?;
    // This is a proposal, not authorization. The streamed source stage below
    // commits each actual interval only after native metric/incidence and joint
    // source-union certificates, refining this same field/support schedule.
    let mut schedule = scheduled_layers(input, &admission, cancel)?;
    let mut count = schedule.len() as u32;
    let mut occurrences = input.occurrences.iter().collect::<Vec<_>>();
    occurrences.sort_by_key(|o| o.occurrence_id);
    // M is sectioned once per certified native material slab. D has its own identity and
    // can change at every refined layer inside that same slab.
    let mut sections: Vec<SectionSpan> = Vec::new();
    let mut retained_section_bytes = 0usize;
    let mut retained_points = 0usize;
    let mut index = 0u32;
    let mut native_slab_end = 0u32;
    let mut native_slab_start = 0u32;
    let mut native_surfaces = Vec::new();
    let mut native_material = None;
    let mut native_material_bytes = 0usize;
    let mut native_contributions = Vec::new();
    let mut native_occupied_shared: Option<Arc<ContinuousOccupied>> = None;
    let mut native_finished = Vec::new();
    let mut native_required = BTreeMap::new();
    let mut native_finished_bytes = 0usize;
    let mut native_source_area_mm2 = 0.0;
    let mut native_points = 0usize;
    let mut native_stage = native::NativeStageState::new([
        input.intent.printer.build_envelope.min[2],
        schedule
            .last()
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "native schedule has no actual terminal layer",
                )
            })?
            .z,
    ])?;
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
            let retained_output_bytes =
                native::output_bytes(&sections, &native_surfaces, retained_section_bytes)
                    .saturating_add(
                        schedule
                            .capacity()
                            .saturating_mul(std::mem::size_of::<LayerElevation>()),
                    )
                    .saturating_add(occurrences.capacity().saturating_mul(std::mem::size_of::<
                        &spiling_contracts::geometry::OccurrenceRecord,
                    >()));
            let captured = native::collect_span(
                &native::NativeSourceInputs {
                    input,
                    occurrences: &occurrences,
                    sources: &prepared_sources,
                },
                &mut admission,
                &mut schedule,
                &native::NativeSpanRequest {
                    index,
                    retained_points,
                    retained_output_bytes,
                },
                &mut native_stage,
                &mut surface_work,
                cancel,
            )?;
            count = schedule.len() as u32;
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
            let live_retained = retained_points
                .checked_add(native_stage.retained_point_count())
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native lookahead plus captured scene point budget exceeded",
                    )
                })?;
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
                                .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
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
                        &mut surface_work,
                        cancel,
                    )?;
                    native_points = native_points
                        .checked_add(region::point_count(&required))
                        .filter(|n| {
                            live_retained
                                .checked_add(*n)
                                .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
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
        let lookahead_points = native_stage.retained_point_count();
        let live_retained = retained_points
            .checked_add(lookahead_points)
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native lookahead plus realized scene point budget exceeded",
                )
            })?;
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
            if !invariant {
                break;
            }
            end += 1;
        }
        let mut next = Arc::clone(native_material.as_ref().expect("captured native material"));
        let mut process_domains = Vec::new();
        if input
            .process_document
            .layers
            .iter()
            .any(|layer| layer.enabled)
        {
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
                    *occurrence_id,
                    Arc::clone(material),
                    *bytes,
                )?);
            }
        }
        let mut next_bytes = native_material_bytes;
        for (occurrence_id, placed) in &native_finished {
            let fields = &admission.surface_fields[occurrence_id];
            live_retained
                .checked_add(native_points)
                .and_then(|n| n.checked_add(region::point_count(&next)))
                .and_then(|n| n.checked_add(region::point_count(placed)))
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
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
                &mut surface_work,
                cancel,
            )?;
            let finished_bytes = native::material_bytes(&mut finished);
            let finished = Arc::new(finished);
            if input
                .process_document
                .layers
                .iter()
                .any(|layer| layer.enabled)
            {
                process_domains.push(process_domain(
                    input,
                    *occurrence_id,
                    Arc::clone(&finished),
                    finished_bytes,
                )?);
            }
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
            &mut surface_work,
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
                    &schedule,
                    live_retained + native_points + region::point_count(&next),
                    &mut surface_work,
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
                    .and_then(|n| n.checked_add(lookahead_points))
                    .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
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
                            if field.owns_native_island(
                                polygon,
                                sample,
                                &mut surface_work,
                                cancel,
                            )? && owner.replace(polygon_index).is_some()
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
                    .filter(|points| {
                        points
                            .checked_add(lookahead_points)
                            .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
                    })
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
    let mut native_coverage = native_stage.finalize(&mut admission.geometry_work, cancel)?;
    // Deferred void projection borrows these immutable domains and recipes.
    // Keep source owners in the native byte pool, not the deposition-point pool.
    let retained_support_native_bytes = prepared_sources
        .values()
        .try_fold(
            native_coverage.material_proof.retained_bytes(),
            |bytes, source| bytes.checked_add(source.retained_bytes()),
        )
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "deferred native source retention capacity overflow",
            )
        })?;
    let proof_units = native_coverage
        .material_proof
        .retained_bytes()
        .div_ceil(std::mem::size_of::<geo::Coord<f64>>());
    retained_points = retained_points
        .checked_add(proof_units)
        .filter(|total| *total <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "accepted native material proof retention exceeds planner admission",
            )
        })?;
    drop(native_finished);
    drop(native_required);
    drop(native_material);
    drop(native_occupied_shared);
    let mut layers = Vec::with_capacity(count as usize);
    let mut segments = 0usize;
    let skin_inset_mm = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
    let mut neighborhoods = SkinNeighborhoods::new(&sections, skin_inset_mm, surface_work)?;
    let mut key = None;
    let mut classified = None;
    let mut layout_span = None;
    let mut wall_layout = None;
    let mut wall_layouts = (0..sections.len()).map(|_| None).collect::<Vec<_>>();
    let mut templates: [Option<(Vec<DepositionPath>, PlanarRegionCoverage)>; 2] = [None, None];
    let mut surface_resolution = Vec::new();
    let mut resolution_allocated_bytes = 0usize;
    let mut diagnostic_work = 0usize;
    for index in 0..count {
        check_cancel(cancel)?;
        let elevation = &schedule[index as usize];
        let z = elevation.z;
        let sample = elevation.sample;
        let height = elevation.height;
        let current = span_at(&sections, index);
        let below = physical_neighbor(&schedule, index, recipe.bottom_skin_thickness_mm, false)
            .map(|i| span_at(&sections, i));
        let above = physical_neighbor(&schedule, index, recipe.top_skin_thickness_mm, true)
            .map(|i| span_at(&sections, i));
        if sections[current].material.0.is_empty() {
            layers.push(PrintLayer {
                index,
                z_mm: z,
                height_mm: height,
                section_z_mm: sample,
                paths: Vec::new(),
                region_coverage: empty_coverage(recipe.infill_fraction),
            });
            continue;
        }
        let next_key = (current, below, above, height.to_bits());
        if key != Some(next_key) {
            // Drop the prior classification before allocating its replacement.
            // The current wall layout survives skin-key changes, not span changes.
            drop(classified.take());
            templates = [None, None];
            if layout_span != Some(current) {
                drop(wall_layout.take());
                let native_span = native_surfaces
                    .get(native_surfaces.partition_point(|span| span.end <= index))
                    .filter(|span| span.start <= index);
                let required_material = required_material(
                    &sections[current].material,
                    native_span,
                    neighborhoods.work_mut(),
                    cancel,
                )?;
                wall_layout = Some(Arc::new(classification::WallLayout::new(
                    &sections[current].material,
                    &required_material,
                    recipe,
                    skin_retained_points(retained_points, &neighborhoods, &[])?,
                    cancel,
                ).map_err(|mut failure| {
                    failure.message = format!(
                        "layer {index} (section Z={sample} mm, deposition Z={z} mm, height={height} mm): {}",
                        failure.message
                    );
                    failure
                })?));
                let layout = wall_layout
                    .as_ref()
                    .expect("current wall layout initialized");
                retained_points += layout.point_count();
                wall_layouts[current] = Some(Arc::clone(layout));
                for fields in admission.surface_fields.values() {
                    surface::certify_first_wall(
                        wall_layout
                            .as_ref()
                            .expect("current wall layout initialized")
                            .first_wall(),
                        fields,
                        sample,
                        recipe.bead_width_mm,
                        cancel,
                    )?;
                }
                layout_span = Some(current);
            }
            let lower = neighborhoods.survival(current, below, retained_points, cancel)?;
            let upper = neighborhoods.survival(
                current,
                above,
                skin_query_retained_points(retained_points, &[&lower])?,
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
                    |scratch| {
                        admit_skin_scratch(
                            retained_points,
                            &mut neighborhoods,
                            &[&lower, &upper],
                            scratch,
                            cancel,
                        )
                    },
                    recipe,
                    cancel,
                )?
                .with_process(
                    &sections[current].process_domains,
                    classification::fixed_point_budget(skin_retained_points(
                        retained_points,
                        &neighborhoods,
                        &[&lower, &upper],
                    )?),
                    neighborhoods.work_mut(),
                    cancel,
                )?,
            );
            key = Some(next_key);
        }
        if !admission.surface_fields.is_empty() {
            let native_span = native_surfaces
                .get(native_surfaces.partition_point(|span| span.end <= index))
                .filter(|span| span.start <= index);
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
                        firstwall: wall_layout
                            .as_ref()
                            .expect("current wall layout initialized")
                            .first_wall(),
                        material: &sections[current].material,
                        native,
                    },
                    fields,
                    sample,
                    index,
                    recipe.bead_width_mm,
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
                            .is_some_and(|total| total <= MAX_MANUFACTURING_BUNDLE_BYTES as usize)
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
        let axis = region::HatchAxis::for_policy(recipe.rectilinear_axis, index);
        let axis_slot = usize::from(axis == region::HatchAxis::Y);
        if templates[axis_slot].is_none() {
            let mut template_segments = segments;
            templates[axis_slot] = Some(
                classified
                    .as_ref()
                    .expect("region classification initialized")
                    .paths(z, height, axis, recipe, &mut template_segments, cancel)?,
            );
        }
        let (template, coverage) = templates[axis_slot]
            .as_ref()
            .expect("current axis template initialized");
        let additional = template
            .iter()
            .map(|p| p.points_mm.len() - 1)
            .sum::<usize>();
        segments = segments
            .checked_add(additional)
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "deposition segment limit exceeded",
                )
            })?;
        let mut paths = Vec::with_capacity(template.len());
        for path in template {
            check_cancel(cancel)?;
            paths.push(DepositionPath {
                kind: path.kind,
                points_mm: path.points_mm.iter().map(|p| [p[0], p[1], z]).collect(),
            });
        }
        let mut coverage = coverage.clone();
        coverage.source_section_area_mm2 = sections[current].source_area_mm2;
        coverage.surface_added_area_mm2 =
            coverage.section_area_mm2 - coverage.source_section_area_mm2;
        layers.push(PrintLayer {
            index,
            z_mm: z,
            height_mm: height,
            section_z_mm: sample,
            paths,
            region_coverage: coverage,
        });
    }
    drop(classified);
    drop(wall_layout);
    drop(templates);
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
    drop(native_surfaces);
    // Support reserves this derived-cache pool once; regeneration may evict and
    // rebuild within it, but cannot silently increase retained support staging.
    neighborhoods.freeze_retained_capacity();
    support::realize(
        &mut layers,
        &support::SupportInput {
            sections: &sections,
            schedule: &schedule,
            native_points: skin_retained_points(retained_points, &neighborhoods, &[])?,
            recipe,
        },
        &mut segments,
        cancel,
        &mut admission.geometry_work,
        |index, axis, segments, retained_support_points, recovery| {
            let live_retained_points = retained_points
                .checked_add(retained_support_points)
                .filter(|total| *total <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "support raster retained point limit exceeded",
                    )
                })?;
            let elevation = &schedule[index];
            let current = span_at(&sections, index as u32);
            let below = physical_neighbor(
                &schedule,
                index as u32,
                recipe.bottom_skin_thickness_mm,
                false,
            )
            .map(|i| span_at(&sections, i));
            let above =
                physical_neighbor(&schedule, index as u32, recipe.top_skin_thickness_mm, true)
                    .map(|i| span_at(&sections, i));
            let lower = neighborhoods.survival(current, below, live_retained_points, cancel)?;
            let upper = neighborhoods.survival(
                current,
                above,
                skin_query_retained_points(live_retained_points, &[&lower])?,
                cancel,
            )?;
            let wall = Arc::clone(
                wall_layouts[current]
                    .as_ref()
                    .expect("D span erosion already admitted"),
            );
            let classified = classification::ClassifiedRegions::new(
                wall,
                &lower,
                &upper,
                |scratch| {
                    admit_skin_scratch(
                        live_retained_points,
                        &mut neighborhoods,
                        &[&lower, &upper],
                        scratch,
                        cancel,
                    )
                },
                recipe,
                cancel,
            )?
            .with_process(
                &sections[current].process_domains,
                classification::fixed_point_budget(skin_retained_points(
                    live_retained_points,
                    &neighborhoods,
                    &[&lower, &upper],
                )?),
                neighborhoods.work_mut(),
                cancel,
            )?;
            let hatch_layer = region::HatchLayer {
                z: elevation.z,
                height: elevation.height,
                axis,
                recipe,
            };
            let recovering_components = recovery.is_some();
            let paths = if let Some(anchors) = recovery {
                let query_points = skin_query_retained_points(0, &[&lower, &upper])?;
                let mut check_component =
                    |candidate: &mut support::RasterCandidate, axis, scratch: usize| {
                        let live = scratch.checked_add(query_points).ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "component and query scratch overflow",
                            )
                        })?;
                        anchors(candidate, axis, live)
                    };
                classified.paths_with_bridge_recovery(
                    &hatch_layer,
                    segments,
                    |scratch, work| {
                        boolean::charge(neighborhoods.work_mut(), work)?;
                        admit_skin_scratch(
                            live_retained_points,
                            &mut neighborhoods,
                            &[&lower, &upper],
                            scratch,
                            cancel,
                        )
                    },
                    &mut check_component,
                    cancel,
                )?
            } else {
                classified
                    .paths(
                        elevation.z,
                        elevation.height,
                        axis,
                        recipe,
                        segments,
                        cancel,
                    )?
                    .0
            };
            let whole_rows = if recovering_components {
                // A bridge band and authored complement are disjoint owners.
                // Global row extension must not re-enter the selected band.
                Vec::new()
            } else {
                classified.whole_rows(&paths, &hatch_layer, segments, cancel)?
            };
            Ok(support::RasterCandidate { paths, whole_rows })
        },
        |demand_bounds, point_limit, support_work, native_work| {
            native::project_protected_voids(
                input,
                &prepared_sources,
                &mut native_coverage.protected_voids,
                &native::ProtectedVoidDemand {
                    native_bounds: &admission.native_bounds,
                    demand_bounds,
                    point_limit,
                    retained_native_owner_bytes: retained_support_native_bytes,
                },
                support_work,
                native_work,
                cancel,
            )
        },
    )?;
    // No-support/no-demand paths never project or retain unused negative-space
    // captures beyond support admission.
    drop(native_coverage.protected_voids);
    drop(native_coverage.material_proof);
    drop(prepared_sources);
    let result = NormalizedPrintPlan {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
        native_geometry: NativeGeometryCoverageSummary {
            requested_shape_error_mm: recipe.native_shape_error_mm,
            achieved_shape_error_mm: native_coverage.achieved_shape_error_mm,
            height_analysis_count: input.occurrences.len() as u32,
            slab_count: native_coverage.slab_count,
            certified_layer_count: count,
            event_count: admission.native_event_bands.len() as u32,
            geometry_work: admission.geometry_work.consumed(),
            evidence: PreflightEvidence::Certified,
            skin_coverage: SkinCoverageScope::ScheduledMaterialDistance,
            support_clearance: SupportClearanceScope::ContinuousTargetOuterOccupancy,
            source_feature_coverage: native_coverage.source_features,
        },
        layers,
        surface_resolution,
        sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
        offset_tolerance_mm: OFFSET_TOLERANCE_MM,
        surface_tolerance_mm: surface::SURFACE_TOLERANCE_MM,
        layer_quantization_mm: LAYER_QUANTIZATION_MM,
    };
    result.validate(input.intent)?;
    Ok(result)
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
}

fn process_domain(
    input: &CompilerInput<'_>,
    occurrence_id: spiling_contracts::geometry::OccurrenceId,
    material: Arc<MultiPolygon<f64>>,
    bytes: usize,
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
            area_mm2: 0.0,
            fraction: value.fraction,
            contributions: vec![ProcessRegionContribution {
                occurrence_id,
                definition_id: occurrence.definition_id.clone(),
                provenance: value.provenance,
            }],
        },
        bytes,
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
            if a.coverage.fraction == b.coverage.fraction {
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
                        "incompatible whole-volume sparse fractions for occurrences {} and {} in final-D interval [{},{}] mm",
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
    fn occupied_pieces(&self) -> impl Iterator<Item = (&geo::Polygon<f64>, Option<[f64; 2]>)> {
        let borrowed = if self.occupied.is_none() {
            self.material.0.as_slice()
        } else {
            &[]
        };
        self.occupied
            .iter()
            .flat_map(|outer| {
                outer
                    .pieces
                    .iter()
                    .map(|piece| (&piece.polygon, Some(piece.height_mm)))
            })
            .chain(borrowed.iter().map(|polygon| (polygon, None)))
    }
}

/// Independent raw conservative operands retain their own certified height
/// scope. In particular an endpoint sheet is not extruded through an otherwise
/// empty slab merely because its XY projection is nonempty.
#[derive(Clone, Default)]
struct ContinuousOccupied {
    pieces: Vec<OccupiedPiece>,
    point_count: usize,
    payload_bytes: usize,
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
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
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

type PreparedGeometrySources<'a> =
    BTreeMap<DefinitionId, spiling_geometry::PreparedGeometrySource<'a>>;

fn geometry_query_limit(
    sources: &PreparedGeometrySources<'_>,
) -> Result<usize, ManufacturingError> {
    sources.values().try_fold(
        spiling_geometry::MAX_PREPARED_SECTION_BYTES,
        |remaining, source| {
            remaining
                .checked_sub(source.retained_bytes())
                .filter(|bytes| *bytes > 0)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "original material geometry exhausts shared query capacity",
                    )
                })
        },
    )
}

fn prepare_sources<'a>(
    sources: PreparedGeometrySources<'a>,
    coexisting_bytes: usize,
    work: &mut spiling_geometry::GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<BTreeMap<DefinitionId, spiling_geometry::PreparedMaterialSource<'a>>, ManufacturingError>
{
    let mut prepared = BTreeMap::new();
    // Source promotion may also use the fixed query headroom. Reserve the
    // still-live proposal buffer before granting any replacement allowance.
    let available = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES
        .checked_sub(coexisting_bytes)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native capture proposals exhaust source preparation capacity",
            )
        })?;
    let mut remaining = sources.values().try_fold(available, |remaining, source| {
        remaining
            .checked_sub(source.retained_bytes())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "aggregate original material geometry capacity exceeded",
                )
            })
    })?;
    for (definition_id, source) in sources {
        check_cancel(cancel)?;
        // Replace this owner's storage; all other original/prepared owners
        // still coexist. Promotion moves its domains/plans without rebuilding.
        let allowance = remaining
            .checked_add(source.retained_bytes())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "material source replacement capacity overflow",
                )
            })?;
        let source = source
            .prepare_sections(allowance, cancel, work)
            .map_err(|e| native_error(e, "native material preparation"))?;
        remaining = allowance
            .checked_sub(source.retained_bytes())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "aggregate prepared native material capacity exceeded",
                )
            })?;
        prepared.insert(definition_id, source);
    }
    Ok(prepared)
}

struct LayerElevation {
    z: f64,
    lower_z: f64,
    sample: f64,
    height: f64,
}

fn scheduled_layers(
    input: &CompilerInput<'_>,
    admission: &preflight::Admission,
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
    events.sort_by(f64::total_cmp);
    events.dedup_by(|a, b| *a == *b);
    let mut row_events = Vec::new();
    for fields in admission.surface_fields.values() {
        for field in fields {
            row_events.clear();
            field.append_z_events(bed, top, &mut row_events, MAX_PRINT_LAYERS as usize, cancel)?;
            // Equal row mappings on repeated occurrences must not consume the
            // event budget repeatedly. Reuse the native event insertion pattern.
            for event in row_events.drain(..) {
                check_cancel(cancel)?;
                if let Err(position) = events.binary_search_by(|old| old.total_cmp(&event)) {
                    if events.len() >= 3 * MAX_PRINT_LAYERS as usize {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "combined native/image event budget exceeded",
                        ));
                    }
                    events.insert(position, event);
                }
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
        row_events.clear();
        for &event in &events {
            check_cancel(cancel)?;
            let scale = event
                .abs()
                .max(recipe.layer_height_mm)
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
                    if row_events.len() + events.len() >= 3 * MAX_PRINT_LAYERS as usize {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "combined native/field/support event budget exceeded",
                        ));
                    }
                    row_events.push(boundary);
                }
            }
        }
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
    refine_surface_schedule(
        layer_schedule(bed, top, recipe.layer_height_mm, &events)?,
        &admission.surface_fields,
        cancel,
    )
}

fn layer_schedule(
    bed: f64,
    top: f64,
    nominal: f64,
    events: &[f64],
) -> Result<Vec<LayerElevation>, ManufacturingError> {
    let identity_roundoff = 4.0 * f64::EPSILON * bed.abs().max(top.abs()).max(1.0);
    let identical =
        |a: f64, b: f64| (a - b).abs() <= 4.0 * f64::EPSILON * a.abs().max(b.abs()).max(1.0);
    // Admit genuine boundaries before growing either buffer. Insert in order so
    // repeated occurrences and arithmetic-identical events consume no budget.
    // No nominal grid participates in event identity or interval construction.
    let mut boundaries = Vec::with_capacity(events.len().min(MAX_PRINT_LAYERS as usize - 1) + 2);
    boundaries.extend([bed, top]);
    for &event in events {
        if event <= bed
            || event >= top
            || !event.is_finite()
            || (event - bed).abs() <= identity_roundoff
            || (event - top).abs() <= identity_roundoff
        {
            continue;
        }
        let position = boundaries.partition_point(|boundary| *boundary < event);
        if identical(boundaries[position - 1], event) || identical(boundaries[position], event) {
            continue;
        }
        if boundaries.len() > MAX_PRINT_LAYERS as usize {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "horizontal native events exceed the layer budget",
            ));
        }
        boundaries.insert(position, event);
    }
    let subdivisions = |start: f64, end: f64| -> Result<usize, ManufacturingError> {
        let width = end - start;
        let ratio = (width / nominal).floor().max(1.0);
        if !width.is_finite()
            || width <= 0.0
            || !nominal.is_finite()
            || nominal <= 0.0
            || !ratio.is_finite()
            || ratio > MAX_PRINT_LAYERS as f64
        {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "layer schedule exceeds the layer budget",
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
        Ok(count)
    };
    let mut layer_count = 0;
    for interval in boundaries.windows(2) {
        layer_count += subdivisions(interval[0], interval[1])?;
        if layer_count > MAX_PRINT_LAYERS as usize {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "layer schedule exceeds the layer budget",
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
            // Genuine close mandatory events retain the native degeneracy
            // refusal; they are never merged to repair an unresolved slab.
            if height <= 2.0 * spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    format!(
                        "native event at Z {z:.17e} above {previous:.17e} creates unresolved layer thickness {height:.17e} mm"
                    ),
                ));
            }
            schedule.push(LayerElevation {
                z,
                lower_z: previous,
                sample: previous + interval_height * 0.5,
                height,
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
            refined.push(layer);
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
        if layer_count > MAX_PRINT_LAYERS as usize {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "image vertical error refinement exceeds the layer budget",
            ));
        }
        pending.push(LayerElevation {
            z: layer.z,
            lower_z: midpoint,
            sample: midpoint + upper_height * 0.5,
            height: upper_height,
        });
        pending.push(LayerElevation {
            z: midpoint,
            lower_z: start,
            sample: start + lower_height * 0.5,
            height: lower_height,
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

fn empty_coverage(_fraction: f64) -> PlanarRegionCoverage {
    PlanarRegionCoverage {
        section_area_mm2: 0.0,
        source_section_area_mm2: 0.0,
        surface_added_area_mm2: 0.0,
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
    }
}

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
                .filter(|points| *points <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "retained skin query point budget exceeded",
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
        .filter(|points| *points <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "retained skin cache point budget exceeded",
            )
        })?;
    skin_query_retained_points(retained, materials)
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
#[cfg(test)]
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
#[cfg(test)]
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
                .filter(|work| *work <= MAX_BOOLEAN_PAIRS)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "required material attribution work budget exceeded",
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
        .filter(|count| *count <= MAX_DEPOSITION_SEGMENTS as usize)
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
    neighborhoods: &mut neighborhood::SkinNeighborhoods<'_>,
    queries: &[&neighborhood::Material<'_>],
    scratch: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let external = skin_query_retained_points(retained, queries)?
        .checked_add(scratch)
        .filter(|total| *total <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "classified external geometry point limit exceeded",
            )
        })?;
    neighborhoods.admit_retained(external, cancel)
}
