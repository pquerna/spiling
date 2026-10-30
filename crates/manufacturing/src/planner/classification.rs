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

pub(super) use super::walls::WallPlan;
mod skin_commands;

/// Immutable ownership between realized D sections and deposition generation.
/// Wall centers are separate from the disjoint interior ownership domains. Holes
/// remain excluded by every Boolean operation; sparse void is a nominal fraction
/// of its interior domain, not a manufactured geometry or strength assertion.
pub(super) struct ClassifiedRegions {
    pub(super) layout: Arc<WallPlan>,
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
    pub(super) workspace_bytes: usize,
}

impl ComponentAnchorAdmission {
    fn retained_points(&self) -> usize {
        self.bridges.capacity() + self.demands.capacity() * 2
    }

    fn retained_bytes(&self) -> usize {
        self.bridges.capacity() * std::mem::size_of::<usize>()
            + self.demands.capacity() * std::mem::size_of::<[[f64; 2]; 2]>()
    }
}

pub(super) type ComponentAnchorCheck<'a> = dyn FnMut(
        &mut super::support::RasterCandidate,
        region::HatchAxis,
        usize,
        usize,
    ) -> Result<ComponentAnchorAdmission, ManufacturingError>
    + 'a;

pub(super) fn byte_sum(a: usize, b: usize) -> Result<usize, ManufacturingError> {
    a.checked_add(b).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "classified owned byte overflow",
        )
    })
}

pub(super) fn contribution_vec_bytes(values: &Vec<ProcessRegionContribution>) -> usize {
    values.capacity() * std::mem::size_of::<ProcessRegionContribution>()
        + values
            .iter()
            .map(|c| c.definition_id.allocated_bytes())
            .sum::<usize>()
}

pub(super) fn contribution_clone_bytes(values: &[ProcessRegionContribution]) -> usize {
    values.len() * std::mem::size_of::<ProcessRegionContribution>()
        + values
            .iter()
            .map(|c| c.definition_id.allocated_bytes())
            .sum::<usize>()
}

pub(super) fn process_coverage_bytes(coverage: &ProcessRegionCoverage) -> usize {
    contribution_vec_bytes(&coverage.contributions)
}

fn process_owned_bytes(
    base: usize,
    original: &mut MultiPolygon<f64>,
    remaining: &mut MultiPolygon<f64>,
    completed: &mut Vec<InteriorRegion>,
) -> Result<usize, ManufacturingError> {
    let mut bytes = byte_sum(base, super::native::material_bytes(original))?;
    bytes = byte_sum(bytes, super::native::material_bytes(remaining))?;
    bytes = byte_sum(
        bytes,
        completed.capacity() * std::mem::size_of::<InteriorRegion>(),
    )?;
    for owner in completed {
        bytes = byte_sum(bytes, super::native::material_bytes(&mut owner.domain))?;
        bytes = byte_sum(bytes, process_coverage_bytes(&owner.coverage))?;
    }
    Ok(bytes)
}

