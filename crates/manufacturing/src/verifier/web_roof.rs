// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Complete construction check after actual-byte/path bijection. Reconstruct
//! the rectangular owner from both full-span web directions, bind its area to
//! classified coverage, and consume every required projected road exactly once.
//! No planner hatch, roof generator or backend formatter is used here.
use super::{cancelled, failed, project_xyz6_point_mm};
use spiling_contracts::manufacturing::*;
use std::{collections::BTreeSet, sync::atomic::AtomicBool};

type Road = [u64; 4];
fn key(a: [f64; 3], b: [f64; 3]) -> Result<Road, ManufacturingError> {
    let mut a = project_xyz6_point_mm(a)?;
    let mut b = project_xyz6_point_mm(b)?;
    if a[0] > b[0] || a[0] == b[0] && a[1] > b[1] {
        std::mem::swap(&mut a, &mut b);
    }
    Ok([
        a[0].to_bits(),
        a[1].to_bits(),
        b[0].to_bits(),
        b[1].to_bits(),
    ])
}
fn charge(work: &mut usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    cancelled(cancel)?;
    super::interfaces::charge(work)
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let roof_z = (plan
        .layers
        .last()
        .ok_or_else(|| failed("web + roof has no physical schedule"))?
        .z_mm
        - recipe.top_skin_thickness_mm
        - recipe.layer_height_mm)
        .max(plan.layers[0].lower_z_mm);
    let nominal_pitch = 2.0
        * (recipe.bead_width_mm - recipe.layer_height_mm
            + std::f64::consts::PI * recipe.layer_height_mm / 4.0)
        / recipe.infill_fraction;
    for layer in &plan.layers {
        cancelled(cancel)?;
        if layer.paths.iter().any(|path| {
            path.kind == DepositionKind::Bridge
                && matches!(path.owner, Some(DepositionOwner::Regional { .. }))
        }) {
            return Err(failed(
                "web + roof interior requires direct earlier anchors, not regional Bridge substitution",
            ));
        }
        let coverage = &layer.region_coverage;
        let expected_sparse = coverage
            .process_regions
            .iter()
            .any(|region| region.area_mm2 > 0.0 && region.fraction > 0.0 && region.fraction < 1.0);
        let sparse_count = layer
            .paths
            .iter()
            .filter(|p| p.kind == DepositionKind::SparseFill)
            .count();
        if !expected_sparse {
            if sparse_count != 0 {
                return Err(failed("web + roof roads lack sparse ownership"));
            }
            continue;
        }
        if sparse_count == 0 || coverage.process_regions.len() != 1 {
            return Err(failed(
                "web + roof is missing its sole rectangular sparse owner",
            ));
        }
        let mut roads = BTreeSet::new();
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for path in layer
            .paths
            .iter()
            .filter(|p| p.kind == DepositionKind::SparseFill)
        {
            charge(work, cancel)?;
            if path.owner != Some(DepositionOwner::Regional { region_index: 0 })
                || path.points_mm.len() != 2
                || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
                || path.width
                    != (WidthProfile::Constant {
                        width_mm: recipe.bead_width_mm,
                    })
            {
                return Err(failed(
                    "web + roof changed its width, interval or rectangular owner",
                ));
            }
            for point in &path.points_mm {
                for axis in 0..2 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
            }
            if !roads.insert(key(path.points_mm[0], path.points_mm[1])?) {
                return Err(failed("web + roof duplicates an actual road"));
            }
        }
        let area = (max[0] - min[0]) * (max[1] - min[1]);
        // Only floating arithmetic accumulation allowance; no source geometry
        // tolerance, density slack or nominal bead expansion supplies area.
        let declared = coverage.process_regions[0].area_mm2;
        if !area.is_finite() || (area - declared).abs() > 1e-9 * declared.max(1.0) {
            return Err(failed(
                "web + roof extent disagrees with its classified rectangular area",
            ));
        }
        let mut count = [0usize; 2];
        let mut first = [0.0; 2];
        for axis in 0..2 {
            let n = ((max[axis] - min[axis]) / nominal_pitch).ceil().max(1.0);
            if !n.is_finite() || n > MAX_DEPOSITION_SEGMENTS as f64 {
                return Err(failed(
                    "web + roof reconstructed lattice exceeds the command budget",
                ));
            }
            count[axis] = n as usize;
            first[axis] = min[axis] + (max[axis] - min[axis] - (n - 1.0) * nominal_pitch) * 0.5;
        }
        let root = |axis: usize, index: usize| -> f64 {
            if index == 0 {
                min[axis]
            } else if index == count[axis] + 1 {
                max[axis]
            } else {
                first[axis] + (index - 1) as f64 * nominal_pitch
            }
        };
        let mut require = |fixed: usize,
                           coordinate: f64,
                           start: f64,
                           stop: f64,
                           message: &'static str,
                           work: &mut usize| {
            charge(work, cancel)?;
            let mut a = [0.0, 0.0, layer.z_mm];
            let mut b = a;
            a[fixed] = coordinate;
            b[fixed] = coordinate;
            a[1 - fixed] = start;
            b[1 - fixed] = stop;
            if !roads.remove(&key(a, b)?) {
                return Err(failed(format!(
                    "{message} at layer {} / Z {:.6}: {:?} to {:?}",
                    layer.index, layer.z_mm, a, b
                )));
            }
            Ok(())
        };
        let mut proposals = 0usize;
        for fixed in 0..2 {
            for row in 0..count[fixed] {
                require(
                    fixed,
                    first[fixed] + row as f64 * nominal_pitch,
                    min[1 - fixed],
                    max[1 - fixed],
                    "web + roof is missing a continuous web",
                    work,
                )?;
            }
            for gap in 0..=count[fixed] {
                charge(work, cancel)?;
                let lower = root(fixed, gap);
                let upper = root(fixed, gap + 1);
                let divisions = ((upper - lower) / (recipe.process.max_bridge_span_mm * 0.75))
                    .ceil()
                    .max(1.0);
                if !divisions.is_finite() || divisions > MAX_DEPOSITION_SEGMENTS as f64 {
                    return Err(failed("web + roof reconstructed contact budget exceeded"));
                }
                if divisions <= 1.0 {
                    continue;
                }
                for cell in 0..=count[1 - fixed] {
                    charge(work, cancel)?;
                    proposals += 1;
                    if proposals > MAX_DEPOSITION_SEGMENTS as usize {
                        return Err(failed("web + roof reconstructed proposal budget exceeded"));
                    }
                    let left = root(1 - fixed, cell);
                    let right = root(1 - fixed, cell + 1);
                    let at_lower_boundary = cell == 0;
                    let at_upper_boundary = cell == count[1 - fixed];
                    let reach = (right - left)
                        * if at_lower_boundary || at_upper_boundary {
                            1.0
                        } else {
                            0.5
                        };
                    let run = (reach - 0.5 * (roof_z - layer.z_mm).max(0.0)).max(0.0);
                    if run < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                        continue;
                    }
                    for contact in 1..divisions as usize {
                        let coordinate = lower + (upper - lower) * contact as f64 / divisions;
                        if run >= reach {
                            require(
                                fixed,
                                coordinate,
                                left,
                                right,
                                "web + roof is missing a merged roof branch",
                                work,
                            )?;
                        } else if at_lower_boundary {
                            require(
                                fixed,
                                coordinate,
                                right - run,
                                right,
                                "web + roof is missing a wallward roof branch",
                                work,
                            )?;
                        } else if at_upper_boundary {
                            require(
                                fixed,
                                coordinate,
                                left,
                                left + run,
                                "web + roof is missing a wallward roof branch",
                                work,
                            )?;
                        } else {
                            require(
                                fixed,
                                coordinate,
                                left,
                                left + run,
                                "web + roof is missing a growing roof branch",
                                work,
                            )?;
                            require(
                                fixed,
                                coordinate,
                                right - run,
                                right,
                                "web + roof is missing a growing roof branch",
                                work,
                            )?;
                        }
                    }
                }
            }
        }
        if !roads.is_empty() {
            return Err(failed(
                "web + roof has roads outside its captured construction",
            ));
        }
    }
    Ok(())
}
