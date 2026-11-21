// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Established part material on the support grid, built bottom-up.
//!
//! Support may stand on the part only where the part is itself held up: by
//! the bed or by established material below. That is decided per centreline
//! interval and stored as cells, never per road, so one local gap removes
//! only the material over it:
//!
//! - every first-layer road is established whole (the bed holds it);
//! - above it, an ordinary road's centreline is proved where it lies in the
//!   established cells of the layer below, or where it meets the exact
//!   donor region of a lower proved subspan: the flat strip of half the
//!   bead (at most half the recipe bead) across the subspan, with a round
//!   cap only where the subspan reaches a real road end. A cut inside a road
//!   never gains a cap. The layer's established cells are its inward model
//!   footprint minus every cell an unproved stretch's bead touches, plus the
//!   cells wholly inside one proved strip or real end cap. Nothing is
//!   granted from a whole donor road, a whole receiving edge or a dilation;
//! - an explicit Bridge road joins the chain only when both of its ends are
//!   proved: a printed bridge holds up the layer above it, but it never
//!   donates a landing itself.
//!
//! Support is not yet deposited when this runs, so it never establishes part
//! material here: part over support is not landing ground.
//!
//! Only roads whose centreline meets a model cell off the layer below take
//! the exact interval work; the rest are proved by the cell screen alone.
//! The set is computed once, layer by layer, in the sweep's up pass; its
//! landing cells per part root are what raster retention and final
//! reconciliation anchor interface on. These cells are producer evidence
//! for choosing landings, not proof; the verifier keeps its own ancestry.

use super::grid::{Bits, Raster, SUPPORT_CELL_MM};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Accumulated nanoseconds per establishment stage, for the phase ledger.
pub(super) static TRACE: [std::sync::atomic::AtomicU64; 6] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 6];
pub(super) const TRACE_NAMES: [&str; 6] = [
    "footprint",
    "lower",
    "holes",
    "unproved",
    "bridges",
    "layers",
];
fn trace(stage: usize, since: std::time::Instant) -> std::time::Instant {
    let now = std::time::Instant::now();
    TRACE[stage].fetch_add(
        now.duration_since(since).as_nanos() as u64,
        Ordering::Relaxed,
    );
    now
}

/// Adjacent roads planned tangent (row spacing equal to bead width) share
/// only a line; inflating each footprint by this much joins them so the
/// shared line is not mistaken for a boundary. It is far below a grid cell.
const FOOTPRINT_JOIN_MM: f64 = 1.0e-3;

/// Guard against floating rounding in the exact donor tests: intervals and
/// distances lose it, so a borderline point is never claimed.
const SLACK_MM: f64 = 1.0e-6;

/// An established lower centreline subspan: its endpoints, its donor
/// radius, and whether each end is a real road end that keeps its cap.
#[derive(Clone, Copy)]
pub(super) struct Donor {
    ends: [[f64; 3]; 2],
    radius: f64,
    caps: [bool; 2],
}

impl Donor {
    fn bounds(&self, by: f64) -> [[f64; 2]; 2] {
        let [a, b] = self.ends;
        [
            [a[0].min(b[0]) - by, a[1].min(b[1]) - by],
            [a[0].max(b[0]) + by, a[1].max(b[1]) + by],
        ]
    }

    /// The donor frame: origin, unit axis and length.
    fn frame(&self) -> ([f64; 2], [f64; 2], f64) {
        let [a, b] = self.ends;
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = dx.hypot(dy);
        let axis = if length > 0.0 {
            [dx / length, dy / length]
        } else {
            [1.0, 0.0]
        };
        ([a[0], a[1]], axis, length)
    }

    /// The open parameter interval of segment `pq` inside the donor region
    /// widened by `extra`: the strip across the subspan, and the round caps
    /// at real ends.
    fn interval(&self, p: [f64; 3], q: [f64; 3], extra: f64) -> Option<(f64, f64)> {
        let radius = self.radius + extra - SLACK_MM;
        if radius <= 0.0 {
            return None;
        }
        let (origin, axis, length) = self.frame();
        let local = |point: [f64; 3]| {
            let (x, y) = (point[0] - origin[0], point[1] - origin[1]);
            [axis[0] * x + axis[1] * y, axis[0] * y - axis[1] * x]
        };
        let (o, e) = (local(p), local(q));
        let c = [e[0] - o[0], e[1] - o[1]];
        let mut result: Option<(f64, f64)> = None;
        let mut include = |span: (f64, f64)| {
            if span.0 < span.1 {
                result = Some(result.map_or(span, |r| (r.0.min(span.0), r.1.max(span.1))));
            }
        };
        // Strip: 0 ≤ u ≤ length, |v| < radius (shrunk by the slack).
        let mut strip = (0.0_f64, 1.0_f64);
        let mut clip = |origin: f64, change: f64, lo: f64, hi: f64| -> bool {
            if change == 0.0 {
                return lo < origin && origin < hi;
            }
            let (t0, t1) = ((lo - origin) / change, (hi - origin) / change);
            strip.0 = strip.0.max(t0.min(t1));
            strip.1 = strip.1.min(t0.max(t1));
            strip.0 < strip.1
        };
        let along = [
            if self.caps[0] { 0.0 } else { SLACK_MM },
            if self.caps[1] {
                length
            } else {
                length - SLACK_MM
            },
        ];
        if clip(o[0], c[0], along[0], along[1]) && clip(o[1], c[1], -radius, radius) {
            include(strip);
        }
        let speed = c[0].hypot(c[1]);
        if speed > 0.0 {
            for (end, centre) in [(0, 0.0), (1, length)] {
                if !self.caps[end] {
                    continue;
                }
                // |o + t c − (centre, 0)| < radius.
                let rel = [o[0] - centre, o[1]];
                let b = (rel[0] * c[0] + rel[1] * c[1]) / (speed * speed);
                let k = (rel[0] * rel[0] + rel[1] * rel[1] - radius * radius) / (speed * speed);
                let disc = b * b - k;
                if disc > 0.0 {
                    let root = disc.sqrt();
                    include(((-b - root).max(0.0), (-b + root).min(1.0)));
                }
            }
        }
        result
    }

    /// Whether the disk of `radius` about `point` overlaps the donor region.
    fn touches(&self, point: [f64; 3], radius: f64) -> bool {
        let (origin, axis, length) = self.frame();
        let (x, y) = (point[0] - origin[0], point[1] - origin[1]);
        let (u, v) = (axis[0] * x + axis[1] * y, (axis[0] * y - axis[1] * x).abs());
        let reach = self.radius - SLACK_MM;
        let mut distance = (u - u.clamp(0.0, length)).hypot((v - reach).max(0.0));
        for (cap, centre) in self.caps.into_iter().zip([0.0, length]) {
            if cap {
                distance = distance.min(((u - centre).hypot(v) - reach).max(0.0));
            }
        }
        distance + SLACK_MM < radius
    }
}

/// What a layer rests on: the established and support cells below, and
/// the support beads as exact donors.
struct Below<'b> {
    cells: &'b Bits,
    support: &'b [Donor],
}

