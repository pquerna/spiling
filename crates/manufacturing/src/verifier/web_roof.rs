// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Complete construction check after actual-byte/path bijection. Reconstruct
//! the independent classified component/hole domains, bind their area to
//! classified coverage, and consume every required projected road exactly once.
//! No planner hatch, roof generator or backend formatter is used here.
use super::{cancelled, failed, project_xyz6_point_mm};
use spiling_contracts::manufacturing::*;
use std::{collections::BTreeSet, sync::atomic::AtomicBool};
mod architecture;
mod joints;

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
    charge_many(work, 1, cancel)
}
fn charge_many(
    work: &mut usize,
    amount: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    cancelled(cancel)?;
    *work = work
        .checked_add(amount)
        .filter(|n| *n <= 128_000_000)
        .ok_or_else(|| failed("web + roof independent reconstruction work budget exceeded"))?;
    Ok(())
}

fn crossing_intervals(
    polygon: &DiaphragmPolygonMm,
    fixed: usize,
    coordinate: f64,
    hits: &mut Vec<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    hits.clear();
    for ring in std::iter::once(&polygon.outer).chain(&polygon.holes) {
        for edge in ring.windows(2) {
            charge(work, cancel)?;
            let [a, b] = [edge[0], edge[1]];
            if (a[fixed] <= coordinate && coordinate < b[fixed])
                || (b[fixed] <= coordinate && coordinate < a[fixed])
            {
                hits.push(
                    a[1 - fixed]
                        + (coordinate - a[fixed]) * (b[1 - fixed] - a[1 - fixed])
                            / (b[fixed] - a[fixed]),
                );
            }
        }
    }
    hits.sort_unstable_by(f64::total_cmp);
    if !hits.len().is_multiple_of(2) {
        return Err(failed("web + roof classified boundary parity is invalid"));
    }
    Ok(())
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let first_layer = plan
        .layers
        .first()
        .ok_or_else(|| failed("web + roof has no physical schedule"))?;
    let roof_z =
        (plan.layers.last().unwrap().z_mm - recipe.top_skin_thickness_mm - recipe.layer_height_mm)
            .max(first_layer.lower_z_mm);
    let pitch = 2.0
        * (recipe.bead_width_mm - recipe.layer_height_mm
            + std::f64::consts::PI * recipe.layer_height_mm / 4.0)
        / recipe.infill_fraction;
    if !pitch.is_finite() || pitch <= 0.0 {
        return Err(failed("web + roof reconstructed pitch is unconditioned"));
    }
    let mut hits = Vec::new();
    for layer in &plan.layers {
        cancelled(cancel)?;
        let coverage = &layer.region_coverage;
        let mut roads = BTreeSet::new();
        for path in layer.paths.iter().filter(|p| {
            p.kind == DepositionKind::SparseFill
                || p.kind == DepositionKind::Bridge
                    && matches!(p.owner, Some(DepositionOwner::Regional { .. }))
        }) {
            charge(work, cancel)?;
            let Some(DepositionOwner::Regional { region_index }) = path.owner else {
                return Err(failed("web + roof road lacks classified owner"));
            };
            if path.points_mm.len() != 2
                || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
                || path.width
                    != (WidthProfile::Constant {
                        width_mm: recipe.bead_width_mm,
                    })
            {
                return Err(failed("web + roof changed width or physical interval"));
            }
            if !roads.insert((region_index, key(path.points_mm[0], path.points_mm[1])?)) {
                return Err(failed("web + roof duplicates an actual road"));
            }
        }
        for (region_index, region) in coverage.process_regions.iter().enumerate() {
            if region.fraction <= 0.0 || region.fraction >= 1.0 || region.area_mm2 <= 0.0 {
                continue;
            }
            let domain = coverage
                .web_roof_domains
                .iter()
                .find(|d| usize::from(d.region_index) == region_index)
                .ok_or_else(|| failed("web + roof lacks its independent classified domain"))?;
            let mut area = 0.0;
            for polygon in &domain.extent {
                let mut min = [f64::INFINITY; 2];
                let mut max = [f64::NEG_INFINITY; 2];
                for (ring_index, ring) in std::iter::once(&polygon.outer)
                    .chain(&polygon.holes)
                    .enumerate()
                {
                    let mut twice = 0.0;
                    for edge in ring.windows(2) {
                        charge(work, cancel)?;
                        // Translate before cross products: distant placements
                        // must not lose the small classified area to cancellation.
                        let origin = ring[0];
                        twice += (edge[0][0] - origin[0]) * (edge[1][1] - origin[1])
                            - (edge[1][0] - origin[0]) * (edge[0][1] - origin[1]);
                        if ring_index == 0 {
                            for axis in 0..2 {
                                min[axis] = min[axis].min(edge[0][axis]);
                                max[axis] = max[axis].max(edge[0][axis]);
                            }
                        }
                    }
                    area += if ring_index == 0 {
                        twice.abs() * 0.5
                    } else {
                        -twice.abs() * 0.5
                    };
                }
                let mut count = [0usize; 2];
                let mut first = [0.0; 2];
                for axis in 0..2 {
                    let n = ((max[axis] - min[axis]) / pitch).ceil().max(1.0);
                    if !n.is_finite() || n > MAX_DEPOSITION_SEGMENTS as f64 {
                        return Err(failed(
                            "web + roof reconstructed lattice exceeds the command budget",
                        ));
                    }
                    count[axis] = n as usize;
                    first[axis] = min[axis] + ((max[axis] - min[axis]) - (n - 1.0) * pitch) * 0.5;
                }
                if count[0].saturating_add(count[1]) > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(failed(
                        "web + roof reconstructed base lattice budget exceeded",
                    ));
                }
                let mut web_coordinates: [Vec<f64>; 2] = std::array::from_fn(|axis| {
                    (0..count[axis])
                        .map(|i| first[axis] + i as f64 * pitch)
                        .collect()
                });
                for fixed in 0..2 {
                    for gap in 0..=count[fixed] {
                        let lo = if gap == 0 {
                            min[fixed]
                        } else {
                            first[fixed] + (gap - 1) as f64 * pitch
                        };
                        let hi = if gap == count[fixed] {
                            max[fixed]
                        } else {
                            first[fixed] + gap as f64 * pitch
                        };
                        let n = ((hi - lo) / recipe.web_roof_spacing_mm()).ceil().max(1.);
                        if !n.is_finite() || n > MAX_DEPOSITION_SEGMENTS as f64 {
                            return Err(failed(
                                "web + roof reconstructed seed contact budget exceeded",
                            ));
                        }
                        for i in 1..n as usize {
                            crossing_intervals(
                                polygon,
                                fixed,
                                lo + (hi - lo) * i as f64 / n,
                                &mut hits,
                                work,
                                cancel,
                            )?;
                            for chord in hits.as_chunks::<2>().0 {
                                charge_many(work, web_coordinates[1 - fixed].len(), cancel)?;
                                if chord[1] - chord[0] > recipe.web_roof_spacing_mm()
                                    && !web_coordinates[1 - fixed]
                                        .iter()
                                        .any(|v| chord[0] <= *v && *v <= chord[1])
                                {
                                    let seed = (chord[0] + chord[1]) * 0.5;
                                    if web_coordinates[1 - fixed]
                                        .iter()
                                        .any(|v| (seed - v).abs() < recipe.bead_width_mm)
                                    {
                                        return Err(failed(
                                            "web + roof reconstructed local root crowds an existing web",
                                        ));
                                    }
                                    if web_coordinates[0].len() + web_coordinates[1].len()
                                        >= MAX_DEPOSITION_SEGMENTS as usize
                                    {
                                        return Err(failed(
                                            "web + roof reconstructed seed budget exceeded",
                                        ));
                                    }
                                    web_coordinates[1 - fixed].push(seed);
                                    web_coordinates[1 - fixed].sort_unstable_by(f64::total_cmp);
                                }
                            }
                        }
                    }
                }
                let a = match recipe.sparse_infill {
                    SparseInfillPolicy::WebRoof { architecture, .. } => architecture,
                    _ => None,
                };
                architecture::admit(
                    a,
                    &web_coordinates,
                    min,
                    max,
                    roof_z,
                    first_layer.lower_z_mm,
                    recipe,
                )?;
                let mut expected = Vec::new();
                let mut require =
                    |fixed: usize, coordinate: f64, start: f64, stop: f64, work: &mut usize| {
                        charge(work, cancel)?;
                        if expected.len() >= MAX_DEPOSITION_SEGMENTS as usize {
                            return Err(failed("web architecture expected-road budget exceeded"));
                        }
                        expected.push((fixed, coordinate, start, stop));
                        Ok(())
                    };
                for fixed in 0..2 {
                    for &c in &web_coordinates[fixed] {
                        crossing_intervals(polygon, fixed, c, &mut hits, work, cancel)?;
                        for pair in hits.as_chunks::<2>().0 {
                            if let Some(opening) = a.and_then(|v| v.opening) {
                                let intervals = architecture::panel_intervals(
                                    [pair[0], pair[1]],
                                    &web_coordinates[1 - fixed],
                                    Some(opening),
                                    layer.z_mm,
                                    work,
                                    cancel,
                                )?;
                                for [lo, hi] in intervals {
                                    require(fixed, c, lo, hi, work)?;
                                }
                            } else {
                                require(fixed, c, pair[0], pair[1], work)?;
                            }
                        }
                    }
                    for gap in 0..=count[fixed] {
                        charge(work, cancel)?;
                        let lo = if gap == 0 {
                            min[fixed]
                        } else {
                            first[fixed] + (gap - 1) as f64 * pitch
                        };
                        let hi = if gap == count[fixed] {
                            max[fixed]
                        } else {
                            first[fixed] + gap as f64 * pitch
                        };
                        let divisions = ((hi - lo) / recipe.web_roof_spacing_mm()).ceil().max(1.0);
                        if !divisions.is_finite() || divisions > MAX_DEPOSITION_SEGMENTS as f64 {
                            return Err(failed("web + roof reconstructed contact budget exceeded"));
                        }
                        for contact in 1..divisions as usize {
                            let c = lo + (hi - lo) * contact as f64 / divisions;
                            if web_coordinates[fixed].contains(&c) {
                                continue;
                            }
                            crossing_intervals(polygon, fixed, c, &mut hits, work, cancel)?;
                            for pair in hits.as_chunks::<2>().0 {
                                let mut pending: Option<[f64; 2]> = None;
                                let mut previous_root = None;
                                // Scan roots in order. Each root's catchment
                                // ends halfway to its real neighbour or at the
                                // current component/hole chord boundary.
                                let along = &web_coordinates[1 - fixed];
                                let begin = along.partition_point(|v| *v < pair[0]);
                                let end = along.partition_point(|v| *v <= pair[1]);
                                for seed in begin..end {
                                    charge(work, cancel)?;
                                    let root = along[seed];
                                    if root < pair[0] || root > pair[1] {
                                        continue;
                                    }
                                    let next = if seed + 1 < end {
                                        Some(along[seed + 1])
                                    } else {
                                        None
                                    };
                                    let left =
                                        previous_root.map_or(pair[0], |p: f64| (p + root) * 0.5);
                                    let right = next.map_or(pair[1], |p| (p + root) * 0.5);
                                    previous_root = Some(root);
                                    let retreat = 0.5 * (roof_z - layer.z_mm).max(0.0);
                                    let interval = [
                                        root - (root - left - retreat).max(0.0),
                                        root + (right - root - retreat).max(0.0),
                                    ];
                                    if interval[1] - interval[0]
                                        < program_coordinates::MIN_DEPOSITION_COMMAND_MM
                                    {
                                        continue;
                                    }
                                    if let Some(p) = &mut pending {
                                        if interval[0] <= p[1] {
                                            p[1] = p[1].max(interval[1]);
                                            continue;
                                        }
                                        require(fixed, c, p[0], p[1], work)?;
                                    }
                                    pending = Some(interval);
                                }
                                if let Some(p) = pending {
                                    require(fixed, c, p[0], p[1], work)?;
                                }
                            }
                        }
                    }
                }
                if let Some(depth) = a.and_then(|v| v.pier_rib_depth_mm) {
                    architecture::pier_roads(
                        polygon,
                        &web_coordinates,
                        depth,
                        recipe.bead_width_mm,
                        &mut expected,
                        work,
                        cancel,
                    )?;
                } else if a.is_none() {
                    joints::reconstruct(
                        polygon,
                        &web_coordinates,
                        min,
                        max,
                        recipe.bead_width_mm,
                        &mut expected,
                        work,
                        cancel,
                    )?;
                }
                architecture::consume(
                    &mut expected,
                    &mut roads,
                    region_index as u16,
                    layer,
                    work,
                    cancel,
                )?;
            }
            if !area.is_finite() || (area - region.area_mm2).abs() > 1e-9 * region.area_mm2.max(1.0)
            {
                return Err(failed(
                    "web + roof classified domain disagrees with regional area",
                ));
            }
        }
        if !roads.is_empty() {
            return Err(failed(
                "web + roof has roads outside its classified construction",
            ));
        }
    }
    Ok(())
}
