// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Compare bounded row traversal with the existing canonical order. This runs
//! after role sorting, before nearest-entry direction is stored in the plan.
use super::*;

fn charge(work: &mut usize, amount: usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work
        .checked_add(amount)
        .filter(|v| *v <= 128_000_000)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "row routing work budget exceeded",
            )
        })?;
    Ok(())
}

fn entry_cost(from: [f64; 3], to: [f64; 3], recipe: &PlanarPrintRecipe) -> f64 {
    let length = distance(from, to);
    if let RetractionPolicy::Enabled {
        minimum_travel_mm,
        distance_mm,
        speed_mm_s,
        z_hop_mm,
    } = recipe.process.retraction
        && length >= minimum_travel_mm
    {
        let travel = if z_hop_mm > 0.0 {
            let z = from[2].max(to[2]) + z_hop_mm;
            z - from[2] + (from[0] - to[0]).hypot(from[1] - to[1]) + z - to[2]
        } else {
            length
        };
        travel / recipe.travel_speed_mm_s + 2.0 * distance_mm / speed_mm_s
    } else {
        length / recipe.travel_speed_mm_s
    }
}

fn cost(
    paths: &[DepositionPath],
    mut from: Option<[f64; 3]>,
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<(f64, Option<[f64; 3]>), ManufacturingError> {
    let mut seconds = 0.0;
    for path in paths {
        check_cancel(cancel)?;
        let [mut a, mut b] = [path.points_mm[0], path.points_mm[1]];
        if let Some(p) = from {
            // Match the later nearest-entry rule exactly; no path is reversed
            // during this comparison or the row sort.
            if distance(p, b) < distance(p, a) {
                std::mem::swap(&mut a, &mut b);
            }
            seconds += entry_cost(project_xyz6_point_mm(p)?, project_xyz6_point_mm(a)?, recipe);
        }
        from = Some(b);
    }
    Ok((seconds, from))
}

pub(super) fn improve(
    paths: &mut [DepositionPath],
    from: Option<[f64; 3]>,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    if paths.len() < 2 {
        return Ok(false);
    }
    // Reserve both the candidate sort and possible restoration, plus linear
    // cost/row scans, before changing anything. No copied motion/index vector.
    let operations = paths
        .len()
        .checked_mul(2 * (paths.len().ilog2() as usize + 1) + 8)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "row routing work overflow",
            )
        })?;
    charge(work, operations, cancel)?;
    if paths.iter().any(|p| {
        p.kind != DepositionKind::SparseFill
            || p.points_mm.len() != 2
            || !matches!(p.width, WidthProfile::Constant { .. })
            || p.points_mm[0][2] != p.points_mm[1][2]
            || (p.points_mm[0][0] != p.points_mm[1][0] && p.points_mm[0][1] != p.points_mm[1][1])
    }) {
        return Ok(false);
    }
    let before = cost(paths, from, recipe, cancel)?;
    let fixed = |p: &DepositionPath| usize::from(p.points_mm[0][0] != p.points_mm[1][0]);
    paths.sort_unstable_by(|a, b| {
        let fa = fixed(a);
        let fb = fixed(b);
        let low = |p: &DepositionPath, f: usize| p.points_mm[0][1 - f].min(p.points_mm[1][1 - f]);
        fa.cmp(&fb)
            .then(a.points_mm[0][fa].total_cmp(&b.points_mm[0][fb]))
            .then(low(a, fa).total_cmp(&low(b, fb)))
            .then_with(|| canonical_order(a, b))
    });
    let mut first = 0;
    let mut previous_axis = None;
    let mut reverse = false;
    while first < paths.len() {
        check_cancel(cancel)?;
        let axis = fixed(&paths[first]);
        if previous_axis != Some(axis) {
            reverse = false;
            previous_axis = Some(axis);
        }
        let coordinate = paths[first].points_mm[0][axis];
        let mut end = first + 1;
        while end < paths.len()
            && fixed(&paths[end]) == axis
            && paths[end].points_mm[0][axis] == coordinate
        {
            end += 1;
        }
        if reverse {
            paths[first..end].reverse();
        }
        reverse = !reverse;
        first = end;
    }
    let after = cost(paths, from, recipe, cancel)?;
    // Charge a conservative extra exit to the old endpoint. Include the
    // largest one-transition retraction/hop overhead when that endpoint moves.
    let exit_allowance = match (before.1, after.1) {
        (Some(a), Some(b)) if a != b => {
            let extra = match recipe.process.retraction {
                RetractionPolicy::Enabled {
                    distance_mm,
                    speed_mm_s,
                    z_hop_mm,
                    ..
                } => 2. * distance_mm / speed_mm_s + 2. * z_hop_mm / recipe.travel_speed_mm_s,
                RetractionPolicy::Disabled => 0.,
            };
            distance(project_xyz6_point_mm(a)?, project_xyz6_point_mm(b)?)
                / recipe.travel_speed_mm_s
                + extra
        }
        _ => 0.,
    };
    if after.0 + exit_allowance < before.0 {
        Ok(true)
    } else {
        paths.sort_unstable_by(canonical_order);
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recipe() -> PlanarPrintRecipe {
        serde_json::from_str::<ManufacturingIntent>(include_str!(
            "../../../../fixtures/manufacturing/web-roof.intent.json"
        ))
        .unwrap()
        .recipe
    }
    fn paths() -> Vec<DepositionPath> {
        [(0., 0., 4.), (0., 6., 10.), (1., 0., 4.), (1., 6., 10.)]
            .into_iter()
            .map(|(y, a, b)| DepositionPath {
                material_domain: None,
                kind: DepositionKind::SparseFill,
                owner: Some(DepositionOwner::Regional { region_index: 0 }),
                width: WidthProfile::Constant { width_mm: 0.45 },
                physical_z_mm: [0.8, 1.],
                points_mm: vec![[a, y, 1.], [b, y, 1.]],
            })
            .collect()
    }
    fn inventory(rows: &[DepositionPath]) -> Vec<String> {
        let mut keys = rows
            .iter()
            .map(|p| serde_json::to_string(p).unwrap())
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }
    #[test]
    fn row_routing_preserves_hole_inventory_and_only_accepts_lower_cost() {
        let mut recipe = recipe();
        recipe.process.retraction = RetractionPolicy::Disabled;
        let mut rows = paths();
        for row in &mut rows {
            for point in &mut row.points_mm {
                point.swap(0, 1);
            }
        }
        rows.push(DepositionPath {
            points_mm: vec![[0., 5., 1.], [10., 5., 1.]],
            ..rows[0].clone()
        });
        rows.sort_unstable_by(canonical_order);
        let before = inventory(&rows);
        let old = cost(&rows, None, &recipe, &AtomicBool::new(false))
            .unwrap()
            .0;
        let changed = improve(&mut rows, None, &recipe, &mut 0, &AtomicBool::new(false)).unwrap();
        assert_eq!(before, inventory(&rows));
        let new = cost(&rows, None, &recipe, &AtomicBool::new(false))
            .unwrap()
            .0;
        assert!(new <= old);
        assert!(changed);
        assert_eq!(rows.len(), 5); // Hole gaps remain travel, with no connectors.
    }
    #[test]
    fn row_routing_refuses_budget_or_cancellation_before_reordering() {
        let recipe = recipe();
        for (mut work, cancel) in [(128_000_000, false), (0, true)] {
            let mut rows = paths();
            let original = inventory(&rows);
            assert!(
                improve(
                    &mut rows,
                    None,
                    &recipe,
                    &mut work,
                    &AtomicBool::new(cancel)
                )
                .is_err()
            );
            assert_eq!(original, inventory(&rows));
        }
    }
    #[test]
    fn row_routing_keeps_existing_order_for_an_equal_candidate() {
        let recipe = recipe();
        let mut rows = paths();
        rows.truncate(2);
        rows.sort_unstable_by(canonical_order);
        let original = serde_json::to_vec(&rows).unwrap();
        assert!(!improve(&mut rows, None, &recipe, &mut 0, &AtomicBool::new(false)).unwrap());
        assert_eq!(original, serde_json::to_vec(&rows).unwrap());
    }
}