fn admit_process_extra(
    admit: &mut impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
    base_units: usize,
    base_bytes: usize,
    original: &mut MultiPolygon<f64>,
    remaining: &mut MultiPolygon<f64>,
    completed: &mut Vec<InteriorRegion>,
    extra: &mut [&mut MultiPolygon<f64>],
) -> Result<(), ManufacturingError> {
    let mut bytes = process_owned_bytes(base_bytes, original, remaining, completed)?;
    let mut units = point_sum(
        base_units,
        process_scratch(original, remaining, completed, &[])?,
    )?;
    for geometry in extra {
        units = point_sum(units, region::point_count(geometry))?;
        bytes = byte_sum(bytes, super::native::material_bytes(geometry))?;
    }
    admit(units, bytes)
}

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
    /// Actual owned capacities; the shared wall layout is charged by its owner.
    pub(super) fn retained_bytes(&mut self) -> usize {
        let mut bytes = super::native::material_bytes(&mut self.bottom)
            + super::native::material_bytes(&mut self.top)
            + self.interiors.capacity() * std::mem::size_of::<InteriorRegion>()
            + self.separator_bands.capacity() * std::mem::size_of::<MultiPolygon<f64>>()
            + super::bonds::coverage_owned_bytes(&self.coverage)
            + self.applied_operations.capacity()
                * std::mem::size_of::<(
                    spiling_contracts::geometry::OccurrenceId,
                    spiling_contracts::process::ProcessProvenance,
                )>();
        for interior in &mut self.interiors {
            bytes += super::native::material_bytes(&mut interior.domain)
                + process_coverage_bytes(&interior.coverage);
        }
        for band in &mut self.separator_bands {
            bytes += super::native::material_bytes(band);
        }
        bytes
    }
    pub(super) fn new(
        layout: Arc<WallPlan>,
        survives_below: &MultiPolygon<f64>,
        survives_above: &MultiPolygon<f64>,
        mut admit_scratch: impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
        recipe: &PlanarPrintRecipe,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        check_cancel(cancel)?;
        // Survival is final-D material across the complete physical horizon,
        // followed by current fill-bead clearance. Apply this span's residual
        // ownership afterward; neighboring wall-owned material is not void.
        if survives_below.0.is_empty() {
            admit_scratch(
                region::point_count(&layout.fill),
                region::cloned_polygons_bytes(&layout.fill.0)?,
            )?;
        }
        let mut bottom = difference(&layout.fill, survives_below, cancel)?;
        let bottom_bytes = super::native::material_bytes(&mut bottom);
        admit_scratch(scratch_points(&[&bottom])?, bottom_bytes)?;
        let mut remaining = if bottom.0.is_empty() {
            let clone_bytes = region::cloned_polygons_bytes(&layout.fill.0)?;
            admit_scratch(
                scratch_points(&[&bottom, &layout.fill])?,
                bottom_bytes.checked_add(clone_bytes).ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "classified clone byte overflow",
                    )
                })?,
            )?;
            layout.fill.clone()
        } else {
            // Both sides of a split use the original operands and coordinate
            // frame, never a newly quantized subtraction of the rounded mask.
            intersection(&layout.fill, survives_below, cancel)?
        };
        let remaining_bytes = super::native::material_bytes(&mut remaining);
        admit_scratch(
            scratch_points(&[&bottom, &remaining])?,
            bottom_bytes.checked_add(remaining_bytes).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "classified split byte overflow",
                )
            })?,
        )?;
        // Bottom ownership wins when physical skin thicknesses overlap.
        if survives_above.0.is_empty() {
            admit_scratch(
                scratch_points(&[&bottom, &remaining, &remaining])?,
                byte_sum(
                    byte_sum(bottom_bytes, remaining_bytes)?,
                    region::cloned_polygons_bytes(&remaining.0)?,
                )?,
            )?;
        }
        let mut top = difference(&remaining, survives_above, cancel)?;
        let top_bytes = super::native::material_bytes(&mut top);
        let split_bytes = bottom_bytes
            .checked_add(top_bytes)
            .and_then(|b| b.checked_add(remaining_bytes))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "classified split byte overflow",
                )
            })?;
        admit_scratch(scratch_points(&[&bottom, &remaining, &top])?, split_bytes)?;
        let mut interior = if top.0.is_empty() {
            remaining
        } else {
            // Intersect the original operands of the top split. Subtracting its
            // already rounded result would introduce a second, different grid.
            let mut result = intersection(&remaining, survives_above, cancel)?;
            let result_bytes = super::native::material_bytes(&mut result);
            admit_scratch(
                scratch_points(&[&bottom, &top, &remaining, &result])?,
                split_bytes.checked_add(result_bytes).ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "classified split byte overflow",
                    )
                })?,
            )?;
            drop(remaining);
            result
        };
        let interior_bytes = super::native::material_bytes(&mut interior);
        let geometry_bytes = bottom_bytes
            .checked_add(top_bytes)
            .and_then(|b| b.checked_add(interior_bytes))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "classified retained byte overflow",
                )
            })?;
        admit_scratch(scratch_points(&[&bottom, &top, &interior])?, geometry_bytes)?;
        let interior_area = interior.unsigned_area();
        let records = usize::from(interior_area > 0.0);
        let record_bytes = records
            * (std::mem::size_of::<InteriorRegion>()
                + std::mem::size_of::<ProcessRegionCoverage>());
        admit_scratch(
            scratch_points(&[&bottom, &top, &interior])?,
            geometry_bytes.checked_add(record_bytes).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "classified record byte overflow",
                )
            })?,
        )?;
        let coverage = PlanarRegionCoverage {
            skin_bridge_domains: Vec::new(),
            process_diaphragms: Vec::new(),
            diaphragm_area_mm2: 0.0,
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
                    material_domain: None,
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
        let mut result = Self {
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
                drop(interior);
                Vec::new()
            },
            separator_bands: Vec::new(),
            coverage,
            applied_operations: Vec::new(),
        };
        admit_scratch(result.retained_points(), result.retained_bytes())?;
        Ok(result)
    }

    /// Resolve only the eligible interior. Global walls, physical skins and
    /// protected holes keep the single final-D classification above.
    pub(super) fn with_process(
        mut self,
        domains: &[super::ProcessDomain],
        recipe: &PlanarPrintRecipe,
        mut admit: impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        if domains.is_empty() || self.interiors.is_empty() {
            return Ok(self);
        }
        let spatial = domains.iter().any(|domain| domain.zone.is_some());
        if !spatial
            && domains
                .iter()
                .all(|d| d.coverage.fraction == domains[0].coverage.fraction)
        {
            let fraction = domains[0].coverage.fraction;
            let count = domains
                .iter()
                .map(|d| d.coverage.contributions.len())
                .sum::<usize>();
            let copied_bytes = domains.iter().try_fold(
                count * std::mem::size_of::<ProcessRegionContribution>(),
                |bytes, d| {
                    byte_sum(
                        bytes,
                        d.coverage
                            .contributions
                            .iter()
                            .map(|c| c.definition_id.allocated_bytes())
                            .sum(),
                    )
                },
            )?;
            admit(
                self.retained_points(),
                byte_sum(
                    self.retained_bytes(),
                    byte_sum(
                        byte_sum(copied_bytes, copied_bytes)?,
                        std::mem::size_of::<ProcessRegionCoverage>(),
                    )?,
                )?,
            )?;
            let interior = &mut self.interiors[0];
            interior.coverage.fraction = fraction;
            interior.coverage.contributions = Vec::with_capacity(count);
            interior.coverage.contributions.extend(
                domains
                    .iter()
                    .flat_map(|domain| domain.coverage.contributions.iter().cloned()),
            );
            let area = interior.coverage.area_mm2;
            self.coverage.process_regions = vec![interior.coverage.clone()];
            self.coverage.solid_interior_area_mm2 = if fraction == 1.0 { area } else { 0.0 };
            self.coverage.sparse_interior_area_mm2 = if fraction < 1.0 { area } else { 0.0 };
            self.coverage.intentional_void_area_mm2 = area * (1.0 - fraction);
            admit(self.retained_points(), self.retained_bytes())?;
            return super::zones::partition(self, domains, recipe, admit, work, cancel);
        }
        let mut original = self
            .interiors
            .pop()
            .expect("initial global interior")
            .domain;
        let base_units = self.retained_points();
        let base_bytes = self.retained_bytes();
        let original_bytes = super::native::material_bytes(&mut original);
        admit(
            point_sum(base_units, scratch_points(&[&original, &original])?)?,
            byte_sum(
                byte_sum(base_bytes, original_bytes)?,
                region::cloned_polygons_bytes(&original.0)?,
            )?,
        )?;
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
            let mut material = std::borrow::Cow::Borrowed(domain.material.as_ref());
            let mut material_bytes = 0;
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
                let contribution_bytes = contribution_clone_bytes(&source.coverage.contributions);
                let live =
                    process_owned_bytes(base_bytes, &mut original, &mut remaining, &mut completed)?;
                admit(
                    point_sum(
                        base_units,
                        process_scratch(&original, &remaining, &completed, &[&material])?,
                    )?,
                    byte_sum(
                        byte_sum(
                            byte_sum(live, material_bytes)?,
                            contribution_vec_bytes(&contributors),
                        )?,
                        contribution_bytes,
                    )?,
                )?;
                contributors.reserve_exact(source.coverage.contributions.len());
                contributors.extend(source.coverage.contributions.iter().cloned());
                let live =
                    process_owned_bytes(base_bytes, &mut original, &mut remaining, &mut completed)?;
                admit(
                    point_sum(
                        base_units,
                        process_scratch(&original, &remaining, &completed, &[&material])?,
                    )?,
                    byte_sum(
                        byte_sum(live, material_bytes)?,
                        contribution_vec_bytes(&contributors),
                    )?,
                )?;
                if index > 0 {
                    let clone_bound = if material.0.is_empty() {
                        region::cloned_polygons_bytes(&source.material.0)?
                    } else if source.material.0.is_empty() {
                        region::cloned_polygons_bytes(&material.0)?
                    } else if !material
                        .bounding_rect()
                        .expect("nonempty")
                        .intersects(&source.material.bounding_rect().expect("nonempty"))
                    {
                        byte_sum(
                            region::cloned_polygons_bytes(&material.0)?,
                            region::cloned_polygons_bytes(&source.material.0)?,
                        )?
                    } else {
                        0
                    };
                    if clone_bound != 0 {
                        admit(
                            point_sum(
                                base_units,
                                process_scratch(
                                    &original,
                                    &remaining,
                                    &completed,
                                    &[&material, &material, &source.material],
                                )?,
                            )?,
                            byte_sum(
                                byte_sum(
                                    byte_sum(live, material_bytes)?,
                                    contribution_vec_bytes(&contributors),
                                )?,
                                clone_bound,
                            )?,
                        )?;
                    }
                    let mut merged = union_accumulated(&material, &source.material, work, cancel)?;
                    let merged_bytes = super::native::material_bytes(&mut merged);
                    let live = process_owned_bytes(
                        base_bytes,
                        &mut original,
                        &mut remaining,
                        &mut completed,
                    )?;
                    admit(
                        point_sum(
                            base_units,
                            process_scratch(
                                &original,
                                &remaining,
                                &completed,
                                &[&material, &merged],
                            )?,
                        )?,
                        byte_sum(
                            byte_sum(byte_sum(live, material_bytes)?, merged_bytes)?,
                            contribution_vec_bytes(&contributors),
                        )?,
                    )?;
                    material = std::borrow::Cow::Owned(merged);
                    material_bytes = merged_bytes;
                }
            }
            let remaining_bytes = super::native::material_bytes(&mut remaining);
            let outside_bytes = byte_sum(
                byte_sum(
                    process_owned_bytes(base_bytes, &mut original, &mut remaining, &mut completed)?
                        - remaining_bytes,
                    material_bytes,
                )?,
                contribution_vec_bytes(&contributors),
            )?;
            let outside_units = point_sum(
                base_units,
                process_scratch(
                    &original,
                    &MultiPolygon(Vec::new()),
                    &completed,
                    &[&material],
                )?,
            )?;
            let (mut owned, next, partial) = split_process_region(
                remaining,
                &material,
                |units, bytes| {
                    admit(
                        point_sum(outside_units, units)?,
                        byte_sum(outside_bytes, bytes)?,
                    )
                },
                work,
                cancel,
            )?;
            remaining = next;
            partial_cut |= partial;
            let owned_bytes = super::native::material_bytes(&mut owned);
            let live =
                process_owned_bytes(base_bytes, &mut original, &mut remaining, &mut completed)?;
            admit(
                point_sum(
                    base_units,
                    process_scratch(&original, &remaining, &completed, &[&material, &owned])?,
                )?,
                byte_sum(
                    byte_sum(byte_sum(live, material_bytes)?, owned_bytes)?,
                    contribution_vec_bytes(&contributors),
                )?,
            )?;
            let area = owned.unsigned_area();
            if !owned.0.is_empty() {
                let target = record_target(&completed, 1);
                let extra = (target - completed.capacity()) * std::mem::size_of::<InteriorRegion>();
                admit(
                    point_sum(
                        base_units,
                        process_scratch(&original, &remaining, &completed, &[&owned])?,
                    )?,
                    byte_sum(
                        byte_sum(
                            byte_sum(byte_sum(live, material_bytes)?, owned_bytes)?,
                            contribution_vec_bytes(&contributors),
                        )?,
                        extra,
                    )?,
                )?;
                completed.reserve_exact(target - completed.len());
                completed.push(InteriorRegion {
                    domain: owned,
                    frame_origin_mm: None,
                    coverage: ProcessRegionCoverage {
                        material_domain: None,
                        area_mm2: area,
                        fraction,
                        contributions: contributors,
                    },
                });
                let live =
                    process_owned_bytes(base_bytes, &mut original, &mut remaining, &mut completed)?;
                admit(
                    point_sum(
                        base_units,
                        process_scratch(&original, &remaining, &completed, &[&material])?,
                    )?,
                    byte_sum(live, material_bytes)?,
                )?;
            }
        }
        if !remaining.0.is_empty() {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "retained occurrence final-D contributions do not cover eligible process interior",
            ));
        }
        // Moving uncut islands is exact; only partial Boolean splits need the
        // independent set checks. Area sums never establish ownership.
        for index in 0..completed.len() {
            if !partial_cut {
                break;
            }
            let mut excess = operation(
                &completed[index].domain,
                &original,
                false,
                Some(work),
                cancel,
            )?;
            admit_process_extra(
                &mut admit,
                base_units,
                base_bytes,
                &mut original,
                &mut remaining,
                &mut completed,
                &mut [&mut excess],
            )?;
            if !excess.0.is_empty() {
                return Err(error(
                    ManufacturingErrorCode::InvalidSpecification,
                    "resolved process region escapes eligible interior",
                ));
            }
            for other in 0..index {
                let mut overlap = intersection_accumulated(
                    &completed[index].domain,
                    &completed[other].domain,
                    work,
                    cancel,
                )?;
                admit_process_extra(
                    &mut admit,
                    base_units,
                    base_bytes,
                    &mut original,
                    &mut remaining,
                    &mut completed,
                    &mut [&mut excess, &mut overlap],
                )?;
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
            for index in 0..completed.len() {
                let domain = &completed[index].domain;
                let clone_bound = if covered.0.is_empty() {
                    region::cloned_polygons_bytes(&domain.0)?
                } else if domain.0.is_empty() {
                    region::cloned_polygons_bytes(&covered.0)?
                } else if !covered
                    .bounding_rect()
                    .expect("nonempty")
                    .intersects(&domain.bounding_rect().expect("nonempty"))
                {
                    byte_sum(
                        region::cloned_polygons_bytes(&covered.0)?,
                        region::cloned_polygons_bytes(&domain.0)?,
                    )?
                } else {
                    0
                };
                if clone_bound != 0 {
                    let units = point_sum(
                        base_units,
                        process_scratch(&original, &remaining, &completed, &[&covered, domain])?,
                    )?;
                    let bytes = byte_sum(
                        process_owned_bytes(
                            base_bytes,
                            &mut original,
                            &mut remaining,
                            &mut completed,
                        )?,
                        super::native::material_bytes(&mut covered),
                    )?;
                    admit(units, byte_sum(bytes, clone_bound)?)?;
                }
                let mut union =
                    union_accumulated(&covered, &completed[index].domain, work, cancel)?;
                admit_process_extra(
                    &mut admit,
                    base_units,
                    base_bytes,
                    &mut original,
                    &mut remaining,
                    &mut completed,
                    &mut [&mut covered, &mut union],
                )?;
                covered = union;
            }
            let mut missing = operation(&original, &covered, false, Some(work), cancel)?;
            admit_process_extra(
                &mut admit,
                base_units,
                base_bytes,
                &mut original,
                &mut remaining,
                &mut completed,
                &mut [&mut covered, &mut missing],
            )?;
            if !missing.0.is_empty() {
                return Err(error(
                    ManufacturingErrorCode::InvalidSpecification,
                    "partial process partition leaves eligible interior uncovered",
                ));
            }
        }
        let coverage_bytes = completed.iter().try_fold(
            completed.len() * std::mem::size_of::<ProcessRegionCoverage>(),
            |bytes, owner| {
                byte_sum(
                    bytes,
                    contribution_clone_bytes(&owner.coverage.contributions),
                )
            },
        )?;
        let live = process_owned_bytes(base_bytes, &mut original, &mut remaining, &mut completed)?;
        admit(
            point_sum(
                base_units,
                process_scratch(&original, &remaining, &completed, &[])?,
            )?,
            byte_sum(live, coverage_bytes)?,
        )?;
        self.coverage.process_regions = completed
            .iter()
            .map(|region| region.coverage.clone())
            .collect();
        self.coverage.solid_interior_area_mm2 = completed
            .iter()
            .filter(|r| r.coverage.fraction == 1.0)
            .map(|r| r.coverage.area_mm2)
            .sum();
        self.coverage.sparse_interior_area_mm2 = completed
            .iter()
            .filter(|r| r.coverage.fraction < 1.0)
            .map(|r| r.coverage.area_mm2)
            .sum();
        self.coverage.intentional_void_area_mm2 = completed
            .iter()
            .map(|r| r.coverage.area_mm2 * (1.0 - r.coverage.fraction))
            .sum();
        self.interiors = completed;
        admit(
            self.retained_points(),
            byte_sum(
                self.retained_bytes(),
                byte_sum(
                    super::native::material_bytes(&mut original),
                    super::native::material_bytes(&mut remaining),
                )?,
            )?,
        )?;
        drop(original);
        drop(remaining);
        super::zones::partition(self, domains, recipe, admit, work, cancel)
    }

    /// Propose one shared ordinary phase without granting final stroke support
    /// or changing any source-framed or mandatory-skin construction.
    pub(super) fn choose_initial_ordinary_phase<'a>(
        &self,
        layer: &region::HatchLayer<'_>,
        previous: impl Iterator<Item = &'a DepositionPath> + Clone,
        anchors: &mut super::support::AnchorWorkspace,
        anchor_work: &mut usize,
        classified_owned_bytes: usize,
        outside_retained_points: usize,
        outside_owned_bytes: usize,
        mut admit: impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
        cancel: &AtomicBool,
    ) -> Result<region::OrdinaryHatchPhase, ManufacturingError> {
        check_cancel(cancel)?;
        let class_points = self.retained_points();
        admit(
            point_sum(class_points, anchors.retained_records())?,
            byte_sum(classified_owned_bytes, anchors.retained_bytes())?,
            0,
        )?;
        if !self
            .interiors
            .iter()
            .any(|interior| interior.frame_origin_mm.is_none() && !interior.domain.0.is_empty())
        {
            return Ok(region::OrdinaryHatchPhase::Centered);
        }
        let mut eligible_previous = false;
        for path in previous.clone() {
            check_cancel(cancel)?;
            if path.physical_z_mm[1] == layer.lower_z && path.checked_edges()?.len() != 0 {
                eligible_previous = true;
                break;
            }
        }
        if !eligible_previous {
            return Ok(region::OrdinaryHatchPhase::Centered);
        }
        'proposal: for phase in region::OrdinaryHatchPhase::CANDIDATES {
            check_cancel(cancel)?;
            let mut candidate = super::support::RasterCandidate {
                paths: Vec::new(),
                whole_rows: Vec::new(),
                process_interfaces: Vec::new(),
                skin_bridge_domains: Vec::new(),
                skin_command_fits: Vec::new(),
            };
            let mut segments = 0;
            let mut candidate_points = 0;
            for (index, interior) in self.interiors.iter().enumerate() {
                if interior.frame_origin_mm.is_some() {
                    continue;
                }
                let fraction = interior.coverage.fraction;
                let kind = if fraction == 1.0 {
                    DepositionKind::SolidFill
                } else {
                    DepositionKind::SparseFill
                };
                for polygon in &interior.domain.0 {
                    check_cancel(cancel)?;
                    let Some(rows) =
                        region::HatchRows::centered_phase(polygon, fraction, layer, phase)?
                    else {
                        continue 'proposal;
                    };
                    let bound = rows.point_bound()?;
                    let live = point_sum(class_points, candidate_points)?;
                    let live_bytes = byte_sum(classified_owned_bytes, candidate_bytes(&candidate))?;
                    // Work is a delta consumed by the caller's cumulative
                    // ledger, including rows from subsequently refused trials.
                    admit(
                        point_sum(point_sum(live, anchors.retained_records())?, bound)?,
                        byte_sum(
                            byte_sum(live_bytes, anchors.retained_bytes())?,
                            hatch_storage_bound(bound)?,
                        )?,
                        bound,
                    )?;
                    let mut paths = Vec::new();
                    match region::hatch_polygon(
                        polygon,
                        kind,
                        &rows,
                        layer,
                        &mut paths,
                        &mut segments,
                        cancel,
                    ) {
                        Ok(()) => {}
                        // A speculative phase may clip a degenerate row even
                        // when a later phase or ordinary recovery is usable.
                        Err(failure)
                            if failure.code == ManufacturingErrorCode::UnsupportedGeometry =>
                        {
                            continue 'proposal;
                        }
                        Err(failure) => return Err(failure),
                    }
                    if paths.is_empty() {
                        continue 'proposal;
                    }
                    for path in &mut paths {
                        path.owner = Some(DepositionOwner::Regional {
                            region_index: index as u16,
                        });
                    }
                    let points = path_points(&paths);
                    let target = record_target(&candidate.paths, paths.len());
                    let replacement_bytes = if target > candidate.paths.capacity() {
                        target.checked_mul(std::mem::size_of::<DepositionPath>())
                    } else {
                        Some(0)
                    }
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "ordinary phase path reservation overflow",
                        )
                    })?;
                    admit(
                        point_sum(point_sum(live, points)?, anchors.retained_records())?,
                        byte_sum(
                            byte_sum(
                                byte_sum(live_bytes, super::bonds::path_bytes(&paths))?,
                                anchors.retained_bytes(),
                            )?,
                            replacement_bytes,
                        )?,
                        0,
                    )?;
                    candidate
                        .paths
                        .try_reserve_exact(target - candidate.paths.len())
                        .map_err(|_| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "ordinary phase candidate allocation refused",
                            )
                        })?;
                    candidate.paths.append(&mut paths);
                    candidate_points = point_sum(candidate_points, points)?;
                    // The emptied polygon record buffer remains live here.
                    admit(
                        point_sum(
                            point_sum(class_points, candidate_points)?,
                            anchors.retained_records(),
                        )?,
                        byte_sum(
                            byte_sum(
                                byte_sum(classified_owned_bytes, candidate_bytes(&candidate))?,
                                super::bonds::path_bytes(&paths),
                            )?,
                            anchors.retained_bytes(),
                        )?,
                        0,
                    )?;
                }
            }
            let live = point_sum(class_points, candidate_points)?;
            let live_bytes = byte_sum(classified_owned_bytes, candidate_bytes(&candidate))?;
            // The outside baseline excludes this classifier and workspace.
            // The existing component allowance owns query/proof growth; only
            // actual capacities, not that unallocated allowance, enter admit.
            admit(
                point_sum(live, anchors.retained_records())?,
                byte_sum(live_bytes, anchors.retained_bytes())?,
                0,
            )?;
            let budget = super::support::component_budget(
                outside_retained_points,
                outside_owned_bytes,
                live,
                live_bytes,
            )?;
            let proof = super::support::component_admission(
                (&mut candidate, layer.axis, budget, false),
                previous.clone(),
                std::iter::empty(),
                layer.recipe,
                anchors,
                anchor_work,
                cancel,
            )?;
            admit(
                point_sum(
                    point_sum(live, anchors.retained_records())?,
                    proof.retained_points(),
                )?,
                byte_sum(
                    byte_sum(live_bytes, anchors.retained_bytes())?,
                    proof.retained_bytes(),
                )?,
                0,
            )?;
            let supported = proof.supported;
            drop(proof);
            drop(candidate);
            if supported {
                return Ok(phase);
            }
        }
        // A guide refusal never substitutes for the actual ordinary/support
        // pipeline's honest result, including an empty centered polygon.
        Ok(region::OrdinaryHatchPhase::Centered)
    }

    pub(super) fn paths(
        &self,
        z: f64,
        lower_z: f64,
        axis: region::HatchAxis,
        top_axis: region::HatchAxis,
        recipe: &PlanarPrintRecipe,
        phase: region::OrdinaryHatchPhase,
        classified_owned_bytes: usize,
        source_domains: &[super::ProcessDomain],
        segments: &mut usize,
        mut admit: impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
        cancel: &AtomicBool,
    ) -> Result<
        (
            Vec<DepositionPath>,
            &PlanarRegionCoverage,
            Vec<SkinCommandFitDomain>,
        ),
        ManufacturingError,
    > {
        let mut paths = Vec::new();
        self.layout
            .append_paths(lower_z, z, &mut paths, segments, cancel)?;
        let layer = region::HatchLayer {
            z,
            lower_z,
            height: z - lower_z,
            axis,
            top_axis,
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
            &region::HatchLayer {
                axis: top_axis,
                ..layer
            },
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
                phase,
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
        let fits = self.fit_skin_commands(
            &mut paths,
            source_domains,
            segments,
            classified_owned_bytes,
            &mut admit,
            cancel,
        )?;
        Ok((paths, &self.coverage, fits))
    }

    pub(super) fn paths_with_bridge_recovery(
        &self,
        layer: &region::HatchLayer<'_>,
        phase: region::OrdinaryHatchPhase,
        source_domains: &[super::ProcessDomain],
        classified_owned_bytes: usize,
        initial_workspace_bytes: usize,
        segments: &mut usize,
        mut admit: impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
        anchors: &mut ComponentAnchorCheck<'_>,
        constructed_domains: &mut Vec<SkinBridgeDomain>,
        skin_command_fits: &mut Vec<SkinCommandFitDomain>,
        cancel: &AtomicBool,
    ) -> Result<Vec<DepositionPath>, ManufacturingError> {
        let mut workspace_bytes = initial_workspace_bytes;
        let mut paths = {
            let mut tracked =
                |candidate: &mut super::support::RasterCandidate, axis, points, bytes| {
                    let proof = anchors(candidate, axis, points, bytes)?;
                    workspace_bytes = proof.workspace_bytes;
                    Ok(proof)
                };
            self.paths_with_bridge_recovery_unfitted(
                layer,
                phase,
                source_domains,
                classified_owned_bytes,
                initial_workspace_bytes,
                segments,
                &mut admit,
                &mut tracked,
                constructed_domains,
                cancel,
            )?
        };
        let bridge_bytes = super::bonds::skin_bridge_bytes(constructed_domains);
        let fits = self.fit_skin_commands(
            &mut paths,
            source_domains,
            segments,
            byte_sum(
                byte_sum(classified_owned_bytes, bridge_bytes)?,
                workspace_bytes,
            )?,
            &mut admit,
            cancel,
        )?;
        if fits.is_empty() {
            *skin_command_fits = fits;
            return Ok(paths);
        }
        // Fitting changes actual deposition, so repeat the whole-candidate
        // anchor obligation. No earlier raw-row success licenses the extension.
        let mut candidate = super::support::RasterCandidate {
            paths,
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
            skin_bridge_domains: Vec::new(),
            skin_command_fits: fits,
        };
        let proof = anchors(
            &mut candidate,
            layer.axis,
            point_sum(
                self.retained_points(),
                super::bonds::skin_bridge_points(constructed_domains),
            )?,
            byte_sum(classified_owned_bytes, bridge_bytes)?,
        )?;
        if !proof.supported {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "minimum skin construction lacks fresh actual whole-candidate anchors",
            ));
        }
        *skin_command_fits = candidate.skin_command_fits;
        Ok(candidate.paths)
    }

    /// Preserve authored role lattices outside demand-bound bridge cells and
    /// their actual anchor corridors. All ownership stays in this same fill.
    fn paths_with_bridge_recovery_unfitted(
        &self,
        layer: &region::HatchLayer<'_>,
        phase: region::OrdinaryHatchPhase,
        source_domains: &[super::ProcessDomain],
        classified_owned_bytes: usize,
        initial_workspace_bytes: usize,
        segments: &mut usize,
        mut admit: impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
        anchors: &mut ComponentAnchorCheck<'_>,
        constructed_domains: &mut Vec<SkinBridgeDomain>,
        cancel: &AtomicBool,
    ) -> Result<Vec<DepositionPath>, ManufacturingError> {
        let initial_segments = *segments;
        let mut normal = super::support::RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            paths: Vec::new(),
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
        };
        let wall_bytes = self.layout.paths.len() * std::mem::size_of::<DepositionPath>()
            + self
                .layout
                .paths
                .iter()
                .map(|path| {
                    path.points_mm.len() * std::mem::size_of::<[f64; 3]>()
                        + match &path.width {
                            WidthProfile::Constant { .. } => 0,
                            WidthProfile::PerEdge { widths_mm } => {
                                widths_mm.len() * std::mem::size_of::<f64>()
                            }
                        }
                })
                .sum::<usize>();
        admit(
            self.retained_points()
                + self
                    .layout
                    .paths
                    .iter()
                    .map(|p| p.points_mm.len())
                    .sum::<usize>(),
            byte_sum(
                byte_sum(classified_owned_bytes, wall_bytes)?,
                initial_workspace_bytes,
            )?,
            0,
        )?;
        normal.paths.reserve_exact(self.layout.paths.len());
        self.layout
            .append_paths(layer.lower_z, layer.z, &mut normal.paths, segments, cancel)?;
        let wall_count = normal.paths.len();
        let mut normal_points = path_points(&normal.paths);
        let class_points = self.retained_points();
        let mut bridge = super::support::RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            paths: Vec::new(),
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
        };
        let mut bridge_points = 0;
        let mut patches: Vec<BridgePatch<'_>> = Vec::new();
        let mut mandatory_demand = false;
        let mut owners = Vec::new();
        let owner_count = self.bottom.0.len()
            + self.top.0.len()
            + self
                .interiors
                .iter()
                .map(|region| region.domain.0.len())
                .sum::<usize>();
        let mut probe = super::support::RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            paths: Vec::new(),
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
        };
        let base = point_sum(class_points, normal_points)?;
        let workspace_receipt = anchors(
            &mut probe,
            layer.axis,
            base,
            classified_owned_bytes + candidate_bytes(&normal),
        )?;
        let workspace = workspace_receipt.workspace_points;
        let workspace_bytes = workspace_receipt.workspace_bytes;
        drop(workspace_receipt);
        admit(
            point_sum(point_sum(base, workspace)?, owner_count)?,
            classified_owned_bytes
                + candidate_bytes(&normal)
                + workspace_bytes
                + owner_count * std::mem::size_of::<NormalOwner<'_>>(),
            0,
        )?;
        owners.try_reserve_exact(owner_count).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "role owner allocation refused",
            )
        })?;
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
            let role_layer = region::HatchLayer {
                axis: if kind == DepositionKind::TopSkin {
                    layer.top_axis
                } else {
                    layer.axis
                },
                ..*layer
            };
            let layer = &role_layer;
            let alternate_layer = region::HatchLayer {
                axis: layer.axis.orthogonal(),
                ..*layer
            };
            let normal_along = usize::from(layer.axis == region::HatchAxis::Y);
            let normal_fixed = 1 - normal_along;
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
                let base_bytes =
                    recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches);
                let workspace_receipt = anchors(&mut probe, layer.axis, base, base_bytes)?;
                let workspace = workspace_receipt.workspace_points;
                let workspace_bytes = workspace_receipt.workspace_bytes;
                drop(workspace_receipt);
                let rows = if let Some(origin) = frame {
                    region::HatchRows::common(polygon, fraction, layer, origin)?
                } else if owner.is_some() {
                    region::HatchRows::centered_phase(polygon, fraction, layer, phase)?.ok_or_else(
                        || {
                            error(
                                ManufacturingErrorCode::UnsupportedGeometry,
                                "frozen ordinary hatch phase changed the polygon lattice count",
                            )
                        },
                    )?
                } else {
                    region::HatchRows::new(polygon, fraction, layer)?
                };
                let bound = rows.point_bound()?;
                admit(
                    point_sum(point_sum(base, workspace)?, bound)?,
                    byte_sum(base_bytes + workspace_bytes, hatch_storage_bound(bound)?)?,
                    bound,
                )?;
                let mut component = super::support::RasterCandidate {
                    skin_command_fits: Vec::new(),
                    skin_bridge_domains: Vec::new(),
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
                if owner.is_some() && frame.is_none() && component.paths.is_empty() {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        "frozen ordinary hatch phase emitted no polygon paths",
                    ));
                }
                if let Some((index, _)) = owner {
                    for path in &mut component.paths {
                        path.owner = Some(DepositionOwner::Regional {
                            region_index: index,
                        });
                    }
                }
                let mut original = anchors(&mut component, layer.axis, base, base_bytes)?;
                let component_points = path_points(&component.paths);
                let live = point_sum(base, component_points)?;
                let mut anchor_workspace_bytes = original.workspace_bytes;
                mandatory_demand |=
                    owner.is_none() && !original.supported && !original.demands.is_empty();
                // Preserve the authored component and its direct demand set.
                // Stage constructed whole rows separately from that original.
                if !original.supported && owner.is_some() {
                    let (region_index, interior) = owner.expect("regional construction");
                    let mut sources = source_domains.iter().filter(|domain| {
                        domain.coverage.fraction == interior.coverage.fraction
                            && domain.coverage.contributions == interior.coverage.contributions
                    });
                    let source_owner = match (sources.next(), sources.next()) {
                        (Some(source), None) => Some(source.material.as_ref()),
                        (None, None) if self.interiors.len() == 1 => Some(&self.layout.fill),
                        _ => None,
                    };
                    let first_mixed = bridge.paths.len();
                    let repaired = if let Some(source_owner) = source_owner {
                        self.repair_skin_terminals(
                            polygon,
                            region_index,
                            interior,
                            source_owner,
                            &component.paths,
                            &original.demands,
                            layer,
                            live + original.retained_points(),
                            base_bytes + candidate_bytes(&component) + original.retained_bytes()
                                - candidate_bytes(&bridge)
                                - capacity_bytes(&patches),
                            &mut anchor_workspace_bytes,
                            &mut bridge,
                            &mut patches,
                            &mut admit,
                            anchors,
                            cancel,
                        )?
                    } else {
                        false
                    };
                    if repaired {
                        let accepted = &bridge.paths[first_mixed..];
                        let work = original
                            .demands
                            .len()
                            .checked_mul(accepted.len())
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "mixed covered-demand work overflow",
                                )
                            })?;
                        admit(
                            live + original.retained_points()
                                + original.workspace_points
                                + path_points(&bridge.paths)
                                + super::bonds::skin_bridge_points(&bridge.skin_bridge_domains),
                            recovery_bytes(
                                classified_owned_bytes,
                                &normal,
                                &bridge,
                                &owners,
                                &patches,
                            ) + candidate_bytes(&component)
                                + original.retained_bytes()
                                + anchor_workspace_bytes,
                            work,
                        )?;
                        original.demands.retain(|gap| {
                            !accepted.iter().any(|path| {
                                let Some(DepositionOwner::RegionalSkinBridge {
                                    skin_first, ..
                                }) = path.owner
                                else {
                                    return false;
                                };
                                let (a, b) = if skin_first {
                                    (path.points_mm[1], path.points_mm[2])
                                } else {
                                    (path.points_mm[0], path.points_mm[1])
                                };
                                gap[0][normal_fixed] == a[normal_fixed]
                                    && gap[1][normal_fixed] == a[normal_fixed]
                                    && gap[0][normal_along].min(gap[1][normal_along])
                                        >= a[normal_along].min(b[normal_along])
                                    && gap[0][normal_along].max(gap[1][normal_along])
                                        <= a[normal_along].max(b[normal_along])
                            })
                        });
                        original.supported = original.demands.is_empty();
                        bridge_points = path_points(&bridge.paths)
                            + super::bonds::skin_bridge_points(&bridge.skin_bridge_domains);
                    }
                }
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
                    let live_bytes =
                        recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                            + candidate_bytes(&component)
                            + original.retained_bytes();
                    admit(
                        point_sum(point_sum(live, original.workspace_points)?, bound)?,
                        byte_sum(
                            live_bytes + anchor_workspace_bytes,
                            hatch_storage_bound(bound)?,
                        )?,
                        bound,
                    )?;
                    let mut alternative = super::support::RasterCandidate {
                        skin_command_fits: Vec::new(),
                        skin_bridge_domains: Vec::new(),
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
                        byte_sum(
                            live_bytes + anchor_workspace_bytes + candidate_bytes(&alternative),
                            whole_row_storage_bound(whole_bound, alternative.paths.len())?,
                        )?,
                        whole_bound,
                    )?;
                    let whole_base_bytes =
                        live_bytes + anchor_workspace_bytes + candidate_bytes(&alternative);
                    alternative.whole_rows = self.whole_rows(
                        &alternative.paths,
                        &alternate_layer,
                        &mut staged_segments,
                        cancel,
                        |units, bytes| {
                            admit(
                                point_sum(live, units)?,
                                byte_sum(whole_base_bytes, bytes)?,
                                0,
                            )
                        },
                    )?;
                    let mut proof = anchors(
                        &mut alternative,
                        alternate_layer.axis,
                        live - alternative_points,
                        live_bytes,
                    )?;
                    anchor_workspace_bytes = proof.workspace_bytes;
                    let actual_live =
                        point_sum(live - alternative_points, path_points(&alternative.paths))?;
                    let proof_live = point_sum(
                        point_sum(actual_live, proof.retained_points())?,
                        proof.workspace_points,
                    )?;
                    let proof_bytes = live_bytes
                        + candidate_bytes(&alternative)
                        + proof.retained_bytes()
                        + anchor_workspace_bytes;
                    let bands = bridge_bands(
                        (polygon, alternate_layer.axis),
                        &alternative.paths,
                        (&proof.bridges, &proof.demands),
                        &original.demands,
                        proof_live,
                        proof_bytes,
                        &mut admit,
                        cancel,
                    )?;

                    let band_points = bands.capacity() * 2;
                    let patch_base = point_sum(proof_live, band_points)?
                        - patches.capacity() * BRIDGE_PATCH_POINTS;
                    let patch_base_bytes =
                        proof_bytes + capacity_bytes(&bands) - capacity_bytes(&patches);
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
                    admit(
                        point_sum(proof_live, band_points)?,
                        proof_bytes + capacity_bytes(&bands),
                        selection_work,
                    )?;
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
                            reserve_recovery_record(
                                &mut patches,
                                BRIDGE_PATCH_POINTS,
                                patch_base,
                                patch_base_bytes,
                                &mut admit,
                            )?;
                            patches.push(BridgePatch {
                                owner: Some(polygon),
                                along_axis: normal_fixed,
                                along,
                                across: [lo, hi],
                            });
                        }
                        proof.bridges[keep] = index;
                        keep += 1;
                    }
                    proof.bridges.truncate(keep);
                    let old_bridge_capacity = bridge.paths.capacity();
                    let target = record_target(&bridge.paths, keep);
                    let growth =
                        (target - bridge.paths.capacity()) * std::mem::size_of::<DepositionPath>();
                    admit(
                        proof_live + band_points,
                        patch_base_bytes + capacity_bytes(&patches) + growth,
                        0,
                    )?;
                    bridge.paths.reserve_exact(target - bridge.paths.len());
                    admit(
                        proof_live + band_points,
                        patch_base_bytes
                            + capacity_bytes(&patches)
                            + (bridge.paths.capacity() - old_bridge_capacity)
                                * std::mem::size_of::<DepositionPath>(),
                        0,
                    )?;
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
                let target = record_target(&normal.paths, component.paths.len());
                let growth =
                    (target - normal.paths.capacity()) * std::mem::size_of::<DepositionPath>();
                admit(
                    live + original.retained_points(),
                    recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                        + candidate_bytes(&component)
                        + original.retained_bytes()
                        + anchor_workspace_bytes
                        + growth,
                    0,
                )?;
                normal.paths.reserve_exact(target - normal.paths.len());
                let begin = normal.paths.len();
                normal.paths.append(&mut component.paths);
                owners.push(NormalOwner {
                    owner: polygon,
                    paths: begin..normal.paths.len(),
                });
                normal_points = point_sum(normal_points, component_points)?;
                admit(
                    normal_points + class_points + bridge_points,
                    recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                        + candidate_bytes(&component)
                        + original.retained_bytes()
                        + anchor_workspace_bytes,
                    0,
                )?;
            }
        }
        let mut anchor_workspace_bytes;
        let mut skin_construction_used = false;
        if bridge.paths.is_empty() {
            if !mandatory_demand {
                return Ok(normal.paths);
            }
            let base = recovery_points(
                class_points,
                normal_points,
                bridge_points,
                owners.capacity(),
                patches.capacity(),
            )?;
            let scratch_units = base - path_points(&normal.paths);
            let scratch_bytes =
                recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                    - candidate_bytes(&normal);
            let direct = anchors(&mut normal, layer.axis, scratch_units, scratch_bytes)?;
            anchor_workspace_bytes = direct.workspace_bytes;
            bridge.paths = self.demanded_skin_bridges(
                &normal.paths,
                &direct.demands,
                layer,
                point_sum(base, direct.retained_points())?,
                recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                    + direct.retained_bytes(),
                &mut anchor_workspace_bytes,
                &mut admit,
                anchors,
                cancel,
            )?;
            if bridge.paths.is_empty() {
                return Ok(normal.paths);
            }
            bridge_points = path_points(&bridge.paths);
            skin_construction_used = true;
        }
        let original_patch_count = patches.len();
        loop {
            patches.truncate(original_patch_count);
            let base = recovery_points(
                class_points,
                normal_points,
                bridge_points,
                owners.capacity(),
                patches.capacity(),
            )?;
            let whole_row_count = bridge
                .paths
                .iter()
                .filter(|path| path.points_mm.len() == 2)
                .count();
            if skin_construction_used || whole_row_count == 0 {
                // Existing bridges and added skin rows already have whole-stroke
                // proofs. Recheck their union, never construct their corridors twice.
                bridge.whole_rows.clear();
            } else {
                let base_bytes =
                    recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches);
                let workspace_receipt = anchors(&mut probe, layer.axis, base, base_bytes)?;
                let workspace = workspace_receipt.workspace_points;
                anchor_workspace_bytes = workspace_receipt.workspace_bytes;
                drop(workspace_receipt);
                let bound = whole_row_bound(&self.layout.fill, whole_row_count)?;
                admit(
                    point_sum(point_sum(base, workspace)?, bound)?,
                    byte_sum(
                        base_bytes + anchor_workspace_bytes,
                        whole_row_storage_bound(bound, whole_row_count)?,
                    )?,
                    bound,
                )?;
                let mut staged_segments = *segments;
                let whole_base_bytes =
                    recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                        + anchor_workspace_bytes;
                bridge.whole_rows = self.whole_rows(
                    &bridge.paths,
                    layer,
                    &mut staged_segments,
                    cancel,
                    |units, bytes| {
                        admit(
                            point_sum(base, units)?,
                            byte_sum(whole_base_bytes, bytes)?,
                            0,
                        )
                    },
                )?;
            }
            let scratch_bytes =
                recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                    - candidate_bytes(&bridge);
            let merge = anchors(&mut bridge, layer.axis, base - bridge_points, scratch_bytes)?;
            anchor_workspace_bytes = merge.workspace_bytes;
            if !merge.supported || merge.bridges.len() != bridge.paths.len() {
                return Ok(normal.paths);
            }
            bridge_points = path_points(&bridge.paths)
                + super::bonds::skin_bridge_points(&bridge.skin_bridge_domains);
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
                if matches!(path.owner, Some(DepositionOwner::RegionalSkinBridge { .. })) {
                    continue;
                }
                let patch_base_bytes =
                    recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                        - capacity_bytes(&patches)
                        + merge.retained_bytes()
                        + anchor_workspace_bytes;
                reserve_recovery_record(
                    &mut patches,
                    BRIDGE_PATCH_POINTS,
                    point_sum(base, merge.workspace_points)?,
                    patch_base_bytes,
                    &mut admit,
                )?;
                let along_axis = if path.points_mm[0][0] == path.points_mm[1][0] {
                    1
                } else {
                    0
                };
                let fixed_axis = 1 - along_axis;
                let coordinate = path.points_mm[0][fixed_axis];
                patches.push(BridgePatch {
                    owner: None,
                    along_axis,
                    along: [
                        path.points_mm[0][along_axis].min(path.points_mm[1][along_axis]),
                        path.points_mm[0][along_axis].max(path.points_mm[1][along_axis]),
                    ],
                    across: [
                        coordinate - region::constant_width(path)? * 0.5,
                        coordinate + region::constant_width(path)? * 0.5,
                    ],
                });
            }
            let staged = point_sum(base, patches.capacity() * BRIDGE_PATCH_POINTS)?;
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
                recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                    + merge.retained_bytes()
                    + anchor_workspace_bytes
                    + patches.len() * std::mem::size_of::<[f64; 2]>(),
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
            let staged_bytes =
                recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                    + merge.retained_bytes()
                    + capacity_bytes(&intervals);
            let mut mixed = super::support::RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
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
                    let normal_along = if a[0] == b[0] { 1 } else { 0 };
                    let normal_fixed = 1 - normal_along;
                    let path_layer = region::HatchLayer {
                        axis: if normal_along == 0 {
                            region::HatchAxis::X
                        } else {
                            region::HatchAxis::Y
                        },
                        ..*layer
                    };
                    for patch in &patches {
                        check_cancel(cancel)?;
                        let fixed_bounds = if patch.along_axis == normal_fixed {
                            patch.along
                        } else {
                            patch.across
                        };
                        let cut = if patch.along_axis == normal_along {
                            patch.along
                        } else {
                            patch.across
                        };
                        let matches_row = if fixed_bounds[0] == fixed_bounds[1] {
                            program_coordinates::project_xyz6_coordinate_mm(a[normal_fixed])?
                                == program_coordinates::project_xyz6_coordinate_mm(fixed_bounds[0])?
                        } else {
                            a[normal_fixed] >= fixed_bounds[0] && a[normal_fixed] <= fixed_bounds[1]
                        };
                        if patch
                            .owner
                            .is_none_or(|polygon| std::ptr::eq(polygon, owner.owner))
                            && matches_row
                            && cut[0] < cut[1]
                        {
                            intervals.push(cut);
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
                            let target = record_target(&mixed.paths, 1);
                            admit(
                                point_sum(
                                    point_sum(
                                        point_sum(staged, merge.workspace_points)?,
                                        mixed_points,
                                    )?,
                                    2,
                                )?,
                                staged_bytes
                                    + candidate_bytes(&mixed)
                                    + anchor_workspace_bytes
                                    + (target - mixed.paths.capacity())
                                        * std::mem::size_of::<DepositionPath>()
                                    + 2 * std::mem::size_of::<[f64; 3]>(),
                                0,
                            )?;
                            mixed.paths.reserve_exact(target - mixed.paths.len());
                            let before = mixed.paths.len();
                            region::append_hatch_row(
                                &[cursor, interval[0].min(end)],
                                a[normal_fixed],
                                path.kind,
                                &path_layer,
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
                        let target = record_target(&mixed.paths, 1);
                        admit(
                            point_sum(
                                point_sum(
                                    point_sum(staged, merge.workspace_points)?,
                                    mixed_points,
                                )?,
                                2,
                            )?,
                            staged_bytes
                                + candidate_bytes(&mixed)
                                + anchor_workspace_bytes
                                + (target - mixed.paths.capacity())
                                    * std::mem::size_of::<DepositionPath>()
                                + 2 * std::mem::size_of::<[f64; 3]>(),
                            0,
                        )?;
                        mixed.paths.reserve_exact(target - mixed.paths.len());
                        let before = mixed.paths.len();
                        region::append_hatch_row(
                            &[cursor, end],
                            a[normal_fixed],
                            path.kind,
                            &path_layer,
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
            admit(
                staged + mixed_points,
                recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                    + candidate_bytes(&mixed)
                    + merge.retained_bytes()
                    + capacity_bytes(&intervals)
                    + anchor_workspace_bytes
                    + (mixed.paths.len() + bridge.paths.len())
                        .saturating_sub(mixed.paths.capacity())
                        * std::mem::size_of::<DepositionPath>(),
                0,
            )?;
            mixed.paths.reserve_exact(bridge.paths.len());
            let ordinary_count = mixed.paths.len();
            let moved_bridge_points = path_points(&bridge.paths);
            mixed.paths.append(&mut bridge.paths);
            let scratch_bytes =
                recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                    + merge.retained_bytes()
                    + capacity_bytes(&intervals);
            let proof = anchors(
                &mut mixed,
                layer.axis,
                staged - moved_bridge_points,
                scratch_bytes,
            )?;
            anchor_workspace_bytes = proof.workspace_bytes;
            if !proof.supported {
                bridge.paths.extend(mixed.paths.drain(ordinary_count..));
                drop(mixed);
                if !skin_construction_used {
                    let retained = recovery_points(
                        class_points,
                        normal_points,
                        path_points(&bridge.paths)
                            + super::bonds::skin_bridge_points(&bridge.skin_bridge_domains),
                        owners.capacity(),
                        patches.capacity(),
                    )?;
                    let mut additional = self.demanded_skin_bridges(
                        &normal.paths,
                        &proof.demands,
                        layer,
                        point_sum(retained, proof.retained_points())?,
                        recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                            + merge.retained_bytes()
                            + proof.retained_bytes()
                            + capacity_bytes(&intervals),
                        &mut anchor_workspace_bytes,
                        &mut admit,
                        anchors,
                        cancel,
                    )?;
                    if !additional.is_empty() {
                        admit(
                            point_sum(retained, path_points(&additional))?,
                            recovery_bytes(
                                classified_owned_bytes,
                                &normal,
                                &bridge,
                                &owners,
                                &patches,
                            ) + merge.retained_bytes()
                                + proof.retained_bytes()
                                + capacity_bytes(&intervals)
                                + anchor_workspace_bytes
                                + super::bonds::path_bytes(&additional)
                                + additional.len() * std::mem::size_of::<DepositionPath>(),
                            additional.len(),
                        )?;
                        bridge
                            .paths
                            .try_reserve_exact(additional.len())
                            .map_err(|_| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "required skin bridge publication allocation refused",
                                )
                            })?;
                        bridge.paths.append(&mut additional);
                        bridge_points = path_points(&bridge.paths)
                            + super::bonds::skin_bridge_points(&bridge.skin_bridge_domains);
                        skin_construction_used = true;
                        continue;
                    }
                }
                return Ok(normal.paths);
            }
            admit(
                point_sum(staged, path_points(&mixed.paths))?,
                recovery_bytes(classified_owned_bytes, &normal, &bridge, &owners, &patches)
                    + candidate_bytes(&mixed)
                    + merge.retained_bytes()
                    + proof.retained_bytes()
                    + capacity_bytes(&intervals)
                    + anchor_workspace_bytes
                    + (wall_count + mixed.paths.len()) * std::mem::size_of::<DepositionPath>(),
                0,
            )?;
            let normal_aux_bytes =
                candidate_bytes(&normal) - super::bonds::path_bytes(&normal.paths);
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
            admit(
                class_points + path_points(&result),
                classified_owned_bytes
                    + normal_aux_bytes
                    + candidate_bytes(&bridge)
                    + candidate_bytes(&mixed)
                    + capacity_bytes(&owners)
                    + capacity_bytes(&patches)
                    + capacity_bytes(&intervals)
                    + merge.retained_bytes()
                    + proof.retained_bytes()
                    + anchor_workspace_bytes
                    + super::bonds::path_bytes(&result),
                0,
            )?;
            *segments = output_segments;
            *constructed_domains = bridge.skin_bridge_domains;
            return Ok(result);
        }
    }

    /// Extend only genuinely demanded original skin rows, after the existing
    /// whole candidate has failed. The original rows and demand set stay intact.
    fn demanded_skin_bridges(
        &self,
        required: &[DepositionPath],
        demands: &[[[f64; 2]; 2]],
        layer: &region::HatchLayer<'_>,
        retained: usize,
        retained_bytes: usize,
        workspace_bytes: &mut usize,
        admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
        anchors: &mut ComponentAnchorCheck<'_>,
        cancel: &AtomicBool,
    ) -> Result<Vec<DepositionPath>, ManufacturingError> {
        let mut output = Vec::new();
        let work = required
            .len()
            .checked_mul(demands.len())
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "required skin demand selection work overflow",
                )
            })?;
        admit(retained, retained_bytes + *workspace_bytes, work)?;
        for axis in [layer.axis, layer.axis.orthogonal()] {
            let fixed = usize::from(axis == region::HatchAxis::X);
            let along = 1 - fixed;
            let selected = |path: &&DepositionPath| {
                let Some(DepositionOwner::MandatorySkin { skin }) = path.owner else {
                    return false;
                };
                let authored = if skin == ProcessSkinKind::TopSkin {
                    layer.top_axis
                } else {
                    layer.axis
                };
                if authored != axis || path.points_mm.len() != 2 {
                    return false;
                }
                let a = path.points_mm[0];
                let b = path.points_mm[1];
                demands.iter().any(|gap| {
                    gap[0][fixed] == a[fixed]
                        && gap[1][fixed] == a[fixed]
                        && gap[0][along].min(gap[1][along]) < a[along].max(b[along])
                        && a[along].min(b[along]) < gap[0][along].max(gap[1][along])
                })
            };
            let count = required.iter().filter(selected).count();
            if count == 0 {
                continue;
            }
            let points = count.checked_mul(2).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "required skin construction point overflow",
                )
            })?;
            let live = point_sum(retained, path_points(&output))?;
            let live_bytes = retained_bytes + super::bonds::path_bytes(&output);
            let clone_bytes = count * std::mem::size_of::<DepositionPath>()
                + required
                    .iter()
                    .filter(selected)
                    .map(|path| path.points_mm.len() * std::mem::size_of::<[f64; 3]>())
                    .sum::<usize>();
            admit(
                point_sum(live, points)?,
                byte_sum(live_bytes + *workspace_bytes, clone_bytes)?,
                points,
            )?;
            let mut candidate = super::support::RasterCandidate {
                skin_command_fits: Vec::new(),
                paths: Vec::new(),
                whole_rows: Vec::new(),
                process_interfaces: Vec::new(),
                skin_bridge_domains: Vec::new(),
            };
            candidate.paths.try_reserve_exact(count).map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "required skin construction allocation refused",
                )
            })?;
            candidate
                .paths
                .extend(required.iter().filter(selected).cloned());
            let role_layer = region::HatchLayer { axis, ..*layer };
            let bound = whole_row_bound(&self.layout.fill, count)?;
            admit(
                point_sum(point_sum(live, points)?, bound)?,
                byte_sum(
                    live_bytes + *workspace_bytes + candidate_bytes(&candidate),
                    whole_row_storage_bound(bound, count)?,
                )?,
                bound,
            )?;
            let whole_base_bytes = live_bytes + *workspace_bytes + candidate_bytes(&candidate);
            candidate.whole_rows = self.whole_rows(
                &candidate.paths,
                &role_layer,
                &mut 0,
                cancel,
                |units, bytes| {
                    admit(
                        point_sum(point_sum(live, points)?, units)?,
                        byte_sum(whole_base_bytes, bytes)?,
                        0,
                    )
                },
            )?;
            let candidate_live =
                point_sum(point_sum(live, points)?, path_points(&candidate.whole_rows))?;
            let proof = anchors(&mut candidate, axis, live, live_bytes)?;
            *workspace_bytes = proof.workspace_bytes;
            let candidate_aux_bytes =
                candidate_bytes(&candidate) - super::bonds::path_bytes(&candidate.paths);
            admit(
                candidate_live + proof.retained_points(),
                live_bytes
                    + candidate_bytes(&candidate)
                    + proof.retained_bytes()
                    + *workspace_bytes
                    + (output.len() + proof.bridges.len()).saturating_sub(output.capacity())
                        * std::mem::size_of::<DepositionPath>(),
                0,
            )?;
            output.try_reserve_exact(proof.bridges.len()).map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "required skin accepted-row allocation refused",
                )
            })?;
            for (index, mut path) in candidate.paths.into_iter().enumerate() {
                if proof.bridges.binary_search(&index).is_ok() {
                    path.kind = DepositionKind::Bridge;
                    output.push(path);
                }
            }
            admit(
                retained + path_points(&output),
                retained_bytes
                    + super::bonds::path_bytes(&output)
                    + candidate_aux_bytes
                    + proof.retained_bytes()
                    + *workspace_bytes,
                0,
            )?;
        }
        Ok(output)
    }

    /// Repartition already-required skin at a genuinely demanded regional
    /// terminal. The authored regional core/lattice never moves or grows.
    fn repair_skin_terminals<'a>(
        &'a self,
        polygon: &'a Polygon<f64>,
        region_index: u16,
        interior: &InteriorRegion,
        source_owner: &MultiPolygon<f64>,
        required: &[DepositionPath],
        demands: &[[[f64; 2]; 2]],
        layer: &region::HatchLayer<'_>,
        retained: usize,
        retained_bytes: usize,
        workspace_bytes: &mut usize,
        bridge: &mut super::support::RasterCandidate,
        patches: &mut Vec<BridgePatch<'a>>,
        admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
        anchors: &mut ComponentAnchorCheck<'_>,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        if demands.is_empty()
            || !self.separator_bands.is_empty()
            || self
                .coverage
                .process_diaphragms
                .iter()
                .any(|plate| plate.region_index == region_index)
        {
            return Ok(false);
        }
        let along = usize::from(layer.axis == region::HatchAxis::Y);
        let fixed = 1 - along;
        let grid = program_coordinates::project_xyz6_coordinate_mm;
        let source_points = region::point_count(source_owner);
        let demand_work = required.len().checked_mul(demands.len()).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "mixed bridge demand selection work overflow",
            )
        })?;
        admit(
            retained,
            retained_bytes + candidate_bytes(bridge) + capacity_bytes(patches) + *workspace_bytes,
            demand_work,
        )?;
        let mut selected = super::support::RasterCandidate {
            skin_command_fits: Vec::new(),
            paths: Vec::new(),
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
            skin_bridge_domains: Vec::new(),
        };
        let mut selected_points = 0usize;
        let mut hits = Vec::new();
        let mut source_hits = Vec::new();
        let polygon_points = |polygon: &Polygon<f64>| {
            polygon.exterior().0.len()
                + polygon
                    .interiors()
                    .iter()
                    .map(|ring| ring.0.len())
                    .sum::<usize>()
        };
        let hit_bound = self
            .bottom
            .0
            .iter()
            .chain(&self.top.0)
            .map(polygon_points)
            .max()
            .unwrap_or(0);
        let source_hit_bound = source_owner.0.iter().map(polygon_points).max().unwrap_or(0);
        'rows: for path in required {
            check_cancel(cancel)?;
            let relevant = demands
                .iter()
                .filter(|gap| {
                    let a = path.points_mm[0];
                    let b = path.points_mm[1];
                    gap[0][fixed] == a[fixed]
                        && gap[0][along].min(gap[1][along]) >= a[along].min(b[along])
                        && gap[0][along].max(gap[1][along]) <= a[along].max(b[along])
                })
                .count();
            if relevant == 0 {
                continue;
            }
            admit(
                retained + hit_bound + source_points,
                retained_bytes
                    + candidate_bytes(bridge)
                    + capacity_bytes(patches)
                    + candidate_bytes(&selected)
                    + *workspace_bytes
                    + (hits.capacity().max(hit_bound)
                        + source_hits.capacity().max(source_hit_bound))
                        * std::mem::size_of::<f64>(),
                0,
            )?;
            hits.clear();
            source_hits.clear();
            hits.reserve_exact(hit_bound);
            source_hits.reserve_exact(source_hit_bound);
            let mut chosen = None;
            for (skin, domain) in [
                (ProcessSkinKind::BottomSkin, &self.bottom),
                (ProcessSkinKind::TopSkin, &self.top),
            ] {
                for skin_polygon in &domain.0 {
                    admit(
                        retained + selected_points + region::point_count(domain) + 3,
                        repair_bytes(
                            retained_bytes,
                            bridge,
                            patches,
                            &selected,
                            &hits,
                            &source_hits,
                        ) + *workspace_bytes
                            + chosen.as_ref().map_or(0, |p: &DepositionPath| {
                                p.points_mm.capacity() * std::mem::size_of::<[f64; 3]>()
                            }),
                        skin_polygon.exterior().0.len()
                            + skin_polygon
                                .interiors()
                                .iter()
                                .map(|r| r.0.len())
                                .sum::<usize>(),
                    )?;
                    hits.clear();
                    region::row_intersections(
                        skin_polygon,
                        path.points_mm[0][fixed],
                        layer.axis,
                        &mut hits,
                        cancel,
                    )?;
                    hits.sort_unstable_by(f64::total_cmp);
                    if !hits.len().is_multiple_of(2) {
                        continue 'rows;
                    }
                    for interval in hits.chunks_exact(2) {
                        for endpoint in 0..2 {
                            let transition = path.points_mm[endpoint];
                            let core = path.points_mm[1 - endpoint];
                            let far = if grid(transition[along])? == grid(interval[0])?
                                && core[along] < transition[along]
                            {
                                Some(interval[1])
                            } else if grid(transition[along])? == grid(interval[1])?
                                && core[along] > transition[along]
                            {
                                Some(interval[0])
                            } else {
                                None
                            };
                            let Some(mut far) = far else {
                                continue;
                            };
                            let mut source_limit = None;
                            for source_polygon in &source_owner.0 {
                                source_hits.clear();
                                admit(
                                    retained
                                        + selected_points
                                        + hits.capacity()
                                        + source_points
                                        + 3,
                                    repair_bytes(
                                        retained_bytes,
                                        bridge,
                                        patches,
                                        &selected,
                                        &hits,
                                        &source_hits,
                                    ) + *workspace_bytes
                                        + chosen.as_ref().map_or(0, |p: &DepositionPath| {
                                            p.points_mm.capacity() * std::mem::size_of::<[f64; 3]>()
                                        }),
                                    source_points,
                                )?;
                                region::row_intersections(
                                    source_polygon,
                                    transition[fixed],
                                    layer.axis,
                                    &mut source_hits,
                                    cancel,
                                )?;
                                source_hits.sort_unstable_by(f64::total_cmp);
                                if !source_hits.len().is_multiple_of(2) {
                                    continue 'rows;
                                }
                                for interval in source_hits.chunks_exact(2) {
                                    if grid(interval[0])?
                                        <= grid(core[along].min(transition[along]))?
                                        && grid(core[along].max(transition[along]))?
                                            <= grid(interval[1])?
                                    {
                                        let limit = if far < transition[along] {
                                            interval[0].max(far)
                                        } else {
                                            interval[1].min(far)
                                        };
                                        if source_limit.replace(limit).is_some() {
                                            continue 'rows;
                                        }
                                    }
                                }
                            }
                            let Some(limit) = source_limit else {
                                continue;
                            };
                            far = limit;
                            if grid(far)? == grid(transition[along])? {
                                continue;
                            }
                            let mut skin_end = transition;
                            skin_end[along] = far;
                            let skin_first = endpoint == 0;
                            let chosen_bytes = chosen.as_ref().map_or(0, |p: &DepositionPath| {
                                p.points_mm.capacity() * std::mem::size_of::<[f64; 3]>()
                            });
                            let scratch_bytes = repair_bytes(
                                retained_bytes,
                                bridge,
                                patches,
                                &selected,
                                &hits,
                                &source_hits,
                            ) + chosen_bytes;
                            admit(
                                retained + selected_points + 3,
                                scratch_bytes
                                    + *workspace_bytes
                                    + std::mem::size_of::<DepositionPath>()
                                    + 3 * std::mem::size_of::<[f64; 3]>(),
                                0,
                            )?;
                            let mut proposal = super::support::RasterCandidate {
                                skin_command_fits: Vec::new(),
                                paths: vec![DepositionPath {
                                    material_domain: None,
                                    kind: DepositionKind::Bridge,
                                    owner: Some(DepositionOwner::RegionalSkinBridge {
                                        region_index,
                                        skin,
                                        skin_first,
                                    }),
                                    width: WidthProfile::Constant {
                                        width_mm: region::constant_width(path)?,
                                    },
                                    physical_z_mm: path.physical_z_mm,
                                    points_mm: if skin_first {
                                        vec![skin_end, transition, core]
                                    } else {
                                        vec![core, transition, skin_end]
                                    },
                                }],
                                whole_rows: Vec::new(),
                                process_interfaces: Vec::new(),
                                skin_bridge_domains: Vec::new(),
                            };
                            let proof = anchors(
                                &mut proposal,
                                layer.axis,
                                retained + selected_points,
                                scratch_bytes,
                            )?;
                            *workspace_bytes = proof.workspace_bytes;
                            if proof.supported && proof.bridges == [0] {
                                let actual = &proposal.paths[0];
                                if grid(actual.points_mm[if skin_first { 0 } else { 2 }][along])?
                                    == grid(transition[along])?
                                {
                                    continue;
                                }
                                if chosen.is_some() {
                                    continue 'rows;
                                }
                                chosen = Some(proposal.paths.remove(0));
                            }
                        }
                    }
                }
            }
            let Some(chosen) = chosen else {
                continue;
            };
            let target = record_target(&selected.paths, 1);
            let growth =
                (target - selected.paths.capacity()) * std::mem::size_of::<DepositionPath>();
            admit(
                retained + selected_points + chosen.points_mm.capacity(),
                repair_bytes(
                    retained_bytes,
                    bridge,
                    patches,
                    &selected,
                    &hits,
                    &source_hits,
                ) + chosen.points_mm.capacity() * std::mem::size_of::<[f64; 3]>()
                    + *workspace_bytes
                    + growth,
                0,
            )?;
            selected.paths.reserve_exact(target - selected.paths.len());
            selected_points = point_sum(selected_points, chosen.points_mm.capacity())?;
            selected.paths.push(chosen);
        }
        if selected.paths.is_empty() {
            return Ok(false);
        }
        let mut staged_patches = Vec::new();
        for path in &selected.paths {
            let Some(DepositionOwner::RegionalSkinBridge {
                skin, skin_first, ..
            }) = path.owner
            else {
                unreachable!()
            };
            let skin_domain = if skin == ProcessSkinKind::TopSkin {
                &self.top
            } else {
                &self.bottom
            };
            let core_end = path.points_mm[if skin_first { 2 } else { 0 }][along];
            let transition = path.points_mm[1][along];
            let skin_end = path.points_mm[if skin_first { 0 } else { 2 }][along];
            let coordinate = path.points_mm[0][fixed];
            let scratch_bytes = repair_bytes(
                retained_bytes,
                bridge,
                patches,
                &selected,
                &hits,
                &source_hits,
            ) + *workspace_bytes;
            reserve_recovery_record(
                &mut staged_patches,
                BRIDGE_PATCH_POINTS,
                retained
                    + selected_points
                    + super::bonds::skin_bridge_points(&selected.skin_bridge_domains),
                scratch_bytes,
                admit,
            )?;
            staged_patches.push(BridgePatch {
                owner: Some(polygon),
                along_axis: along,
                along: [core_end.min(transition), core_end.max(transition)],
                across: [coordinate, coordinate],
            });
            for skin_polygon in &skin_domain.0 {
                let scratch_bytes = repair_bytes(
                    retained_bytes,
                    bridge,
                    patches,
                    &selected,
                    &hits,
                    &source_hits,
                ) + *workspace_bytes;
                reserve_recovery_record(
                    &mut staged_patches,
                    BRIDGE_PATCH_POINTS,
                    retained
                        + selected_points
                        + super::bonds::skin_bridge_points(&selected.skin_bridge_domains),
                    scratch_bytes,
                    admit,
                )?;
                staged_patches.push(BridgePatch {
                    owner: Some(skin_polygon),
                    along_axis: along,
                    along: [skin_end.min(transition), skin_end.max(transition)],
                    across: [coordinate, coordinate],
                });
            }
            if !bridge
                .skin_bridge_domains
                .iter()
                .chain(&selected.skin_bridge_domains)
                .any(|domain| domain.region_index == region_index && domain.skin == skin)
            {
                let points = region::point_count(&interior.domain)
                    + region::point_count(skin_domain)
                    + source_points;
                let metadata =
                    (interior.domain.0.len() + skin_domain.0.len() + source_owner.0.len())
                        * std::mem::size_of::<DiaphragmPolygonMm>()
                        / std::mem::size_of::<geo::Coord<f64>>()
                        + std::mem::size_of::<SkinBridgeDomain>()
                            .div_ceil(std::mem::size_of::<geo::Coord<f64>>());
                admit(
                    retained
                        + selected_points
                        + points
                        + metadata
                        + super::bonds::skin_bridge_points(&selected.skin_bridge_domains)
                        + staged_patches.capacity() * BRIDGE_PATCH_POINTS,
                    repair_bytes(
                        retained_bytes,
                        bridge,
                        patches,
                        &selected,
                        &hits,
                        &source_hits,
                    ) + capacity_bytes(&staged_patches)
                        + *workspace_bytes
                        + encoded_domain_bytes(&interior.domain)
                        + encoded_domain_bytes(skin_domain)
                        + encoded_domain_bytes(source_owner)
                        + std::mem::size_of::<SkinBridgeDomain>(),
                    points,
                )?;
                selected
                    .skin_bridge_domains
                    .try_reserve_exact(1)
                    .map_err(|_| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "mixed bridge domain table allocation refused",
                        )
                    })?;
                let encode = |domain: &MultiPolygon<f64>| {
                    domain
                        .0
                        .iter()
                        .map(|polygon| DiaphragmPolygonMm {
                            outer: polygon.exterior().0.iter().map(|p| [p.x, p.y]).collect(),
                            holes: polygon
                                .interiors()
                                .iter()
                                .map(|ring| ring.0.iter().map(|p| [p.x, p.y]).collect())
                                .collect(),
                        })
                        .collect()
                };
                selected.skin_bridge_domains.push(SkinBridgeDomain {
                    region_index,
                    skin,
                    regional: encode(&interior.domain),
                    mandatory: encode(skin_domain),
                    source_owner: encode(source_owner),
                });
                admit(
                    retained
                        + selected_points
                        + super::bonds::skin_bridge_points(&selected.skin_bridge_domains)
                        + staged_patches.capacity() * BRIDGE_PATCH_POINTS,
                    repair_bytes(
                        retained_bytes,
                        bridge,
                        patches,
                        &selected,
                        &hits,
                        &source_hits,
                    ) + capacity_bytes(&staged_patches)
                        + *workspace_bytes,
                    0,
                )?;
            }
        }
        let staged_points = selected_points
            + super::bonds::skin_bridge_points(&selected.skin_bridge_domains)
            + staged_patches.capacity() * BRIDGE_PATCH_POINTS;
        let patch_bound = patches
            .len()
            .checked_add(staged_patches.len())
            .and_then(|count| count.checked_mul(BRIDGE_PATCH_POINTS))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "mixed patch publication size overflow",
                )
            })?;
        let domain_bound = bridge
            .skin_bridge_domains
            .len()
            .checked_add(selected.skin_bridge_domains.len())
            .and_then(|count| {
                count.checked_mul(
                    std::mem::size_of::<SkinBridgeDomain>()
                        .div_ceil(std::mem::size_of::<geo::Coord<f64>>()),
                )
            })
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "mixed domain publication size overflow",
                )
            })?;
        admit(
            point_sum(
                point_sum(point_sum(retained, staged_points)?, patch_bound)?,
                domain_bound,
            )?,
            repair_bytes(
                retained_bytes,
                bridge,
                patches,
                &selected,
                &hits,
                &source_hits,
            ) + capacity_bytes(&staged_patches)
                + *workspace_bytes
                + (patches.len() + staged_patches.len()).saturating_sub(patches.capacity())
                    * std::mem::size_of::<BridgePatch<'_>>()
                + (bridge.skin_bridge_domains.len() + selected.skin_bridge_domains.len())
                    .saturating_sub(bridge.skin_bridge_domains.capacity())
                    * std::mem::size_of::<SkinBridgeDomain>()
                + (bridge.paths.len() + selected.paths.len())
                    .saturating_sub(bridge.paths.capacity())
                    * std::mem::size_of::<DepositionPath>(),
            0,
        )?;
        patches
            .try_reserve_exact(staged_patches.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "mixed patch publication allocation refused",
                )
            })?;
        bridge
            .skin_bridge_domains
            .try_reserve_exact(selected.skin_bridge_domains.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "mixed domain publication allocation refused",
                )
            })?;
        bridge
            .paths
            .try_reserve_exact(selected.paths.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "mixed path publication allocation refused",
                )
            })?;
        patches.append(&mut staged_patches);
        bridge
            .skin_bridge_domains
            .append(&mut selected.skin_bridge_domains);
        bridge.paths.append(&mut selected.paths);
        admit(
            retained + staged_points,
            repair_bytes(
                retained_bytes,
                bridge,
                patches,
                &selected,
                &hits,
                &source_hits,
            ) + capacity_bytes(&staged_patches)
                + *workspace_bytes,
            0,
        )?;
        Ok(true)
    }

    /// Positive-fill row bounds at the actual classified hatch coordinates.
    /// These permit bounded anchor extensions, never unrequested sparse rows.
    /// Admit LOCAL owned row staging separately from borrowed classification and
    /// requested paths; callers add those external owners exactly once.
    pub(super) fn whole_rows(
        &self,
        required: &[DepositionPath],
        layer: &region::HatchLayer<'_>,
        segments: &mut usize,
        cancel: &AtomicBool,
        mut admit: impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
    ) -> Result<Vec<DepositionPath>, ManufacturingError> {
        let mut rows = Vec::new();
        for axis in [layer.axis, layer.axis.orthogonal()] {
            let row_layer = region::HatchLayer { axis, ..*layer };
            let fixed = usize::from(axis == region::HatchAxis::X);
            if !required.iter().any(|path| {
                path.points_mm.len() == 2 && path.points_mm[0][fixed] == path.points_mm[1][fixed]
            }) {
                continue;
            }
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
                let mandatory =
                    matches!(kind, DepositionKind::BottomSkin | DepositionKind::TopSkin);
                let corridor = if mandatory { &self.layout.fill } else { domain };
                let excluded = if mandatory {
                    &self.separator_bands[..]
                } else {
                    &[]
                };
                let requested = required
                    .iter()
                    .filter(|path| {
                        (path.kind == kind || path.kind == DepositionKind::Bridge)
                            && path.owner == original
                            && path.points_mm.len() == 2
                            && path.points_mm[0][fixed] == path.points_mm[1][fixed]
                    })
                    .count();
                let bound = if requested == 0 {
                    0
                } else {
                    whole_row_bound(corridor, requested)?
                };
                let exclusion_points = excluded.iter().map(region::point_count).sum::<usize>();
                admit(
                    path_points(&rows) + bound + exclusion_points,
                    byte_sum(
                        super::bonds::path_bytes(&rows),
                        byte_sum(
                            whole_row_storage_bound(bound, requested)?,
                            exclusion_points * std::mem::size_of::<f64>(),
                        )?,
                    )?,
                )?;
                let owned = region::bridge_rows(
                    corridor,
                    excluded,
                    required.iter().filter(|path| {
                        (path.kind == kind || path.kind == DepositionKind::Bridge)
                            && path.owner == original
                            && path.points_mm.len() == 2
                            && path.points_mm[0][fixed] == path.points_mm[1][fixed]
                    }),
                    &row_layer,
                    segments,
                    cancel,
                )?;
                let target = record_target(&rows, owned.len());
                admit(
                    path_points(&rows) + path_points(&owned),
                    super::bonds::path_bytes(&rows)
                        + super::bonds::path_bytes(&owned)
                        + (target - rows.capacity()) * std::mem::size_of::<DepositionPath>(),
                )?;
                rows.reserve_exact(target - rows.len());
                rows.extend(owned);
                admit(path_points(&rows), super::bonds::path_bytes(&rows))?;
            }
        }
        Ok(rows)
    }
}

