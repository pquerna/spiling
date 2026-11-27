// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Branching support: a region generator inside the support sweep.
//!
//! A tree overhang keeps its roof as an ordinary column through the
//! interface band; below it, tips sampled on the roof descend as discs that
//! lean toward each other, merge, grow, and walk around the part to the bed.
//! Routing reads only a coarse [`Guide`]: the free radius each layer leaves
//! around the part and the plate-reach radius, the largest disc from which a
//! branch can still descend to the bed at its lean. The guide is a screen:
//! the sweep clips every slice by exact occupancy, voids and nominal beads
//! and the up pass keeps only what stands, so a guide error can lose a
//! branch but never place support in material.

use super::grid::{Bits, CellFrame, Raster};
use crate::check_cancel;
use geo::{LineString, Polygon};
use spiling_contracts::manufacturing::ManufacturingError;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

/// Guide cell edge in millimetres.
pub(super) const GUIDE_CELL_MM: f64 = 0.5;
/// Radius quantum of the guide's fields; values floor to it.
const QUANTUM_MM: f64 = 0.05;
/// Directions a branch tries each layer.
const DIRECTIONS: usize = 16;

/// Tree geometry, derived from the bead width (PrusaSlicer organic and
/// Bambu defaults, see the TS1 strategy): 40° lean, 5° growth, tips one bead
/// wide in radius ramping to a 1 mm branch, a 45° bed flare.
#[derive(Clone, Copy, Debug)]
pub(super) struct Params {
    pub(super) tip: f64,
    pub(super) branch: f64,
    pub(super) base: f64,
    pub(super) max_radius: f64,
    pub(super) spacing: f64,
    pub(super) pull: f64,
    lean_cap: f64,
    tan_lean: f64,
    tan_growth: f64,
    tip_layers: u32,
}

impl Params {
    pub(super) fn new(bead_width: f64) -> Self {
        Self {
            // A tip must still print a loop after the half-bead erosion.
            tip: 1.5 * bead_width,
            branch: (2.0 * bead_width).max(1.0),
            base: 3.75,
            max_radius: 3.75,
            spacing: 4.0 * bead_width,
            pull: 8.0,
            // A row end may overhang its lower bead by half a bead.
            lean_cap: 0.4 * bead_width,
            tan_lean: 40f64.to_radians().tan(),
            tan_growth: 5f64.to_radians().tan(),
            tip_layers: 5,
        }
    }

    /// Lateral move a branch may make over a layer of height `h`.
    pub(super) fn lean(&self, h: f64) -> f64 {
        (self.tan_lean * h).min(self.lean_cap)
    }
}

/// One guide block: layers sharing one plate-reach step.
struct Block {
    /// Free radius (distance to occupancy less the XY clearance), quantized.
    free: Vec<u8>,
    /// Plate-reach radius, quantized.
    reach: Vec<u8>,
    /// Cells occupied on some lower block, one bit per cell.
    below: Vec<u64>,
}

/// Consecutive layers `..end` sharing one obstacle set: its key and polygons.
type Run<'p> = (u64, Vec<&'p Polygon<f64>>, usize);
/// One obstacle set's quantized free radius and occupied-cell bits.
type Field = (Vec<u8>, Vec<u64>);

/// Coarse routing fields over the sweep's layers `0..block_of.len()`.
pub(super) struct Guide {
    frame: CellFrame,
    block_of: Vec<usize>,
    blocks: Vec<Block>,
}

