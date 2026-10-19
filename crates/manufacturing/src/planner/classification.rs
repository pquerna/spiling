// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{boolean_budget, region};
use crate::{check_cancel, error};
use geo::{
    Area, BooleanOps, BoundingRect, Contains, Intersects, Line, MultiPolygon, Point, Polygon,
};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, atomic::AtomicBool};

/// One admitted erosion per realized D span, shared by both raster axes.
pub(super) struct WallLayout {
    wall_centers: Vec<MultiPolygon<f64>>,
    pub(super) fill: MultiPolygon<f64>,
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
    pub(super) layout: Arc<WallLayout>,
    pub(super) bottom: MultiPolygon<f64>,
    pub(super) top: MultiPolygon<f64>,
    pub(super) interiors: Vec<InteriorRegion>,
    pub(super) separator_bands: Vec<MultiPolygon<f64>>,
    pub(super) coverage: PlanarRegionCoverage,
    pub(super) applied_operations: Vec<(
        spiling_contracts::geometry::OccurrenceId,
        spiling_contracts::process::ProcessProvenance,
    )>,
}

pub(super) struct InteriorRegion {
    pub(super) domain: MultiPolygon<f64>,
    pub(super) coverage: ProcessRegionCoverage,
    pub(super) frame_origin_mm: Option<[f64; 2]>,
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
    pub(super) fn retained_points(&self) -> usize {
        region::point_count(&self.bottom)
            + region::point_count(&self.top)
            + self
                .separator_bands
                .iter()
                .map(region::point_count)
                .sum::<usize>()
            + self
                .interiors
                .iter()
                .map(|i| region::point_count(&i.domain))
                .sum::<usize>()
            + super::bonds::coverage_bytes(&self.coverage)
                .div_ceil(std::mem::size_of::<geo::Coord<f64>>())
    }
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
            separator_area_mm2: 0.0,
            process_interfaces: Vec::new(),
            process_regions: if interior_area > 0.0 {
                vec![ProcessRegionCoverage {
                    area_mm2: interior_area,
                    fraction: recipe.infill_fraction,
                    contributions: Vec::new(),
                }]
            } else {
                Vec::new()
            },
            support_area_mm2: 0.0,
            support_nominal_deposition_volume_mm3: 0.0,
        };
        Ok(Self {
            layout,
            bottom,
            top,
            interiors: if interior_area > 0.0 {
                vec![InteriorRegion {
                    domain: interior,
                    coverage: coverage.process_regions[0].clone(),
                    frame_origin_mm: None,
                }]
            } else {
                Vec::new()
            },
            separator_bands: Vec::new(),
            coverage,
            applied_operations: Vec::new(),
        })
    }

    /// Resolve only the eligible interior. Global walls, physical skins and
    /// protected holes keep the single final-D classification above.
    pub(super) fn with_process(
        mut self,
        domains: &[super::ProcessDomain],
        recipe: &PlanarPrintRecipe,
        admit: impl FnMut(usize) -> Result<(), ManufacturingError>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        if domains.is_empty() || self.interiors.is_empty() {
            return Ok(self);
        }
        // Spatial cuts retain each captured source owner. Coalescing their base
        // fractions first would leave stale contributors after one owner is cut.
        let spatial = domains.iter().any(|domain| domain.zone.is_some());
        if !spatial
            && domains
                .iter()
                .all(|domain| domain.coverage.fraction == domains[0].coverage.fraction)
        {
            let fraction = domains[0].coverage.fraction;
            let interior = &mut self.interiors[0];
            interior.coverage.fraction = fraction;
            interior.coverage.contributions = domains
                .iter()
                .flat_map(|domain| domain.coverage.contributions.iter().cloned())
                .collect();
            let area = interior.coverage.area_mm2;
            self.coverage.process_regions = vec![interior.coverage.clone()];
            self.coverage.solid_interior_area_mm2 = if fraction == 1.0 { area } else { 0.0 };
            self.coverage.sparse_interior_area_mm2 = if fraction < 1.0 { area } else { 0.0 };
            self.coverage.intentional_void_area_mm2 = area * (1.0 - fraction);
            return super::zones::partition(self, domains, recipe, admit, work, cancel);
        }
        let original = self
            .interiors
            .pop()
            .expect("initial global interior")
            .domain;
        let skin_points = scratch_points(&[&self.bottom, &self.top])?;
        let mut outer_admit = admit;
        let mut admit = |scratch| outer_admit(point_sum(skin_points, scratch)?);
        admit(scratch_points(&[&original, &original])?)?;
        let mut remaining = original.clone();
        let mut completed: Vec<InteriorRegion> = Vec::new();
        let mut partial_cut = false;
        for (domain_index, domain) in domains.iter().enumerate() {
            let fraction = domain.coverage.fraction;
            if !spatial
                && domains[..domain_index]
                    .iter()
                    .any(|prior| prior.coverage.fraction == fraction)
            {
                continue;
            }
            check_cancel(cancel)?;
            let mut material = Arc::clone(&domain.material);
            let mut contributors = Vec::new();
            for (index, source) in domains
                .iter()
                .filter(|source| {
                    if spatial {
                        std::ptr::eq(*source, domain)
                    } else {
                        source.coverage.fraction == fraction
                    }
                })
                .enumerate()
            {
                contributors.extend(source.coverage.contributions.iter().cloned());
                if index > 0 {
                    admit(process_scratch(
                        &original,
                        &remaining,
                        &completed,
                        &[&material, &source.material, &material, &source.material],
                    )?)?;
                    let merged = union_accumulated(&material, &source.material, work, cancel)?;
                    admit(process_scratch(
                        &original,
                        &remaining,
                        &completed,
                        &[&material, &merged],
                    )?)?;
                    material = Arc::new(merged);
                }
            }
            let (owned, next, partial) = split_process_region(remaining, &material, work, cancel)?;
            partial_cut |= partial;
            admit(process_scratch(
                &original,
                &next,
                &completed,
                &[&material, &owned],
            )?)?;
            remaining = next;
            let area = owned.unsigned_area();
            if !owned.0.is_empty() {
                completed.push(InteriorRegion {
                    domain: owned,
                    frame_origin_mm: None,
                    coverage: ProcessRegionCoverage {
                        area_mm2: area,
                        fraction,
                        contributions: contributors,
                    },
                });
            }
        }
        if !remaining.0.is_empty() {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "retained occurrence final-D contributions do not cover eligible process interior",
            ));
        }
        // Moving each original admitted island exactly once is itself an exact
        // disjoint/complete partition. Only actual partial Boolean cuts need
        // the extra output-set checks; area sums never establish ownership.
        for (index, owned) in completed.iter().enumerate().filter(|_| partial_cut) {
            let excess = operation(&owned.domain, &original, false, Some(work), cancel)?;
            admit(process_scratch(
                &original,
                &remaining,
                &completed,
                &[&excess],
            )?)?;
            if !excess.0.is_empty() {
                return Err(error(
                    ManufacturingErrorCode::InvalidSpecification,
                    "resolved process region escapes eligible interior",
                ));
            }
            for other in &completed[..index] {
                let overlap = intersection_accumulated(&owned.domain, &other.domain, work, cancel)?;
                admit(process_scratch(
                    &original,
                    &remaining,
                    &completed,
                    &[&overlap],
                )?)?;
                if !overlap.0.is_empty() {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "resolved process interior ownership overlaps",
                    ));
                }
            }
        }
        if partial_cut {
            let mut covered = MultiPolygon(Vec::new());
            for region in &completed {
                admit(process_scratch(
                    &original,
                    &remaining,
                    &completed,
                    &[&covered, &region.domain, &covered, &region.domain],
                )?)?;
                let union = union_accumulated(&covered, &region.domain, work, cancel)?;
                admit(process_scratch(
                    &original,
                    &remaining,
                    &completed,
                    &[&covered, &union],
                )?)?;
                covered = union;
            }
            let missing = operation(&original, &covered, false, Some(work), cancel)?;
            admit(process_scratch(
                &original,
                &remaining,
                &completed,
                &[&covered, &missing],
            )?)?;
            if !missing.0.is_empty() {
                return Err(error(
                    ManufacturingErrorCode::InvalidSpecification,
                    "partial process partition leaves eligible interior uncovered",
                ));
            }
        }
        self.coverage.process_regions = completed
            .iter()
            .map(|region| region.coverage.clone())
            .collect();
        self.coverage.solid_interior_area_mm2 = completed
            .iter()
            .filter(|region| region.coverage.fraction == 1.0)
            .map(|region| region.coverage.area_mm2)
            .sum();
        self.coverage.sparse_interior_area_mm2 = completed
            .iter()
            .filter(|region| region.coverage.fraction < 1.0)
            .map(|region| region.coverage.area_mm2)
            .sum();
        self.coverage.intentional_void_area_mm2 = completed
            .iter()
            .map(|region| region.coverage.area_mm2 * (1.0 - region.coverage.fraction))
            .sum();
        self.interiors = completed;
        super::zones::partition(self, domains, recipe, outer_admit, work, cancel)
    }

    pub(super) fn paths(
        &self,
        z: f64,
        height: f64,
        axis: region::HatchAxis,
        recipe: &PlanarPrintRecipe,
        segments: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(Vec<DepositionPath>, &PlanarRegionCoverage), ManufacturingError> {
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
        for (region_index, interior) in self.interiors.iter().enumerate() {
            let fraction = interior.coverage.fraction;
            let begin = paths.len();
            region::hatch_with_phase(
                &interior.domain,
                if fraction == 1.0 {
                    DepositionKind::SolidFill
                } else {
                    DepositionKind::SparseFill
                },
                fraction,
                &layer,
                interior.frame_origin_mm,
                &mut paths,
                segments,
                cancel,
            )?;
            for path in &mut paths[begin..] {
                path.owner = Some(DepositionOwner::Regional {
                    region_index: region_index as u16,
                });
            }
        }
        if paths.is_empty() {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "section has no printable nominal deposition paths",
            ));
        }
        // Final actual-stroke volume is computed once after support/recovery,
        // including constructed separator endpoints and genuine Bridge process.
        Ok((paths, &self.coverage))
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
            process_interfaces: Vec::new(),
        };
        for centers in &self.layout.wall_centers {
            region::wall_paths(centers, layer.z, &mut normal.paths, segments, cancel)?;
        }
        let wall_count = normal.paths.len();
        let mut normal_points = path_points(&normal.paths);
        let class_points = scratch_points(&[&self.bottom, &self.top])?
            + self
                .interiors
                .iter()
                .map(|region| region::point_count(&region.domain))
                .sum::<usize>();
        let mut bridge = super::support::RasterCandidate {
            paths: Vec::new(),
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
        };
        let mut bridge_points = 0;
        let mut patches: Vec<BridgePatch<'_>> = Vec::new();
        let mut owners = Vec::new();
        let owner_count = self.bottom.0.len()
            + self.top.0.len()
            + self
                .interiors
                .iter()
                .map(|region| region.domain.0.len())
                .sum::<usize>();
        let mut probe = super::support::RasterCandidate {
            paths: Vec::new(),
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
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
        let alternate_layer = region::HatchLayer {
            axis: layer.axis.orthogonal(),
            ..*layer
        };
        let normal_along = usize::from(layer.axis == region::HatchAxis::Y);
        let normal_fixed = 1 - normal_along;
        for (domain, kind, fraction, owner) in [
            (&self.bottom, DepositionKind::BottomSkin, 1.0, None),
            (&self.top, DepositionKind::TopSkin, 1.0, None),
        ]
        .into_iter()
        .chain(self.interiors.iter().enumerate().map(|(index, interior)| {
            let fraction = interior.coverage.fraction;
            (
                &interior.domain,
                if fraction == 1.0 {
                    DepositionKind::SolidFill
                } else {
                    DepositionKind::SparseFill
                },
                fraction,
                Some((index as u16, interior)),
            )
        })) {
            let frame = owner.and_then(|(_, owner)| owner.frame_origin_mm);
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
                let rows = if let Some(origin) = frame {
                    region::HatchRows::common(polygon, fraction, layer, origin)?
                } else {
                    region::HatchRows::new(polygon, fraction, layer)?
                };
                let bound = rows.point_bound()?;
                admit(point_sum(point_sum(base, workspace)?, bound)?, bound)?;
                let mut component = super::support::RasterCandidate {
                    paths: Vec::new(),
                    whole_rows: Vec::new(),
                    process_interfaces: Vec::new(),
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
                if let Some((index, _)) = owner {
                    for path in &mut component.paths {
                        path.owner = Some(DepositionOwner::Regional {
                            region_index: index,
                        });
                    }
                }
                let component_points = path_points(&component.paths);
                let live = point_sum(base, component_points)?;
                let original = anchors(&mut component, layer.axis, live)?;
                if !original.supported && !original.demands.is_empty() {
                    let alternate_rows = if let Some(mut origin) = frame {
                        let fixed = usize::from(alternate_layer.axis == region::HatchAxis::X);
                        if let Some(interface) =
                            self.coverage.process_interfaces.iter().find(|interface| {
                                usize::from(interface.axis) == fixed
                                    && owner.is_some_and(|(_, owner)| {
                                        owner.coverage.contributions.iter().any(|c| {
                                            c.occurrence_id == interface.occurrence_id
                                                && c.definition_id == interface.definition_id
                                        })
                                    })
                            })
                        {
                            let coordinates = || {
                                polygon
                                    .exterior()
                                    .0
                                    .iter()
                                    .map(|p| if fixed == 0 { p.x } else { p.y })
                            };
                            let side = if coordinates().all(|value| value < interface.coordinate_mm)
                            {
                                -1.
                            } else if coordinates().all(|value| value > interface.coordinate_mm) {
                                1.
                            } else {
                                return Err(error(
                                    ManufacturingErrorCode::InvalidSpecification,
                                    "UnsupportedInterface: recovery owner crosses its separator",
                                ));
                            };
                            // One shared interface frame, mirrored outward into
                            // each owner. Quarter-bead physical overlap leaves
                            // representation margin within half-bead permission.
                            origin[fixed] = interface.coordinate_mm
                                + side
                                    * (layer.recipe.bead_width_mm
                                        - interface.bonding_overlap_mm * 0.5);
                        }
                        region::HatchRows::common(polygon, fraction, &alternate_layer, origin)?
                    } else {
                        region::HatchRows::new(polygon, fraction, &alternate_layer)?
                    };
                    let bound = alternate_rows.point_bound()?;
                    let live = point_sum(live, original.retained_points())?;
                    admit(
                        point_sum(point_sum(live, original.workspace_points)?, bound)?,
                        bound,
                    )?;
                    let mut alternative = super::support::RasterCandidate {
                        paths: Vec::new(),
                        whole_rows: Vec::new(),
                        process_interfaces: Vec::new(),
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
                    if let Some((index, _)) = owner {
                        for path in &mut alternative.paths {
                            path.owner = Some(DepositionOwner::Regional {
                                region_index: index,
                            });
                        }
                    }
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
                            // The role changes, not the original polygon's
                            // mandatory owner (including admitted extensions).
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
            process_interfaces: Vec::new(),
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
                            mixed.paths.last_mut().unwrap().owner = path.owner;
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
                        mixed.paths.last_mut().unwrap().owner = path.owner;
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
        let mut rows = Vec::new();
        for (domain, kind, original) in [
            (
                &self.bottom,
                DepositionKind::BottomSkin,
                Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::BottomSkin,
                }),
            ),
            (
                &self.top,
                DepositionKind::TopSkin,
                Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::TopSkin,
                }),
            ),
        ]
        .into_iter()
        .chain(self.interiors.iter().enumerate().map(|(index, interior)| {
            (
                &interior.domain,
                if interior.coverage.fraction == 1.0 {
                    DepositionKind::SolidFill
                } else {
                    DepositionKind::SparseFill
                },
                Some(DepositionOwner::Regional {
                    region_index: index as u16,
                }),
            )
        })) {
            let mandatory = matches!(kind, DepositionKind::BottomSkin | DepositionKind::TopSkin);
            let corridor = if mandatory { &self.layout.fill } else { domain };
            let excluded = if mandatory {
                &self.separator_bands[..]
            } else {
                &[]
            };
            let owned = region::bridge_rows(
                corridor,
                excluded,
                required.iter().filter(|path| {
                    (path.kind == kind || path.kind == DepositionKind::Bridge)
                        && path.owner == original
                }),
                layer,
                segments,
                cancel,
            )?;
            rows.extend(owned);
        }
        Ok(rows)
    }
}

