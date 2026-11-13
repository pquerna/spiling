// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! ADR012 tolerant spans: one layer's material from each occurrence's placed
//! tessellation, in the same shape the exact native collector produces, so
//! walls, skins, infill and supports are unchanged. No native certificate is
//! produced or consumed here.

use super::{ContinuousOccupied, LayerElevation, native, region};
use crate::{check_cancel, error};
use geo::{BooleanOps, LineString, MultiPolygon, Polygon};
use spiling_contracts::geometry::{OccurrenceId, OccurrenceRecord};
use spiling_contracts::manufacturing::*;
use spiling_geometry::tolerant::{TolerantModel, TolerantSection};
use std::collections::BTreeMap;
use std::sync::{Arc, atomic::AtomicBool};

/// Section loops as polygons: each hole belongs to the smallest outer loop
/// that contains it.
pub(super) fn polygons(section: &TolerantSection) -> MultiPolygon<f64> {
    let ring = |points: &[[f64; 2]]| -> LineString<f64> {
        let mut coords: Vec<geo::Coord<f64>> = points
            .iter()
            .map(|p| geo::Coord { x: p[0], y: p[1] })
            .collect();
        if let Some(first) = coords.first().copied() {
            coords.push(first);
        }
        LineString(coords)
    };
    let outers: Vec<usize> = (0..section.loops.len())
        .filter(|&i| section.loops[i].area > 0.0)
        .collect();
    let mut holes: Vec<Vec<LineString<f64>>> = vec![Vec::new(); outers.len()];
    for hole in section.loops.iter().filter(|l| l.area < 0.0) {
        let probe = hole.points[0];
        let owner = outers
            .iter()
            .enumerate()
            .filter(|(_, o)| contains(&section.loops[**o].points, probe))
            .min_by(|a, b| {
                section.loops[*a.1]
                    .area
                    .total_cmp(&section.loops[*b.1].area)
            })
            .map(|(k, _)| k);
        if let Some(k) = owner {
            holes[k].push(ring(&hole.points));
        }
    }
    MultiPolygon(
        outers
            .iter()
            .zip(holes)
            .map(|(o, h)| Polygon::new(ring(&section.loops[*o].points), h))
            .collect(),
    )
}