impl Guide {
    /// Build the guide bottom-up. `obstacles(l)` names layer `l`'s
    /// occupancy polygons with a key equal for equal sets; `moves[l]` is the
    /// layer's lean. A new block starts once the accumulated lean reaches a
    /// guide cell; between blocks the reach grows by one cell (alternating
    /// 4- and 8-neighbourhoods approximate the lean disc).
    pub(super) fn build<'p>(
        bounds: [[f64; 2]; 2],
        moves: &[f64],
        mut obstacles: impl FnMut(usize) -> Result<(u64, Vec<&'p Polygon<f64>>), ManufacturingError>
        + 'p,
        xy: f64,
        admit: &(dyn Fn(usize) -> Result<(), ManufacturingError> + Sync),
        cancel: &AtomicBool,
        work: &mut usize,
    ) -> Result<Self, ManufacturingError> {
        let frame = CellFrame::new(bounds, GUIDE_CELL_MM)?;
        let [cols, rows] = frame.size();
        let cells = cols.saturating_mul(rows);
        let raster = Raster {
            frame,
            cancel,
            admit,
            work: std::sync::atomic::AtomicUsize::new(0),
        };
        let words = cells.div_ceil(64);
        let block_bytes = cells.saturating_mul(2).saturating_add(words * 8);
        let mut block_of = Vec::with_capacity(moves.len());
        let mut blocks: Vec<Block> = Vec::new();
        // Runs of consecutive layers with one obstacle set; their fields are
        // built in parallel a chunk at a time and consumed in layer order.
        let mut runs: Vec<Run<'p>> = Vec::new();
        for l in 0..moves.len() {
            check_cancel(cancel)?;
            let (key, polygons) = obstacles(l)?;
            match runs.last_mut() {
                Some((k, _, end)) if *k == key => *end = l + 1,
                _ => runs.push((key, polygons, l + 1)),
            }
        }
        let build_field = |occupied: Option<Bits>| -> Result<Field, ManufacturingError> {
            let occupied = occupied.unwrap_or_default();
            let mut bits = vec![0u64; words];
            occupied.for_each_cell(|c, r| {
                let i = r * cols + c;
                bits[i / 64] |= 1 << (i % 64);
            });
            Ok((free_field(&bits, cols, rows, xy), bits))
        };
        const CHUNK: usize = 16;
        let mut fields: std::collections::VecDeque<Field> = std::collections::VecDeque::new();
        let mut next_run = 0usize;
        let mut run = 0usize;
        // Occupancy of every closed block, and of the open block's layers.
        let mut lower = vec![0u64; words];
        let mut open = vec![0u64; words];
        let mut travel = 0.0;
        let mut held = 0usize;
        let mut current: Option<Field> = None;
        for (l, &lean) in moves.iter().enumerate() {
            check_cancel(cancel)?;
            if current.is_none() || l >= runs[run].2 {
                if current.is_some() {
                    run += 1;
                }
                if fields.is_empty() {
                    use rayon::prelude::*;
                    let upto = (next_run + CHUNK).min(runs.len());
                    admit(held.saturating_add((upto - next_run) * (cells + words * 8)))?;
                    let working_set = crate::working_set::current();
                    // The chunk's occupancy rasters are one kernel batch.
                    let groups: Vec<&[&'p Polygon<f64>]> = runs[next_run..upto]
                        .iter()
                        .map(|(_, polygons, _)| polygons.as_slice())
                        .collect();
                    let occupied = raster.outward_groups(&groups, 0.0)?;
                    let built: Vec<Result<Field, ManufacturingError>> = occupied
                        .into_par_iter()
                        .map(|occupied| {
                            crate::working_set::scope(working_set, || build_field(occupied))
                        })
                        .collect();
                    for field in built {
                        fields.push_back(field?);
                    }
                    *work = work.saturating_add((upto - next_run) * cells);
                    next_run = upto;
                }
                current = fields.pop_front();
            }
            let (free, occupied) = current.as_ref().expect("layer field");
            travel += lean;
            if blocks.is_empty() || travel >= GUIDE_CELL_MM {
                travel = 0.0;
                held = held.saturating_add(block_bytes);
                admit(held)?;
                for (acc, o) in lower.iter_mut().zip(&mut open) {
                    *acc |= *o;
                    *o = 0;
                }
                let reach = match blocks.last() {
                    Some(last) => {
                        let grown =
                            max_filter(&last.reach, cols, rows, blocks.len().is_multiple_of(2));
                        grown.iter().zip(free).map(|(g, f)| (*g).min(*f)).collect()
                    }
                    None => free.clone(),
                };
                blocks.push(Block {
                    free: free.clone(),
                    reach,
                    below: lower.clone(),
                });
                *work = work.saturating_add(cells);
            } else {
                let block = blocks.last_mut().expect("open block");
                for ((f, r), v) in block.free.iter_mut().zip(&mut block.reach).zip(free) {
                    *f = (*f).min(*v);
                    *r = (*r).min(*v);
                }
            }
            for (o, v) in open.iter_mut().zip(occupied) {
                *o |= v;
            }
            block_of.push(blocks.len() - 1);
        }
        *work = work.saturating_add(raster.take_work());
        Ok(Self {
            frame,
            block_of,
            blocks,
        })
    }

    fn index(&self, p: [f64; 2]) -> Option<usize> {
        let [cols, _] = self.frame.size();
        self.frame.cell_of(p).map(|[c, r]| r * cols + c)
    }

    fn block(&self, l: usize) -> Option<&Block> {
        self.blocks.get(*self.block_of.get(l)?)
    }

    /// Plate-reach radius at layer `l` and `p`; zero outside the guide.
    pub(super) fn reach(&self, l: usize, p: [f64; 2]) -> f64 {
        match (self.block(l), self.index(p)) {
            (Some(b), Some(i)) => f64::from(b.reach[i]) * QUANTUM_MM,
            _ => 0.0,
        }
    }

    /// Free radius at layer `l` and `p`; zero outside the guide.
    pub(super) fn free(&self, l: usize, p: [f64; 2]) -> f64 {
        match (self.block(l), self.index(p)) {
            (Some(b), Some(i)) => f64::from(b.free[i]) * QUANTUM_MM,
            _ => 0.0,
        }
    }

    /// Whether occupancy lies under `p` on some block below layer `l`'s.
    pub(super) fn shadowed(&self, l: usize, p: [f64; 2]) -> bool {
        match (self.block(l), self.index(p)) {
            (Some(b), Some(i)) => b.below[i / 64] >> (i % 64) & 1 == 1,
            _ => false,
        }
    }

    /// Guide cell edge in millimetres.
    pub(super) fn cell(&self) -> f64 {
        self.frame.cell()
    }

    /// Bytes the guide holds.
    pub(super) fn bytes(&self) -> usize {
        self.blocks
            .iter()
            .map(|b| b.free.len() + b.reach.len() + b.below.len() * 8)
            .sum::<usize>()
            .saturating_add(self.block_of.len() * 8)
    }
}

/// Exact Euclidean distance to the nearest occupied cell centre, less the
/// XY clearance and a half-diagonal (occupied cells touch the material
/// anywhere inside), quantized down.
fn free_field(occupied: &[u64], cols: usize, rows: usize, xy: f64) -> Vec<u8> {
    const FAR: f64 = 1.0e12;
    let cell = GUIDE_CELL_MM;
    let set = |i: usize| occupied[i / 64] >> (i % 64) & 1 == 1;
    let mut squared = vec![0.0f64; cols * rows];
    let mut line = Vec::new();
    let mut out = Vec::new();
    for c in 0..cols {
        line.clear();
        line.extend((0..rows).map(|r| if set(r * cols + c) { 0.0 } else { FAR }));
        edt(&line, &mut out);
        for r in 0..rows {
            squared[r * cols + c] = out[r];
        }
    }
    for r in 0..rows {
        line.clear();
        line.extend_from_slice(&squared[r * cols..(r + 1) * cols]);
        edt(&line, &mut out);
        squared[r * cols..(r + 1) * cols].copy_from_slice(&out);
    }
    let slack = xy + cell * std::f64::consts::FRAC_1_SQRT_2;
    squared
        .iter()
        .map(|d| {
            let radius = d.sqrt() * cell - slack;
            (radius / QUANTUM_MM).floor().clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// One-dimensional squared distance transform (Felzenszwalb–Huttenlocher).
fn edt(f: &[f64], out: &mut Vec<f64>) {
    let n = f.len();
    out.clear();
    out.resize(n, 0.0);
    if n == 0 {
        return;
    }
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let at = |q: usize| f[q] + (q * q) as f64;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = (at(q) - at(p)) / (2.0 * (q as f64 - p as f64));
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // k == 0 and the new parabola dominates from the start.
                v[0] = q;
                z[1] = f64::INFINITY;
                break;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
            break;
        }
    }
    k = 0;
    for (q, slot) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let d = q as f64 - v[k] as f64;
        *slot = d * d + f[v[k]];
    }
}

/// Grow a quantized field by one cell: the maximum over the 4-neighbourhood,
/// or the 8-neighbourhood when `diagonal`.
fn max_filter(field: &[u8], cols: usize, rows: usize, diagonal: bool) -> Vec<u8> {
    let mut out = field.to_vec();
    for r in 0..rows {
        for c in 0..cols {
            let mut best = field[r * cols + c];
            for (dc, dr) in [
                (-1i64, 0i64),
                (1, 0),
                (0, -1),
                (0, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
                (1, 1),
            ]
            .iter()
            .take(if diagonal { 8 } else { 4 })
            {
                let (nc, nr) = (c as i64 + dc, r as i64 + dr);
                if nc >= 0 && nr >= 0 && (nc as usize) < cols && (nr as usize) < rows {
                    best = best.max(field[nr as usize * cols + nc as usize]);
                }
            }
            out[r * cols + c] = best;
        }
    }
    out
}

/// The roof's lowest row lattice: rows lie at multiples of `pitch` on
/// coordinate axis `fixed`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Rows {
    pub(super) fixed: usize,
    pub(super) pitch: f64,
}

/// One branch at its current layer.
#[derive(Clone, Copy, Debug)]
struct Node {
    p: [f64; 2],
    r: f64,
    /// Layers below its tip.
    depth: u32,
}

/// Every live branch, in a deterministic order.
#[derive(Default)]
pub(super) struct Forest {
    nodes: Vec<Node>,
}

impl Forest {
    pub(super) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Plant tips under a roof's bottom layer, per connected roof piece: a
    /// build-frame lattice at the tip spacing inside the piece eroded by the
    /// tip radius, and that eroded outline at the same spacing. A piece too
    /// thin to erode (a lip narrower than a tip) takes tips along its own
    /// outline; one too thin to trace, one at its cell centroid. Returns the
    /// number planted.
    pub(super) fn plant(
        &mut self,
        raster: &Raster<'_>,
        roof: &Bits,
        params: &Params,
        rows: Rows,
    ) -> Result<usize, ManufacturingError> {
        let before = self.nodes.len();
        let s = params.spacing;
        // Tip rows lie on the roof's own lowest row lines, so each tip row
        // has a roof row straight through it.
        let step = rows.pitch * (s / rows.pitch).round().max(1.0);
        let axis_step = |axis: usize| if axis == rows.fixed { step } else { s };
        let mut taken: HashMap<(i64, i64), Vec<[f64; 2]>> = HashMap::new();
        let mut points: Vec<[f64; 2]> = Vec::new();
        let mut offer = |p: [f64; 2], points: &mut Vec<[f64; 2]>| {
            let key = ((p[0] / s).floor() as i64, (p[1] / s).floor() as i64);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(near) = taken.get(&(key.0 + dx, key.1 + dy))
                        && near
                            .iter()
                            .any(|q| (q[0] - p[0]).hypot(q[1] - p[1]) < 0.5 * s)
                    {
                        return;
                    }
                }
            }
            taken.entry(key).or_default().push(p);
            points.push(p);
        };
        for piece in raster.components(roof)? {
            check_cancel(raster.cancel)?;
            let found = points.len();
            let eroded = raster.erode(&piece, params.tip)?;
            let outline = if eroded.is_empty() { &piece } else { &eroded };
            for polygon in &raster.contour(outline)?.0 {
                for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                    for p in along_ring(ring, s) {
                        offer(p, &mut points);
                    }
                }
            }
            let frame = &raster.frame;
            if !eroded.is_empty()
                && let (Some(x), Some(y)) = (eroded.extent(frame, 0), eroded.extent(frame, 1))
            {
                let (sx, sy) = (axis_step(0), axis_step(1));
                let mut j = (y[0] / sy).floor() as i64;
                while (j as f64) * sy <= y[1] {
                    let mut i = (x[0] / sx).floor() as i64;
                    while (i as f64) * sx <= x[1] {
                        let p = [i as f64 * sx, j as f64 * sy];
                        if raster.holds_all(&eroded, p) {
                            offer(p, &mut points);
                        }
                        i += 1;
                    }
                    j += 1;
                }
            }
            if points.len() == found {
                let (mut sum, mut count) = ([0.0, 0.0], 0usize);
                piece.for_each_cell(|c, r| {
                    let p = frame.centre(c, r);
                    sum[0] += p[0];
                    sum[1] += p[1];
                    count += 1;
                });
                if count > 0 {
                    offer([sum[0] / count as f64, sum[1] / count as f64], &mut points);
                }
            }
        }
        raster.charge(points.len());
        self.nodes.reserve(points.len());
        self.nodes.extend(points.into_iter().map(|p| Node {
            p,
            r: params.tip,
            depth: 0,
        }));
        Ok(self.nodes.len() - before)
    }

    /// Discs of every live branch at its current layer.
    pub(super) fn discs(&self) -> impl Iterator<Item = ([f64; 2], f64)> + '_ {
        self.nodes.iter().map(|n| (n.p, n.r))
    }

    /// Move every branch from the layer above down to layer `l` (height `h`,
    /// lower face `z`): lean toward neighbours while the plate stays
    /// reachable, grow, then let larger branches absorb the ones they hold.
    /// A branch whose disc no longer fits beside the part ends.
    #[expect(
        clippy::too_many_arguments,
        reason = "the layer's index, height and floor stay explicit beside cancellation and work"
    )]
    pub(super) fn descend(
        &mut self,
        guide: &Guide,
        l: usize,
        h: f64,
        z: f64,
        params: &Params,
        cancel: &AtomicBool,
        work: &mut usize,
    ) -> Result<(), ManufacturingError> {
        if self.nodes.is_empty() {
            return Ok(());
        }
        let lean = params.lean(h);
        let pull = params.pull;
        let buckets = bucket(&self.nodes);
        let previous = self.nodes.clone();
        let mut next: Vec<Node> = Vec::with_capacity(previous.len());
        let mut from: Vec<usize> = Vec::with_capacity(previous.len());
        for (i, node) in previous.iter().enumerate() {
            check_cancel(cancel)?;
            // Half-way to the 1/d² centroid of neighbours: two branches
            // pulled at each other meet instead of swapping places.
            let (mut sum, mut weight) = ([0.0, 0.0], 0.0);
            visit_near(&buckets, &previous, node.p, pull, |j, other| {
                if j != i {
                    let d2 = (other.p[0] - node.p[0]).powi(2) + (other.p[1] - node.p[1]).powi(2);
                    let w = 1.0 / d2.max(1.0e-6);
                    sum[0] += w * other.p[0];
                    sum[1] += w * other.p[1];
                    weight += w;
                }
            });
            let desired = if weight > 0.0 {
                [
                    0.5 * (node.p[0] + sum[0] / weight),
                    0.5 * (node.p[1] + sum[1] / weight),
                ]
            } else {
                node.p
            };
            let mut grown = if node.depth < params.tip_layers {
                node.r + (params.branch - params.tip) / f64::from(params.tip_layers)
            } else {
                node.r + params.tan_growth * h
            };
            if z < params.base {
                grown = grown.max(params.base - z);
            }
            grown = grown.min(params.max_radius);
            let toward = {
                let d = [desired[0] - node.p[0], desired[1] - node.p[1]];
                let len = d[0].hypot(d[1]);
                let step = len.min(lean);
                if len > 0.0 {
                    [node.p[0] + d[0] / len * step, node.p[1] + d[1] / len * step]
                } else {
                    node.p
                }
            };
            let mut candidates = Vec::with_capacity(DIRECTIONS + 2);
            candidates.push(toward);
            candidates.push(node.p);
            for k in 0..DIRECTIONS {
                let a = std::f64::consts::TAU * k as f64 / DIRECTIONS as f64;
                candidates.push([node.p[0] + lean * a.cos(), node.p[1] + lean * a.sin()]);
            }
            *work = work.saturating_add(candidates.len());
            // Prefer moves that keep a guide cell of plate reach beyond the
            // branch, then any that still reach the bed with a tip; among
            // those, the one closest to the pull.
            let closest = |pool: &mut dyn Iterator<Item = [f64; 2]>| {
                pool.min_by(|a, b| {
                    let da = (a[0] - desired[0]).hypot(a[1] - desired[1]);
                    let db = (b[0] - desired[0]).hypot(b[1] - desired[1]);
                    da.total_cmp(&db)
                })
            };
            let margin = node.r.max(params.tip) + guide.cell();
            let safe = closest(
                &mut candidates
                    .iter()
                    .copied()
                    .filter(|&c| guide.reach(l, c) >= margin),
            )
            .or_else(|| {
                closest(
                    &mut candidates
                        .iter()
                        .copied()
                        .filter(|&c| guide.reach(l, c) >= params.tip),
                )
            });
            let (p, room) = match safe {
                Some(p) => (p, guide.reach(l, p)),
                None => {
                    // Escape toward the deepest reach; a lost branch keeps
                    // descending while a tip still fits beside the part.
                    let p = candidates
                        .iter()
                        .copied()
                        .max_by(|a, b| guide.reach(l, *a).total_cmp(&guide.reach(l, *b)))
                        .unwrap_or(node.p);
                    (p, guide.free(l, p))
                }
            };
            if room < params.tip {
                // The disc collides here: the branch ends on what is below.
                continue;
            }
            // A branch may narrow to the room it has: the up pass trims the
            // wider disc above to what stands, a cone, not a lost trunk.
            let r = grown.min(room);
            from.push(i);
            next.push(Node {
                p,
                r,
                depth: node.depth + 1,
            });
        }
        // Larger branches absorb smaller ones they hold: the absorbed disc
        // one layer up stays within the larger disc grown by the lean.
        let before: Vec<([f64; 2], f64)> = from
            .iter()
            .map(|&i| (previous[i].p, previous[i].r))
            .collect();
        let mut order: Vec<usize> = (0..next.len()).collect();
        order.sort_by(|&a, &b| next[b].r.total_cmp(&next[a].r).then(a.cmp(&b)));
        let merge = bucket(&next);
        let mut absorbed = vec![false; next.len()];
        for &i in &order {
            if absorbed[i] {
                continue;
            }
            let reach = next[i].r + lean + params.max_radius;
            let mut taken = Vec::new();
            visit_near(&merge, &next, next[i].p, reach, |j, other| {
                if j == i || absorbed[j] || other.r > next[i].r {
                    return;
                }
                let (above, radius) = before[j];
                let d = (above[0] - next[i].p[0]).hypot(above[1] - next[i].p[1]);
                if d + radius <= next[i].r + lean {
                    taken.push(j);
                }
            });
            *work = work.saturating_add(taken.len() + 1);
            for j in taken {
                absorbed[j] = true;
                next[i].depth = next[i].depth.max(next[j].depth);
            }
        }
        let mut kept = Vec::with_capacity(next.len());
        kept.extend(
            next.into_iter()
                .zip(absorbed)
                .filter_map(|(n, gone)| (!gone).then_some(n)),
        );
        self.nodes = kept;
        Ok(())
    }
}

