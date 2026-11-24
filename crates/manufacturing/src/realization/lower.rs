// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Road lowering in the plan (RFC 11): simplify a road's own points within
//! its class budget, only in a direction its owning checks still accept.
//!
//! Structure-class roads (inner walls, covered skins and solid fill, bridges,
//! support interface) drop points by Douglas–Peucker within the structure
//! budget; arc and connector runs, width and Z changes and every edge a road
//! of the layer above (or support across its contact gap, or a connector of
//! this layer) is proved against stay exact. Exposed walls and exposed top
//! skin are the surface class and are never lowered.

use crate::{check_cancel, error};
use rayon::prelude::*;
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

/// Half the factor of the mean bead width within which a wall lies beside an
/// exposed wall: adjacent lanes sit about one bead apart; a wall across a slot
/// at least a quarter bead wide does not count.
const BESIDE_FACTOR: f64 = 0.625;

/// Shortest chord lowering creates: well above the minimum command length, so
/// XYZ6 rounding never shortens one below it (as brim loop edges).
const MIN_LOWERED_CHORD_MM: f64 =
    spiling_contracts::manufacturing::program_coordinates::MIN_DEPOSITION_COMMAND_MM * 2.5;

/// Simplify every structure-class road of `plan` (inner walls, covered skins
/// and solid fill, bridges, support interface) within `budget_mm` of its own
/// points. Exposed walls and exposed top skin are the surface class and keep
/// every point. Returns the number of edges removed.
pub(super) fn structure_roads(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    budget_mm: f64,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    // Fixed-dual jobs prove separator contact and functional joins from the
    // actual strokes of every wall at offsets far below the structure budget.
    if budget_mm.is_nan()
        || budget_mm <= 0.0
        || matches!(intent.printer.workflow, JobWorkflow::FixedDualMarlin { .. })
    {
        return Ok(0);
    }
    // Roads above were planned against this layer's exact roads: a bridge's
    // ends sit at the nominal capsule of the road below, skin rows are proved
    // supported by it, and support roots on part roads across its contact
    // gap. Keep every edge within a bead and the budget of the next layer's
    // bridges and skin rows, of support up to that gap and of this layer's
    // connectors exact.
    let contact_gap_mm = match intent.recipe.supports {
        SupportPolicy::Breakaway { z_gap_mm, .. } => z_gap_mm,
        _ => 0.0,
    };
    let layers = &plan.layers;
    let keep_out: Vec<KeepOut> = (0..layers.len())
        .into_par_iter()
        .map(|index| {
            let top = layers[index].z_mm + contact_gap_mm + 1e-6;
            let mut keep_out = KeepOut::default();
            // A separator's endpoint bond names an exact wall segment.
            for interface in &layers[index].region_coverage.process_interfaces {
                for bond in interface.endpoint_bonds.iter().flatten() {
                    let (ProcessEndpointBond::Wall { wall_segment_mm }
                    | ProcessEndpointBond::Skin {
                        wall_segment_mm, ..
                    }) = bond;
                    let [a, b] = wall_segment_mm.map(|p| [p[0], p[1], 0.0]);
                    keep_out.insert_segment(a, b, budget_mm);
                }
            }
            // A connector must stay in the bead band of this layer's actual walls.
            for path in &layers[index].paths {
                for run in &path.connectors {
                    keep_out.insert_edges(
                        path,
                        run.first as usize..run.last as usize,
                        1.0,
                        budget_mm,
                    );
                }
            }
            for (offset, above) in layers[index + 1..].iter().enumerate() {
                if offset > 0 && above.lower_z_mm > top {
                    break;
                }
                for path in &above.paths {
                    // A skin bridge ends on the exact capsule below; a wall
                    // lowered to Bridge rests on the roads below like a row.
                    let wall = path.owner == Some(DepositionOwner::Wall);
                    if path.kind.is_support()
                        || (offset == 0 && path.kind == DepositionKind::Bridge && !wall)
                    {
                        keep_out.insert(path, budget_mm);
                    } else if offset == 0 && rests_on_layer_below(path.kind) {
                        keep_out.insert_row_ends(path, budget_mm);
                    }
                }
            }
            keep_out
        })
        .collect();
    // A road over the void or at the edge of the layer below would move toward
    // the overhang: only points deep inside the layer below's model roads (a
    // half bead less the budget from a lower centreline) may go. The first
    // layer rests on the bed throughout.
    let below: Vec<Option<KeepOut>> = (0..layers.len())
        .into_par_iter()
        .map(|index| {
            let lower = &layers[index.checked_sub(1)?];
            let mut support = KeepOut::default();
            for path in lower.paths.iter().filter(|path| path.kind.is_model()) {
                support.insert_edges(path, 0..path.points_mm.len() - 1, 0.5, -budget_mm);
            }
            Some(support)
        })
        .collect();
    // A road of the next layer whose centreline runs along the rim of a road
    // here (half its width, within the budget) is covered or not by a hair:
    // keep such edges exact. Roads squarely on top or clear of it are not.
    let rims: Vec<Rims> = (0..layers.len())
        .into_par_iter()
        .map(|index| {
            let mut rims = Rims::default();
            if let Some(above) = layers.get(index + 1) {
                for path in above.paths.iter().filter(|path| path.kind.is_model()) {
                    rims.insert(path);
                }
            }
            rims
        })
        .collect();
    let working_set = crate::working_set::current();
    plan.layers
        .par_iter_mut()
        .zip(
            keep_out
                .par_iter()
                .zip(below.par_iter().zip(rims.par_iter())),
        )
        .map(|(layer, (keep_out, (below, rims)))| {
            let context = Context {
                keep_out,
                below: below.as_ref(),
                rims,
            };
            crate::working_set::scope(working_set, || {
                layer_structure(layer, intent, budget_mm, &context, cancel)
            })
        })
        .try_fold(|| 0usize, |total, removed| removed.map(|r| total + r))
        .try_reduce(|| 0, |a, b| Ok(a + b))
}

