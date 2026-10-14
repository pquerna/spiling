// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod boolean;
use boolean::budget as boolean_budget;
mod classification;
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
use geo::{Area, BooleanOps, MultiPolygon};
use spiling_contracts::geometry::{
    GeometryErrorCode, PlaneMm, RigidPoseMm, SECTION_SAMPLING_TOLERANCE_MM,
};
use spiling_contracts::manufacturing::*;
use std::{
    borrow::Cow,
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
    let bounds = admission
        .bounds
        .expect("eligible native geometry has bounds");
    let bed = input.intent.printer.build_envelope.min[2];
    let recipe = &input.intent.recipe;
    let mut events = admission.horizontal_events.clone();
    if let SupportPolicy::Breakaway {
        z_gap_mm,
        interface_thickness_mm,
        ..
    } = recipe.supports
    {
        for event in &admission.horizontal_events {
            let stop = event - z_gap_mm;
            let interface_start = stop - interface_thickness_mm;
            if stop > bed {
                events.push(stop);
            }
            if interface_start > bed {
                events.push(interface_start);
            }
        }
    }
    let schedule = layer_schedule(bed, bounds.max[2], recipe.layer_height_mm, &events)?;
    let count = schedule.len() as u32;
    let mut occurrences = input.occurrences.iter().collect::<Vec<_>>();
    occurrences.sort_by_key(|o| o.occurrence_id);
    let mut slab_boundaries = BTreeSet::new();
    for event in &admission.horizontal_events {
        let index =
            schedule.partition_point(|layer| layer.z <= *event + LAYER_QUANTIZATION_MM) as u32;
        slab_boundaries.insert(index);
    }
    // Retain one native footprint per certified slab, not one polygon copy per
    // layer and not a second full classified polygon plan.
    let mut sections: Vec<SectionSpan> = Vec::new();
    let mut retained_points = 0usize;
    let mut prepared_sources = BTreeMap::new();
    let mut remaining_prepared_bytes = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES;
    let mut surface_work = 0usize;
    let mut index = 0u32;
    while index < count {
        check_cancel(cancel)?;
        let sample = schedule[index as usize].sample;
        let end = if admission.reuse_slabs {
            slab_boundaries
                .range((index + 1)..)
                .next()
                .copied()
                .unwrap_or(count)
                .min(count)
        } else {
            index + 1
        };
        let mut next = MultiPolygon(Vec::new());
        let mut finished_material = Vec::new();
        let mut finished_points = 0usize;
        let mut finished_source_area_mm2 = 0.0;
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
            let fields = admission.surface_fields.get(&occurrence.occurrence_id);
            if fields.is_none() && !admission.surface_fields.is_empty() {
                retained_points
                    .checked_add(finished_points)
                    .and_then(|v| v.checked_add(2 * region::point_count(&next)))
                    .and_then(|v| v.checked_add(2 * region::point_count(&placed)))
                    .filter(|v| *v <= MAX_DEPOSITION_SEGMENTS as usize)
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "native plus finished occurrence staging point budget exceeded",
                        )
                    })?;
            }
            if let Some(fields) = fields {
                finished_source_area_mm2 += placed.unsigned_area();
                let finished = surface::realize(
                    placed,
                    fields,
                    sample,
                    retained_points + region::point_count(&next) + finished_points,
                    &mut surface_work,
                    cancel,
                )?;
                finished_points += region::point_count(&finished);
                // Preflight proves finished occurrence XY bounds are disjoint
                // (including ring contact) in every overlapping source Z slab.
                // Append these exact native-preserving rings, never re-overlay
                // them through a changed floating-coordinate Boolean frame.
                finished_material.extend(finished.0);
            } else if next.0.is_empty() {
                next = placed;
            } else {
                boolean_budget(&next, &placed, cancel)?;
                next = next.union(&placed);
            }
            check_cancel(cancel)?;
            region::check_size(&next)?;
        }
        let source_area_mm2 = next.unsigned_area() + finished_source_area_mm2;
        next.0.extend(finished_material);
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
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
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
        index = end;
    }
    drop(prepared_sources);
    let mut layers = Vec::with_capacity(count as usize);
    let mut segments = 0usize;
    let mut neighborhood_work = surface_work;
    let mut key = None;
    let mut classified = None;
    let mut layout_span = None;
    let mut wall_layout = None;
    let mut templates: [Option<(Vec<DepositionPath>, PlanarRegionCoverage)>; 2] = [None, None];
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
                wall_layout = Some(Arc::new(classification::WallLayout::new(
                    &sections[current].material,
                    recipe,
                    retained_points,
                    cancel,
                ).map_err(|mut failure| {
                    failure.message = format!(
                        "layer {index} (section Z={sample} mm, deposition Z={z} mm, height={height} mm): {}",
                        failure.message
                    );
                    failure
                })?));
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
            let lower = survival(&sections, current, below, &mut neighborhood_work, cancel)?;
            let upper = survival(&sections, current, above, &mut neighborhood_work, cancel)?;
            classified = Some(classification::ClassifiedRegions::new(
                Arc::clone(
                    wall_layout
                        .as_ref()
                        .expect("current wall layout initialized"),
                ),
                &lower,
                &upper,
                recipe,
                cancel,
            )?);
            key = Some(next_key);
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
    support::realize(
        &mut layers,
        &sections,
        retained_points,
        recipe,
        &mut segments,
        cancel,
        |index, axis, segments| {
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
            let lower = survival(&sections, current, below, &mut neighborhood_work, cancel)?;
            let upper = survival(&sections, current, above, &mut neighborhood_work, cancel)?;
            let wall = Arc::new(classification::WallLayout::new(
                &sections[current].material,
                recipe,
                retained_points,
                cancel,
            )?);
            for fields in admission.surface_fields.values() {
                surface::certify_first_wall(
                    wall.first_wall(),
                    fields,
                    elevation.sample,
                    recipe.bead_width_mm,
                    cancel,
                )?;
            }
            let classified =
                classification::ClassifiedRegions::new(wall, &lower, &upper, recipe, cancel)?;
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

fn layer_schedule(
    bed: f64,
    top: f64,
    nominal: f64,
    events: &[f64],
) -> Result<Vec<LayerElevation>, ManufacturingError> {
    let nominal_count = ((top - bed) / nominal).ceil();
    if !nominal_count.is_finite() || nominal_count < 1.0 || nominal_count > MAX_PRINT_LAYERS as f64
    {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "layer schedule exceeds the layer budget",
        ));
    }
    let mut ends = Vec::with_capacity(nominal_count as usize + events.len() + 1);
    for index in 1..nominal_count as usize {
        let z = bed + index as f64 * nominal;
        if z < top - 4.0 * f64::EPSILON * top.abs().max(z.abs()).max(1.0) {
            ends.push(z);
        }
    }
    ends.extend(events.iter().copied().filter(|z| *z > bed && *z < top));
    ends.push(top);
    // Event identities are subject only to bounded floating arithmetic roundoff,
    // including cancellation at the bed. This is not the native plane tolerance.
    let identity_roundoff = 4.0 * f64::EPSILON * bed.abs().max(top.abs()).max(1.0);
    for z in &mut ends {
        if (*z - bed).abs() <= identity_roundoff {
            *z = bed;
        } else if (*z - top).abs() <= identity_roundoff {
            *z = top;
        } else {
            let grid = bed + ((*z - bed) / nominal).round_ties_even() * nominal;
            if grid > bed && grid < top && (*z - grid).abs() <= identity_roundoff {
                *z = grid;
            }
        }
    }
    ends.retain(|z| *z > bed);
    ends.sort_by(f64::total_cmp);
    // Only floating arithmetic roundoff may coalesce; a genuinely short native
    // interval still reaches the explicit degeneracy/thickness refusal below.
    ends.dedup_by(|a, b| (*a - *b).abs() <= 4.0 * f64::EPSILON * a.abs().max(b.abs()).max(1.0));
    if ends.len() > MAX_PRINT_LAYERS as usize {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "horizontal native events exceed the layer budget",
        ));
    }
    let mut previous = bed;
    let mut schedule = Vec::with_capacity(ends.len());
    for z in ends {
        let height = z - previous;
        // The midpoint must remain away from native degeneracy gates and both
        // boundaries must survive six-decimal emitted coordinate precision.
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
            sample: previous + height * 0.5,
            height,
        });
        previous = z;
    }
    Ok(schedule)
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

fn survival<'a>(
    sections: &'a [SectionSpan],
    current: usize,
    neighbor: Option<usize>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Cow<'a, MultiPolygon<f64>>, ManufacturingError> {
    let Some(neighbor) = neighbor else {
        return Ok(Cow::Owned(MultiPolygon(Vec::new())));
    };
    let mut survives = Cow::Borrowed(&sections[current].material);
    let first = current.min(neighbor);
    let last = current.max(neighbor);
    for (offset, section) in sections[first..=last].iter().enumerate() {
        let other = first + offset;
        check_cancel(cancel)?;
        if other == current {
            continue;
        }
        let pairs = region::point_count(&survives)
            .checked_mul(region::point_count(&section.material))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "skin neighborhood work overflow",
                )
            })?;
        *work = work
            .checked_add(pairs)
            .filter(|n| *n <= MAX_BOOLEAN_PAIRS)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "skin neighborhood Boolean work limit exceeded",
                )
            })?;
        survives = Cow::Owned(classification::intersection(
            &survives,
            &section.material,
            cancel,
        )?);
        if survives.0.is_empty() {
            break;
        }
    }
    Ok(survives)
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
