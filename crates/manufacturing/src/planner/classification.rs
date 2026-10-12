// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{boolean_budget, region};
use crate::{check_cancel, error};
use geo::{Area, BooleanOps, MultiPolygon};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, atomic::AtomicBool};

/// Cached only for the current native span. Sharing never clones its polygons.
pub(super) struct WallLayout {
    wall_centers: Vec<MultiPolygon<f64>>,
    fill: MultiPolygon<f64>,
    section_area_mm2: f64,
    wall_area_mm2: f64,
    protected_hole_area_mm2: f64,
    retained_points: usize,
}

impl WallLayout {
    pub(super) fn new(
        section: &MultiPolygon<f64>,
        recipe: &PlanarPrintRecipe,
        retained_native_points: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        retained_budget(retained_native_points, &[])?;
        let wall_centers = region::wall_centers(section, recipe, retained_native_points, cancel)?;
        let wall_points = wall_centers.iter().map(region::point_count).sum::<usize>();
        let base_points = retained_native_points
            .checked_add(wall_points)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "wall layout point accounting overflow",
                )
            })?;
        retained_budget(base_points, &[])?;
        let fill = region::offset(
            section,
            -(recipe.perimeter_count as f64) * recipe.bead_width_mm,
            cancel,
        )?;
        retained_budget(base_points, &[&fill])?;
        let retained_points = base_points + region::point_count(&fill);
        let section_area_mm2 = section.unsigned_area();
        let wall_area_mm2 = (section_area_mm2 - fill.unsigned_area()).max(0.0);
        let protected_hole_area_mm2 = section
            .0
            .iter()
            .flat_map(|p| p.interiors())
            .map(|ring| {
                ring.0
                    .windows(2)
                    .map(|e| e[0].x * e[1].y - e[1].x * e[0].y)
                    .sum::<f64>()
                    .abs()
                    * 0.5
            })
            .sum();
        Ok(Self {
            wall_centers,
            fill,
            section_area_mm2,
            wall_area_mm2,
            protected_hole_area_mm2,
            retained_points,
        })
    }
}

/// Immutable ownership between native material sections and deposition generation.
/// Wall centers are separate from the disjoint interior ownership domains. Holes
/// remain excluded by every Boolean operation; sparse void is a nominal fraction
/// of its interior domain, not a manufactured geometry or strength assertion.
pub(super) struct ClassifiedRegions {
    layout: Arc<WallLayout>,
    bottom: MultiPolygon<f64>,
    top: MultiPolygon<f64>,
    interior: MultiPolygon<f64>,
    coverage: PlanarRegionCoverage,
}

impl ClassifiedRegions {
    pub(super) fn new(
        layout: Arc<WallLayout>,
        survives_below: &MultiPolygon<f64>,
        survives_above: &MultiPolygon<f64>,
        recipe: &PlanarPrintRecipe,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        check_cancel(cancel)?;
        let bottom = difference(&layout.fill, survives_below, cancel)?;
        retained_budget(layout.retained_points, &[&bottom])?;
        let remaining = difference(&layout.fill, &bottom, cancel)?;
        retained_budget(layout.retained_points, &[&bottom, &remaining])?;
        // Bottom ownership wins when physical skin thicknesses overlap.
        let top = difference(&remaining, survives_above, cancel)?;
        retained_budget(layout.retained_points, &[&bottom, &remaining, &top])?;
        let interior = if top.0.is_empty() {
            remaining
        } else {
            let result = difference(&remaining, &top, cancel)?;
            drop(remaining);
            result
        };
        retained_budget(layout.retained_points, &[&bottom, &top, &interior])?;
        let interior_area = interior.unsigned_area();
        let coverage = PlanarRegionCoverage {
            section_area_mm2: layout.section_area_mm2,
            wall_area_mm2: layout.wall_area_mm2,
            bottom_skin_area_mm2: bottom.unsigned_area(),
            top_skin_area_mm2: top.unsigned_area(),
            solid_interior_area_mm2: if recipe.infill_fraction == 1.0 {
                interior_area
            } else {
                0.0
            },
            sparse_interior_area_mm2: if recipe.infill_fraction < 1.0 {
                interior_area
            } else {
                0.0
            },
            intentional_void_area_mm2: interior_area * (1.0 - recipe.infill_fraction),
            protected_hole_area_mm2: layout.protected_hole_area_mm2,
            nominal_deposition_volume_mm3: 0.0,
            requested_infill_fraction: recipe.infill_fraction,
        };
        Ok(Self {
            layout,
            bottom,
            top,
            interior,
            coverage,
        })
    }