/// The structure road class (RFC 11): hidden roads whose exact position no
/// fitting surface depends on.
fn structure_class(path: &DepositionPath) -> bool {
    if path.exposed || path.points_mm.len() < 3 {
        return false;
    }
    match path.kind {
        DepositionKind::Perimeter => path.owner == Some(DepositionOwner::Wall),
        // A skin bridge's middle point is its sole ownership transition.
        DepositionKind::Bridge => {
            !matches!(path.owner, Some(DepositionOwner::RegionalSkinBridge { .. }))
        }
        DepositionKind::SolidFill
        | DepositionKind::BottomSkin
        | DepositionKind::TopSkin
        | DepositionKind::SupportInterface => true,
        _ => false,
    }
}

/// What lowering one layer must respect from its neighbours.
struct Context<'a> {
    keep_out: &'a KeepOut,
    below: Option<&'a KeepOut>,
    rims: &'a Rims,
}

fn edge_width(path: &DepositionPath, edge: usize) -> f64 {
    match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm[edge],
    }
}

/// A centreline point of a road and the road's unit direction.
type RimSample = ([f64; 3], [f64; 2]);

/// Centreline samples of the next layer's model roads, at most
/// [`Rims::SPACING_MM`] apart, with their road direction, in a uniform grid.
#[derive(Default)]
struct Rims {
    cells: std::collections::HashMap<(i64, i64), Vec<RimSample>>,
}

impl Rims {
    const SPACING_MM: f64 = 0.1;
    /// Sine of the widest angle between a road above and an edge here for the
    /// road to run along the edge's rim; a crossing road or one ending on the
    /// edge only shifts where its coverage starts.
    const MAX_SINE: f64 = 0.5;

