// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{boolean_budget, region};
use crate::{check_cancel, error};
use geo::{Area, BooleanOps, BoundingRect, MultiPolygon, Polygon};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, atomic::AtomicBool};

/// One admitted erosion per realized D span, shared by both raster axes.
pub(super) struct WallLayout {
    wall_centers: Vec<MultiPolygon<f64>>,
    fill: MultiPolygon<f64>,
    section_area_mm2: f64,
    wall_area_mm2: f64,
    protected_hole_area_mm2: f64,
    retained_points: usize,
}

impl WallLayout {
    pub(super) fn point_count(&self) -> usize {
        self.retained_points
    }
    pub(super) fn first_wall(&self) -> &MultiPolygon<f64> {
        &self.wall_centers[0]
    }
    pub(super) fn new(
        section: &MultiPolygon<f64>,
        required_material: &region::RequiredMaterial<'_>,
        recipe: &PlanarPrintRecipe,
        retained_geometry_points: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        retained_budget(retained_geometry_points, &[])?;
        let wall_centers = region::wall_centers(
            section,
            required_material,
            recipe,
            retained_geometry_points,
            cancel,
        )?;
        let wall_points = wall_centers.iter().map(region::point_count).sum::<usize>();
        let base_points = retained_geometry_points
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
        let retained_points = wall_points + region::point_count(&fill);
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

/// Immutable ownership between realized D sections and deposition generation.
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

pub(super) struct ComponentAnchorAdmission {
    pub(super) supported: bool,
    /// Strictly ascending indices into this exact component proposal.
    pub(super) bridges: Vec<usize>,
    pub(super) demands: Vec<[[f64; 2]; 2]>,
    pub(super) workspace_points: usize,
}

impl ComponentAnchorAdmission {
    fn retained_points(&self) -> usize {
        self.bridges.capacity() + self.demands.capacity() * 2
    }
}

pub(super) type ComponentAnchorCheck<'a> = dyn FnMut(
        &mut super::support::RasterCandidate,
        region::HatchAxis,
        usize,
    ) -> Result<ComponentAnchorAdmission, ManufacturingError>
    + 'a;

impl ClassifiedRegions {
    pub(super) fn new(
        layout: Arc<WallLayout>,
        survives_below: &MultiPolygon<f64>,
        survives_above: &MultiPolygon<f64>,
        mut admit_scratch: impl FnMut(usize) -> Result<(), ManufacturingError>,
        recipe: &PlanarPrintRecipe,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        check_cancel(cancel)?;
        // Operands already intersect final-D leaf insets at the innermost wall
        // center. The remaining half-bead overlaps the neighboring deposition;
        // never erode this aggregate again or round a complementary skin mask.
        let bottom = difference(&layout.fill, survives_below, cancel)?;
        admit_scratch(scratch_points(&[&bottom])?)?;
        let remaining = if bottom.0.is_empty() {
            layout.fill.clone()
        } else {
            // Both sides of a split use the original operands and coordinate
            // frame, never a newly quantized subtraction of the rounded mask.
            intersection(&layout.fill, survives_below, cancel)?
        };
        admit_scratch(scratch_points(&[&bottom, &remaining])?)?;
        // Bottom ownership wins when physical skin thicknesses overlap.
        let top = difference(&remaining, survives_above, cancel)?;
        admit_scratch(scratch_points(&[&bottom, &remaining, &top])?)?;
        let interior = if top.0.is_empty() {
            remaining
        } else {
            // Intersect the original operands of the top split. Subtracting its
            // already rounded result would introduce a second, different grid.
            let result = intersection(&remaining, survives_above, cancel)?;
            drop(remaining);
            result
        };
        admit_scratch(scratch_points(&[&bottom, &top, &interior])?)?;
        let interior_area = interior.unsigned_area();
        let coverage = PlanarRegionCoverage {
            section_area_mm2: layout.section_area_mm2,
            source_section_area_mm2: layout.section_area_mm2,
            surface_added_area_mm2: 0.0,
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
            support_area_mm2: 0.0,
            support_nominal_deposition_volume_mm3: 0.0,
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
        height: f64,
        axis: region::HatchAxis,
        recipe: &PlanarPrintRecipe,
        segments: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(Vec<DepositionPath>, PlanarRegionCoverage), ManufacturingError> {
        let mut paths = Vec::new();
        for centers in &self.layout.wall_centers {
            region::wall_paths(centers, z, &mut paths, segments, cancel)?;
        }
        let layer = region::HatchLayer {
            z,
            height,
            axis,
            recipe,
        };
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
        let h = height;
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

    /// Preserve authored role lattices outside demand-bound bridge cells and
    /// their actual anchor corridors. All ownership stays in this same fill.
    pub(super) fn paths_with_bridge_recovery(
        &self,
        layer: &region::HatchLayer<'_>,
        segments: &mut usize,
        mut admit: impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
        anchors: &mut ComponentAnchorCheck<'_>,
        cancel: &AtomicBool,
    ) -> Result<Vec<DepositionPath>, ManufacturingError> {
        let initial_segments = *segments;
        let mut normal = super::support::RasterCandidate {
            paths: Vec::new(),
            whole_rows: Vec::new(),
        };
        for centers in &self.layout.wall_centers {
            region::wall_paths(centers, layer.z, &mut normal.paths, segments, cancel)?;
        }
        let wall_count = normal.paths.len();
        let mut normal_points = path_points(&normal.paths);
        let class_points = scratch_points(&[&self.bottom, &self.top, &self.interior])?;
        let mut bridge = super::support::RasterCandidate {
            paths: Vec::new(),
            whole_rows: Vec::new(),
        };
        let mut bridge_points = 0;
        let mut patches: Vec<BridgePatch<'_>> = Vec::new();
        let mut owners = Vec::new();
        let owner_count = self.bottom.0.len() + self.top.0.len() + self.interior.0.len();
        let mut probe = super::support::RasterCandidate {
            paths: Vec::new(),
            whole_rows: Vec::new(),
        };
        let base = point_sum(class_points, normal_points)?;
        let workspace = anchors(&mut probe, layer.axis, base)?.workspace_points;
        admit(point_sum(point_sum(base, workspace)?, owner_count)?, 0)?;
        owners.try_reserve_exact(owner_count).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "role owner allocation refused",
            )
        })?;
        let interior_kind = if layer.recipe.infill_fraction == 1.0 {
            DepositionKind::SolidFill
        } else {
            DepositionKind::SparseFill
        };
        let alternate_layer = region::HatchLayer {
            axis: layer.axis.orthogonal(),
            ..*layer
        };
        let normal_along = usize::from(layer.axis == region::HatchAxis::Y);
        let normal_fixed = 1 - normal_along;
        for (domain, kind, fraction) in [
            (&self.bottom, DepositionKind::BottomSkin, 1.0),
            (&self.top, DepositionKind::TopSkin, 1.0),
            (&self.interior, interior_kind, layer.recipe.infill_fraction),
        ] {
            for polygon in &domain.0 {
                check_cancel(cancel)?;
                let base = recovery_points(
                    class_points,
                    normal_points,
                    bridge_points,
                    owners.capacity(),
                    patches.capacity(),
                )?;
                let workspace = anchors(&mut probe, layer.axis, base)?.workspace_points;
                let rows = region::HatchRows::new(polygon, fraction, layer)?;
                let bound = rows.point_bound()?;
                admit(point_sum(point_sum(base, workspace)?, bound)?, bound)?;
                let mut component = super::support::RasterCandidate {
                    paths: Vec::new(),
                    whole_rows: Vec::new(),
                };
                region::hatch_polygon(
                    polygon,
                    kind,
                    &rows,
                    layer,
                    &mut component.paths,
                    segments,
                    cancel,
                )?;
                let component_points = path_points(&component.paths);
                let live = point_sum(base, component_points)?;
                let original = anchors(&mut component, layer.axis, live)?;
                if !original.supported && !original.demands.is_empty() {
                    let alternate_rows =
                        region::HatchRows::new(polygon, fraction, &alternate_layer)?;
                    let bound = alternate_rows.point_bound()?;
                    let live = point_sum(live, original.retained_points())?;
                    admit(
                        point_sum(point_sum(live, original.workspace_points)?, bound)?,
                        bound,
                    )?;
                    let mut alternative = super::support::RasterCandidate {
                        paths: Vec::new(),
                        whole_rows: Vec::new(),
                    };
                    let mut staged_segments = *segments;
                    region::hatch_polygon(
                        polygon,
                        kind,
                        &alternate_rows,
                        &alternate_layer,
                        &mut alternative.paths,
                        &mut staged_segments,
                        cancel,
                    )?;
                    let alternative_points = path_points(&alternative.paths);
                    let live = point_sum(live, alternative_points)?;
                    let whole_bound = whole_row_bound(&self.layout.fill, alternative.paths.len())?;
                    admit(
                        point_sum(point_sum(live, original.workspace_points)?, whole_bound)?,
                        whole_bound,
                    )?;
                    alternative.whole_rows = self.whole_rows(
                        &alternative.paths,
                        &alternate_layer,
                        &mut staged_segments,
                        cancel,
                    )?;
                    let whole_points = path_points(&alternative.whole_rows);
                    let mut proof = anchors(
                        &mut alternative,
                        alternate_layer.axis,
                        point_sum(live, whole_points)?,
                    )?;
                    let actual_live =
                        point_sum(live - alternative_points, path_points(&alternative.paths))?;
                    let proof_live = point_sum(
                        point_sum(actual_live, proof.retained_points())?,
                        proof.workspace_points,
                    )?;
                    let bands = bridge_bands(
                        (polygon, alternate_layer.axis),
                        &alternative.paths,
                        (&proof.bridges, &proof.demands),
                        &original.demands,
                        proof_live,
                        &mut admit,
                        cancel,
                    )?;
                    let band_points = bands.capacity() * 2;
                    let patch_base = point_sum(proof_live, band_points)? - patches.capacity() * 2;
                    let selection_work = alternative
                        .paths
                        .len()
                        .checked_mul(
                            original.demands.len() + usize::BITS as usize
                                - alternative.paths.len().max(1).leading_zeros() as usize
                                + 3,
                        )
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "bridge demand selection work overflow",
                            )
                        })?;
                    admit(point_sum(proof_live, band_points)?, selection_work)?;
                    let bounds = polygon.bounding_rect().ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            "bridge role owner has no bounds",
                        )
                    })?;
                    let across_bounds = if normal_along == 0 {
                        [bounds.min().x, bounds.max().x]
                    } else {
                        [bounds.min().y, bounds.max().y]
                    };
                    let mut keep = 0;
                    let mut band_index = 0;
                    for offset in 0..proof.bridges.len() {
                        let index = proof.bridges[offset];
                        while band_index < bands.len() && bands[band_index].paths.end <= index {
                            band_index += 1;
                        }
                        if band_index == bands.len() || !bands[band_index].paths.contains(&index) {
                            continue;
                        }
                        let path = &alternative.paths[index];
                        let along = if normal_fixed == 0 {
                            [bounds.min().x, bounds.max().x]
                        } else {
                            [bounds.min().y, bounds.max().y]
                        };
                        let coordinate = path.points_mm[0][normal_along];
                        let first = alternative
                            .paths
                            .partition_point(|path| path.points_mm[0][normal_along] < coordinate);
                        let end = alternative
                            .paths
                            .partition_point(|path| path.points_mm[0][normal_along] <= coordinate);
                        let mut lo = if first == 0 {
                            across_bounds[0]
                        } else {
                            let previous = alternative.paths[first - 1].points_mm[0][normal_along];
                            previous + (coordinate - previous) * 0.5
                        };
                        let mut hi = alternative.paths.get(end).map_or(across_bounds[1], |next| {
                            coordinate + (next.points_mm[0][normal_along] - coordinate) * 0.5
                        });
                        // At a run edge, a neighboring fully-supported row is
                        // a finite boundary proposal, not an extra emitted row.
                        if first == bands[band_index].paths.start {
                            lo = bands[band_index].interval[0];
                        }
                        if end == bands[band_index].paths.end {
                            hi = bands[band_index].interval[1];
                        }
                        if index == first {
                            reserve_recovery_record(&mut patches, 2, patch_base, &mut admit)?;
                            patches.push(BridgePatch {
                                owner: Some(polygon),
                                along,
                                across: [lo, hi],
                            });
                        }
                        proof.bridges[keep] = index;
                        keep += 1;
                    }
                    proof.bridges.truncate(keep);
                    let mut selected = proof.bridges.iter().copied().peekable();
                    for (index, mut path) in alternative.paths.into_iter().enumerate() {
                        if selected.peek() == Some(&index) {
                            selected.next();
                            path.kind = DepositionKind::Bridge;
                            bridge_points = point_sum(bridge_points, path.points_mm.capacity())?;
                            bridge.paths.push(path);
                        }
                    }
                }
                let begin = normal.paths.len();
                normal.paths.append(&mut component.paths);
                owners.push(NormalOwner {
                    owner: polygon,
                    paths: begin..normal.paths.len(),
                });
                normal_points = point_sum(normal_points, component_points)?;
            }
        }
        if bridge.paths.is_empty() {
            return Ok(normal.paths);
        }
        let base = recovery_points(
            class_points,
            normal_points,
            bridge_points,
            owners.capacity(),
            patches.capacity(),
        )?;
        let workspace = anchors(&mut probe, layer.axis, base)?.workspace_points;
        let bound = whole_row_bound(&self.layout.fill, bridge.paths.len())?;
        admit(point_sum(point_sum(base, workspace)?, bound)?, bound)?;
        let mut staged_segments = *segments;
        bridge.whole_rows = self.whole_rows(
            &bridge.paths,
            &alternate_layer,
            &mut staged_segments,
            cancel,
        )?;
        let whole_points = path_points(&bridge.whole_rows);
        let merge = anchors(
            &mut bridge,
            alternate_layer.axis,
            point_sum(base, whole_points)?,
        )?;
        if !merge.supported || merge.bridges.len() != bridge.paths.len() {
            return Ok(normal.paths);
        }
        bridge_points = path_points(&bridge.paths);
        let base = point_sum(
            recovery_points(
                class_points,
                normal_points,
                bridge_points,
                owners.capacity(),
                0,
            )?,
            merge.retained_points(),
        )?;
        for path in &bridge.paths {
            reserve_recovery_record(
                &mut patches,
                2,
                point_sum(base, merge.workspace_points)?,
                &mut admit,
            )?;
            let coordinate = path.points_mm[0][normal_along];
            patches.push(BridgePatch {
                owner: None,
                along: [
                    path.points_mm[0][normal_fixed].min(path.points_mm[1][normal_fixed]),
                    path.points_mm[0][normal_fixed].max(path.points_mm[1][normal_fixed]),
                ],
                across: [
                    coordinate - layer.recipe.bead_width_mm * 0.5,
                    coordinate + layer.recipe.bead_width_mm * 0.5,
                ],
            });
        }
        let staged = point_sum(base, patches.capacity() * 2)?;
        let work = normal
            .paths
            .len()
            .checked_mul(patches.len())
            .and_then(|n| {
                n.checked_mul(
                    usize::BITS as usize - patches.len().max(1).leading_zeros() as usize + 3,
                )
            })
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "corridor clipping work overflow",
                )
            })?;
        admit(
            point_sum(point_sum(staged, merge.workspace_points)?, patches.len())?,
            work,
        )?;
        let mut intervals: Vec<[f64; 2]> = Vec::new();
        intervals.try_reserve_exact(patches.len()).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "corridor interval allocation refused",
            )
        })?;
        let staged = point_sum(staged, intervals.capacity())?;
        let mut mixed = super::support::RasterCandidate {
            paths: Vec::new(),
            whole_rows: Vec::new(),
        };
        let mut mixed_points = 0;
        let mut output_segments = initial_segments;
        for owner in &owners {
            for path in &normal.paths[owner.paths.clone()] {
                check_cancel(cancel)?;
                intervals.clear();
                let a = path.points_mm[0];
                let b = path.points_mm[1];
                for patch in &patches {
                    check_cancel(cancel)?;
                    if patch
                        .owner
                        .is_none_or(|polygon| std::ptr::eq(polygon, owner.owner))
                        && a[normal_fixed] >= patch.along[0]
                        && a[normal_fixed] <= patch.along[1]
                    {
                        intervals.push(patch.across);
                    }
                }
                intervals
                    .sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
                let mut cursor = a[normal_along].min(b[normal_along]);
                let end = a[normal_along].max(b[normal_along]);
                for interval in &intervals {
                    if interval[1] <= cursor {
                        continue;
                    }
                    if interval[0] >= end {
                        break;
                    }
                    if interval[0] > cursor {
                        admit(
                            point_sum(
                                point_sum(
                                    point_sum(staged, merge.workspace_points)?,
                                    mixed_points,
                                )?,
                                2,
                            )?,
                            0,
                        )?;
                        let before = mixed.paths.len();
                        region::append_hatch_row(
                            &[cursor, interval[0].min(end)],
                            a[normal_fixed],
                            path.kind,
                            layer,
                            a[normal_along] > b[normal_along],
                            &mut mixed.paths,
                            &mut output_segments,
                        )?;
                        if mixed.paths.len() != before {
                            mixed_points += mixed.paths.last().unwrap().points_mm.capacity();
                        }
                    }
                    cursor = cursor.max(interval[1]);
                    if cursor >= end {
                        break;
                    }
                }
                if cursor < end {
                    admit(
                        point_sum(
                            point_sum(point_sum(staged, merge.workspace_points)?, mixed_points)?,
                            2,
                        )?,
                        0,
                    )?;
                    let before = mixed.paths.len();
                    region::append_hatch_row(
                        &[cursor, end],
                        a[normal_fixed],
                        path.kind,
                        layer,
                        a[normal_along] > b[normal_along],
                        &mut mixed.paths,
                        &mut output_segments,
                    )?;
                    if mixed.paths.len() != before {
                        mixed_points += mixed.paths.last().unwrap().points_mm.capacity();
                    }
                }
            }
        }
        output_segments = output_segments
            .checked_add(
                bridge
                    .paths
                    .iter()
                    .map(|path| path.points_mm.len() - 1)
                    .sum::<usize>(),
            )
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "mixed corridor segment limit exceeded",
                )
            })?;
        mixed.paths.append(&mut bridge.paths);
        let proof = anchors(&mut mixed, layer.axis, point_sum(staged, mixed_points)?)?;
        if !proof.supported {
            return Ok(normal.paths);
        }
        let mut result = Vec::with_capacity(wall_count + mixed.paths.len());
        for path in normal.paths.into_iter().take(wall_count) {
            output_segments = output_segments
                .checked_add(path.points_mm.len() - 1)
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "mixed corridor wall limit exceeded",
                    )
                })?;
            result.push(path);
        }
        result.append(&mut mixed.paths);
        *segments = output_segments;
        Ok(result)
    }

    /// Positive-fill row bounds at the actual classified hatch coordinates.
    /// These permit bounded anchor extensions, never unrequested sparse rows.
    pub(super) fn whole_rows(
        &self,
        required: &[DepositionPath],
        layer: &region::HatchLayer<'_>,
        segments: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<DepositionPath>, ManufacturingError> {
        region::bridge_rows(&self.layout.fill, required, layer, segments, cancel)
    }
}

