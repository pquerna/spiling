// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Continuous skin roads: join adjacent straight mandatory skin rows into
//! one zigzag path, so they print as one extrusion instead of a travel, stop
//! and restart per row. Every row keeps its exact geometry; the only material
//! added is a short connector between neighbouring row ends where the region
//! boundary is nearly perpendicular to the rows, inside the band the row ends
//! already deposit. Rows that cannot be joined stay as they are.
use super::*;

/// Largest step between adjacent rows, across the rows, in bead widths.
const MAX_ROW_STEP_WIDTHS: f64 = 1.5;
/// Largest offset of the next row's end along the rows, in bead widths. Keeps
/// a connector where the boundary meets the rows nearly at a right angle.
const MAX_END_OFFSET_WIDTHS: f64 = 0.5;

fn chainable(path: &DepositionPath) -> Option<(usize, f64)> {
    if !matches!(
        path.kind,
        DepositionKind::BottomSkin | DepositionKind::TopSkin
    ) || !matches!(path.owner, Some(DepositionOwner::MandatorySkin { .. }))
        || path.points_mm.len() != 2
        || !path.arcs.is_empty()
    {
        return None;
    }
    let WidthProfile::Constant { width_mm } = path.width else {
        return None;
    };
    let [a, b] = [path.points_mm[0], path.points_mm[1]];
    // The fixed coordinate of a row along X is Y, and the reverse.
    let fixed = if a[1] == b[1] && a[0] != b[0] {
        1
    } else if a[0] == b[0] && a[1] != b[1] {
        0
    } else {
        return None;
    };
    (a[2] == b[2]).then_some((fixed, width_mm))
}

