// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod boolean;
use boolean::budget as boolean_budget;
mod classification;
mod neighborhood;
use neighborhood::SkinNeighborhoods;
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
use geo::{Area, BooleanOps, Intersects, MultiPolygon, Point};
use spiling_contracts::geometry::{
    GeometryErrorCode, PlaneMm, RigidPoseMm, SECTION_SAMPLING_TOLERANCE_MM,
};
use spiling_contracts::manufacturing::*;
use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    sync::{Arc, atomic::AtomicBool},
};

pub const LAYER_QUANTIZATION_MM: f64 = 0.000001;
pub const OFFSET_TOLERANCE_MM: f64 = 0.005;
const MAX_BOOLEAN_PAIRS: usize = 20_000_000;

pub(crate) fn plan(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<NormalizedPrintPlan, ManufacturingError> {
    let admission = preflight::admit(input, cancel)?;
    if !admission.report.eligible {
        let check = admission
            .report
            .checks
            .iter()
            .find(|check| check.severity == PreflightSeverity::Error)
            .expect("ineligible metadata has an error");
        let code = match check.code.as_str() {
            "resource_limit" | "event_limit" => ManufacturingErrorCode::ResourceLimit,
            "empty_project" => ManufacturingErrorCode::EmptyProject,
            "invalid_intent"
            | "source_identity"
            | "invalid_pose"
            | "missing_definition"
            | "invalid_surface_targets" => ManufacturingErrorCode::InvalidSpecification,
            _ => ManufacturingErrorCode::UnsupportedGeometry,
        };
        return Err(error(code, check.message.clone()));
    }
    let recipe = &input.intent.recipe;
    let schedule = admission
        .schedule
        .as_ref()
        .expect("eligible input has an admitted schedule");
    let count = schedule.len() as u32;
    let mut occurrences = input.occurrences.iter().collect::<Vec<_>>();
    occurrences.sort_by_key(|o| o.occurrence_id);
    let mut slab_boundaries = BTreeSet::new();
    for event in &admission.horizontal_events {
        let index =
            schedule.partition_point(|layer| layer.z <= *event + LAYER_QUANTIZATION_MM) as u32;
        slab_boundaries.insert(index);
    }
    // M is sectioned once per native invariant slab. D has its own identity and
    // can change at every refined layer inside that same slab.
    let mut sections: Vec<SectionSpan> = Vec::new();
    let mut retained_points = 0usize;
    let mut prepared_sources = BTreeMap::new();
    let mut remaining_prepared_bytes = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES;
    let mut surface_work = 0usize;
    let mut index = 0u32;
    let mut native_slab_end = 0u32;
    let mut native_slab_start = 0u32;
    let mut native_surfaces = Vec::new();
    let mut native_material = MultiPolygon(Vec::new());
    let mut native_finished = Vec::new();
    let mut native_required = BTreeMap::new();
    let mut native_source_area_mm2 = 0.0;
    let mut native_points = 0usize;
    while index < count {
        check_cancel(cancel)?;
        let sample = schedule[index as usize].sample;
        if index == native_slab_end {
            native_slab_start = index;
            native_slab_end = if admission.reuse_slabs {
                slab_boundaries
                    .range((index + 1)..)
                    .next()
                    .copied()
                    .unwrap_or(count)
                    .min(count)
            } else {
                index + 1
            };
            native_material = MultiPolygon(Vec::new());
            native_finished.clear();
            native_required.clear();
            native_points = 0;
            native_source_area_mm2 = 0.0;
            for occurrence in &occurrences {
                check_cancel(cancel)?;
                let definition = &input.definitions[&occurrence.definition_id];
                let plane = PlaneMm {
                    origin_mm: inverse(&occurrence.pose, [0.0, 0.0, sample]),
                    normal: rotate_inverse(&occurrence.pose, [0.0, 0.0, 1.0]),
                };
                let native_error = |e: spiling_contracts::geometry::GeometryError| {
                    let code = match e.code {
                        GeometryErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
                        GeometryErrorCode::ResourceLimit => ManufacturingErrorCode::ResourceLimit,
                        _ => ManufacturingErrorCode::UnsupportedGeometry,
                    };
                    error(code, format!("native section at layer {index}: {e}"))
                };
                let source = match prepared_sources.entry(&occurrence.definition_id) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => {
                        let source = spiling_geometry::prepare_section_source(
                            definition,
                            remaining_prepared_bytes,
                            cancel,
                        )
                        .map_err(native_error)?;
                        remaining_prepared_bytes = remaining_prepared_bytes
                            .checked_sub(source.retained_bytes())
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "aggregate prepared native section capacity exceeded",
                                )
                            })?;
                        entry.insert(source)
                    }
                };
                let native = source
                    .section_with_byte_limit(plane, cancel, 4 * 1024 * 1024)
                    .map_err(native_error)?;
                let placed = region::from_native(native, &occurrence.pose, sample, cancel)?;
                if placed.0.is_empty() {
                    continue;
                }
                if admission
                    .surface_fields
                    .contains_key(&occurrence.occurrence_id)
                {
                    native_points = native_points
                        .checked_add(region::point_count(&placed))
                        .filter(|n| {
                            retained_points
                                .checked_add(*n)
                                .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
                        })
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "retained native image-carrier footprint budget exceeded",
                            )
                        })?;
                    native_source_area_mm2 += placed.unsigned_area();
                    native_finished.push((occurrence.occurrence_id, Arc::new(placed)));
                } else if native_material.0.is_empty() {
                    native_material = placed;
                } else {
                    boolean_budget(&native_material, &placed, cancel)?;
                    native_material = native_material.union(&placed);
                }
                check_cancel(cancel)?;
                region::check_size(&native_material)?;
            }
            native_source_area_mm2 += native_material.unsigned_area();
            native_points = native_points
                .checked_add(region::point_count(&native_material))
                .filter(|n| {
                    retained_points
                        .checked_add(*n)
                        .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
                })
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "retained native slab point budget exceeded",
                    )
                })?;
            for (occurrence_id, native) in &native_finished {
                let fields = &admission.surface_fields[occurrence_id];
                if !fields.iter().any(|field| field.is_image()) {
                    continue;
                }
                let required = if fields.iter().any(|field| !field.is_image()) {
                    let staged = retained_points
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
                    let required = surface::realize_required(
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
                            retained_points
                                .checked_add(*n)
                                .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
                        })
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "retained mandatory carrier point budget exceeded",
                            )
                        })?;
                    Arc::new(required)
                } else {
                    Arc::clone(native)
                };
                native_required.insert(*occurrence_id, required);
            }
        }
        let slab_start = schedule[index as usize].z - schedule[index as usize].height;
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
        let mut next = if native_finished.is_empty() && end == native_slab_end {
            native_points = 0;
            std::mem::replace(&mut native_material, MultiPolygon::new(Vec::new()))
        } else {
            retained_points
                .checked_add(native_points)
                .and_then(|n| n.checked_add(region::point_count(&native_material)))
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native plus realized staging point budget exceeded",
                    )
                })?;
            native_material.clone()
        };
        for (occurrence_id, placed) in &native_finished {
            let fields = &admission.surface_fields[occurrence_id];
            retained_points
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
            let finished = surface::realize(
                placed.as_ref().clone(),
                fields,
                sample,
                retained_points + native_points + region::point_count(&next),
                &mut surface_work,
                cancel,
            )?;
            // Expanded occurrence separation is certified in preflight. Preserve
            // exact source vertices rather than resnapping them through a union.
            next.0.extend(finished.0);
        }
        let source_area_mm2 = native_source_area_mm2;
        region::check_size(&next)?;
        region::canonicalize(&mut next);
        let realized_area_mm2 = next.unsigned_area();
        if realized_area_mm2 < source_area_mm2 {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "surface construction lost source material",
            ));
        }
        // Empty native intervals are retained for genuine floating material.
        // Their support paths are generated after actual object-anchor analysis.
        retained_points = retained_points
            .checked_add(region::point_count(&next))
            .filter(|n| {
                n.checked_add(native_points)
                    .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
            })
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "retained native footprint point budget exceeded",
                )
            })?;
        sections.push(SectionSpan {
            start: index,
            end,
            material: next,
            source_area_mm2,
        });
        if end == native_slab_end && !native_finished.is_empty() {
            // Image coverage needs M for diagnostics and C for mandatory coverage.
            native_finished.retain(|(occurrence_id, _)| {
                admission.surface_fields[occurrence_id]
                    .iter()
                    .any(|field| field.is_image())
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
                    .filter(|points| *points <= MAX_DEPOSITION_SEGMENTS as usize)
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
                });
            }
        }
        index = end;
    }
    drop(prepared_sources);
    drop(native_finished);
    drop(native_required);
    drop(native_material);
    let mut layers = Vec::with_capacity(count as usize);
    let mut segments = 0usize;
    let mut neighborhoods = SkinNeighborhoods::new(&sections, surface_work)?;
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
            classified = Some(classification::ClassifiedRegions::new(
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
            )?);
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
                    wall_layout
                        .as_ref()
                        .expect("current wall layout initialized")
                        .first_wall(),
                    &sections[current].material,
                    native,
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
        let parity = index as usize % 2;
        if templates[parity].is_none() {
            let mut template_segments = segments;
            templates[parity] = Some(
                classified
                    .as_ref()
                    .expect("region classification initialized")
                    .paths(
                        z,
                        height,
                        region::HatchAxis::preferred(index),
                        recipe,
                        &mut template_segments,
                        cancel,
                    )?,
            );
        }
        let (template, coverage) = templates[parity]
            .as_ref()
            .expect("current parity template initialized");
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
        &sections,
        skin_retained_points(retained_points, &neighborhoods, &[])?,
        recipe,
        &mut segments,
        cancel,
        |index, axis, segments, retained_support_points| {
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
            )?;
            let paths = classified
                .paths(
                    elevation.z,
                    elevation.height,
                    axis,
                    recipe,
                    segments,
                    cancel,
                )?
                .0;
            let whole_rows = classified.whole_rows(
                elevation.z,
                elevation.height,
                axis,
                recipe,
                segments,
                cancel,
            )?;
            Ok(support::RasterCandidate { paths, whole_rows })
        },
    )?;
    let result = NormalizedPrintPlan {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
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
    material: MultiPolygon<f64>,
    source_area_mm2: f64,
}