fn path_points(paths: &[DepositionPath]) -> usize {
    paths.iter().map(|path| path.points_mm.capacity()).sum()
}

pub(super) fn intersection(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    operation(a, b, true, None, cancel)
}

pub(super) fn intersection_accumulated(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    operation(a, b, true, Some(work), cancel)
}
fn difference(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    operation(a, b, false, None, cancel)
}
fn operation(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    intersect: bool,
    work: Option<&mut usize>,
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
    match work {
        Some(work) => super::boolean::budget_accumulated(a, b, work, cancel)?,
        None => boolean_budget(a, b, cancel)?,
    }
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

/// Fixed authority for consumers with no reclaimable derived geometry.
pub(super) fn fixed_point_budget(
    retained_points: usize,
) -> impl FnMut(usize) -> Result<(), ManufacturingError> {
    move |scratch| {
        retained_points
            .checked_add(scratch)
            .filter(|total| *total <= MAX_DEPOSITION_SEGMENTS as usize)
            .map(|_| ())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "retained classified region point limit exceeded",
                )
            })
    }
}

fn scratch_points(regions: &[&MultiPolygon<f64>]) -> Result<usize, ManufacturingError> {
    regions.iter().try_fold(0usize, |points, domain| {
        points
            .checked_add(region::point_count(domain))
            .filter(|total| *total <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "classified scratch point limit exceeded",
                )
            })
    })
}