    fn insert(&mut self, path: &DepositionPath) {
        for w in path.points_mm.windows(2) {
            let (a, b) = (w[0], w[1]);
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            if length.is_nan() || length <= 0.0 {
                continue;
            }
            let direction = [(b[0] - a[0]) / length, (b[1] - a[1]) / length];
            let steps = (length / Self::SPACING_MM).ceil().clamp(1.0, 1e7) as usize;
            for k in 0..=steps {
                let t = k as f64 / steps as f64;
                let p = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), a[2]];
                self.cells
                    .entry((KeepOut::cell(p[0]), KeepOut::cell(p[1])))
                    .or_default()
                    .push((p, direction));
            }
        }
    }

    /// Whether a sample running along the edge `a..b` lies within
    /// `budget_mm` of its rim `radius_mm` from it.
    fn on_rim(&self, a: [f64; 3], b: [f64; 3], radius_mm: f64, budget_mm: f64) -> bool {
        if self.cells.is_empty() {
            return false;
        }
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = dx.hypot(dy);
        let reach = radius_mm + budget_mm;
        if length.is_nan() || length <= 0.0 {
            return false;
        }
        // Only a sample whose foot falls on the edge: one past an end belongs
        // to the road continuing it, not to a road along its rim.
        let along = |(p, u): &RimSample| {
            let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (length * length);
            let offset = ((p[1] - a[1]) * dx - (p[0] - a[0]) * dy) / length;
            (0.0..=1.0).contains(&t)
                && (u[0] * dy - u[1] * dx).abs() <= Self::MAX_SINE * length
                && (offset.abs() - radius_mm).abs() <= budget_mm
        };
        (KeepOut::cell(a[0].min(b[0]) - reach)..=KeepOut::cell(a[0].max(b[0]) + reach)).any(|x| {
            (KeepOut::cell(a[1].min(b[1]) - reach)..=KeepOut::cell(a[1].max(b[1]) + reach)).any(
                |y| {
                    self.cells
                        .get(&(x, y))
                        .is_some_and(|samples| samples.iter().any(along))
                },
            )
        })
    }
}

/// Roads whose anchoring or support is proved from the actual roads of the
/// layer below: bridges, and skin and solid rows.
fn rests_on_layer_below(kind: DepositionKind) -> bool {
    matches!(
        kind,
        DepositionKind::Bridge
            | DepositionKind::TopSkin
            | DepositionKind::BottomSkin
            | DepositionKind::SolidFill
    )
}

/// Edges of the roads lowering must keep clear of, each with its bead width
/// plus the budget as reach, in a uniform grid of their bounds.
#[derive(Default)]
struct KeepOut {
    edges: Vec<([f64; 3], [f64; 3], f64)>,
    cells: std::collections::HashMap<(i64, i64), Vec<u32>>,
}

impl KeepOut {
    const CELL_MM: f64 = 2.0;
    /// Length of a row end carried by the roads at its end.
    const ROW_END_MM: f64 = 1.0;

    fn cell(value: f64) -> i64 {
        (value / Self::CELL_MM).floor() as i64
    }

    fn insert(&mut self, path: &DepositionPath, margin_mm: f64) {
        self.insert_edges(
            path,
            0..path.points_mm.len().saturating_sub(1),
            1.0,
            margin_mm,
        );
    }

    /// A skin or solid row is proved supported from the roads below along
    /// its length; only near where it ends does one road below carry it
    /// alone. Insert the last [`KeepOut::ROW_END_MM`] of each end of each of
    /// its roads (path ends and connector boundaries).
    fn insert_row_ends(&mut self, path: &DepositionPath, margin_mm: f64) {
        let points = &path.points_mm;
        let n = points.len();
        if n < 2 {
            return;
        }
        // (end point, its edge, the edge's other point)
        let mut ends = vec![(0, 0, 1), (n - 1, n - 2, n - 2)];
        for run in &path.connectors {
            let (first, last) = (run.first as usize, run.last as usize);
            ends.push((first, first - 1, first - 1));
            ends.push((last, last, last + 1));
        }
        for (end, edge, other) in ends {
            let (a, b) = (points[end], points[other]);
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            let t = if length > Self::ROW_END_MM {
                Self::ROW_END_MM / length
            } else {
                1.0
            };
            let inner = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), a[2]];
            self.insert_segment(a, inner, edge_width(path, edge) + margin_mm);
        }
    }

    fn insert_segment(&mut self, a: [f64; 3], b: [f64; 3], reach: f64) {
        if reach.is_nan() || reach <= 0.0 {
            return;
        }
        let id = self.edges.len() as u32;
        self.edges.push((a, b, reach));
        for x in Self::cell(a[0].min(b[0]) - reach)..=Self::cell(a[0].max(b[0]) + reach) {
            for y in Self::cell(a[1].min(b[1]) - reach)..=Self::cell(a[1].max(b[1]) + reach) {
                self.cells.entry((x, y)).or_default().push(id);
            }
        }
    }

    /// Insert `edges` of `path` with reach `width * width_factor + margin_mm`;
    /// an edge with no positive reach is left out.
    fn insert_edges(
        &mut self,
        path: &DepositionPath,
        edges: std::ops::Range<usize>,
        width_factor: f64,
        margin_mm: f64,
    ) {
        for edge in edges {
            let reach = edge_width(path, edge) * width_factor + margin_mm;
            self.insert_segment(path.points_mm[edge], path.points_mm[edge + 1], reach);
        }
    }

    /// Whether `p` lies within reach plus `extra_mm` of an edge.
    fn within(&self, p: [f64; 3], extra_mm: f64) -> bool {
        let cell = (Self::cell(p[0]), Self::cell(p[1]));
        (cell.0 - 1..=cell.0 + 1).any(|x| {
            (cell.1 - 1..=cell.1 + 1).any(|y| {
                self.cells.get(&(x, y)).is_some_and(|ids| {
                    ids.iter().any(|&id| {
                        let (a, b, reach) = self.edges[id as usize];
                        segment_distance(p, a, b) <= reach + extra_mm
                    })
                })
            })
        })
    }

    /// Whether the edge `a..b` comes within reach of a kept-out edge.
    fn touches(&self, a: [f64; 3], b: [f64; 3]) -> bool {
        if self.edges.is_empty() {
            return false;
        }
        (Self::cell(a[0].min(b[0]))..=Self::cell(a[0].max(b[0]))).any(|x| {
            (Self::cell(a[1].min(b[1]))..=Self::cell(a[1].max(b[1]))).any(|y| {
                self.cells.get(&(x, y)).is_some_and(|ids| {
                    ids.iter().any(|&id| {
                        let (p, q, reach) = self.edges[id as usize];
                        segments_distance(a, b, p, q) <= reach
                    })
                })
            })
        })
    }
}