fn admit_split_inventory(
    admit: &mut impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
    old_records: usize,
    pending: &mut [Polygon<f64>],
    polygon: &mut Polygon<f64>,
    owned: &mut MultiPolygon<f64>,
    next: &mut MultiPolygon<f64>,
    extra: &mut [&mut MultiPolygon<f64>],
    growth_bytes: usize,
) -> Result<(), ManufacturingError> {
    let mut bytes = byte_sum(old_records, growth_bytes)?;
    let mut units = 0;
    for p in pending.iter_mut().chain(std::iter::once(polygon)) {
        bytes = byte_sum(bytes, super::native::polygon_bytes(p))?;
        units = point_sum(
            units,
            p.exterior().0.len() + p.interiors().iter().map(|r| r.0.len()).sum::<usize>(),
        )?;
    }
    for geometry in std::iter::once(owned)
        .chain(std::iter::once(next))
        .chain(extra.iter_mut().map(|p| &mut **p))
    {
        bytes = byte_sum(bytes, super::native::material_bytes(geometry))?;
        units = point_sum(units, region::point_count(geometry))?;
    }
    admit(units, bytes)
}

/// A conditioned exact set relation avoids re-quantizing already-contained
/// eligible islands. Partial cuts use complementary operations on the same
/// original operands; any unresolved residual remains an error, never an epsilon.
pub(super) fn split_process_region(
    remaining: MultiPolygon<f64>,
    material: &MultiPolygon<f64>,
    mut admit: impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(MultiPolygon<f64>, MultiPolygon<f64>, bool), ManufacturingError> {
    let mut owned = MultiPolygon(Vec::new());
    let mut next = MultiPolygon(Vec::new());
    let mut partial = false;
    let old_records = remaining.0.capacity() * std::mem::size_of::<Polygon<f64>>();
    let mut pending = remaining.0.into_iter();
    while let Some(mut polygon) = pending.next() {
        let owned_target = if owned.0.len() == owned.0.capacity() {
            owned.0.capacity().saturating_mul(2).max(4)
        } else {
            owned.0.capacity()
        };
        let next_target = if next.0.len() == next.0.capacity() {
            next.0.capacity().saturating_mul(2).max(4)
        } else {
            next.0.capacity()
        };
        let growth = (owned_target - owned.0.capacity() + next_target - next.0.capacity())
            * std::mem::size_of::<Polygon<f64>>();
        admit_split_inventory(
            &mut admit,
            old_records,
            pending.as_mut_slice(),
            &mut polygon,
            &mut owned,
            &mut next,
            &mut [],
            growth,
        )?;
        owned.0.reserve_exact(owned_target - owned.0.len());
        next.0.reserve_exact(next_target - next.0.len());
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
        admit_split_inventory(
            &mut admit,
            old_records,
            pending.as_mut_slice(),
            &mut polygon,
            &mut owned,
            &mut next,
            &mut [],
            std::mem::size_of::<Polygon<f64>>(),
        )?;
        let mut original = MultiPolygon(vec![polygon]);
        let mut inside = intersection_accumulated(&original, material, work, cancel)?;
        admit_split_inventory(
            &mut admit,
            old_records,
            pending.as_mut_slice(),
            &mut original.0[0],
            &mut owned,
            &mut next,
            &mut [&mut inside],
            std::mem::size_of::<Polygon<f64>>(),
        )?;
        // The first output coexists with its unchanged original and the second
        // output. Measure each actual owner immediately after the opaque call.
        let mut outside = operation(&original, material, false, Some(work), cancel)?;
        let extra_records = (owned.0.len() + inside.0.len()).saturating_sub(owned.0.capacity())
            + (next.0.len() + outside.0.len()).saturating_sub(next.0.capacity());
        let mut bytes = old_records
            + pending
                .as_mut_slice()
                .iter_mut()
                .map(super::native::polygon_bytes)
                .sum::<usize>()
            + super::native::material_bytes(&mut original)
            + super::native::material_bytes(&mut owned)
            + super::native::material_bytes(&mut next)
            + super::native::material_bytes(&mut inside)
            + super::native::material_bytes(&mut outside);
        bytes = byte_sum(bytes, extra_records * std::mem::size_of::<Polygon<f64>>())?;
        let units = pending
            .as_slice()
            .iter()
            .map(|p| p.exterior().0.len() + p.interiors().iter().map(|r| r.0.len()).sum::<usize>())
            .sum::<usize>()
            + scratch_points(&[&original, &owned, &next, &inside, &outside])?;
        admit(units, bytes)?;
        owned.0.reserve_exact(inside.0.len());
        next.0.reserve_exact(outside.0.len());
        owned.0.extend(inside.0);
        next.0.extend(outside.0);
        region::check_size(&owned)?;
        region::check_size(&next)?;
    }
    admit(
        scratch_points(&[&owned, &next])?,
        byte_sum(
            old_records,
            byte_sum(
                super::native::material_bytes(&mut owned),
                super::native::material_bytes(&mut next),
            )?,
        )?,
    )?;
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
    owned_bytes: usize,
) -> impl FnMut(usize) -> Result<(), ManufacturingError> {
    move |scratch| {
        if scratch > super::remaining_geometry_units(retained_points, owned_bytes)? {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "classified geometry staging reservation exceeded",
            ));
        }
        Ok(())
    }
}