fn contains(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub(super) fn section_at(
    model: &TolerantModel,
    z: f64,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let section = model.section(z).map_err(|e| {
        error(
            ManufacturingErrorCode::UnsupportedGeometry,
            format!("tolerant section at Z {z} mm: {}", e.message),
        )
    })?;
    let mut result = polygons(&section);
    // Chords of flat faces cross the plane in runs of collinear points, and
    // freeform faces in sub-micrometre steps. Drop vertices within a quarter
    // of the profile tolerance, the sag analytic curves were densified to, so
    // walls see the section's shape rather than its triangle fans. Kept
    // vertices stay on the BREP.
    // Simplification keeps each ring's first point, so start every ring at
    // its lexicographically least vertex, a corner of its hull, not at
    // whichever mesh crossing the chain began on: a prism's straight edges
    // then lose every crossing and its layers share one bit-identical section.
    let epsilon = model.policy().profile_mm * 0.25;
    for polygon in &mut result.0 {
        polygon.exterior_mut(start_at_least);
        polygon.interiors_mut(|rings| rings.iter_mut().for_each(start_at_least));
    }
    result = geo::Simplify::simplify(&result, epsilon);
    result
        .0
        .retain(|polygon| geo::Area::unsigned_area(polygon) > epsilon * epsilon);
    Ok(result)
}

fn start_at_least(ring: &mut LineString<f64>) {
    if ring.0.len() < 4 {
        return;
    }
    ring.0.pop();
    let start = (0..ring.0.len())
        .min_by(|&a, &b| {
            ring.0[a]
                .x
                .total_cmp(&ring.0[b].x)
                .then(ring.0[a].y.total_cmp(&ring.0[b].y))
        })
        .unwrap_or(0);
    ring.0.rotate_left(start);
    ring.0.push(ring.0[0]);
}

/// Sections of one layer for every occurrence, computed while deciding a
/// span's extent; the next span starts from them.
pub(super) type Prefetched = Option<(u32, u64, Vec<MultiPolygon<f64>>)>;

fn sections_at(
    occurrences: &[&OccurrenceRecord],
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    z: f64,
    cancel: &AtomicBool,
) -> Result<Vec<MultiPolygon<f64>>, ManufacturingError> {
    let mut sections = Vec::with_capacity(occurrences.len());
    for occurrence in occurrences {
        check_cancel(cancel)?;
        let mut placed = section_at(&models[&occurrence.occurrence_id], z)?;
        region::canonicalize(&mut placed);
        sections.push(placed);
    }
    Ok(sections)
}

/// A span of layers as a native-shaped span: material at its first layer's
/// sample, and an occupied enclosure streamed from the source over the
/// complete interval when supports are enabled; sampled occupancy remains for
/// ordinary classification. The first section is held over each following
/// layer of `layers` whose own sections agree with it within the profile
/// tolerance, compared against the held section so error never accumulates;
/// the span ends at the first layer that does not. A source prism thus
/// slices as one span, as its vertical faces do: walls, permanent plate
/// extents, combined bands and material constructions see one section.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_span(
    occurrences: &[&OccurrenceRecord],
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    admission: &super::preflight::Admission,
    layers: &[LayerElevation],
    index: u32,
    prefetched: &mut Prefetched,
    supports: bool,
    cancel: &AtomicBool,
) -> Result<native::NativeSpan, ManufacturingError> {
    let profile = spiling_geometry::tolerant::TolerantPolicy::default().profile_mm;
    let sections = match prefetched.take() {
        Some((at, sample, sections)) if at == index && sample == layers[0].sample.to_bits() => {
            sections
        }
        _ => sections_at(occurrences, models, layers[0].sample, cancel)?,
    };
    let mut held = 1;
    while held < layers.len() {
        let next = sections_at(occurrences, models, layers[held].sample, cancel)?;
        if !sections
            .iter()
            .zip(&next)
            .all(|(held, other)| held.0 == other.0 || super::same_within(held, other, profile))
        {
            *prefetched = Some((index + held as u32, layers[held].sample.to_bits(), next));
            break;
        }
        held += 1;
    }
    let layers = &layers[..held];
    let (lower, upper) = (layers[0].lower_z, layers[held - 1].z);
    let mut material = MultiPolygon(Vec::new());
    let mut contributions = Vec::with_capacity(occurrences.len());
    let mut finished = Vec::new();
    let mut finished_bytes = 0usize;
    let mut finished_area_mm2 = 0.0;
    let mut occupied = ContinuousOccupied::default();
    for (occurrence, mut placed) in occurrences.iter().zip(sections) {
        check_cancel(cancel)?;
        let model = &models[&occurrence.occurrence_id];
        let bounds = admission.native_bounds[&occurrence.occurrence_id];
        let height = [lower.max(bounds.min[2]), upper.min(bounds.max[2])];
        if supports {
            occupied.tolerant.push(super::TolerantOccupied {
                model: Arc::clone(model),
                interval: [lower, upper],
            });
        } else if height[0] <= height[1] {
            for layer in layers {
                let inset = (0.25 * (layer.z - layer.lower_z)).min(0.01);
                for z in [layer.lower_z + inset, layer.sample, layer.z - inset] {
                    for polygon in section_at(model, z)?.0 {
                        occupied.push(polygon, height)?;
                    }
                }
            }
        }
        if placed.0.is_empty() {
            continue;
        }
        if admission
            .surface_fields
            .contains_key(&occurrence.occurrence_id)
        {
            // Finished occurrences stay their own M: preflight keeps their
            // expanded bounds clear of every other occurrence, and D = M
            // plus the finish displacement is realized per layer.
            finished_area_mm2 += geo::Area::unsigned_area(&placed);
            finished_bytes = finished_bytes
                .saturating_add(native::material_bytes(&mut placed))
                .saturating_add(native::MATERIAL_ARC_BYTES);
            finished.push((occurrence.occurrence_id, Arc::new(placed)));
            continue;
        }
        material = if material.0.is_empty() {
            placed.clone()
        } else {
            material.union(&placed)
        };
        let bytes = native::material_bytes(&mut placed);
        contributions.push((occurrence.occurrence_id, Arc::new(placed), bytes));
    }
    if contributions.len() > 1 {
        // Solids that touch (a shared face, a vertex on a face) print fused.
        // Their union can leave them as separate rings a few nanometres apart;
        // closing by half the profile tolerance fuses them and moves nothing
        // farther than that.
        let fuse = 0.5 * super::OFFSET_TOLERANCE_MM;
        let admit =
            |_: usize, _: usize, _: Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>| Ok(());
        let grown = region::offset_admitted(&material, fuse, admit, cancel)?;
        material = region::offset_admitted(&grown, -fuse, admit, cancel)?;
    }
    region::canonicalize(&mut material);
    region::check_size(&material)?;
    // Overlapping occurrences share material: the source area is the union's.
    let source_area_mm2 = geo::Area::unsigned_area(&material) + finished_area_mm2;
    let material_bytes = native::material_bytes(&mut material);
    let point_count = finished
        .iter()
        .map(|(_, placed)| region::point_count(placed))
        .fold(region::point_count(&material), usize::saturating_add);
    Ok(native::NativeSpan::tolerant(
        index + held as u32,
        Arc::new(material),
        contributions,
        finished,
        finished_bytes,
        Arc::new(occupied),
        source_area_mm2,
        point_count,
        material_bytes,
    ))
}