/// Planar distance between the segments `a..b` and `p..q`.
fn segments_distance(a: [f64; 3], b: [f64; 3], p: [f64; 3], q: [f64; 3]) -> f64 {
    let cross = |o: [f64; 3], u: [f64; 3], v: [f64; 3]| {
        (u[0] - o[0]) * (v[1] - o[1]) - (u[1] - o[1]) * (v[0] - o[0])
    };
    let (d1, d2) = (cross(p, q, a), cross(p, q, b));
    let (d3, d4) = (cross(a, b, p), cross(a, b, q));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    segment_distance(a, p, q)
        .min(segment_distance(b, p, q))
        .min(segment_distance(p, a, b))
        .min(segment_distance(q, a, b))
}

fn layer_structure(
    layer: &mut PrintLayer,
    intent: &ManufacturingIntent,
    budget_mm: f64,
    context: &Context<'_>,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    check_cancel(cancel)?;
    let recipe = &intent.recipe;
    let mut removed = 0usize;
    let mut model_changed = false;
    let mut support_change = 0.0f64;
    // A wall path is exposed only when all of it is on the outermost lane; one
    // that is partly on it is not. Only a wall point beside an exposed wall
    // (the next lane, about a bead away) is known to lie inside the surface
    // lane.
    let mut surface = KeepOut::default();
    for path in layer.paths.iter().filter(|path| path.exposed) {
        if path.owner == Some(DepositionOwner::Wall) {
            surface.insert_edges(path, 0..path.points_mm.len() - 1, BESIDE_FACTOR, 0.0);
        }
    }
    for path in &mut layer.paths {
        if !structure_class(path) {
            continue;
        }
        check_cancel(cancel)?;
        let wall = path.owner == Some(DepositionOwner::Wall);
        let keep = road_keep(path, budget_mm, context, wall.then_some(&surface));
        let kept = keep.iter().filter(|k| **k).count();
        if kept == path.points_mm.len() {
            continue;
        }
        let before = if path.kind.is_support() {
            crate::planner::nominal_volume(path, layer.index, recipe)?
        } else {
            0.0
        };
        removed += apply_keep(path, &keep);
        if path.kind.is_support() {
            support_change += crate::planner::nominal_volume(path, layer.index, recipe)? - before;
        } else {
            model_changed = true;
        }
    }
    if model_changed {
        crate::planner::support::object_volumes(std::slice::from_mut(layer), intent)?;
    }
    if support_change != 0.0 {
        let coverage = &mut layer.region_coverage;
        coverage.support_nominal_deposition_volume_mm3 += support_change;
        coverage.support_area_mm2 =
            coverage.support_nominal_deposition_volume_mm3 / (layer.z_mm - layer.lower_z_mm);
        if !coverage.support_nominal_deposition_volume_mm3.is_finite() {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "lowered support interface volume is not finite",
            ));
        }
    }
    Ok(removed)
}