/// Owners that expose actual material/container capacities do not reinterpret
/// their geometry inventory as bytes. Both independent allowances must fit.
pub(super) fn fixed_owned_budget(
    retained_units: usize,
    owned_bytes: usize,
) -> impl FnMut(usize, usize) -> Result<(), ManufacturingError> {
    move |units, bytes| {
        if retained_units
            .checked_add(units)
            .is_none_or(|n| n > super::MAX_RETAINED_GEOMETRY_UNITS)
            || owned_bytes
                .checked_add(bytes)
                .is_none_or(|n| n as u64 > MAX_MANUFACTURING_RETAINED_BYTES)
        {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "classified allocated geometry budget exceeded",
            ));
        }
        Ok(())
    }
}

fn scratch_points(regions: &[&MultiPolygon<f64>]) -> Result<usize, ManufacturingError> {
    regions.iter().try_fold(0usize, |points, domain| {
        points
            .checked_add(region::point_count(domain))
            .filter(|total| *total <= super::MAX_RETAINED_GEOMETRY_UNITS)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "classified scratch point limit exceeded",
                )
            })
    })
}

struct NormalOwner<'a> {
    owner: &'a Polygon<f64>,
    paths: std::ops::Range<usize>,
}

struct BridgePatch<'a> {
    // A full hatch cell belongs only to its classified role. Actual bead
    // corridors reserve cross-role anchor extrusion without erasing skin cells.
    owner: Option<&'a Polygon<f64>>,
    along_axis: usize,
    along: [f64; 2],
    across: [f64; 2],
}
const BRIDGE_PATCH_POINTS: usize =
    std::mem::size_of::<BridgePatch<'static>>().div_ceil(std::mem::size_of::<geo::Coord<f64>>());