/// ADR012 full-height inner material: `eligible` grown by `radius` lies
/// inside the union of `occurrences`' material over the whole slab
/// `interval`, within the profile tolerance. An occurrence's material over
/// the slab is its section there minus the projection of every facet that
/// crosses the slab (`TolerantModel::slab_crossings`); flat faces within the
/// printer's Z step of either end bound the slab rather than cross it, as
/// they bound its layers. This is the guarantee a permanent plate or a
/// combined sparse band needs: no wall, overhang or roof inside its height.
pub(super) fn inner_material_admits(
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    occurrences: impl IntoIterator<Item = OccurrenceId>,
    eligible: &MultiPolygon<f64>,
    interval: [f64; 2],
    radius: f64,
    height_mm: f64,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    use geo::BoundingRect;
    let admit = |_: usize, _: usize, _: Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>| Ok(());
    let profile = spiling_geometry::tolerant::TolerantPolicy::default().profile_mm;
    let slab = [interval[0] + height_mm, interval[1] - height_mm];
    if eligible.0.is_empty() || slab[0] >= slab[1] || !radius.is_finite() {
        return Ok(false);
    }
    let required = region::offset_admitted(eligible, (radius - profile).max(0.0), admit, cancel)?;
    let Some(bounds) = required.bounding_rect() else {
        return Ok(false);
    };
    let within = [
        [bounds.min().x, bounds.min().y],
        [bounds.max().x, bounds.max().y],
    ];
    let mut inner = MultiPolygon(Vec::new());
    for occurrence_id in occurrences {
        check_cancel(cancel)?;
        let Some(model) = models.get(&occurrence_id) else {
            return Ok(false);
        };
        let section = section_at(model, 0.5 * (slab[0] + slab[1]))?;
        let crossings: Vec<Polygon<f64>> = model
            .slab_crossings(slab, within, cancel)
            .map_err(|e| super::native_error(e, "tolerant full-height inner material"))?
            .into_iter()
            .map(polygon)
            .collect();
        check_cancel(cancel)?;
        let material = section.difference(&geo::unary_union(&crossings));
        inner = if inner.0.is_empty() {
            material
        } else {
            inner.union(&material)
        };
    }
    Ok(required
        .difference(&inner)
        .0
        .iter()
        .all(|residue| geo::Area::unsigned_area(residue) <= profile * profile))
}

pub(super) fn polygon(points: Vec<[f64; 2]>) -> Polygon<f64> {
    let mut coords: Vec<geo::Coord<f64>> = points
        .into_iter()
        .map(|[x, y]| geo::Coord { x, y })
        .collect();
    if let Some(first) = coords.first().copied() {
        coords.push(first);
    }
    Polygon::new(LineString(coords), Vec::new())
}

struct ProjectedVoid {
    outer: Polygon<f64>,
    bounds: geo::Rect<f64>,
    scope: spiling_geometry::ProtectedVoidHeightScope,
}

/// Each occurrence's all-height void projections, independent of demand.
/// The facet inventory never changes across schedule retries of one lane, so
/// a model is projected once, on its first relevant demand, and its admitted
/// outer masks are reused by every later pass.
#[derive(Default)]
pub(super) struct VoidMemo {
    models: BTreeMap<OccurrenceId, Vec<ProjectedVoid>>,
    points: usize,
    bytes: usize,
}

impl VoidMemo {
    pub(super) fn retained_bytes(&self) -> usize {
        self.bytes
    }
}