/// Which points of `path` stay: its ends, every point of an arc or connector
/// run, every width or Z change, both ends of every edge within reach of a
/// `keep_out` edge, the ends of every edge on the rim of a road above, every
/// point not deep inside the roads below, and whatever Douglas–Peucker over
/// each run of free points between them needs to stay within `budget_mm`. A
/// closed loop that would keep fewer than four points keeps all of them.
fn road_keep(
    path: &DepositionPath,
    budget_mm: f64,
    context: &Context<'_>,
    beside: Option<&KeepOut>,
) -> Vec<bool> {
    let points = &path.points_mm;
    let n = points.len();
    let mut fixed = vec![false; n];
    fixed[0] = true;
    fixed[n - 1] = true;
    for run in &path.arcs {
        fixed[run.first as usize..=run.last as usize].fill(true);
    }
    for run in &path.connectors {
        fixed[run.first as usize..=run.last as usize].fill(true);
    }
    if let WidthProfile::PerEdge { widths_mm } = &path.width {
        for i in 1..n - 1 {
            if widths_mm[i - 1] != widths_mm[i] {
                fixed[i] = true;
            }
        }
    }
    for i in 1..n - 1 {
        if points[i][2] != points[i - 1][2] || points[i][2] != points[i + 1][2] {
            fixed[i] = true;
        }
    }
    for i in 0..n - 1 {
        let (a, b) = (points[i], points[i + 1]);
        let radius = edge_width(path, i) * 0.5;
        if context.keep_out.touches(a, b) || context.rims.on_rim(a, b, radius, budget_mm) {
            fixed[i] = true;
            fixed[i + 1] = true;
        }
    }
    if let Some(beside) = beside {
        for i in 1..n - 1 {
            let half = edge_width(path, i - 1).min(edge_width(path, i)) * BESIDE_FACTOR;
            if !beside.within(points[i], half) {
                fixed[i] = true;
            }
        }
    }
    if let Some(below) = context.below {
        for i in 1..n - 1 {
            if !below.touches(points[i], points[i]) {
                fixed[i] = true;
            }
        }
    }
    let mut keep = fixed.clone();
    let mut start = 0;
    for (end, _) in fixed.iter().enumerate().skip(1).filter(|(_, f)| **f) {
        if end > start + 1 {
            douglas_peucker(points, start, end, budget_mm, &mut keep);
        }
        start = end;
    }
    let closed = points[0] == points[n - 1];
    if closed && keep.iter().filter(|k| **k).count() < 4 {
        keep.fill(true);
    }
    keep
}

/// Mark the points of `first..=last` a chord list within `budget_mm` needs.
/// A chord replacing several edges is at least [`MIN_LOWERED_CHORD_MM`]
/// long; a shorter one splits at its farthest point like an excess.
fn douglas_peucker(
    points: &[[f64; 3]],
    first: usize,
    last: usize,
    budget_mm: f64,
    keep: &mut [bool],
) {
    let mut stack = vec![(first, last)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (far, distance) = (a + 1..b)
            .map(|i| (i, segment_distance(points[i], points[a], points[b])))
            .fold(
                (a + 1, -1.0),
                |best, next| if next.1 > best.1 { next } else { best },
            );
        let chord = (points[b][0] - points[a][0]).hypot(points[b][1] - points[a][1]);
        if distance <= budget_mm && chord >= MIN_LOWERED_CHORD_MM {
            continue;
        }
        keep[far] = true;
        stack.push((a, far));
        stack.push((far, b));
    }
}

