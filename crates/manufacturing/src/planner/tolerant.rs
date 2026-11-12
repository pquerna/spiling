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
use spiling_contracts::geometry::{AabbMm, OccurrenceId, OccurrenceRecord};
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

fn section_at(model: &TolerantModel, z: f64) -> Result<MultiPolygon<f64>, ManufacturingError> {
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
    let epsilon = model.policy().profile_mm * 0.25;
    result = geo::Simplify::simplify(&result, epsilon);
    result
        .0
        .retain(|polygon| geo::Area::unsigned_area(polygon) > epsilon * epsilon);
    Ok(result)
}

/// One layer as a native-shaped span: material at the layer sample, and an
/// occupied enclosure streamed from the source over the complete interval when
/// supports are enabled; sampled occupancy remains for ordinary classification.
pub(super) fn collect_span(
    occurrences: &[&OccurrenceRecord],
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    native_bounds: &BTreeMap<OccurrenceId, AabbMm>,
    layer: &LayerElevation,
    index: u32,
    supports: bool,
    cancel: &AtomicBool,
) -> Result<native::NativeSpan, ManufacturingError> {
    let (lower, upper, sample) = (layer.lower_z, layer.z, layer.sample);
    let inset = (0.25 * (upper - lower)).min(0.01);
    let mut material = MultiPolygon(Vec::new());
    let mut contributions = Vec::with_capacity(occurrences.len());
    let mut contribution_bytes = 0usize;
    let mut occupied = ContinuousOccupied::default();
    for occurrence in occurrences {
        check_cancel(cancel)?;
        let model = &models[&occurrence.occurrence_id];
        let mut placed = section_at(model, sample)?;
        region::canonicalize(&mut placed);
        let bounds = native_bounds[&occurrence.occurrence_id];
        let height = [lower.max(bounds.min[2]), upper.min(bounds.max[2])];
        if supports {
            occupied.tolerant.push(super::TolerantOccupied {
                model: Arc::clone(model),
                interval: [lower, upper],
            });
        } else if height[0] <= height[1] {
            for z in [lower + inset, sample, upper - inset] {
                for polygon in section_at(model, z)?.0 {
                    occupied.push(polygon, height, Some(occurrence.occurrence_id))?;
                }
            }
        }
        if placed.0.is_empty() {
            continue;
        }
        material = if material.0.is_empty() {
            placed.clone()
        } else {
            material.union(&placed)
        };
        let bytes = native::material_bytes(&mut placed);
        contribution_bytes = contribution_bytes.saturating_add(bytes);
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
    let source_area_mm2 = geo::Area::unsigned_area(&material);
    let material_bytes = native::material_bytes(&mut material);
    let point_count = region::point_count(&material);
    Ok(native::NativeSpan::tolerant(
        index + 1,
        Arc::new(material),
        contributions,
        contribution_bytes,
        Arc::new(occupied),
        source_area_mm2,
        point_count,
        material_bytes,
    ))
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