pub(super) fn record_target<T>(paths: &Vec<T>, additional: usize) -> usize {
    let needed = paths.len().saturating_add(additional);
    if needed <= paths.capacity() {
        paths.capacity()
    } else {
        needed.max(paths.capacity().saturating_mul(2)).max(4)
    }
}

fn repair_bytes(
    retained: usize,
    bridge: &super::support::RasterCandidate,
    patches: &Vec<BridgePatch<'_>>,
    selected: &super::support::RasterCandidate,
    hits: &Vec<f64>,
    source_hits: &Vec<f64>,
) -> usize {
    retained
        + candidate_bytes(bridge)
        + capacity_bytes(patches)
        + candidate_bytes(selected)
        + capacity_bytes(hits)
        + capacity_bytes(source_hits)
}

fn candidate_bytes(candidate: &super::support::RasterCandidate) -> usize {
    super::bonds::path_bytes(&candidate.paths)
        + super::bonds::path_bytes(&candidate.whole_rows)
        + super::bonds::skin_bridge_bytes(&candidate.skin_bridge_domains)
        + super::bonds::skin_command_fit_bytes(&candidate.skin_command_fits)
        + super::bonds::interface_owned_bytes(&candidate.process_interfaces)
}

fn capacity_bytes<T>(values: &Vec<T>) -> usize {
    values.capacity() * std::mem::size_of::<T>()
}