struct LayerElevation {
    z: f64,
    sample: f64,
    height: f64,
}

fn scheduled_layers(
    input: &CompilerInput<'_>,
    admission: &preflight::Admission,
    cancel: &AtomicBool,
) -> Result<Vec<LayerElevation>, ManufacturingError> {
    let bed = input.intent.printer.build_envelope.min[2];
    let top = admission.bounds.expect("schedule has native bounds").max[2];
    let recipe = &input.intent.recipe;
    let mut events = admission.horizontal_events.clone();
    if let SupportPolicy::Breakaway {
        z_gap_mm,
        interface_thickness_mm,
        ..
    } = recipe.supports
    {
        for event in &admission.horizontal_events {
            for stop in [event - z_gap_mm, event - z_gap_mm - interface_thickness_mm] {
                if stop > bed && stop < top {
                    if events.len() >= 3 * MAX_PRINT_LAYERS as usize {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "support schedule event budget exceeded",
                        ));
                    }
                    events.push(stop);
                }
            }
        }
    }
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
                        "native event at Z {z:.9} creates unresolved layer thickness {height:.9} mm"
                    ),
                ));
            }
            schedule.push(LayerElevation {
                z,
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
        let start = layer.z - layer.height;
        let mut split = false;
        for occurrence_fields in fields.values() {
            for field in occurrence_fields {
                if let Some(allowed) = field.allowed_vertical_error() {
                    split |= field.vertical_error_bound(start, layer.z)? > allowed;
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
            sample: midpoint + upper_height * 0.5,
            height: upper_height,
        });
        pending.push(LayerElevation {
            z: midpoint,
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
        let bottom = schedule[i].z - schedule[i].height;
        let target = bottom - thickness;
        if target < schedule[0].z - schedule[0].height - LAYER_QUANTIZATION_MM {
            return None;
        }
        let j = schedule.partition_point(|layer| layer.z <= target + LAYER_QUANTIZATION_MM);
        Some(j as u32)
    }
}

fn empty_coverage(fraction: f64) -> PlanarRegionCoverage {
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
        requested_infill_fraction: fraction,
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