/// A conditioned exact set relation avoids re-quantizing already-contained
/// eligible islands. Partial cuts use complementary operations on the same
/// original operands; any unresolved residual remains an error, never an epsilon.
pub(super) fn split_process_region(
    remaining: MultiPolygon<f64>,
    material: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(MultiPolygon<f64>, MultiPolygon<f64>, bool), ManufacturingError> {
    let mut owned = MultiPolygon(Vec::new());
    let mut next = MultiPolygon(Vec::new());
    let mut partial = false;
    for polygon in remaining.0 {
        check_cancel(cancel)?;
        let points = polygon.exterior().0.len()
            + polygon
                .interiors()
                .iter()
                .map(|ring| ring.0.len())
                .sum::<usize>();
        let material_points = region::point_count(material);
        super::boolean::charge(work, point_sum(points, material_points)?)?;
        if let (Some(a), Some(b)) = (polygon.bounding_rect(), material.bounding_rect())
            && !a.intersects(&b)
        {
            next.0.push(polygon);
            continue;
        }
        let conditioned = std::iter::once(&polygon)
            .chain(&material.0)
            .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
            .flat_map(|ring| &ring.0)
            .all(|point| {
                super::native::merge_predicate_coordinate(point.x)
                    && super::native::merge_predicate_coordinate(point.y)
            });
        if conditioned {
            if strictly_contained_process_polygon(&polygon, material, work, cancel)? {
                owned.0.push(polygon);
                continue;
            }
            super::boolean::charge(
                work,
                points
                    .checked_add(4)
                    .and_then(|a| {
                        material_points
                            .checked_add(4)
                            .and_then(|b| a.checked_mul(b))
                    })
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "process set relation work overflow",
                        )
                    })?,
            )?;
            if !polygon.intersects(material) {
                next.0.push(polygon);
                continue;
            }
        }
        partial = true;
        let original = MultiPolygon(vec![polygon]);
        owned
            .0
            .extend(intersection_accumulated(&original, material, work, cancel)?.0);
        next.0
            .extend(operation(&original, material, false, Some(work), cancel)?.0);
        region::check_size(&owned)?;
        region::check_size(&next)?;
    }
    Ok((owned, next, partial))
}