fn recovery_bytes(
    classified: usize,
    normal: &super::support::RasterCandidate,
    bridge: &super::support::RasterCandidate,
    owners: &Vec<NormalOwner<'_>>,
    patches: &Vec<BridgePatch<'_>>,
) -> usize {
    classified
        + candidate_bytes(normal)
        + candidate_bytes(bridge)
        + capacity_bytes(owners)
        + capacity_bytes(patches)
}

/// A reservation bound, not an allocation receipt: hatch endpoints are XYZ,
/// emitted records have their own stride, and intersection values are scalars.
/// Include geometric Vec growth slack; actual returned capacities are measured.
fn hatch_storage_bound(points: usize) -> Result<usize, ManufacturingError> {
    if points == 0 {
        return Ok(0);
    }
    points
        .checked_mul(
            std::mem::size_of::<[f64; 3]>()
                + std::mem::size_of::<DepositionPath>()
                + 2 * std::mem::size_of::<f64>(),
        )
        .and_then(|bytes| bytes.checked_add(4 * std::mem::size_of::<DepositionPath>()))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "hatch storage bound overflow",
            )
        })
}

fn whole_row_storage_bound(points: usize, rows: usize) -> Result<usize, ManufacturingError> {
    // bridge_rows owns two indexes of (three grid bounds, coordinate, owner),
    // plus hatch and exclusion intersection scalars beside output XYZ paths.
    let indexes = rows
        .checked_mul(4 * std::mem::size_of::<(f64, f64, f64, f64, Option<DepositionOwner>)>())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "whole row index bound overflow",
            )
        })?;
    byte_sum(hatch_storage_bound(points)?, indexes)
}