fn retained_budget(
    retained_points: usize,
    regions: &[&MultiPolygon<f64>],
) -> Result<(), ManufacturingError> {
    fixed_point_budget(retained_points)(scratch_points(regions)?)
}

struct NormalOwner<'a> {
    owner: &'a Polygon<f64>,
    paths: std::ops::Range<usize>,
}

struct BridgePatch<'a> {
    // A full hatch cell belongs only to its classified role. Actual bead
    // corridors reserve cross-role anchor extrusion without erasing skin cells.
    owner: Option<&'a Polygon<f64>>,
    along: [f64; 2],
    across: [f64; 2],
}

fn recovery_points(
    class: usize,
    normal: usize,
    bridge: usize,
    owners: usize,
    patches: usize,
) -> Result<usize, ManufacturingError> {
    point_sum(
        point_sum(point_sum(point_sum(class, normal)?, bridge)?, owners)?,
        patches.checked_mul(2).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "bridge patch retention overflow",
            )
        })?,
    )
}

fn whole_row_bound(fill: &MultiPolygon<f64>, rows: usize) -> Result<usize, ManufacturingError> {
    region::point_count(fill)
        .checked_mul(rows + 2)
        .and_then(|points| points.checked_add(rows.checked_mul(8)?))
        .filter(|points| *points <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "bridge corridor row bound exceeded",
            )
        })
}