/// Everything one establishment step borrows from its caller.
pub(super) struct Step<'r, 'a, 's> {
    pub raster: &'r Raster<'a>,
    pub layers: &'s [PrintLayer],
    /// Half the recipe bead: the most any bead donates.
    pub allowance: f64,
    /// The recipe, for splitting Bridge rows as they will print.
    pub recipe: &'r PlanarPrintRecipe,
    /// Per layer, its hatch alternation key.
    pub courses: &'r [u32],
    /// The recipe's longest two-ended unsupported bridge span.
    pub bridge_limit: f64,
    /// The highest layer any later query will establish: footprints are
    /// rasterized ahead up to it.
    pub horizon: usize,
    /// The support that layer `l` may rest on: cells wholly inside support
    /// beads, and those beads as exact donors. The sweep passes its kept
    /// cells (no donors) to propose landings; raster retention passes the
    /// support actually deposited, which requalifies them.
    pub support: &'r SupportBelow<'r>,
    pub owned_bytes: usize,
}

/// The support layer `l` may rest on, as cells and exact donors.
pub(super) type SupportBelow<'r> =
    dyn Fn(&Raster<'_>, usize) -> Result<(Bits, Vec<Donor>), ManufacturingError> + Sync + 'r;

/// No support below any layer.
#[cfg(test)]
pub(super) fn no_support(
    _: &Raster<'_>,
    _: usize,
) -> Result<(Bits, Vec<Donor>), ManufacturingError> {
    Ok((Bits::default(), Vec::new()))
}

/// The support beneath layer `l` of `layers`, from actual paths: every
/// support road of layer `l − 1`, and the interface of the contact top
/// below it across the Z gap. Cells lie wholly inside the beads.
pub(super) fn deposited_support(
    raster: &Raster<'_>,
    layers: &[PrintLayer],
    contact_top: Option<usize>,
    l: usize,
    owned_bytes: usize,
) -> Result<(Bits, Vec<Donor>), ManufacturingError> {
    let direct = layers[l - 1]
        .paths
        .iter()
        .filter(|path| support_role(path.kind));
    let separated = contact_top
        .filter(|top| top + 1 < l)
        .into_iter()
        .flat_map(|top| {
            layers[top]
                .paths
                .iter()
                .filter(|path| path.kind == DepositionKind::SupportInterface)
        });
    let paths = direct.chain(separated);
    let mut donors = Vec::new();
    for path in paths.clone() {
        let Ok(edges) = path.checked_edges() else {
            continue;
        };
        for edge in edges {
            reserve_owned(&mut donors, 1, owned_bytes)?;
            donors.push(Donor {
                ends: [edge.points_mm[0], edge.points_mm[1]],
                radius: edge.width_mm * 0.5,
                caps: [true, true],
            });
        }
    }
    Ok((bridge_cells(raster, paths, f64::INFINITY)?, donors))
}

/// One layer's established cells.
#[derive(Default)]
struct Layer {
    /// The chain set: what the layer above stands on.
    chain: Bits,
    /// Established cells wholly under ordinary roads: landings.
    landing: Bits,
    /// The inward footprint of the layer's ordinary roads.
    model: Bits,
    /// The proved centreline subspans of its model roads: the donors the
    /// layer above is proved against.
    proved: Vec<Donor>,
}

impl Layer {
    fn bytes(&self) -> usize {
        self.chain
            .bytes()
            .saturating_add(self.landing.bytes())
            .saturating_add(self.model.bytes())
            .saturating_add(vec_bytes(&self.proved))
    }
}

/// Every edge of `paths` as a whole proved centreline with both caps.
fn whole_edges<'p>(
    paths: impl Iterator<Item = &'p DepositionPath>,
    allowance: f64,
    proved: &mut Vec<Donor>,
    owned_bytes: usize,
) -> Result<(), ManufacturingError> {
    for path in paths {
        let Ok(edges) = path.checked_edges() else {
            continue;
        };
        for edge in edges {
            reserve_owned(proved, 1, owned_bytes)?;
            proved.push(Donor {
                ends: [edge.points_mm[0], edge.points_mm[1]],
                radius: (edge.width_mm * 0.5).min(allowance),
                caps: [true, true],
            });
        }
    }
    Ok(())
}

/// The running established set: layers `[0, next)` are done; `last` holds
/// layer `next − 1`, and `older` the model footprint of `next − 2`, reused
/// when a layer repeats its roads two layers on (alternating infill). Only
/// these are retained; their bytes are held in the caller's live counter.
pub(super) struct Establishment<'l> {
    live: &'l AtomicUsize,
    next: usize,
    last: Layer,
    older: Bits,
    /// Footprints of upcoming layers rasterized ahead in parallel, in layer
    /// order; their bytes are held in the live counter until taken.
    prefetched: std::collections::VecDeque<(usize, Bits)>,
}

impl Drop for Establishment<'_> {
    fn drop(&mut self) {
        self.retain(Layer::default(), Bits::default());
        self.clear_prefetched();
    }
}

impl<'l> Establishment<'l> {
    pub(super) fn new(live: &'l AtomicUsize) -> Self {
        Self {
            live,
            next: 0,
            last: Layer::default(),
            older: Bits::default(),
            prefetched: std::collections::VecDeque::new(),
        }
    }