type Buckets = HashMap<(i64, i64), Vec<usize>>;

fn bucket(nodes: &[Node]) -> Buckets {
    let mut buckets: Buckets = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        buckets.entry(key(n.p)).or_default().push(i);
    }
    buckets
}

/// Visit nodes within `radius` of `p`, in bucket then index order.
fn visit_near(
    buckets: &Buckets,
    nodes: &[Node],
    p: [f64; 2],
    radius: f64,
    mut visit: impl FnMut(usize, &Node),
) {
    let span = (radius / BUCKET_MM).ceil() as i64;
    let (kx, ky) = key(p);
    for dy in -span..=span {
        for dx in -span..=span {
            if let Some(list) = buckets.get(&(kx + dx, ky + dy)) {
                for &j in list {
                    let n = &nodes[j];
                    if (n.p[0] - p[0]).hypot(n.p[1] - p[1]) <= radius {
                        visit(j, n);
                    }
                }
            }
        }
    }
}

const BUCKET_MM: f64 = 8.0;

fn key(p: [f64; 2]) -> (i64, i64) {
    (
        (p[0] / BUCKET_MM).floor() as i64,
        (p[1] / BUCKET_MM).floor() as i64,
    )
}

/// Points every `step` millimetres along a closed ring, starting at its
/// first vertex.
fn along_ring(ring: &LineString<f64>, step: f64) -> Vec<[f64; 2]> {
    let mut points = Vec::new();
    let mut carry = 0.0;
    for line in ring.lines() {
        let (a, b) = (line.start, line.end);
        let length = (b.x - a.x).hypot(b.y - a.y);
        let mut t = carry;
        while t < length {
            let f = t / length;
            points.push([a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f]);
            t += step;
        }
        carry = t - length;
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_transform_is_exact_on_a_line() {
        let f = [1.0e12, 1.0e12, 0.0, 1.0e12, 1.0e12, 1.0e12, 0.0];
        let mut out = Vec::new();
        edt(&f, &mut out);
        assert_eq!(out, vec![4.0, 1.0, 0.0, 1.0, 4.0, 1.0, 0.0]);
    }

    #[test]
    fn free_field_measures_distance_to_the_nearest_occupied_cell() {
        let (cols, rows): (usize, usize) = (9, 9);
        let mut bits = vec![0u64; (cols * rows).div_ceil(64)];
        let i = 4 * cols + 4;
        bits[i / 64] |= 1 << (i % 64);
        let free = free_field(&bits, cols, rows, 0.0);
        let at = |c: usize, r: usize| f64::from(free[r * cols + c]) * QUANTUM_MM;
        let slack = GUIDE_CELL_MM * std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(at(4, 4), 0.0);
        assert!((at(8, 4) - (4.0 * GUIDE_CELL_MM - slack)).abs() <= QUANTUM_MM);
        assert!((at(7, 7) - (18f64.sqrt() * GUIDE_CELL_MM - slack)).abs() <= QUANTUM_MM);
    }

    #[test]
    fn plate_reach_walks_out_from_under_a_lid() {
        // A 10 mm lid over layers 20..: the plate is reachable beside it
        // everywhere, and under its centre only from well below it.
        let lid = Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
                (0.0, 0.0),
            ]),
            vec![],
        );
        let moves = vec![0.17; 40];
        let cancel = AtomicBool::new(false);
        let admit = |_: usize| Ok(());
        let mut work = 0;
        let guide = Guide::build(
            [[-20.0, -20.0], [30.0, 30.0]],
            &moves,
            |l| {
                Ok(if l >= 20 {
                    (1, vec![&lid])
                } else {
                    (0, vec![])
                })
            },
            0.3,
            &admit,
            &cancel,
            &mut work,
        )
        .unwrap();
        assert!(guide.reach(30, [-10.0, 5.0]) > 2.0);
        assert_eq!(guide.reach(30, [5.0, 5.0]), 0.0);
        assert!(guide.reach(10, [5.0, 5.0]) > 2.0);
        assert!(guide.shadowed(30, [5.0, 5.0]));
        assert!(!guide.shadowed(30, [-10.0, 5.0]));
    }

    fn raster<'a>(
        cancel: &'a AtomicBool,
        admit: &'a (dyn Fn(usize) -> Result<(), ManufacturingError> + Sync),
    ) -> Raster<'a> {
        Raster {
            frame: CellFrame::new(
                [[-10.0, -10.0], [80.0, 20.0]],
                super::super::grid::SUPPORT_CELL_MM,
            )
            .unwrap(),
            cancel,
            admit,
            work: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon<f64> {
        Polygon::new(
            LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]),
            vec![],
        )
    }

    const ROWS: Rows = Rows {
        fixed: 1,
        pitch: 0.44,
    };

    #[test]
    fn a_thin_roof_strip_gets_tips_along_its_length() {
        let cancel = AtomicBool::new(false);
        let admit = |_: usize| Ok(());
        let raster = raster(&cancel, &admit);
        let roof = raster.inward([&rect(0.0, 0.0, 55.0, 1.4)]).unwrap();
        let params = Params::new(0.45);
        let mut forest = Forest::default();
        let planted = forest.plant(&raster, &roof, &params, ROWS).unwrap();
        assert!(planted >= 55.0 as usize / 2, "planted {planted}");
        // A lip narrower than a tip still gets tips along its length.
        let lip = raster.inward([&rect(0.0, 5.0, 55.0, 5.6)]).unwrap();
        let mut forest = Forest::default();
        let planted = forest.plant(&raster, &lip, &params, ROWS).unwrap();
        assert!(planted >= 55.0 as usize / 2, "lip planted {planted}");
    }

    #[test]
    fn branches_lean_together_merge_and_stand_on_each_other() {
        // Two tips 2 mm apart over open space: they lean toward each other,
        // one absorbs the other, and every disc stays within the lower
        // disc grown by the layer's lean (what the up pass keeps).
        let moves = vec![0.15; 60];
        let cancel = AtomicBool::new(false);
        let admit = |_: usize| Ok(());
        let mut work = 0;
        let guide = Guide::build(
            [[-20.0, -20.0], [20.0, 20.0]],
            &moves,
            |_| Ok((0, vec![])),
            0.3,
            &admit,
            &cancel,
            &mut work,
        )
        .unwrap();
        let params = Params::new(0.45);
        let mut forest = Forest {
            nodes: vec![
                Node {
                    p: [-1.0, 0.0],
                    r: params.tip,
                    depth: 0,
                },
                Node {
                    p: [1.0, 0.0],
                    r: params.tip,
                    depth: 0,
                },
            ],
        };
        let mut above: Vec<([f64; 2], f64)> = forest.discs().collect();
        for l in (0..59).rev() {
            forest
                .descend(
                    &guide,
                    l,
                    0.2,
                    0.2 * l as f64 + 6.0,
                    &params,
                    &cancel,
                    &mut work,
                )
                .unwrap();
            let below: Vec<([f64; 2], f64)> = forest.discs().collect();
            let lean = params.lean(0.2);
            for (p, r) in &above {
                assert!(
                    below
                        .iter()
                        .any(|(q, s)| (p[0] - q[0]).hypot(p[1] - q[1]) + r <= s + lean + 1e-9),
                    "disc at {p:?} r {r} is not held below at layer {l}"
                );
            }
            above = below;
        }
        assert_eq!(forest.nodes.len(), 1, "the two tips merged");
        assert!(forest.nodes[0].r > params.tip);
    }
}