    pub(super) fn paths(
        &self,
        z: f64,
        index: u32,
        recipe: &PlanarPrintRecipe,
        segments: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(Vec<DepositionPath>, PlanarRegionCoverage), ManufacturingError> {
        let mut paths = Vec::new();
        for centers in &self.layout.wall_centers {
            region::wall_paths(centers, z, &mut paths, segments, cancel)?;
        }
        let layer = region::HatchLayer { z, index, recipe };
        region::hatch(
            &self.bottom,
            DepositionKind::BottomSkin,
            1.0,
            &layer,
            &mut paths,
            segments,
            cancel,
        )?;
        region::hatch(
            &self.top,
            DepositionKind::TopSkin,
            1.0,
            &layer,
            &mut paths,
            segments,
            cancel,
        )?;
        let kind = if recipe.infill_fraction == 1.0 {
            DepositionKind::SolidFill
        } else {
            DepositionKind::SparseFill
        };
        region::hatch(
            &self.interior,
            kind,
            recipe.infill_fraction,
            &layer,
            &mut paths,
            segments,
            cancel,
        )?;
        if paths.is_empty() {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "section has no printable nominal deposition paths",
            ));
        }
        let h = recipe.layer_height_mm;
        let bead_area = ((recipe.bead_width_mm - h) * h + std::f64::consts::PI * (h * 0.5).powi(2))
            * recipe.flow_multiplier;
        let mut coverage = self.coverage.clone();
        coverage.nominal_deposition_volume_mm3 = paths
            .iter()
            .flat_map(|p| p.points_mm.windows(2))
            .map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]) * bead_area)
            .sum();
        if !coverage.nominal_deposition_volume_mm3.is_finite() {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "nominal deposition volume exceeds finite numeric range",
            ));
        }
        Ok((paths, coverage))
    }
}

pub(super) fn intersection(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    operation(a, b, true, cancel)
}
fn difference(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    operation(a, b, false, cancel)
}
fn operation(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    intersect: bool,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    // These operands are already admitted canonical material domains. Sending
    // an identity through float-to-integer overlay needlessly re-quantizes
    // independent island offsets in the aggregate bounding box, changing area.
    if a.0.is_empty() || (intersect && b.0.is_empty()) {
        return Ok(MultiPolygon(Vec::new()));
    }
    if b.0.is_empty() {
        region::check_size(a)?;
        let result = a.clone();
        check_cancel(cancel)?;
        return Ok(result);
    }
    boolean_budget(a, b)?;
    let mut result = if intersect {
        a.intersection(b)
    } else {
        a.difference(b)
    };
    check_cancel(cancel)?;
    region::check_size(&result)?;
    region::canonicalize(&mut result);
    Ok(result)
}

fn retained_budget(
    wall_points: usize,
    regions: &[&MultiPolygon<f64>],
) -> Result<(), ManufacturingError> {
    let mut total = wall_points;
    if total > MAX_DEPOSITION_SEGMENTS as usize {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "retained classified region point limit exceeded",
        ));
    }
    for domain in regions {
        total = total
            .checked_add(region::point_count(domain))
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "retained classified region point limit exceeded",
                )
            })?;
    }
    Ok(())
}