fn reserve_recovery_record<T>(
    records: &mut Vec<T>,
    units: usize,
    other: usize,
    admit: &mut impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
) -> Result<(), ManufacturingError> {
    if records.len() == records.capacity() {
        let remaining = (MAX_DEPOSITION_SEGMENTS as usize)
            .checked_sub(other)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "bridge recovery workspace exceeded",
                )
            })?
            / units;
        let capacity = records.capacity().saturating_mul(2).max(4).min(remaining);
        if capacity <= records.len() {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "bridge recovery record limit exceeded",
            ));
        }
        admit(point_sum(other, capacity * units)?, 0)?;
        records
            .try_reserve_exact(capacity - records.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "bridge recovery allocation refused",
                )
            })?;
    }
    Ok(())
}

struct BridgeBand {
    interval: [f64; 2],
    paths: std::ops::Range<usize>,
}

fn point_sum(a: usize, b: usize) -> Result<usize, ManufacturingError> {
    a.checked_add(b)
        .filter(|total| *total <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "component staging point budget exceeded",
            )
        })
}

/// Bands partition the same borrowed role polygon by exact axis intervals.
/// Every selected row is unchanged, wholly Bridge-certified and demand-bound.
fn bridge_bands(
    (polygon, axis): (&Polygon<f64>, region::HatchAxis),
    paths: &[DepositionPath],
    (bridges, alternative_demands): (&[usize], &[[[f64; 2]; 2]]),
    demands: &[[[f64; 2]; 2]],
    scratch: usize,
    admit: &mut impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<Vec<BridgeBand>, ManufacturingError> {
    if paths.is_empty() || bridges.is_empty() || demands.is_empty() {
        return Ok(Vec::new());
    }
    let work = demands
        .len()
        .checked_add(alternative_demands.len())
        .and_then(|n| n.checked_add(3))
        .and_then(|n| paths.len().checked_mul(n))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "bridge band work overflow",
            )
        })?;
    admit(scratch, work)?;
    let fixed = usize::from(axis == region::HatchAxis::X);
    let bounds = polygon.bounding_rect().ok_or_else(|| {
        error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "bridge owner has no finite bounds",
        )
    })?;
    let (min, max) = if fixed == 0 {
        (bounds.min().x, bounds.max().x)
    } else {
        (bounds.min().y, bounds.max().y)
    };
    let mut bands: Vec<BridgeBand> = Vec::new();
    let mut push = |band: BridgeBand| -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        if !demands.iter().any(|gap| {
            gap[0][fixed].min(gap[1][fixed]).max(band.interval[0])
                < gap[0][fixed].max(gap[1][fixed]).min(band.interval[1])
                && paths[band.paths.clone()].iter().any(|path| {
                    let a = path.points_mm[0][1 - fixed];
                    let b = path.points_mm[1][1 - fixed];
                    gap[0][1 - fixed] >= a.min(b) && gap[0][1 - fixed] <= a.max(b)
                })
        }) {
            return Ok(());
        }
        if bands.len() == bands.capacity() {
            let capacity = bands.capacity().saturating_mul(2).max(4).min(paths.len());
            admit(point_sum(scratch, capacity * 2)?, 0)?;
            bands
                .try_reserve_exact(capacity - bands.len())
                .map_err(|_| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "bridge band allocation refused",
                    )
                })?;
        }
        bands.push(band);
        Ok(())
    };
    let mut certified = bridges.iter().copied().peekable();
    let mut cursor = 0;
    let mut previous = None;
    let mut active: Option<BridgeBand> = None;
    while cursor < paths.len() {
        check_cancel(cancel)?;
        let coordinate = paths[cursor].points_mm[0][fixed];
        let first = cursor;
        while cursor < paths.len() && paths[cursor].points_mm[0][fixed] == coordinate {
            cursor += 1;
        }
        let mut genuine = true;
        let mut any_bridge = false;
        for index in first..cursor {
            while certified.peek().is_some_and(|value| *value < index) {
                certified.next();
            }
            let bridge = certified.peek() == Some(&index);
            genuine &= bridge;
            any_bridge |= bridge;
        }
        let supported = !any_bridge
            && !alternative_demands
                .iter()
                .any(|gap| gap[0][fixed] == coordinate);
        let lo = previous.map_or(min, |(value, supported): (f64, bool)| {
            if supported {
                value
            } else {
                value + (coordinate - value) * 0.5
            }
        });
        let hi = paths.get(cursor).map_or(max, |path| {
            coordinate + (path.points_mm[0][fixed] - coordinate) * 0.5
        });
        if genuine && lo < hi {
            if let Some(band) = &mut active {
                band.interval[1] = hi;
                band.paths.end = cursor;
            } else {
                active = Some(BridgeBand {
                    interval: [lo, hi],
                    paths: first..cursor,
                });
            }
        } else if let Some(mut band) = active.take() {
            if supported {
                band.interval[1] = coordinate;
            }
            push(band)?;
        }
        previous = Some((coordinate, supported));
    }
    if let Some(band) = active {
        push(band)?;
    }
    Ok(bands)
}
