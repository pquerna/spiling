// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent inventory of perforated panels and pier flanges. This module
//! consumes classified source rings and policy, never planner roads/helpers.
use super::*;
type IntervalRoad = (usize, f64, f64, f64);

pub(super) fn admit(
    a: Option<WebArchitecture>,
    roots: &[Vec<f64>; 2],
    min: [f64; 2],
    max: [f64; 2],
    roof: f64,
    bottom: f64,
    recipe: &PlanarPrintRecipe,
) -> Result<(), ManufacturingError> {
    let Some(o) = a.and_then(|a| a.opening) else {
        return Ok(());
    };
    let mut largest: f64 = 0.;
    for axis in 0..2 {
        let mut lo = min[axis];
        for hi in roots[axis]
            .iter()
            .copied()
            .chain(std::iter::once(max[axis]))
        {
            largest = largest.max(hi - lo);
            lo = hi;
        }
    }
    if o.bottom_z_mm < bottom + recipe.bottom_skin_thickness_mm + recipe.layer_height_mm
        || o.bottom_z_mm + o.height_mm
            > roof - 2. * largest - recipe.bead_width_mm - recipe.layer_height_mm
    {
        return Err(failed(
            "web opening severs a required landing or roof-root corridor",
        ));
    }
    Ok(())
}

pub(super) fn panel_intervals(
    chord: [f64; 2],
    roots: &[f64],
    opening: Option<WebOpening>,
    z: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<[f64; 2]>, ManufacturingError> {
    let Some(o) = opening else {
        return Ok(vec![chord]);
    };
    if z <= o.bottom_z_mm || z >= o.bottom_z_mm + o.height_mm {
        return Ok(vec![chord]);
    }
    let remaining = o.height_mm - (z - o.bottom_z_mm);
    let radius = (remaining / o.cap_height_mm).min(1.) * (o.width_mm * 0.5);
    let mut intervals = Vec::new();
    let mut start = chord[0];
    for bay in roots.windows(2) {
        charge(work, cancel)?;
        if bay[0] < chord[0]
            || bay[1] > chord[1]
            || bay[1] - bay[0] < o.width_mm + 2. * o.pier_width_mm
        {
            continue;
        }
        let center = bay[0] + (bay[1] - bay[0]) * 0.5;
        if intervals.len() >= MAX_DEPOSITION_SEGMENTS as usize {
            return Err(failed("web opening reconstructed interval budget exceeded"));
        }
        intervals.push([start, center - radius]);
        start = center + radius;
    }
    intervals.push([start, chord[1]]);
    Ok(intervals)
}

pub(super) fn pier_roads(
    polygon: &DiaphragmPolygonMm,
    roots: &[Vec<f64>; 2],
    depth: f64,
    width: f64,
    expected: &mut Vec<IntervalRoad>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut spans = Vec::new();
    for fixed in 0..2 {
        for &c in &roots[fixed] {
            for coordinate in [c - width, c + width] {
                crossing_intervals(polygon, fixed, coordinate, &mut spans, work, cancel)?;
                for pair in spans.as_chunks::<2>().0 {
                    for &along in &roots[1 - fixed] {
                        charge(work, cancel)?;
                        let lo = pair[0].max(along - depth);
                        let hi = pair[1].min(along + depth);
                        if hi - lo < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                            continue;
                        }
                        if expected.len() >= MAX_DEPOSITION_SEGMENTS as usize {
                            return Err(failed("web pier reconstructed road budget exceeded"));
                        }
                        expected.push((fixed, coordinate, lo, hi));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn consume(
    expected: &mut [IntervalRoad],
    roads: &mut BTreeSet<(u16, Road)>,
    owner: u16,
    layer: &PrintLayer,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    charge_many(
        work,
        expected
            .len()
            .saturating_mul(1 + expected.len().max(1).ilog2() as usize),
        cancel,
    )?;
    expected.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.total_cmp(&b.2))
    });
    let require = |r: IntervalRoad, roads: &mut BTreeSet<_>| {
        let mut a = [0., 0., layer.z_mm];
        let mut b = a;
        a[r.0] = r.1;
        b[r.0] = r.1;
        a[1 - r.0] = r.2;
        b[1 - r.0] = r.3;
        if !roads.remove(&(owner, key(a, b)?)) {
            return Err(failed(format!(
                "web + roof missing a required web/roof road at layer {}: {:?} to {:?}",
                layer.index, a, b
            )));
        }
        Ok(())
    };
    let mut i = 0;
    while i < expected.len() {
        charge(work, cancel)?;
        let mut r = expected[i];
        i += 1;
        while i < expected.len()
            && expected[i].0 == r.0
            && expected[i].1 == r.1
            && expected[i].2 <= r.3
        {
            charge(work, cancel)?;
            r.3 = r.3.max(expected[i].3);
            i += 1;
        }
        require(r, roads)?;
    }
    Ok(())
}