fn encoded_domain_bytes(domain: &MultiPolygon<f64>) -> usize {
    domain.0.len() * std::mem::size_of::<DiaphragmPolygonMm>()
        + domain
            .0
            .iter()
            .map(|polygon| {
                polygon.interiors().len() * std::mem::size_of::<Vec<[f64; 2]>>()
                    + (polygon.exterior().0.len()
                        + polygon
                            .interiors()
                            .iter()
                            .map(|ring| ring.0.len())
                            .sum::<usize>())
                        * std::mem::size_of::<[f64; 2]>()
            })
            .sum::<usize>()
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
        patches.checked_mul(BRIDGE_PATCH_POINTS).ok_or_else(|| {
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
        .filter(|points| *points <= super::MAX_RETAINED_GEOMETRY_UNITS)
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
    other_bytes: usize,
    admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
) -> Result<(), ManufacturingError> {
    if records.len() == records.capacity() {
        let remaining = super::MAX_RETAINED_GEOMETRY_UNITS
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
        admit(
            point_sum(other, capacity * units)?,
            byte_sum(other_bytes, capacity * std::mem::size_of::<T>())?,
            0,
        )?;
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
        .filter(|total| *total <= super::MAX_RETAINED_GEOMETRY_UNITS)
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
    scratch_bytes: usize,
    admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
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
    admit(scratch, scratch_bytes, work)?;
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
            admit(
                point_sum(scratch, capacity * 2)?,
                byte_sum(scratch_bytes, capacity * std::mem::size_of::<BridgeBand>())?,
                0,
            )?;
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
    admit(
        point_sum(scratch, bands.capacity() * 2)?,
        byte_sum(scratch_bytes, capacity_bytes(&bands))?,
        0,
    )?;
    Ok(bands)
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use geo::polygon;

    #[test]
    fn classification_records_match_empty_and_positive_interior_ownership() {
        let recipe = crate::planner::tests::recipe(0.20);
        let cancel = AtomicBool::new(false);
        let fill = MultiPolygon(vec![
            geo::Rect::new(geo::Coord { x: 0., y: 0. }, geo::Coord { x: 10., y: 10. }).to_polygon(),
        ]);
        for interior_survives in [false, true] {
            let layout = Arc::new(WallPlan {
                paths: Vec::new(),
                correspondence: Vec::new(),
                fill: fill.clone(),
                section_area_mm2: 100.,
                wall_area_mm2: 0.,
                protected_hole_area_mm2: 0.,
                retained_points: 0,
                retained_bytes: 0,
            });
            let empty = MultiPolygon(Vec::new());
            let survival = if interior_survives { &fill } else { &empty };
            let mut last = (0, 0);
            let mut classified = ClassifiedRegions::new(
                layout,
                survival,
                survival,
                |units, bytes| {
                    last = (units, bytes);
                    Ok(())
                },
                &recipe,
                &cancel,
            )
            .unwrap();
            assert_eq!(classified.interiors.len(), usize::from(interior_survives));
            assert_eq!(
                classified.coverage.process_regions.len(),
                usize::from(interior_survives)
            );
            assert_eq!(
                last,
                (classified.retained_points(), classified.retained_bytes())
            );
        }
    }

    #[test]
    fn degenerate_phase_does_not_hide_later_anchored_ordinary_rows() {
        let cancel = AtomicBool::new(false);
        let mut recipe = crate::planner::tests::recipe(0.20);
        let h = recipe.layer_height_mm;
        let solid_spacing =
            ((recipe.bead_width_mm - h) * h + std::f64::consts::PI * (h * 0.5).powi(2)) / h;
        recipe.infill_fraction = solid_spacing / 4.0;
        let fill = MultiPolygon(vec![polygon![
            (x: 0., y: 0.), (x: 16., y: 8.), (x: 0., y: 8.), (x: 0., y: 0.)
        ]]);
        let layout = Arc::new(WallPlan {
            paths: Vec::new(),
            correspondence: Vec::new(),
            fill: fill.clone(),
            section_area_mm2: fill.unsigned_area(),
            wall_area_mm2: 0.,
            protected_hole_area_mm2: 0.,
            retained_points: 0,
            retained_bytes: 0,
        });
        let mut classified =
            ClassifiedRegions::new(layout, &fill, &fill, |_, _| Ok(()), &recipe, &cancel).unwrap();
        let owned_bytes = classified.retained_bytes();
        let layer = region::HatchLayer {
            z: 2.0 * h,
            lower_z: h,
            height: h,
            axis: region::HatchAxis::X,
            top_axis: region::HatchAxis::Y,
            recipe: &recipe,
        };
        // Only rows y=1 and y=5 have actual lower strokes. The earlier Half
        // proposal touches the minimum vertex and cannot construct its row.
        let previous = [1.0, 5.0].map(|y| DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            owner: Some(DepositionOwner::Regional { region_index: 0 }),
            width: WidthProfile::Constant {
                width_mm: recipe.bead_width_mm,
            },
            physical_z_mm: [0., h],
            points_mm: vec![[0., y, h], [2.0 * y, y, h]],
        });
        let mut workspace = super::super::support::AnchorWorkspace::default();
        let mut work = 0;
        let phase = classified
            .choose_initial_ordinary_phase(
                &layer,
                previous.iter(),
                &mut workspace,
                &mut work,
                owned_bytes,
                0,
                0,
                |_, _, _| Ok(()),
                &cancel,
            )
            .unwrap();
        let mut paths = Vec::new();
        let mut segments = 0;
        region::hatch_with_phase(
            &fill,
            DepositionKind::SparseFill,
            recipe.infill_fraction,
            &layer,
            None,
            phase,
            &mut paths,
            &mut segments,
            &cancel,
        )
        .unwrap();
        let mut rows: Vec<_> = paths
            .iter()
            .map(|path| {
                let [a, b] = path.points_mm.as_slice() else {
                    panic!("ordinary row must be a segment")
                };
                (a[1], a[0].min(b[0]), a[0].max(b[0]))
            })
            .collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(rows, [(1.0, 0.0, 2.0), (5.0, 0.0, 10.0)]);

        let failure = classified
            .choose_initial_ordinary_phase(
                &layer,
                previous.iter(),
                &mut workspace,
                &mut work,
                owned_bytes,
                0,
                0,
                |_, _, _| {
                    Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "test capacity",
                    ))
                },
                &cancel,
            )
            .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let failure = classified
            .choose_initial_ordinary_phase(
                &layer,
                previous.iter(),
                &mut workspace,
                &mut work,
                owned_bytes,
                0,
                0,
                |_, _, _| Ok(()),
                &cancel,
            )
            .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::Cancelled);
    }

    #[test]
    fn fixed_owned_admission_separates_units_from_bytes() {
        let mut admit = fixed_owned_budget(0, 0);
        admit(MAX_DEPOSITION_SEGMENTS as usize + 1, 0).unwrap();
        assert!(admit(0, MAX_MANUFACTURING_RETAINED_BYTES as usize + 1).is_err());
        assert!(admit(super::super::MAX_RETAINED_GEOMETRY_UNITS + 1, 0).is_err());
    }
}