pub(super) fn strictly_contained_process_polygon(
    polygon: &Polygon<f64>,
    material: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    // A connected eligible exterior starts strictly in D and cannot leave it
    // without a boundary intersection. Every wholly enclosed D hole is checked
    // independently, including holes not crossed by the eligible boundary.
    super::boolean::charge(work, region::point_count(material))?;
    if !material.contains(&Point(polygon.exterior().0[0])) {
        return Ok(false);
    }
    for a in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
        for b in material
            .0
            .iter()
            .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
        {
            for a in a.0.windows(2) {
                check_cancel(cancel)?;
                let a = Line::new(a[0], a[1]);
                for b in b.0.windows(2) {
                    super::boolean::charge(work, 1)?;
                    if a.intersects(&Line::new(b[0], b[1])) {
                        return Ok(false);
                    }
                }
            }
        }
    }
    let points = polygon.exterior().0.len()
        + polygon
            .interiors()
            .iter()
            .map(|ring| ring.0.len())
            .sum::<usize>();
    for hole in material.0.iter().flat_map(|p| p.interiors()) {
        check_cancel(cancel)?;
        super::boolean::charge(work, points)?;
        if polygon.contains(&Point(hole.0[0])) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn process_scratch(
    original: &MultiPolygon<f64>,
    remaining: &MultiPolygon<f64>,
    completed: &[InteriorRegion],
    extra: &[&MultiPolygon<f64>],
) -> Result<usize, ManufacturingError> {
    let base = scratch_points(&[original, remaining])?;
    let completed = completed.iter().try_fold(base, |points, region| {
        point_sum(points, region::point_count(&region.domain))
    })?;
    point_sum(completed, scratch_points(extra)?)
}

pub(super) fn union_accumulated(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    if a.0.is_empty() {
        return Ok(b.clone());
    }
    if b.0.is_empty() {
        return Ok(a.clone());
    }
    super::boolean::budget_accumulated(a, b, work, cancel)?;
    let mut result = if !a
        .bounding_rect()
        .expect("nonempty")
        .intersects(&b.bounding_rect().expect("nonempty"))
    {
        MultiPolygon(a.0.iter().chain(&b.0).cloned().collect())
    } else {
        a.union(b)
    };
    region::check_size(&result)?;
    region::canonicalize(&mut result);
    Ok(result)
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