/// Planar distance of `p` from the segment `a..b` (from `a` when they meet).
fn segment_distance(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// Drop the points `keep` excludes, remapping per-edge widths and arc and
/// connector runs (whose points are all kept). Returns the edges removed.
fn apply_keep(path: &mut DepositionPath, keep: &[bool]) -> usize {
    let mut new_index = Vec::with_capacity(keep.len());
    let mut next = 0u32;
    for &k in keep {
        new_index.push(next);
        next += u32::from(k);
    }
    if let WidthProfile::PerEdge { widths_mm } = &mut path.width {
        // Each kept edge starts at a kept point; the edges it replaces share
        // that point's width.
        *widths_mm = widths_mm
            .iter()
            .zip(keep)
            .filter(|(_, k)| **k)
            .map(|(w, _)| *w)
            .collect();
    }
    for run in &mut path.arcs {
        run.first = new_index[run.first as usize];
        run.last = new_index[run.last as usize];
    }
    for run in &mut path.connectors {
        run.first = new_index[run.first as usize];
        run.last = new_index[run.last as usize];
    }
    let before = path.points_mm.len();
    let mut i = 0;
    path.points_mm.retain(|_| {
        let k = keep[i];
        i += 1;
        k
    });
    before - path.points_mm.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road(points: Vec<[f64; 3]>, width: WidthProfile) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width,
            physical_z_mm: [0.8, 1.0],
            points_mm: points,
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }
    }

    /// A gently waving open road along x: 0.01 mm noise every 0.1 mm.
    fn wavy(count: usize) -> Vec<[f64; 3]> {
        (0..count)
            .map(|i| [i as f64 * 0.1, if i % 2 == 0 { 0.0 } else { 0.01 }, 1.0])
            .collect()
    }

    fn lowered(path: &DepositionPath, budget: f64, keep_out: &KeepOut) -> DepositionPath {
        let mut out = path.clone();
        let rims = Rims::default();
        let context = Context {
            keep_out,
            below: None,
            rims: &rims,
        };
        let keep = road_keep(path, budget, &context, None);
        apply_keep(&mut out, &keep);
        out
    }

    fn within(original: &[[f64; 3]], lowered: &[[f64; 3]], budget: f64) -> bool {
        original.iter().all(|p| {
            lowered
                .windows(2)
                .any(|w| segment_distance(*p, w[0], w[1]) <= budget + 1e-12)
        })
    }

    #[test]
    fn structure_road_keeps_ends_and_stays_within_budget() {
        let path = road(wavy(41), WidthProfile::Constant { width_mm: 0.45 });
        let out = lowered(&path, 0.04, &KeepOut::default());
        assert!(out.points_mm.len() < path.points_mm.len());
        assert_eq!(out.points_mm.first(), path.points_mm.first());
        assert_eq!(out.points_mm.last(), path.points_mm.last());
        assert!(within(&path.points_mm, &out.points_mm, 0.04));
        for w in out.points_mm.windows(2) {
            let length = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
            assert!(length >= MIN_LOWERED_CHORD_MM || path.points_mm.windows(2).any(|e| e == w));
        }
    }

    #[test]
    fn width_breakpoints_and_connectors_survive_with_their_edges() {
        let points = wavy(41);
        let mut widths = vec![0.45; 40];
        widths[20..].fill(0.5);
        let mut path = road(points.clone(), WidthProfile::PerEdge { widths_mm: widths });
        path.kind = DepositionKind::SolidFill;
        path.owner = Some(DepositionOwner::Regional { region_index: 0 });
        path.connectors = vec![ConnectorRun { first: 8, last: 12 }];
        let out = lowered(&path, 0.04, &KeepOut::default());
        assert!(out.points_mm.len() < path.points_mm.len());
        assert!(out.points_mm.contains(&points[20]));
        let run = out.connectors[0];
        assert_eq!(run.last - run.first, 4);
        assert_eq!(
            out.points_mm[run.first as usize..=run.last as usize],
            points[8..=12]
        );
        let WidthProfile::PerEdge { widths_mm } = &out.width else {
            panic!("per-edge widths")
        };
        assert_eq!(widths_mm.len(), out.points_mm.len() - 1);
        for (edge, w) in out.points_mm.windows(2).zip(widths_mm) {
            let start = points
                .iter()
                .position(|p| *p == edge[0])
                .expect("kept point");
            assert_eq!(*w, if start < 20 { 0.45 } else { 0.5 });
        }
    }

    #[test]
    fn surface_roads_and_skin_bridge_transitions_are_not_structure() {
        let mut path = road(wavy(9), WidthProfile::Constant { width_mm: 0.45 });
        assert!(structure_class(&path));
        path.exposed = true;
        assert!(!structure_class(&path));
        path.exposed = false;
        path.kind = DepositionKind::TopSkin;
        path.owner = Some(DepositionOwner::MandatorySkin {
            skin: ProcessSkinKind::TopSkin,
        });
        assert!(structure_class(&path));
        path.exposed = true;
        assert!(!structure_class(&path));
        path.exposed = false;
        path.kind = DepositionKind::Bridge;
        path.owner = Some(DepositionOwner::RegionalSkinBridge {
            region_index: 0,
            skin: ProcessSkinKind::TopSkin,
            skin_first: true,
        });
        assert!(!structure_class(&path));
        path.kind = DepositionKind::SparseFill;
        path.owner = Some(DepositionOwner::Regional { region_index: 0 });
        assert!(!structure_class(&path));
    }

    #[test]
    fn closed_loop_stays_closed_with_four_points() {
        let points: Vec<[f64; 3]> = (0..=360)
            .map(|i| {
                let a = (i % 360) as f64 * std::f64::consts::TAU / 360.0;
                [5.0 * a.cos(), 5.0 * a.sin(), 1.0]
            })
            .collect();
        let path = road(points.clone(), WidthProfile::Constant { width_mm: 0.45 });
        let out = lowered(&path, 0.04, &KeepOut::default());
        assert!(out.points_mm.len() >= 4 && out.points_mm.len() < points.len());
        assert_eq!(out.points_mm.first(), out.points_mm.last());
        assert!(within(&points, &out.points_mm, 0.04));
        // A tiny loop cannot drop below four points.
        let tiny = road(
            vec![
                [0.0, 0.0, 1.0],
                [0.01, 0.0, 1.0],
                [0.01, 0.01, 1.0],
                [0.0, 0.01, 1.0],
                [0.0, 0.0, 1.0],
            ],
            WidthProfile::Constant { width_mm: 0.45 },
        );
        assert_eq!(
            lowered(&tiny, 0.04, &KeepOut::default()).points_mm,
            tiny.points_mm
        );
    }

    #[test]
    fn points_over_the_void_below_stay() {
        let path = road(wavy(41), WidthProfile::Constant { width_mm: 0.45 });
        // The layer below holds a road only under x <= 2.
        let lower = road(
            vec![[-1.0, 0.0, 0.8], [2.0, 0.0, 0.8]],
            WidthProfile::Constant { width_mm: 0.45 },
        );
        let mut below = KeepOut::default();
        below.insert_edges(&lower, 0..1, 0.5, -0.04);
        let mut out = path.clone();
        let (keep_out, rims) = (KeepOut::default(), Rims::default());
        let context = Context {
            keep_out: &keep_out,
            below: Some(&below),
            rims: &rims,
        };
        let keep = road_keep(&path, 0.04, &context, None);
        apply_keep(&mut out, &keep);
        assert!(out.points_mm.len() < path.points_mm.len());
        for p in path.points_mm.iter().filter(|p| p[0] > 2.2) {
            assert!(out.points_mm.contains(p), "{p:?}");
        }
    }

    #[test]
    fn wall_points_stay_unless_beside_an_exposed_wall() {
        let path = road(wavy(41), WidthProfile::Constant { width_mm: 0.45 });
        // An exposed wall runs beside the first half only, beads touching.
        let mut outer = road(
            vec![[-1.0, -0.4, 1.0], [2.0, -0.4, 1.0]],
            WidthProfile::Constant { width_mm: 0.45 },
        );
        outer.exposed = true;
        let mut beside = KeepOut::default();
        beside.insert_edges(&outer, 0..1, BESIDE_FACTOR, 0.0);
        let (keep_out, rims) = (KeepOut::default(), Rims::default());
        let context = Context {
            keep_out: &keep_out,
            below: None,
            rims: &rims,
        };
        let mut out = path.clone();
        apply_keep(&mut out, &road_keep(&path, 0.04, &context, Some(&beside)));
        assert!(out.points_mm.len() < path.points_mm.len());
        for p in path.points_mm.iter().filter(|p| p[0] > 2.7) {
            assert!(out.points_mm.contains(p), "{p:?}");
        }
    }

    #[test]
    fn edges_under_roads_above_stay_exact() {
        let path = road(wavy(41), WidthProfile::Constant { width_mm: 0.45 });
        let mut above = road(
            vec![[2.0, -1.0, 1.2], [2.0, 1.0, 1.2]],
            WidthProfile::Constant { width_mm: 0.1 },
        );
        above.kind = DepositionKind::Bridge;
        let mut keep_out = KeepOut::default();
        keep_out.insert(&above, 0.04);
        let out = lowered(&path, 0.04, &keep_out);
        assert!(out.points_mm.len() < path.points_mm.len());
        for p in path.points_mm.iter().filter(|p| (p[0] - 2.0).abs() < 0.1) {
            assert!(out.points_mm.contains(p), "{p:?}");
        }
    }
}
