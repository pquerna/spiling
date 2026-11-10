// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub(super) struct Columns {
    pub roots: [Vec<f64>; 2],
    pub joints: Vec<(usize, f64, f64, f64)>,
}

fn lattice(frame: [[f64; 2]; 2], pitch: f64) -> Result<[Vec<f64>; 2], ManufacturingError> {
    let mut result: [Vec<f64>; 2] = std::array::from_fn(|_| Vec::new());
    for axis in 0..2 {
        let count = ((frame[1][axis] - frame[0][axis]) / pitch).ceil().max(1.0);
        if !count.is_finite() || count > MAX_DEPOSITION_SEGMENTS as f64 {
            return Err(failed(
                "Cathedral column lattice exceeds the command budget",
            ));
        }
        let first =
            frame[0][axis] + ((frame[1][axis] - frame[0][axis]) - (count - 1.0) * pitch) * 0.5;
        result[axis].extend((0..count as usize).map(|index| first + index as f64 * pitch));
    }
    if result[0].len() + result[1].len() > MAX_DEPOSITION_SEGMENTS as usize {
        return Err(failed("Cathedral column base inventory exceeded"));
    }
    Ok(result)
}

pub(super) fn seeds(
    polygon: &DiaphragmPolygonMm,
    frame: [[f64; 2]; 2],
    pitch: f64,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<[Vec<f64>; 2], ManufacturingError> {
    let mut roots = lattice(frame, pitch)?;
    let base = roots.clone();
    let mut hits = Vec::new();
    for fixed in 0..2 {
        for gap in 0..=base[fixed].len() {
            let lo = gap
                .checked_sub(1)
                .map_or(frame[0][fixed], |index| base[fixed][index]);
            let hi = base[fixed].get(gap).copied().unwrap_or(frame[1][fixed]);
            let count = ((hi - lo) / recipe.web_roof_spacing_mm()).ceil().max(1.0);
            if !count.is_finite() || count > MAX_DEPOSITION_SEGMENTS as f64 {
                return Err(failed("Cathedral reconstructed seed contacts exceeded"));
            }
            for index in 1..count as usize {
                crossing_intervals(
                    polygon,
                    fixed,
                    lo + (hi - lo) * index as f64 / count,
                    &mut hits,
                    work,
                    cancel,
                )?;
                for chord in hits.as_chunks::<2>().0 {
                    charge_many(work, roots[1 - fixed].len(), cancel)?;
                    if chord[1] - chord[0] > recipe.web_roof_spacing_mm()
                        && !roots[1 - fixed]
                            .iter()
                            .any(|root| chord[0] <= *root && *root <= chord[1])
                    {
                        let seed = (chord[0] + chord[1]) * 0.5;
                        if roots[1 - fixed]
                            .iter()
                            .any(|root| (seed - root).abs() < recipe.bead_width_mm)
                        {
                            continue;
                        }
                        if roots[0].len() + roots[1].len() >= MAX_DEPOSITION_SEGMENTS as usize {
                            return Err(failed(
                                "Cathedral reconstructed local seed inventory exceeded",
                            ));
                        }
                        roots[1 - fixed].push(seed);
                        roots[1 - fixed].sort_unstable_by(f64::total_cmp);
                    }
                }
            }
        }
    }
    Ok(roots)
}

pub(super) fn reconstruct<'a>(
    domains: impl Iterator<Item = &'a WebRoofDomain> + Clone,
    recipe: &PlanarPrintRecipe,
    pitch: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Columns>, ManufacturingError> {
    if !matches!(
        recipe.sparse_infill,
        SparseInfillPolicy::WebRoof {
            architecture: None,
            ..
        }
    ) {
        return Ok(None);
    }
    let Some(frame) = domains.clone().next().map(|domain| domain.frame_mm) else {
        return Ok(None);
    };
    let mut columns = Columns {
        roots: lattice(frame, pitch)?,
        joints: Vec::new(),
    };
    let mut proposals = Vec::new();
    for joints in [false, true] {
        for domain in domains.clone() {
            if domain.frame_mm != frame {
                return Err(failed("Cathedral column frame changes between layers"));
            }
            for polygon in &domain.extent {
                if joints {
                    proposals.clear();
                    joints::reconstruct(
                        polygon,
                        &columns.roots,
                        frame[0],
                        frame[1],
                        recipe.bead_width_mm,
                        &mut proposals,
                        work,
                        cancel,
                    )?;
                    for &proposal in &proposals {
                        charge_many(
                            work,
                            columns.joints.len().max(1).ilog2().saturating_add(1) as usize,
                            cancel,
                        )?;
                        match columns
                            .joints
                            .binary_search_by(|existing| compare(*existing, proposal))
                        {
                            Ok(_) => {}
                            Err(index) => {
                                charge_many(work, columns.joints.len() - index, cancel)?;
                                if columns.joints.len() >= MAX_DEPOSITION_SEGMENTS as usize {
                                    return Err(failed(
                                        "Cathedral joint column inventory exceeded",
                                    ));
                                }
                                columns.joints.insert(index, proposal);
                            }
                        }
                    }
                } else {
                    let local = seeds(polygon, frame, pitch, recipe, work, cancel)?;
                    let mut root_count = columns.roots[0].len() + columns.roots[1].len();
                    for (roots, proposals) in columns.roots.iter_mut().zip(local) {
                        for coordinate in proposals {
                            charge_many(
                                work,
                                roots.len().max(1).ilog2().saturating_add(1) as usize,
                                cancel,
                            )?;
                            let index = roots.partition_point(|root| *root < coordinate);
                            if roots.get(index).is_some_and(|root| {
                                (root - coordinate).abs() < recipe.bead_width_mm
                            }) || index.checked_sub(1).is_some_and(|previous| {
                                (roots[previous] - coordinate).abs() < recipe.bead_width_mm
                            }) {
                                continue;
                            }
                            if root_count >= MAX_DEPOSITION_SEGMENTS as usize {
                                return Err(failed("Cathedral seed column inventory exceeded"));
                            }
                            charge_many(work, roots.len() - index, cancel)?;
                            roots.insert(index, coordinate);
                            root_count += 1;
                        }
                    }
                }
            }
        }
    }
    Ok(Some(columns))
}

fn compare(left: (usize, f64, f64, f64), right: (usize, f64, f64, f64)) -> std::cmp::Ordering {
    left.0
        .cmp(&right.0)
        .then_with(|| left.1.total_cmp(&right.1))
        .then_with(|| left.2.total_cmp(&right.2))
        .then_with(|| left.3.total_cmp(&right.3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(minimum: [f64; 2], maximum: [f64; 2]) -> WebRoofDomain {
        WebRoofDomain {
            region_index: 0,
            frame_mm: [[0.0, 0.0], [40.0, 40.0]],
            ceilings: Vec::new(),
            extent: vec![DiaphragmPolygonMm {
                outer: vec![
                    minimum,
                    [maximum[0], minimum[1]],
                    maximum,
                    [minimum[0], maximum[1]],
                    minimum,
                ],
                holes: Vec::new(),
            }],
        }
    }

    #[test]
    fn replay_reconstructs_future_seed_columns_from_geometry_not_path_labels() {
        let recipe = PlanarPrintRecipe {
            sparse_infill: SparseInfillPolicy::web_roof(),
            ..serde_json::from_str::<ManufacturingIntent>(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../fixtures/manufacturing/web-roof.intent.json"
            )))
            .unwrap()
            .recipe
        };
        let domains = [
            domain([5.5, 0.0], [14.0, 40.0]),
            domain([0.0, 0.0], [40.0, 40.0]),
        ];
        let columns = reconstruct(
            domains.iter(),
            &recipe,
            10.0,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        assert!(columns.roots[0].contains(&9.75));
        assert!(columns.roots[0].contains(&5.0));
        assert!(columns.roots[0].contains(&35.0));
    }

    #[test]
    fn replay_column_inventory_rejects_frame_drift_and_cancellation() {
        let recipe = serde_json::from_str::<ManufacturingIntent>(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/manufacturing/web-roof.intent.json"
        )))
        .unwrap()
        .recipe;
        let mut domains = [
            domain([0.0, 0.0], [40.0, 40.0]),
            domain([0.0, 0.0], [40.0, 40.0]),
        ];
        domains[1].frame_mm[1][0] += 1.0;
        assert!(
            reconstruct(
                domains.iter(),
                &recipe,
                10.0,
                &mut 0,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            reconstruct(
                domains[..1].iter(),
                &recipe,
                10.0,
                &mut 0,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