/// Join rows on every single-tool layer that has no minimum-command skin fits
/// (those fits identify their rows by exact ends).
pub(super) fn chain_rows(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    routed: bool,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if routed {
        return Ok(());
    }
    let count = plan.layers.len();
    for index in 0..count {
        check_cancel(cancel)?;
        // Separators, diaphragms, skin-bridge domains and minimum-command fits
        // name individual rows of this layer: leave its rows as rows.
        let layer = &plan.layers[index];
        if layer
            .paths
            .iter()
            .any(|p| p.kind == DepositionKind::SeparatorWall)
            || !layer.region_coverage.process_diaphragms.is_empty()
            || !layer.region_coverage.skin_bridge_domains.is_empty()
            || !layer.skin_command_fits.is_empty()
        {
            continue;
        }
        // A bridge on the next layer was decided from the rows below it; a
        // connector near its footprint could change its anchors. Keep a
        // margin of two bead widths around each such bridge.
        let bridges: Vec<[f64; 4]> = plan
            .layers
            .get(index + 1)
            .into_iter()
            .flat_map(|above| &above.paths)
            .filter(|p| {
                p.kind == DepositionKind::Bridge
                    || matches!(p.owner, Some(DepositionOwner::RegionalSkinBridge { .. }))
            })
            .map(|p| {
                let width = match &p.width {
                    WidthProfile::Constant { width_mm } => *width_mm,
                    WidthProfile::PerEdge { widths_mm } => {
                        widths_mm.iter().copied().fold(0.0, f64::max)
                    }
                };
                let margin = 2.0 * width;
                let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                for q in &p.points_mm {
                    for axis in 0..2 {
                        lo[axis] = lo[axis].min(q[axis]);
                        hi[axis] = hi[axis].max(q[axis]);
                    }
                }
                [
                    lo[0] - margin,
                    lo[1] - margin,
                    hi[0] + margin,
                    hi[1] + margin,
                ]
            })
            .collect();
        let near_bridge = |a: [f64; 3], b: [f64; 3]| {
            bridges.iter().any(|r| {
                a[0].min(b[0]) <= r[2]
                    && a[0].max(b[0]) >= r[0]
                    && a[1].min(b[1]) <= r[3]
                    && a[1].max(b[1]) >= r[1]
            })
        };
        let layer = &mut plan.layers[index];
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut keys: Vec<usize> = Vec::new();
        for (index, path) in layer.paths.iter().enumerate() {
            let Some((fixed, width)) = chainable(path) else {
                continue;
            };
            let same = |other: &DepositionPath| {
                other.kind == path.kind
                    && other.owner == path.owner
                    && other.material_domain == path.material_domain
                    && other.physical_z_mm == path.physical_z_mm
                    && other.exposed == path.exposed
                    && chainable(other) == Some((fixed, width))
            };
            match keys.iter().position(|k| same(&layer.paths[*k])) {
                Some(g) => groups[g].push(index),
                None => {
                    keys.push(index);
                    groups.push(vec![index]);
                }
            }
        }
        if groups.iter().all(|g| g.len() < 2) {
            continue;
        }
        let mut taken = vec![false; layer.paths.len()];
        let mut chains: Vec<DepositionPath> = Vec::new();
        for group in &groups {
            let (fixed, width) = chainable(&layer.paths[group[0]]).unwrap();
            let along = 1 - fixed;
            let mut order = group.clone();
            let key = |i: &usize| {
                let p = &layer.paths[*i].points_mm;
                (p[0][fixed], p[0][along].min(p[1][along]))
            };
            order.sort_by(|a, b| {
                let (ka, kb) = (key(a), key(b));
                ka.0.total_cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
            });
            let mut used = vec![false; order.len()];
            for start in 0..order.len() {
                check_cancel(cancel)?;
                if used[start] {
                    continue;
                }
                used[start] = true;
                let first = &layer.paths[order[start]];
                let [a, b] = [first.points_mm[0], first.points_mm[1]];
                let mut points = if a[along] <= b[along] {
                    vec![a, b]
                } else {
                    vec![b, a]
                };
                let mut members = vec![order[start]];
                loop {
                    let end = *points.last().unwrap();
                    // The next row level is the nearest greater fixed
                    // coordinate among unused rows; only a row there may join,
                    // so a connector never steps over another row.
                    let level = order
                        .iter()
                        .zip(&used)
                        .filter(|(_, u)| !**u)
                        .map(|(i, _)| layer.paths[*i].points_mm[0][fixed])
                        .filter(|f| *f > end[fixed])
                        .fold(f64::INFINITY, f64::min);
                    if !level.is_finite() || level - end[fixed] > MAX_ROW_STEP_WIDTHS * width {
                        break;
                    }
                    let next = order.iter().enumerate().find_map(|(k, i)| {
                        let p = &layer.paths[*i].points_mm;
                        if used[k] || p[0][fixed] != level {
                            return None;
                        }
                        let near = [0, 1]
                            .into_iter()
                            .min_by(|x, y| {
                                (p[*x][along] - end[along])
                                    .abs()
                                    .total_cmp(&(p[*y][along] - end[along]).abs())
                            })
                            .unwrap();
                        ((p[near][along] - end[along]).abs() <= MAX_END_OFFSET_WIDTHS * width)
                            .then_some((k, near))
                    });
                    let Some((k, near)) = next else {
                        break;
                    };
                    if near_bridge(end, layer.paths[order[k]].points_mm[near]) {
                        break;
                    }
                    // Rows at one level are disjoint, and the connector stays
                    // within half a bead of the shared end, so it crosses none.
                    used[k] = true;
                    let p = &layer.paths[order[k]].points_mm;
                    points.push(p[near]);
                    points.push(p[1 - near]);
                    members.push(order[k]);
                }
                if members.len() < 2 {
                    continue;
                }
                let mut chain = first.clone();
                chain.points_mm = points;
                chains.push(chain);
                for i in members {
                    taken[i] = true;
                }
            }
        }
        if chains.is_empty() {
            continue;
        }
        let mut kept = Vec::with_capacity(layer.paths.len());
        for (i, path) in std::mem::take(&mut layer.paths).into_iter().enumerate() {
            if !taken[i] {
                kept.push(path);
            }
        }
        kept.extend(chains);
        layer.paths = kept;
        // Connectors are deposited material: the layer's nominal volume is
        // recomputed from its paths exactly as planning computed it.
        crate::planner::support::object_volumes(std::slice::from_mut(layer), intent)?;
    }
    Ok(())
}
