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
/// occupied envelope from sections at the bottom, sample and top.
pub(super) fn collect_span(
    occurrences: &[&OccurrenceRecord],
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    native_bounds: &BTreeMap<OccurrenceId, AabbMm>,
    layer: &LayerElevation,
    index: u32,
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
        if height[0] <= height[1] {
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

/// Protected-void masks for support from sampled sections: none. Native masks
/// stand in for cavity geometry the exact path cannot certify. Here every
/// layer's material is sampled, and the support column is clipped against the
/// occupied material of every layer it crosses, so a cavity with a floor
/// blocks the column by that floor and a cavity open to the bed is a real
/// support corridor.
pub(super) fn protected_voids() -> super::support::ProtectedVoidSet {
    super::support::ProtectedVoidSet {
        regions: MultiPolygon(Vec::new()),
        scopes: Vec::new(),
    }
}