    fn release(&self, bits: &Bits) {
        self.live.fetch_sub(
            bits.bytes().min(self.live.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
    }

    fn clear_prefetched(&mut self) {
        for (_, bits) in std::mem::take(&mut self.prefetched) {
            self.release(&bits);
        }
    }

    /// Rasterizes, in parallel and in layer order, the footprints of the
    /// next layers up to `target` that do not repeat the roads one or two
    /// layers below, as many as the pool has workers and the live byte
    /// allowance admits at the frame's dense size.
    fn prefetch(
        &mut self,
        step: &Step<'_, '_, '_>,
        target: usize,
    ) -> Result<(), ManufacturingError> {
        use rayon::prelude::*;
        let raster = step.raster;
        let workers = rayon::current_num_threads().max(1);
        let mut wanted = Vec::new();
        let mut l = self.next;
        while l <= target.min(step.layers.len() - 1) && wanted.len() < 2 * workers {
            check_cancel(raster.cancel)?;
            let layer = &step.layers[l];
            let repeats = (l >= 1 && same_ordinary(raster, layer, &step.layers[l - 1]))
                || (l >= 2 && same_ordinary(raster, layer, &step.layers[l - 2]));
            if !repeats {
                reserve_owned(&mut wanted, 1, step.owned_bytes)?;
                wanted.push(l);
            }
            l += 1;
        }
        if wanted.len() < 2 {
            return Ok(());
        }
        // Each concurrent footprint may hold up to a dense frame layer.
        let dense = raster.frame.dense_bytes();
        while wanted.len() > 1 && (raster.admit)(dense.saturating_mul(wanted.len())).is_err() {
            wanted.pop();
        }
        if wanted.len() < 2 {
            return Ok(());
        }
        let working_set = crate::working_set::current();
        let built: Vec<Result<Bits, ManufacturingError>> = wanted
            .par_iter()
            .map(|&l| {
                crate::working_set::scope(working_set, || {
                    footprint(
                        raster,
                        ordinary_roads(&step.layers[l]),
                        step.allowance,
                        step.owned_bytes,
                    )
                })
            })
            .collect();
        for (l, bits) in wanted.into_iter().zip(built) {
            let bits = bits?;
            self.live.fetch_add(bits.bytes(), Ordering::Relaxed);
            self.prefetched.push_back((l, bits));
        }
        Ok(())
    }

    fn take_prefetched(&mut self, l: usize) -> Option<Bits> {
        while self.prefetched.front().is_some_and(|(at, _)| *at < l) {
            let (_, bits) = self.prefetched.pop_front()?;
            self.release(&bits);
        }
        if self.prefetched.front().is_some_and(|(at, _)| *at == l) {
            let (_, bits) = self.prefetched.pop_front()?;
            self.release(&bits);
            return Some(bits);
        }
        None
    }

    fn bytes(&self) -> usize {
        self.last.bytes().saturating_add(self.older.bytes())
    }

    fn retain(&mut self, last: Layer, older: Bits) {
        let old = self.bytes();
        self.live.fetch_sub(
            old.min(self.live.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        self.last = last;
        self.older = older;
        self.live.fetch_add(self.bytes(), Ordering::Relaxed);
    }

    /// The cells of `target` that support may land on: established, and
    /// wholly under its ordinary (non-Bridge) model roads. Advances from the
    /// last layer established; a lower target restarts from the bed.
    pub(super) fn landing(
        &mut self,
        step: &Step<'_, '_, '_>,
        target: usize,
    ) -> Result<Bits, ManufacturingError> {
        if target + 1 < self.next {
            self.next = 0;
            self.retain(Layer::default(), Bits::default());
            self.clear_prefetched();
        }
        while self.next <= target {
            check_cancel(step.raster.cancel)?;
            if self.prefetched.is_empty() {
                self.prefetch(step, step.horizon.max(target))?;
            }
            let precomputed = self.take_prefetched(self.next);
            let layer = self.layer(step, self.next, precomputed)?;
            let older = std::mem::take(&mut self.last.model);
            self.retain(layer, older);
            self.next += 1;
        }
        step.raster.difference(&self.last.landing, &Bits::default())
    }

    /// Layer `l`'s established cells from the retained layer `l − 1`.
    fn layer(
        &self,
        step: &Step<'_, '_, '_>,
        l: usize,
        precomputed: Option<Bits>,
    ) -> Result<Layer, ManufacturingError> {
        let raster = step.raster;
        let t = std::time::Instant::now();
        TRACE[5].fetch_add(1, Ordering::Relaxed);
        // Straight skin rows labelled Bridge are split after support at their
        // genuine gaps, and the supported pieces take back their own role:
        // establish the layer as it will print.
        let layer = &step.layers[l];
        // A layer repeating the ordinary roads of one or two layers below
        // reuses that footprint rather than rasterizing it again.
        let model = if l >= 1 && same_ordinary(raster, layer, &step.layers[l - 1]) {
            raster.difference(&self.last.model, &Bits::default())?
        } else if l >= 2 && same_ordinary(raster, layer, &step.layers[l - 2]) {
            raster.difference(&self.older, &Bits::default())?
        } else if let Some(model) = precomputed {
            model
        } else {
            footprint(
                raster,
                ordinary_roads(layer),
                step.allowance,
                step.owned_bytes,
            )?
        };
        let t = trace(0, t);
        let bridges = || {
            layer
                .paths
                .iter()
                .filter(|path| path.kind == DepositionKind::Bridge)
        };
        let mut proved = Vec::new();
        if l == 0 {
            // The bed holds every first-layer road, a bridge included.
            let chain = raster.union(&model, &bridge_cells(raster, bridges(), step.allowance)?)?;
            let landing = raster.difference(&model, &Bits::default())?;
            whole_edges(
                layer.paths.iter().filter(|path| path.kind.is_model()),
                step.allowance,
                &mut proved,
                step.owned_bytes,
            )?;
            return Ok(Layer {
                chain,
                landing,
                model,
                proved,
            });
        }
        let (support_cells, support) = (step.support)(raster, l)?;
        let lower = &raster.union(&self.last.chain, &support_cells)?;
        drop(support_cells);
        if lower.is_empty() {
            return Ok(Layer {
                model,
                ..Layer::default()
            });
        }
        let below = Below {
            cells: lower,
            support: &support,
        };
        let t = trace(1, t);
        // Model cells off the layer below: only centrelines meeting them need
        // proof.
        let holes = raster.difference(&model, lower)?;
        let t = trace(2, t);
        let landing = if holes.is_empty() {
            whole_edges(
                ordinary_roads(layer),
                step.allowance,
                &mut proved,
                step.owned_bytes,
            )?;
            raster.difference(&model, &Bits::default())?
        } else {
            let (unproved, inside) = self.unproved(step, layer, &below, &holes, &mut proved)?;
            // Cells off every unproved bead, or wholly inside one proved
            // strip or real end cap.
            let kept = raster.difference(&model, &unproved)?;
            raster.union(&kept, &raster.intersection(&model, &inside)?)?
        };
        drop(holes);
        let t = trace(3, t);
        // A bridge chains when it roots as an actual bridge stroke on the
        // layer below: both ends bonded, and every unproved gap along each
        // stroke run within the bridge limit (or a tolerated one-sided tip).
        let mut chained: Vec<&DepositionPath> = Vec::new();
        let mut pieces = Vec::new();
        let mut covered = Vec::new();
        // A straight skin row labelled Bridge is split after support at its
        // genuine gaps (the planner's own split rule, run here against the
        // layer below): the pieces that stay Bridge chain as bridge strokes,
        // and the supported remainders chain only where their centreline is
        // proved and never donate. Any other Bridge chains as a whole stroke.
        let mut split_strips: Vec<Polygon<f64>> = Vec::new();
        let mut split_cells = Bits::default();
        let mut split_anchors = AnchorWorkspace::default();
        let mut split_caps: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
        let mut spans: Vec<(f64, f64)> = Vec::new();
        let diagonal = SUPPORT_CELL_MM * std::f64::consts::SQRT_2;
        for path in bridges() {
            check_cancel(raster.cancel)?;
            let splits = match step.courses.get(l) {
                Some(&course) => super::super::bridge_split::splittable(
                    layer,
                    path,
                    &step.layers[l - 1],
                    step.recipe,
                    course,
                )?
                .is_some(),
                None => true,
            };
            let split = if splits {
                let mut work = 0usize;
                let pieces = super::super::bridge_split::split_path(
                    layer,
                    path,
                    &step.layers[l - 1],
                    step.recipe,
                    step.courses[l],
                    &mut split_anchors,
                    &mut work,
                    raster.cancel,
                )?;
                raster.charge(work);
                pieces
            } else {
                None
            };
            let Some(split) = split else {
                if self.bridge_roots(step, path, &below, &mut pieces, &mut covered)? {
                    reserve_owned(&mut chained, 1, step.owned_bytes)?;
                    chained.push(path);
                }
                continue;
            };
            // The pieces that stay Bridge chain as bridge strokes; the
            // supported remainders chain only where proved and never donate.
            for piece in split
                .iter()
                .filter(|piece| piece.kind == DepositionKind::Bridge)
            {
                if self.bridge_roots(step, piece, &below, &mut pieces, &mut covered)? {
                    whole_edges(
                        std::iter::once(piece),
                        step.allowance,
                        &mut proved,
                        step.owned_bytes,
                    )?;
                    let cells = bridge_cells(raster, std::iter::once(piece), step.allowance)?;
                    split_cells = raster.union(&split_cells, &cells)?;
                }
            }
            for edge in split
                .iter()
                .filter(|piece| piece.kind != DepositionKind::Bridge)
                .filter_map(|piece| piece.checked_edges().ok())
                .flatten()
            {
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                let radius = (edge.width_mm * 0.5).min(step.allowance);
                spans.clear();
                raster.along(lower, lower, [p[0], p[1]], [q[0], q[1]], &mut pieces)?;
                let mut start = 0.0;
                for &(end, inside) in &pieces {
                    if inside {
                        reserve_owned(&mut spans, 1, step.owned_bytes)?;
                        spans.push((start, end));
                    }
                    start = end;
                }
                let near = self.last.proved.iter().chain(below.support);
                let extra = (step.allowance - radius).max(0.0);
                exact_cover(raster, near, p, q, extra, &mut spans, step.owned_bytes)?;
                spans.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
                let mut merged: Option<(f64, f64)> = None;
                let mut record = |span: (f64, f64)| -> Result<(), ManufacturingError> {
                    let at3 = |t: f64| {
                        [
                            p[0] + (q[0] - p[0]) * t,
                            p[1] + (q[1] - p[1]) * t,
                            p[2] + (q[2] - p[2]) * t,
                        ]
                    };
                    let caps = [span.0 <= 0.0, span.1 >= 1.0];
                    reserve_owned(&mut proved, 1, step.owned_bytes)?;
                    proved.push(Donor {
                        ends: [at3(span.0), at3(span.1)],
                        radius,
                        caps,
                    });
                    let (a, b) = (at3(span.0), at3(span.1));
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let length = dx.hypot(dy);
                    if length > 0.0 {
                        let n = [-dy / length * radius, dx / length * radius];
                        reserve_owned(&mut split_strips, 1, step.owned_bytes)?;
                        split_strips.push(Polygon::new(
                            geo::LineString::from(vec![
                                (a[0] + n[0], a[1] + n[1]),
                                (b[0] + n[0], b[1] + n[1]),
                                (b[0] - n[0], b[1] - n[1]),
                                (a[0] - n[0], a[1] - n[1]),
                                (a[0] + n[0], a[1] + n[1]),
                            ]),
                            Vec::new(),
                        ));
                    }
                    if radius > diagonal {
                        for (cap, point) in caps.into_iter().zip([a, b]) {
                            if cap {
                                reserve_owned(&mut split_caps, 1, step.owned_bytes)?;
                                split_caps.push((
                                    [point[0], point[1]],
                                    [point[0], point[1]],
                                    radius - diagonal,
                                ));
                            }
                        }
                    }
                    Ok(())
                };
                for &(start, end) in &spans {
                    match &mut merged {
                        Some(span) if start <= span.1 => span.1 = span.1.max(end),
                        _ => {
                            if let Some(span) = merged.take() {
                                record(span)?;
                            }
                            merged = Some((start, end));
                        }
                    }
                }
                if let Some(span) = merged.filter(|span| span.0 < span.1) {
                    record(span)?;
                }
            }
        }
        whole_edges(
            chained.iter().copied(),
            step.allowance,
            &mut proved,
            step.owned_bytes,
        )?;
        let mut chain = raster.union(
            &landing,
            &bridge_cells(raster, chained.into_iter(), step.allowance)?,
        )?;
        if !split_strips.is_empty() {
            chain = raster.union(&chain, &raster.inward(&split_strips)?)?;
            chain = raster.union(&chain, &raster.capsules(split_caps)?)?;
        }
        chain = raster.union(&chain, &split_cells)?;

        trace(4, t);
        Ok(Layer {
            chain,
            landing,
            model,
            proved,
        })
    }

    /// The cells touched by the bead of every unproved centreline stretch of
    /// layer `l`'s ordinary roads, and the cells wholly inside one proved
    /// stretch's strip or real end cap. A centreline lying wholly in the
    /// cells below is proved by the cells alone; one meeting a `holes` cell
    /// is proved where it lies in those cells or inside the exact region of
    /// a lower proved donor. Proved stretches are recorded as the donors of
    /// the layer above.
    fn unproved(
        &self,
        step: &Step<'_, '_, '_>,
        layer: &PrintLayer,
        below: &Below<'_>,
        holes: &Bits,
        proved_spans: &mut Vec<Donor>,
    ) -> Result<(Bits, Bits), ManufacturingError> {
        let raster = step.raster;
        let lower = below.cells;
        let screen = raster.coarse(holes)?;
        let meets = |rect: [[f64; 2]; 2], by: f64| {
            let rect = [
                [rect[0][0] - by, rect[0][1] - by],
                [rect[1][0] + by, rect[1][1] + by],
            ];
            screen.meets(&raster.frame, rect) && holes.meets_rect(&raster.frame, rect)
        };
        let bounds = |p: [f64; 3], q: [f64; 3]| {
            [
                [p[0].min(q[0]), p[1].min(q[1])],
                [p[0].max(q[0]), p[1].max(q[1])],
            ]
        };
        let diagonal = SUPPORT_CELL_MM * std::f64::consts::SQRT_2;
        // A centreline point lies in its own inward footprint only when the
        // bead is wider than a cell diagonal; thinner beads always take proof.
        let thin = |radius: f64| radius <= diagonal;
        let mut pieces = Vec::new();
        let mut donors: Option<Vec<Donor>> = None;
        let mut anchored: Vec<(f64, f64)> = Vec::new();
        let mut unproved: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
        let mut strips: Vec<Polygon<f64>> = Vec::new();
        let mut caps: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
        for path in ordinary_roads(layer) {
            let Ok(edges) = path.checked_edges() else {
                continue;
            };
            for edge in edges {
                check_cancel(raster.cancel)?;
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                let radius = (edge.width_mm * 0.5).min(step.allowance);
                raster.charge(1);
                if !thin(radius) && !meets(bounds(p, q), 0.0) {
                    reserve_owned(proved_spans, 1, step.owned_bytes)?;
                    proved_spans.push(Donor {
                        ends: [p, q],
                        radius,
                        caps: [true, true],
                    });
                    continue;
                }
                anchored.clear();
                raster.along(lower, lower, [p[0], p[1]], [q[0], q[1]], &mut pieces)?;
                let mut start = 0.0;
                for &(end, inside) in &pieces {
                    if inside {
                        anchored.push((start, end));
                    }
                    start = end;
                }
                if !(anchored.len() == 1 && anchored[0] == (0.0, 1.0)) {
                    // Exact recovery of what cell rounding left, against the
                    // donor regions of lower proved subspans.
                    let donors = match &mut donors {
                        Some(donors) => donors,
                        None => donors.insert(self.donors(step, below, &meets)?),
                    };
                    let extra = (step.allowance - radius).max(0.0);
                    exact_cover(
                        raster,
                        donors.iter(),
                        p,
                        q,
                        extra,
                        &mut anchored,
                        step.owned_bytes,
                    )?;
                }
                anchored.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
                let at = |t: f64| [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
                let at3 = |t: f64| {
                    [
                        p[0] + (q[0] - p[0]) * t,
                        p[1] + (q[1] - p[1]) * t,
                        p[2] + (q[2] - p[2]) * t,
                    ]
                };
                let half = edge.width_mm * 0.5 + FOOTPRINT_JOIN_MM;
                // Merge the anchored intervals; record each proved stretch
                // and each unproved one between them.
                let mut reached = 0.0_f64;
                let mut current: Option<(f64, f64)> = None;
                let mut spans: Vec<(f64, f64)> = Vec::new();
                for &(start, end) in &anchored {
                    match &mut current {
                        Some(span) if start <= span.1 => span.1 = span.1.max(end),
                        _ => {
                            if let Some(span) = current.take() {
                                spans.push(span);
                            }
                            current = Some((start, end));
                        }
                    }
                }
                spans.extend(current);
                for &(start, end) in &spans {
                    if start > reached {
                        reserve_owned(&mut unproved, 1, step.owned_bytes)?;
                        unproved.push((at(reached), at(start), half));
                    }
                    if end > start {
                        let caps_here = [start <= 0.0, end >= 1.0];
                        reserve_owned(proved_spans, 1, step.owned_bytes)?;
                        proved_spans.push(Donor {
                            ends: [at3(start), at3(end)],
                            radius,
                            caps: caps_here,
                        });
                        // Cells wholly inside the flat strip, and inside the
                        // cap at a real road end.
                        let (a, b) = (at(start), at(end));
                        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                        let length = dx.hypot(dy);
                        if length > 0.0 {
                            let n = [-dy / length * radius, dx / length * radius];
                            reserve_owned(&mut strips, 1, step.owned_bytes)?;
                            strips.push(Polygon::new(
                                geo::LineString::from(vec![
                                    (a[0] + n[0], a[1] + n[1]),
                                    (b[0] + n[0], b[1] + n[1]),
                                    (b[0] - n[0], b[1] - n[1]),
                                    (a[0] - n[0], a[1] - n[1]),
                                    (a[0] + n[0], a[1] + n[1]),
                                ]),
                                Vec::new(),
                            ));
                        }
                        if radius > diagonal {
                            for (cap, point) in caps_here.into_iter().zip([a, b]) {
                                if cap {
                                    reserve_owned(&mut caps, 1, step.owned_bytes)?;
                                    caps.push((point, point, radius - diagonal));
                                }
                            }
                        }
                    }
                    reached = reached.max(end);
                }
                if reached < 1.0 {
                    reserve_owned(&mut unproved, 1, step.owned_bytes)?;
                    unproved.push((at(reached), at(1.0), half));
                }
            }
        }
        let inside = raster.union(&raster.inward(&strips)?, &raster.capsules(caps)?)?;
        Ok((raster.capsules(unproved)?, inside))
    }

    /// Whether a Bridge road roots on the layer below as the verifier's
    /// bridge stroke rule demands: per stroke run, centreline coverage from
    /// the cells below or exact donor regions, both run ends bonded, and each
    /// uncovered gap tolerated or a two-ended span within the bridge limit.
    fn bridge_roots(
        &self,
        step: &Step<'_, '_, '_>,
        path: &DepositionPath,
        below: &Below<'_>,
        pieces: &mut Vec<(f64, bool)>,
        covered: &mut Vec<(f64, f64)>,
    ) -> Result<bool, ManufacturingError> {
        let raster = step.raster;
        let lower = below.cells;
        let edges: Vec<_> = match path.checked_edges() {
            Ok(edges) => edges.collect(),
            Err(_) => return Ok(false),
        };
        let Ok(runs) = StrokeRuns::for_path(path) else {
            return Ok(false);
        };
        let tolerance = 2.0 * crate::planner::OFFSET_TOLERANCE_MM;
        let mut spans = Vec::new();
        for (first, last, width) in runs {
            let count = if last > first {
                last - first
            } else {
                edges.len() - first + last
            };
            covered.clear();
            let mut length = 0.0_f64;
            let mut ends = [false; 2];
            for k in 0..count {
                check_cancel(raster.cancel)?;
                let edge = &edges[(first + k) % edges.len()];
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                let edge_length = (q[0] - p[0]).hypot(q[1] - p[1]);
                spans.clear();
                raster.along(lower, lower, [p[0], p[1]], [q[0], q[1]], pieces)?;
                let mut start = 0.0;
                for &(end, inside) in pieces.iter() {
                    if inside {
                        reserve_owned(&mut spans, 1, step.owned_bytes)?;
                        spans.push((start, end));
                    }
                    start = end;
                }
                let near = self
                    .last
                    .proved
                    .iter()
                    .chain(below.support)
                    .filter(|donor| {
                        let d = donor.bounds(donor.radius + edge.width_mm);
                        d[0][0] <= p[0].max(q[0])
                            && p[0].min(q[0]) <= d[1][0]
                            && d[0][1] <= p[1].max(q[1])
                            && p[1].min(q[1]) <= d[1][1]
                    });
                exact_cover(
                    raster,
                    near.clone(),
                    p,
                    q,
                    0.0,
                    &mut spans,
                    step.owned_bytes,
                )?;
                for (end, point) in [(0, p), (1, q)] {
                    if (end == 0 && k == 0) || (end == 1 && k + 1 == count) {
                        ends[end] |= raster.holds_point(lower, [point[0], point[1]])
                            || near
                                .clone()
                                .any(|donor| donor.touches(point, edge.width_mm * 0.5));
                    }
                }
                for &(start, end) in &spans {
                    reserve_owned(covered, 1, step.owned_bytes)?;
                    covered.push((length + start * edge_length, length + end * edge_length));
                }
                length += edge_length;
            }
            covered.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            if !ends[0] && covered.first().is_none_or(|span| span.0 > 0.0)
                || !ends[1] && !covered.iter().any(|span| span.1 >= length)
            {
                return Ok(false);
            }
            let accepts = |span: f64, left: bool, right: bool| {
                (left || right)
                    && (span <= tolerance || (!(left && right) && span <= width * 0.5 + tolerance))
                    || left && right && span <= step.bridge_limit
            };
            let mut reached = 0.0_f64;
            for &(start, end) in covered.iter() {
                if start > reached && !accepts(start - reached, reached > 0.0 || ends[0], true) {
                    return Ok(false);
                }
                reached = reached.max(end);
            }
            if reached < length && !accepts(length - reached, reached > 0.0 || ends[0], ends[1]) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The proved centreline subspans of layer `l − 1` near a hole cell.
    fn donors(
        &self,
        step: &Step<'_, '_, '_>,
        below: &Below<'_>,
        meets: &dyn Fn([[f64; 2]; 2], f64) -> bool,
    ) -> Result<Vec<Donor>, ManufacturingError> {
        let raster = step.raster;
        let mut donors = Vec::new();
        for donor in self.last.proved.iter().chain(below.support) {
            check_cancel(raster.cancel)?;
            raster.charge(1);
            if meets(donor.bounds(0.0), 2.0 * donor.radius + 2.0 * step.allowance) {
                reserve_owned(&mut donors, 1, step.owned_bytes)?;
                donors.push(*donor);
            }
        }
        Ok(donors)
    }
}

/// Appends the parameter intervals of segment `pq` inside each donor's exact
/// region widened by `extra`.
fn exact_cover<'d>(
    raster: &Raster<'_>,
    donors: impl Iterator<Item = &'d Donor>,
    p: [f64; 3],
    q: [f64; 3],
    extra: f64,
    out: &mut Vec<(f64, f64)>,
    owned_bytes: usize,
) -> Result<(), ManufacturingError> {
    let rect = [
        [p[0].min(q[0]), p[1].min(q[1])],
        [p[0].max(q[0]), p[1].max(q[1])],
    ];
    for donor in donors {
        raster.charge(1);
        let d = donor.bounds(donor.radius + extra);
        if d[0][0] > rect[1][0]
            || rect[0][0] > d[1][0]
            || d[0][1] > rect[1][1]
            || rect[0][1] > d[1][1]
        {
            continue;
        }
        if let Some(span) = donor.interval(p, q, extra) {
            reserve_owned(out, 1, owned_bytes)?;
            out.push(span);
        }
    }
    Ok(())
}

/// Cells wholly inside the beads of `paths` (at most `allowance` from the
/// centreline): each bead shrunk by a cell diagonal and rasterized outward,
/// without a union overlay.
fn bridge_cells<'p>(
    raster: &Raster<'_>,
    paths: impl Iterator<Item = &'p DepositionPath>,
    allowance: f64,
) -> Result<Bits, ManufacturingError> {
    let shrink = SUPPORT_CELL_MM * std::f64::consts::SQRT_2;
    raster.capsules(
        paths
            .filter_map(|path| path.checked_edges().ok())
            .flatten()
            .filter(|edge| (edge.width_mm * 0.5).min(allowance) > shrink)
            .map(|edge| {
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                (
                    [p[0], p[1]],
                    [q[0], q[1]],
                    (edge.width_mm * 0.5).min(allowance) - shrink,
                )
            }),
    )
}

fn ordinary_roads(layer: &PrintLayer) -> impl Iterator<Item = &DepositionPath> + Clone {
    layer
        .paths
        .iter()
        .filter(|path| path.kind.is_model() && path.kind != DepositionKind::Bridge)
}

/// Whether two layers' ordinary roads have the same XY centrelines and
/// widths, in order, so their footprints are equal.
fn same_ordinary(raster: &Raster<'_>, a: &PrintLayer, b: &PrintLayer) -> bool {
    let (mut a, mut b) = (ordinary_roads(a), ordinary_roads(b));
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) => {
                raster.charge(p.points_mm.len());
                if p.width != q.width
                    || p.points_mm.len() != q.points_mm.len()
                    || p.points_mm
                        .iter()
                        .zip(&q.points_mm)
                        .any(|(u, v)| u[0] != v[0] || u[1] != v[1])
                {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

/// Cells lying entirely inside the nominal footprint of `paths`: each edge's
/// centreline dilated by half its bead width (at most `allowance`), unioned,
/// then rasterized inward. Edges wholly outside the frame are skipped.
pub(super) fn footprint<'p>(
    raster: &Raster<'_>,
    paths: impl Iterator<Item = &'p DepositionPath>,
    allowance: f64,
    owned_bytes: usize,
) -> Result<Bits, ManufacturingError> {
    let bounds = raster.frame.bounds();
    let mut capsules: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut extent = bounds;
    for path in paths {
        check_cancel(raster.cancel)?;
        let Ok(edges) = path.checked_edges() else {
            continue;
        };
        for edge in edges {
            let &[p, q] = edge.points_mm else {
                continue;
            };
            let half = (edge.width_mm * 0.5).min(allowance) + FOOTPRINT_JOIN_MM;
            let (a, b) = ([p[0], p[1]], [q[0], q[1]]);
            let reach = [
                [a[0].min(b[0]) - half, a[1].min(b[1]) - half],
                [a[0].max(b[0]) + half, a[1].max(b[1]) + half],
            ];
            raster.charge(1);
            if !rects_meet(reach, bounds) || half.is_nan() || half <= 0.0 {
                continue;
            }
            extent = union_frame(Some(extent), reach);
            reserve_owned(&mut capsules, 1, owned_bytes)?;
            capsules.push(inscribed_capsule(a, b, half));
        }
    }
    if capsules.is_empty() {
        return Ok(Bits::default());
    }
    use i_overlay::core::overlay::ShapeType;
    use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
    use i_overlay::float::overlay::FloatOverlay;
    use i_overlay::i_float::{adapter::FloatPointAdapter, float::rect::FloatRect};
    let points: usize = capsules.iter().map(Vec::len).sum();
    (raster.admit)(points.saturating_mul(64))?;
    let adapter = FloatPointAdapter::new(FloatRect {
        min_x: extent[0][0],
        min_y: extent[0][1],
        max_x: extent[1][0],
        max_y: extent[1][1],
    });
    let shapes = FloatOverlay::with_adapter(adapter, points)
        .unsafe_add_source(&capsules, ShapeType::Subject)
        .overlay(OverlayRule::Subject, FillRule::NonZero);
    drop(capsules);
    raster.charge(points);
    let union: Vec<Polygon<f64>> = shapes
        .into_iter()
        .map(|shape| {
            let mut rings = shape.into_iter().map(|ring| {
                let mut line = geo::LineString(ring.into_iter().map(geo::Coord::from).collect());
                line.close();
                line
            });
            Polygon::new(
                rings.next().unwrap_or_else(geo::LineString::empty),
                rings.collect(),
            )
        })
        .collect();
    raster.inward(&union)
}

fn rects_meet(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

/// A polygon inside the round capsule of `ab` and `radius`: the straight
/// sides and caps whose vertices lie on the circles.
fn inscribed_capsule(a: [f64; 2], b: [f64; 2], radius: f64) -> Vec<[f64; 2]> {
    const STEPS: usize = 8;
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx.hypot(dy);
    let angle = if length > 0.0 { dy.atan2(dx) } else { 0.0 };
    let mut points = Vec::with_capacity(2 * STEPS + 2);
    // Around b from +normal through +direction to −normal, then around a.
    for (centre, start) in [
        (b, angle + std::f64::consts::FRAC_PI_2),
        (a, angle - std::f64::consts::FRAC_PI_2),
    ] {
        for i in 0..=STEPS {
            let t = start - std::f64::consts::PI * i as f64 / STEPS as f64;
            points.push([centre[0] + radius * t.cos(), centre[1] + radius * t.sin()]);
        }
    }
    points
}

/// Support anchoring on established part cells: refines the gaps a stroke
/// run left on deposited support by the run's centreline pieces inside
/// `cells`. A gap end bounded by established cells is a contact end.
pub(super) struct CellContacts<'r, 'a> {
    pub raster: &'r Raster<'a>,
    pub cells: &'r Bits,
}

impl CellContacts<'_, '_> {
    /// The sub-gaps of `gap` (parameters along `ab`) outside the cells, as
    /// `(start, end, left, right, direct_ends)`, given the run's pieces.
    pub(super) fn refine(
        pieces: &[(f64, bool)],
        gap: &super::anchors::AnchorGap,
        mut visit: impl FnMut(f64, f64, bool, bool, bool) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        let mut start = 0.0_f64;
        for &(end, inside) in pieces {
            let (lo, hi) = (start.max(gap.start), end.min(gap.end));
            start = end;
            // A degenerate gap is judged as it was, once.
            if inside || lo > hi || (lo == hi && (gap.start < gap.end || end < gap.end)) {
                continue;
            }
            let left_original = lo <= gap.start;
            let right_original = hi >= gap.end;
            visit(
                lo,
                hi,
                if left_original { gap.left } else { true },
                if right_original { gap.right } else { true },
                left_original && right_original && gap.direct_ends,
            )?;
        }
        Ok(())
    }
}

/// The union of the sweep's established landing cells of every part root
/// first supported at `index`. With `current` (layers below `index` and the
/// live bytes beside them), each is first met with the inward footprint of
/// its object layer's present ordinary roads, so a road regenerated into a
/// Bridge or removed no longer donates.
pub(super) fn root_contacts(
    raster: &Raster<'_>,
    roots: &[Root],
    contacts: &[Option<Bits>],
    index: usize,
    current: Option<(&[PrintLayer], f64, usize)>,
) -> Result<Bits, ManufacturingError> {
    let mut union = Bits::default();
    for (root, cells) in roots.iter().zip(contacts) {
        check_cancel(raster.cancel)?;
        let (
            Root::Part {
                object_layer,
                first_support_layer,
                ..
            },
            Some(cells),
        ) = (*root, cells)
        else {
            continue;
        };
        if first_support_layer != index {
            continue;
        }
        let cells = match current {
            Some((layers, allowance, owned_bytes)) => {
                let Some(layer) = layers.get(object_layer) else {
                    continue;
                };
                let ordinary = footprint(
                    raster,
                    layer
                        .paths
                        .iter()
                        .filter(|path| path.kind.is_model() && path.kind != DepositionKind::Bridge),
                    allowance,
                    owned_bytes,
                )?;
                std::borrow::Cow::Owned(raster.intersection(cells, &ordinary)?)
            }
            None => std::borrow::Cow::Borrowed(cells),
        };
        union = raster.union(&union, &cells)?;
    }
    Ok(union)
}

#[cfg(test)]
mod tests {
    use super::super::grid::{CellFrame, SUPPORT_CELL_MM};
    use super::*;

    const WIDTH: f64 = 0.6;

    fn road(kind: DepositionKind, x: [f64; 2], y: f64, layer: usize) -> DepositionPath {
        let z = (layer + 1) as f64 * 0.2;
        DepositionPath {
            material_domain: None,
            kind,
            width: WidthProfile::Constant { width_mm: WIDTH },
            physical_z_mm: [layer as f64 * 0.2, z],
            owner: (kind == DepositionKind::Perimeter).then_some(DepositionOwner::Wall),
            points_mm: vec![[x[0], y, z], [x[1], y, z]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }
    }

    fn layers(paths: Vec<Vec<DepositionPath>>) -> Vec<PrintLayer> {
        paths
            .into_iter()
            .enumerate()
            .map(|(index, paths)| PrintLayer {
                index: index as u32,
                hatch_course: index as u32,
                z_mm: (index + 1) as f64 * 0.2,
                lower_z_mm: index as f64 * 0.2,
                height_mm: 0.2,
                section_z_mm: (index as f64 + 0.5) * 0.2,
                paths,
                region_coverage: super::super::super::empty_coverage(1.0),
                skin_command_fits: Vec::new(),
            })
            .collect()
    }

    fn raster(cancel: &AtomicBool) -> Raster<'_> {
        static ADMIT: fn(usize) -> Result<(), ManufacturingError> = |_| Ok(());
        Raster {
            frame: CellFrame::new([[-2.0, -2.0], [26.0, 3.0]], SUPPORT_CELL_MM).unwrap(),
            cancel,
            admit: &ADMIT,
            work: AtomicUsize::new(0),
        }
    }

    /// The landing cells of `target`, established bottom-up from the bed.
    fn landing(layers: &[PrintLayer], raster: &Raster<'_>, target: usize) -> Bits {
        let live = AtomicUsize::new(0);
        let mut establishment = Establishment::new(&live);
        let step = Step {
            raster,
            layers,
            allowance: crate::planner::tests::recipe(1.0).bead_width_mm * 0.5,
            bridge_limit: crate::planner::tests::recipe(1.0)
                .process
                .max_bridge_span_mm,
            horizon: target,
            recipe: &crate::planner::tests::recipe(1.0),
            courses: &spiling_contracts::manufacturing::hatch_alternation_keys(layers),
            support: &no_support,
            owned_bytes: 0,
        };
        establishment.landing(&step, target).unwrap()
    }

    /// Interface strokes at layer `first` that survive anchoring on the part
    /// root at `object`.
    fn surviving(
        layers: &[PrintLayer],
        object: usize,
        first: usize,
        mut strokes: Vec<DepositionPath>,
    ) -> Vec<DepositionPath> {
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        let recipe = crate::planner::tests::recipe(1.0);
        let mut anchors = AnchorWorkspace::default();
        anchors.set_budget(usize::MAX);
        let roots = [Root::Part {
            object_layer: object,
            first_support_layer: first,
            landing: 0,
        }];
        // The sweep's landing cells, as raster retention receives them.
        let landing = Some(landing(&layers[..first], &raster, object));
        let cells =
            root_contacts(&raster, &roots, std::slice::from_ref(&landing), first, None).unwrap();
        let contacts = CellContacts {
            raster: &raster,
            cells: &cells,
        };
        super::super::raster::retain_anchored(
            &mut strokes,
            0,
            &mut strokes::LowerStrokes::new(
                layers[first - 1]
                    .paths
                    .iter()
                    .filter(|path| support_role(path.kind)),
                std::iter::empty(),
            ),
            Some(&contacts),
            &recipe,
            0,
            0,
            &mut anchors,
            &mut 0,
            &cancel,
        )
        .unwrap();
        strokes
    }

    #[test]
    fn one_short_skin_gap_removes_only_the_cells_over_it() {
        use DepositionKind::*;
        // A long bottom-skin road crosses one short unsupported gap between
        // two bed roads; the part above it stays established on both sides.
        let layers = layers(vec![
            vec![
                road(Perimeter, [0.0, 10.0], 0.0, 0),
                road(Perimeter, [13.0, 23.0], 0.0, 0),
            ],
            vec![road(BottomSkin, [0.0, 23.0], 0.0, 1)],
            vec![road(SolidFill, [0.0, 23.0], 0.0, 2)],
            vec![road(TopSkin, [0.0, 23.0], 0.0, 3)],
            Vec::new(),
            Vec::new(),
        ]);
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        let skin = landing(&layers, &raster, 1);
        assert!(raster.holds_point(&skin, [5.0, 0.0]));
        assert!(raster.holds_point(&skin, [18.0, 0.0]));
        assert!(!raster.holds_point(&skin, [11.5, 0.0]));
        let top = landing(&layers, &raster, 3);
        assert!(raster.holds_point(&top, [2.0, 0.0]));
        assert!(raster.holds_point(&top, [21.0, 0.0]));
        assert!(!raster.holds_point(&top, [11.5, 0.0]));
        // Support standing on either side of the gap survives.
        let strokes = vec![
            road(SupportInterface, [1.0, 9.0], 0.0, 5),
            road(SupportInterface, [14.0, 22.0], 0.0, 5),
        ];
        assert_eq!(surviving(&layers, 3, 5, strokes.clone()), strokes);
    }

    #[test]
    fn landings_over_a_bridge_or_a_permitted_sparse_span_are_refused() {
        use DepositionKind::*;
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        for span in [Bridge, SparseFill] {
            let layers = layers(vec![
                vec![
                    road(Perimeter, [0.0, 10.0], 0.0, 0),
                    road(Perimeter, [14.0, 24.0], 0.0, 0),
                ],
                vec![road(span, [0.0, 24.0], 0.0, 1)],
                vec![road(SolidFill, [0.0, 24.0], 0.0, 2)],
                Vec::new(),
            ]);
            let over = landing(&layers, &raster, 1);
            assert!(!raster.holds_point(&over, [12.0, 0.0]), "{span:?}");
            let stroke = vec![road(SupportInterface, [11.0, 13.0], 0.0, 3)];
            assert!(surviving(&layers, 1, 3, stroke).is_empty(), "{span:?}");
            // A bridge held at both ends chains the layer above it; a
            // permitted sparse span does not.
            let above = landing(&layers, &raster, 2);
            assert_eq!(
                raster.holds_point(&above, [12.0, 0.0]),
                span == Bridge,
                "{span:?}"
            );
        }
    }

    #[test]
    fn a_road_past_its_anchor_is_established_only_within_the_allowance() {
        use DepositionKind::*;
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        let layers = layers(vec![
            vec![road(Perimeter, [0.0, 10.0], 0.0, 0)],
            vec![road(Perimeter, [0.0, 12.0], 0.0, 1)],
        ]);
        let cells = landing(&layers, &raster, 1);
        // The lower bead ends at 10.3, so the upper centreline is proved to
        // there; a cut inside a road keeps no cap, so nothing past it holds.
        assert!(raster.holds_point(&cells, [10.2, 0.0]));
        assert!(!raster.holds_point(&cells, [10.4, 0.0]));
        assert!(!raster.holds_point(&cells, [10.7, 0.0]));
        assert!(!raster.holds_point(&cells, [11.5, 0.0]));
    }

    #[test]
    fn a_road_beside_its_lower_bead_is_not_established() {
        use DepositionKind::*;
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        // Each upper bead overlaps or nearly meets the lower one; only the
        // one whose centreline lies on the lower bead is established.
        for (offset, anchored) in [(0.75, false), (0.5, false), (0.25, true)] {
            let layers = layers(vec![
                vec![road(Perimeter, [0.0, 10.0], 0.0, 0)],
                vec![road(Perimeter, [0.0, 10.0], offset, 1)],
            ]);
            assert_eq!(
                !landing(&layers, &raster, 1).is_empty(),
                anchored,
                "{offset}"
            );
        }
    }

    #[test]
    fn one_anchor_at_an_edge_end_witnesses_only_cells_near_it() {
        use DepositionKind::*;
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        // The long upper edge is anchored only over the short road at its
        // left end. A parallel lower road runs beside it, its footprint
        // within the allowance of the upper bead's edge, but the upper
        // centreline never reaches it.
        let layers = layers(vec![
            vec![
                road(Perimeter, [0.0, 1.0], 0.0, 0),
                road(Perimeter, [0.0, 10.0], -0.75, 0),
            ],
            vec![road(Perimeter, [0.0, 10.0], 0.0, 1)],
        ]);
        let cells = landing(&layers, &raster, 1);
        assert!(raster.holds_point(&cells, [0.5, 0.0]));
        assert!(raster.holds_point(&cells, [1.2, 0.0]));
        assert!(!raster.holds_point(&cells, [1.4, 0.0]));
        // Beside the parallel road, far from the anchor: not established.
        assert!(!raster.holds_point(&cells, [8.0, -0.25]));
        assert!(!raster.holds_point(&cells, [5.0, -0.25]));
        assert!(!raster.holds_point(&cells, [8.0, 0.0]));
    }

    #[test]
    fn support_beyond_the_proved_subspans_of_raw_donors_is_refused() {
        use DepositionKind::*;
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        // Part 26 L25 in small: top-skin donors run along Y from a short bed
        // anchor, so only their first stretch is proved. Support crossing
        // their raw chords just beyond the proved beads is not on
        // established ground; support over the proved stretch is.
        let skins = |layer| {
            [4.8, 5.0, 5.2]
                .map(|x| DepositionPath {
                    points_mm: vec![[x, 0.0, 0.4], [x, 2.5, 0.4]],
                    ..road(TopSkin, [0.0, 0.0], 0.0, layer)
                })
                .to_vec()
        };
        let layers = layers(vec![
            vec![DepositionPath {
                points_mm: vec![[5.0, 0.0, 0.2], [5.0, 1.0, 0.2]],
                ..road(Perimeter, [0.0, 0.0], 0.0, 0)
            }],
            skins(1),
            Vec::new(),
            Vec::new(),
        ]);
        let cells = landing(&layers, &raster, 1);
        // The proved centreline ends at 1.3 (inside the bed bead); its bead
        // at 1.6.
        assert!(raster.holds_point(&cells, [5.0, 1.2]));
        assert!(!raster.holds_point(&cells, [5.0, 1.7]));
        let crossing = |y: f64| vec![road(SupportInterface, [4.2, 5.8], y, 3)];
        assert!(surviving(&layers, 1, 3, crossing(1.68)).is_empty());
        assert!(surviving(&layers, 1, 3, crossing(2.2)).is_empty());
        assert_eq!(surviving(&layers, 1, 3, crossing(0.8)).len(), 1);
        // Part 19 L41 in small: a closed loop over raw top skin with no
        // proved interval on any of its edges.
        let loop_ = vec![DepositionPath {
            points_mm: vec![
                [4.6, 1.8, 0.8],
                [5.4, 1.8, 0.8],
                [5.4, 2.4, 0.8],
                [4.6, 2.4, 0.8],
                [4.6, 1.8, 0.8],
            ],
            ..road(SupportInterface, [0.0, 0.0], 0.0, 3)
        }];
        assert!(surviving(&layers, 1, 3, loop_).is_empty());
    }

    #[test]
    fn part_over_kept_support_or_an_overlong_bridge_is_not_landing_ground() {
        use DepositionKind::*;
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        // Part 19 L41 in small: part material standing on support below is
        // never established here, since that support is not yet deposited.
        let stacked = layers(vec![
            vec![road(SupportInterface, [0.0, 10.0], 0.0, 0)],
            vec![road(Perimeter, [0.0, 10.0], 0.0, 1)],
            vec![road(SolidFill, [0.0, 10.0], 0.0, 2)],
        ]);
        assert!(landing(&stacked, &raster, 2).is_empty());
        // A bridge whose unsupported middle exceeds the bridge limit (6 mm
        // here) does not chain the layer above it, however its ends stand;
        // one within the limit does.
        for (gap, chains) in [(8.0, false), (4.0, true)] {
            let bridged = layers(vec![
                vec![
                    road(Perimeter, [0.0, 2.0], 0.0, 0),
                    road(Perimeter, [2.0 + gap, 4.0 + gap], 0.0, 0),
                ],
                vec![road(Bridge, [0.5, 3.5 + gap], 0.0, 1)],
                vec![road(SolidFill, [0.5, 3.5 + gap], 0.0, 2)],
            ]);
            let above = landing(&bridged, &raster, 2);
            assert_eq!(
                raster.holds_point(&above, [2.0 + gap / 2.0, 0.0]),
                chains,
                "{gap}"
            );
        }
    }

    #[test]
    fn a_splittable_skin_bridge_chains_its_proved_stretches_and_genuine_gap() {
        use DepositionKind::*;
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        // Part 26 L20 in small: a straight top-skin row labelled Bridge is
        // split after support, and its supported remainders become top skin
        // again, so they chain only where their centreline is proved.
        let skin_bridge = DepositionPath {
            owner: Some(DepositionOwner::MandatorySkin {
                skin: ProcessSkinKind::TopSkin,
            }),
            ..road(Bridge, [0.5, 7.5], 0.0, 1)
        };
        let layers = layers(vec![
            vec![
                road(Perimeter, [0.0, 2.0], 0.0, 0),
                road(Perimeter, [6.0, 8.0], 0.0, 0),
            ],
            vec![skin_bridge],
            vec![road(SolidFill, [0.5, 7.5], 0.0, 2)],
        ]);
        // The proved stretches chain, and so does the genuine two-ended gap
        // that stays Bridge after the split; the row itself never donates.
        let above = landing(&layers, &raster, 2);
        assert!(raster.holds_point(&above, [1.0, 0.0]));
        assert!(raster.holds_point(&above, [4.0, 0.0]));
        assert!(landing(&layers, &raster, 1).is_empty());
    }
}