fn project_model(
    model: &TolerantModel,
    cancel: &AtomicBool,
) -> Result<Vec<ProjectedVoid>, ManufacturingError> {
    use geo::{Area, BoundingRect};
    let mut projected = Vec::new();
    for void in model
        .support_voids(cancel)
        .map_err(|e| super::native_error(e, "tolerant all-height void projection"))?
    {
        check_cancel(cancel)?;
        let outer = polygon(void.polygon);
        let Some(bounds) = outer.bounding_rect() else {
            continue;
        };
        let bottom = polygon(void.bottom);
        let floor_parts: Vec<_> = void
            .floor
            .into_iter()
            .map(|triangle| polygon(triangle.to_vec()))
            .collect();
        let floor = geo::unary_union(&floor_parts);
        let closed = bottom.unsigned_area() == 0.0
            || MultiPolygon(vec![bottom]).difference(&floor).0.is_empty();
        let scope = if closed {
            spiling_geometry::ProtectedVoidHeightScope::Bounded {
                lower_mm: void.height_mm[0],
                upper_mm: void.height_mm[1],
            }
        } else {
            spiling_geometry::ProtectedVoidHeightScope::OpenBelow {
                upper_mm: void.height_mm[1],
            }
        };
        projected.push(ProjectedVoid {
            outer,
            bounds,
            scope,
        });
    }
    Ok(projected)
}

pub(super) fn project_protected_voids(
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    memo: &mut VoidMemo,
    demand_bounds: &[[[f64; 2]; 2]],
    truncated: bool,
    retained_units: usize,
    live_bytes: usize,
    cancel: &AtomicBool,
) -> Result<super::support::ProtectedVoidSet, ManufacturingError> {
    let mut regions = Vec::new();
    let mut scopes = Vec::new();
    let mut unclaimed = Vec::new();
    let mut points = 0usize;
    for (occurrence_id, model) in models {
        check_cancel(cancel)?;
        let (min, max) = model.bounds();
        let claimed_model = demand_bounds
            .iter()
            .any(|b| (0..2).all(|a| min[a] <= b[a][1] && max[a] >= b[a][0]));
        if !claimed_model && !truncated {
            continue;
        }
        if !memo.models.contains_key(occurrence_id) {
            let projected = project_model(model, cancel)?;
            let added_points = projected
                .iter()
                .map(|void| void.outer.exterior().0.len())
                .sum::<usize>();
            let added_bytes = projected.capacity() * std::mem::size_of::<ProjectedVoid>()
                + added_points * std::mem::size_of::<geo::Coord<f64>>();
            let memo_points = memo.points.saturating_add(added_points);
            let memo_bytes = memo.bytes.saturating_add(added_bytes);
            super::remaining_geometry_units(
                retained_units.saturating_add(memo_points),
                live_bytes.saturating_add(memo_bytes),
            )?;
            memo.points = memo_points;
            memo.bytes = memo_bytes;
            memo.models.insert(*occurrence_id, projected);
        }
        for void in &memo.models[occurrence_id] {
            check_cancel(cancel)?;
            let bounds = void.bounds;
            if !claimed_model
                || !demand_bounds.iter().any(|b| {
                    bounds.min().x <= b[0][1]
                        && bounds.max().x >= b[0][0]
                        && bounds.min().y <= b[1][1]
                        && bounds.max().y >= b[1][0]
                })
            {
                if truncated {
                    // Gate operands only; never subtracted. Admitted like masks.
                    points = points
                        .checked_add(void.outer.exterior().0.len())
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "tolerant void projection size overflow",
                            )
                        })?;
                    super::remaining_geometry_units(
                        retained_units + points,
                        live_bytes
                            .saturating_add(memo.bytes)
                            .saturating_add(points * std::mem::size_of::<geo::Coord<f64>>()),
                    )?;
                    unclaimed.push((void.outer.clone(), void.scope));
                }
                continue;
            }
            points = points
                .checked_add(void.outer.exterior().0.len())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "tolerant void projection size overflow",
                    )
                })?;
            super::remaining_geometry_units(
                retained_units + points,
                live_bytes
                    .saturating_add(memo.bytes)
                    .saturating_add(points * std::mem::size_of::<geo::Coord<f64>>()),
            )?;
            regions.push(void.outer.clone());
            scopes.push(void.scope);
        }
    }
    Ok(super::support::ProtectedVoidSet {
        regions: MultiPolygon(regions),
        scopes,
        unclaimed,
    })
}