#[cfg(test)]
mod skin_bridge_tests {
    use super::*;
    use crate::planner::support::{self, AnchorWorkspace};
    use geo::polygon;

    #[test]
    fn demanded_sparse_terminal_joins_only_adjacent_required_skin_at_nearest_actual_anchor() {
        let cancel = AtomicBool::new(false);
        let mut recipe = crate::planner::tests::recipe(0.20);
        recipe.bead_width_mm = 0.45;
        recipe.process.max_bridge_span_mm = 10.;
        let transition = [106.57809278913417, 108.65631235184321, 3.3422000008334];
        let floor = 3.243900000808888;
        let top = MultiPolygon(vec![polygon![
            (x:106.,y:108.65631235184321),(x:114.,y:108.65631235184321),
            (x:114.,y:114.),(x:106.,y:114.),(x:106.,y:108.65631235184321)]]);
        let regional = Polygon::new(
            polygon![
            (x:98.9,y:101.9),(x:121.1,y:101.9),(x:121.1,y:118.1),
            (x:98.9,y:118.1),(x:98.9,y:101.9)]
            .exterior()
            .clone(),
            vec![top.0[0].exterior().clone()],
        );
        let interior = InteriorRegion {
            domain: MultiPolygon(vec![regional.clone()]),
            coverage: ProcessRegionCoverage {
                material_domain: None,
                area_mm2: regional.unsigned_area(),
                fraction: 0.20,
                contributions: Vec::new(),
            },
            frame_origin_mm: None,
        };
        let classified = ClassifiedRegions {
            layout: Arc::new(WallPlan {
                paths: Vec::new(),
                correspondence: Vec::new(),
                fill: MultiPolygon(vec![Polygon::new(regional.exterior().clone(), Vec::new())]),
                section_area_mm2: 1.,
                wall_area_mm2: 0.,
                protected_hole_area_mm2: 0.,
                retained_points: 0,
                retained_bytes: 0,
            }),
            bottom: MultiPolygon(Vec::new()),
            top,
            interiors: Vec::new(),
            separator_bands: Vec::new(),
            coverage: crate::planner::empty_coverage(0.20),
            applied_operations: Vec::new(),
        };
        let mut previous = [
            103.14452319728332,
            105.28904639456687,
            107.43356959185041,
            109.57809278913395,
            111.7226159864175,
        ]
        .map(|y| DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [floor - 0.0983, floor],
            points_mm: vec![[98.9, y, floor], [121.1, y, floor]],
        })
        .to_vec();
        previous.push(DepositionPath {
            material_domain: None,
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [floor - 0.0983, floor],
            points_mm: vec![
                [100., 101.6750000119207, floor],
                [120., 101.6750000119207, floor],
            ],
        });
        let required = vec![DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            owner: Some(DepositionOwner::Regional { region_index: 0 }),
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [floor, transition[2]],
            points_mm: vec![
                transition,
                [transition[0], 101.90000000596024, transition[2]],
            ],
        }];
        let mut workspace = AnchorWorkspace::default();
        let mut work = 0;
        let mut check =
            |candidate: &mut support::RasterCandidate, axis, _scratch, _scratch_bytes| {
                support::component_admission(
                    (candidate, axis, 100_000, true),
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut workspace,
                    &mut work,
                    &cancel,
                )
            };
        let mut probe = support::RasterCandidate {
            paths: required.clone(),
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
            skin_bridge_domains: Vec::new(),
            skin_command_fits: Vec::new(),
        };
        let original = check(&mut probe, region::HatchAxis::Y, 0, 0).unwrap();
        assert!(!original.supported);
        let layer = region::HatchLayer {
            z: transition[2],
            lower_z: floor,
            height: transition[2] - floor,
            axis: region::HatchAxis::Y,
            top_axis: region::HatchAxis::Y,
            recipe: &recipe,
        };
        let mut bridges = support::RasterCandidate {
            paths: Vec::new(),
            whole_rows: Vec::new(),
            process_interfaces: Vec::new(),
            skin_bridge_domains: Vec::new(),
            skin_command_fits: Vec::new(),
        };
        let mut patches = Vec::new();
        assert!(
            classified
                .repair_skin_terminals(
                    &regional,
                    0,
                    &interior,
                    &classified.layout.fill,
                    &required,
                    &original.demands,
                    &layer,
                    0,
                    0,
                    &mut 0,
                    &mut bridges,
                    &mut patches,
                    &mut |_, _, _| Ok(()),
                    &mut check,
                    &cancel
                )
                .unwrap()
        );
        assert_eq!(bridges.paths.len(), 1);
        let bridge = &bridges.paths[0];
        assert_eq!(bridge.points_mm[1..], required[0].points_mm);
        assert_eq!(
            bridge.owner,
            Some(DepositionOwner::RegionalSkinBridge {
                region_index: 0,
                skin: ProcessSkinKind::TopSkin,
                skin_first: true
            })
        );
        assert!((bridge.points_mm[0][1] - (109.57809278913395 - 0.225)).abs() < 1e-12);
        assert!(
            check(&mut bridges, region::HatchAxis::Y, 0, 0)
                .unwrap()
                .supported
        );
        // No skin-wide bead corridor: only exact coincident commanded rows may
        // lose their duplicate subsegment; offset skin lattices remain intact.
        assert!(
            patches
                .iter()
                .all(|patch| patch.across[0] == patch.across[1])
        );
        previous.retain(|path| path.points_mm[0][1] != 109.57809278913395);
        let refusal = support::strokes::analyze(
            &bridges.paths,
            previous.iter(),
            std::iter::empty(),
            &recipe,
            &mut workspace,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            refusal.failure.is_some(),
            "the same constructed Bridge must refuse without its actual lower endpoint anchor"
        );
    }
}
