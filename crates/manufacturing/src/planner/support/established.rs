// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Established part material on the support grid, built bottom-up.
//!
//! Support may stand on the part only where the part is itself held up: by
//! the bed, by deposited support, or by established material below. That is
//! decided per centreline interval and stored as cells, never per road, so
//! one local gap removes only the material over it:
//!
//! - every first-layer road is established whole (the bed holds it);
//! - above it, with `L = E[l − 1] ∪ S[l − 1] ∪ S[contact top]` (`S` is the
//!   support kept on those layers), the proved part of an ordinary road's
//!   centreline is where it lies over `L`'s cells, or where an exact capsule
//!   interval reaches the established centreline subspans of a lower road.
//!   Its established material is the bead of that proved centreline: the
//!   cells of the layer's inward model footprint that no unproved
//!   centreline's bead touches. Nothing is granted from a whole donor road,
//!   a whole receiving edge, or a dilation;
//! - an explicit Bridge road joins the chain only when both of its ends are
//!   proved: a printed bridge holds up the layer above it, but it never
//!   donates a landing itself.
//!
//! Only roads whose centreline meets a model cell off `L` take the exact
//! interval work; the rest are proved by the cell screen alone. The set is
//! computed once, layer by layer, in the sweep's up pass as each layer's
//! support commits; its landing cells per part root are what raster
//! retention and final reconciliation anchor interface on. These cells are
//! producer evidence for choosing landings, not proof; the verifier keeps its
//! own ancestry.

use super::grid::{Bits, Raster, SUPPORT_CELL_MM};
use super::*;
use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Adjacent roads planned tangent (row spacing equal to bead width) share
/// only a line; inflating each footprint by this much joins them so the
/// shared line is not mistaken for a boundary. It is far below a grid cell.
const FOOTPRINT_JOIN_MM: f64 = 1.0e-3;

/// An established lower centreline subspan and its bead width.
type Donor = ([[f64; 3]; 2], f64);

/// The support kept at one lower layer, as cells.
pub(super) type LowerSupport<'s> = Cow<'s, Bits>;

/// Everything one establishment step borrows from its caller.
pub(super) struct Step<'r, 'a, 's> {
    pub raster: &'r Raster<'a>,
    pub layers: &'s [PrintLayer],
    pub contact_tops: &'r [Option<usize>],
    pub anchors: &'r mut AnchorWorkspace,
    pub work: &'r mut usize,
    pub owned_bytes: usize,
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

/// Every edge of `paths` as a whole proved centreline.
fn whole_edges<'p>(
    paths: impl Iterator<Item = &'p DepositionPath>,
    proved: &mut Vec<Donor>,
    owned_bytes: usize,
) -> Result<(), ManufacturingError> {
    for path in paths {
        let Ok(edges) = path.checked_edges() else {
            continue;
        };
        for edge in edges {
            reserve_owned(proved, 1, owned_bytes)?;
            proved.push(([edge.points_mm[0], edge.points_mm[1]], edge.width_mm));
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
}

impl Drop for Establishment<'_> {
    fn drop(&mut self) {
        self.retain(Layer::default(), Bits::default());
    }
}

impl<'l> Establishment<'l> {
    pub(super) fn new(live: &'l AtomicUsize) -> Self {
        Self {
            live,
            next: 0,
            last: Layer::default(),
            older: Bits::default(),
        }
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
    pub(super) fn landing<'s>(
        &mut self,
        step: &mut Step<'_, '_, 's>,
        target: usize,
        support: &mut dyn FnMut(usize) -> LowerSupport<'s>,
    ) -> Result<Bits, ManufacturingError> {
        if target + 1 < self.next {
            self.next = 0;
            self.retain(Layer::default(), Bits::default());
        }
        while self.next <= target {
            check_cancel(step.raster.cancel)?;
            let layer = self.layer(step, self.next, support)?;
            let older = std::mem::take(&mut self.last.model);
            self.retain(layer, older);
            self.next += 1;
        }
        step.raster.difference(&self.last.landing, &Bits::default())
    }

    /// Layer `l`'s established cells from the retained layer `l − 1`.
    fn layer<'s>(
        &self,
        step: &mut Step<'_, '_, 's>,
        l: usize,
        support: &mut dyn FnMut(usize) -> LowerSupport<'s>,
    ) -> Result<Layer, ManufacturingError> {
        let raster = step.raster;
        let layer = &step.layers[l];
        // A layer repeating the ordinary roads of one or two layers below
        // reuses that footprint rather than rasterizing it again.
        let model = if l >= 1 && same_ordinary(raster, layer, &step.layers[l - 1]) {
            raster.difference(&self.last.model, &Bits::default())?
        } else if l >= 2 && same_ordinary(raster, layer, &step.layers[l - 2]) {
            raster.difference(&self.older, &Bits::default())?
        } else {
            footprint(raster, ordinary_roads(layer), step.owned_bytes)?
        };
        let bridges = || {
            layer
                .paths
                .iter()
                .filter(|path| path.kind == DepositionKind::Bridge)
        };
        let mut proved = Vec::new();
        if l == 0 {
            // The bed holds every first-layer road, a bridge included.
            let chain = raster.union(&model, &bridge_cells(raster, bridges())?)?;
            let landing = raster.difference(&model, &Bits::default())?;
            whole_edges(
                layer.paths.iter().filter(|path| path.kind.is_model()),
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
        let below = support(l - 1);
        let contact = match step.contact_tops.get(l).copied().flatten() {
            Some(top) if top + 1 < l => Some(support(top)),
            _ => None,
        };
        let mut lower = raster.union(&self.last.chain, &below)?;
        if let Some(contact) = &contact {
            lower = raster.union(&lower, contact)?;
        }
        if lower.is_empty() {
            return Ok(Layer {
                model,
                ..Layer::default()
            });
        }
        // Model cells off `L`: only centrelines meeting them need proof.
        let holes = raster.difference(&model, &lower)?;
        let landing = if holes.is_empty() {
            whole_edges(ordinary_roads(layer), &mut proved, step.owned_bytes)?;
            raster.difference(&model, &Bits::default())?
        } else {
            let (unproved, inside) = self.unproved(step, l, &lower, &holes, &mut proved)?;
            // Cells off every unproved bead, or wholly inside one proved bead.
            let kept = raster.difference(&model, &unproved)?;
            raster.union(&kept, &raster.intersection(&model, &inside)?)?
        };
        drop(holes);
        // A bridge chains when both its ends are proved: in `L`'s cells, or
        // by exact endpoint or interval contact with a proved lower subspan.
        let mut chained: Vec<&DepositionPath> = Vec::new();
        for path in bridges() {
            check_cancel(raster.cancel)?;
            let Ok(edges) = path.checked_edges() else {
                continue;
            };
            let (Some(first), Some(last)) = (edges.clone().next(), edges.clone().next_back())
            else {
                continue;
            };
            raster.charge(1);
            let mut ends = [false; 2];
            for (end, edge, at) in [(0, &first, 0), (1, &last, 1)] {
                let point = edge.points_mm[at];
                if raster.holds_point(&lower, [point[0], point[1]]) {
                    ends[end] = true;
                    continue;
                }
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                let gaps = step.anchors.gaps_edges(
                    AnchorCandidate {
                        endpoints_mm: [p, q],
                        bead_width_mm: edge.width_mm,
                    },
                    self.last
                        .proved
                        .iter()
                        .filter(|(donor, width)| {
                            let by = edge.width_mm + width;
                            donor[0][0].min(donor[1][0]) <= point[0] + by
                                && point[0] - by <= donor[0][0].max(donor[1][0])
                                && donor[0][1].min(donor[1][1]) <= point[1] + by
                                && point[1] - by <= donor[0][1].max(donor[1][1])
                        })
                        .map(|&(donor, width)| (Ok(donor), true, width)),
                    step.work,
                    raster.cancel,
                )?;
                ends[end] = if at == 0 {
                    gaps.first().is_none_or(|gap| gap.start > 0.0 || gap.left)
                } else {
                    gaps.last().is_none_or(|gap| gap.end < 1.0 || gap.right)
                };
            }
            if ends == [true, true] {
                reserve_owned(&mut chained, 1, step.owned_bytes)?;
                chained.push(path);
            }
        }
        whole_edges(chained.iter().copied(), &mut proved, step.owned_bytes)?;
        let chain = raster.union(&landing, &bridge_cells(raster, chained.into_iter())?)?;
        Ok(Layer {
            chain,
            landing,
            model,
            proved,
        })
    }

    /// The cells touched by the bead of every unproved centreline portion of
    /// layer `l`'s ordinary roads, and the cells wholly inside one proved
    /// portion's bead of the same roads. A centreline lying wholly in `L`'s cells
    /// is proved by the cells alone; one meeting a `holes` cell is proved
    /// where it lies in `L`'s cells or where an exact capsule interval meets
    /// the established centreline subspans of a lower road.
    fn unproved(
        &self,
        step: &mut Step<'_, '_, '_>,
        l: usize,
        lower: &Bits,
        holes: &Bits,
        proved_spans: &mut Vec<Donor>,
    ) -> Result<(Bits, Bits), ManufacturingError> {
        let raster = step.raster;
        let layers = step.layers;
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
        // A centreline point lies in its own inward footprint only when the
        // bead is wider than a cell diagonal; thinner beads always take proof.
        let thin = |width: f64| width * 0.5 <= SUPPORT_CELL_MM * std::f64::consts::SQRT_2;
        let mut pieces = Vec::new();
        let mut donors: Option<Vec<Donor>> = None;
        let mut anchored: Vec<(f64, f64)> = Vec::new();
        let mut unproved: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
        let mut proved: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
        let diagonal = SUPPORT_CELL_MM * std::f64::consts::SQRT_2;
        for path in ordinary_roads(&layers[l]) {
            let Ok(edges) = path.checked_edges() else {
                continue;
            };
            for edge in edges {
                check_cancel(raster.cancel)?;
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                raster.charge(1);
                if !thin(edge.width_mm) && !meets(bounds(p, q), 0.0) {
                    reserve_owned(proved_spans, 1, step.owned_bytes)?;
                    proved_spans.push(([p, q], edge.width_mm));
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
                    // Exact recovery of what cell rounding left.
                    let donors = match &mut donors {
                        Some(donors) => donors,
                        None => donors.insert(self.donors(step, &meets)?),
                    };
                    let rect = bounds(p, q);
                    let reach = edge.width_mm;
                    let gaps = step.anchors.gaps_edges(
                        AnchorCandidate {
                            endpoints_mm: [p, q],
                            bead_width_mm: edge.width_mm,
                        },
                        donors
                            .iter()
                            .filter(|(donor, width)| {
                                let d = bounds(donor[0], donor[1]);
                                let by = reach + width;
                                d[0][0] <= rect[1][0] + by
                                    && rect[0][0] <= d[1][0] + by
                                    && d[0][1] <= rect[1][1] + by
                                    && rect[0][1] <= d[1][1] + by
                            })
                            .map(|&(donor, width)| (Ok(donor), true, width)),
                        step.work,
                        raster.cancel,
                    )?;
                    let mut covered = 0.0_f64;
                    for gap in gaps {
                        if gap.start > covered {
                            anchored.push((covered, gap.start));
                        }
                        covered = covered.max(gap.end);
                    }
                    if covered < 1.0 {
                        anchored.push((covered, 1.0));
                    }
                }
                // The unproved complement of the anchored intervals.
                anchored.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
                let at = |t: f64| [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
                let half = edge.width_mm * 0.5 + FOOTPRINT_JOIN_MM;
                let mut reached = 0.0_f64;
                for &(start, end) in anchored.iter().chain([(1.0, 1.0)].iter()) {
                    if start > reached {
                        reserve_owned(&mut unproved, 1, step.owned_bytes)?;
                        unproved.push((at(reached), at(start), half));
                    }
                    if end > start {
                        let at3 = |t: f64| {
                            [
                                p[0] + (q[0] - p[0]) * t,
                                p[1] + (q[1] - p[1]) * t,
                                p[2] + (q[2] - p[2]) * t,
                            ]
                        };
                        reserve_owned(proved_spans, 1, step.owned_bytes)?;
                        proved_spans.push(([at3(start), at3(end)], edge.width_mm));
                        if edge.width_mm * 0.5 > diagonal {
                            reserve_owned(&mut proved, 1, step.owned_bytes)?;
                            proved.push((at(start), at(end), edge.width_mm * 0.5 - diagonal));
                        }
                    }
                    reached = reached.max(end);
                }
                if anchored.is_empty() {
                    // The whole edge, endpoint caps included.
                    reserve_owned(&mut unproved, 1, step.owned_bytes)?;
                    unproved.push((at(0.0), at(1.0), half));
                }
            }
        }
        Ok((raster.capsules(unproved)?, raster.capsules(proved)?))
    }

    /// The proved centreline subspans of layer `l − 1` near a hole cell.
    fn donors(
        &self,
        step: &mut Step<'_, '_, '_>,
        meets: &dyn Fn([[f64; 2]; 2], f64) -> bool,
    ) -> Result<Vec<Donor>, ManufacturingError> {
        let raster = step.raster;
        let mut donors = Vec::new();
        for &([p, q], width) in &self.last.proved {
            check_cancel(raster.cancel)?;
            raster.charge(1);
            let rect = [
                [p[0].min(q[0]), p[1].min(q[1])],
                [p[0].max(q[0]), p[1].max(q[1])],
            ];
            if meets(rect, 2.0 * width) {
                reserve_owned(&mut donors, 1, step.owned_bytes)?;
                donors.push(([p, q], width));
            }
        }
        Ok(donors)
    }
}

/// Cells wholly inside the beads of `paths`: each bead shrunk by a cell
/// diagonal and rasterized outward, without a union overlay.
fn bridge_cells<'p>(
    raster: &Raster<'_>,
    paths: impl Iterator<Item = &'p DepositionPath>,
) -> Result<Bits, ManufacturingError> {
    let shrink = SUPPORT_CELL_MM * std::f64::consts::SQRT_2;
    raster.capsules(
        paths
            .filter_map(|path| path.checked_edges().ok())
            .flatten()
            .filter(|edge| edge.width_mm * 0.5 > shrink)
            .map(|edge| {
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                ([p[0], p[1]], [q[0], q[1]], edge.width_mm * 0.5 - shrink)
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
/// centreline dilated by half its bead width, unioned, then rasterized
/// inward. Edges wholly outside the frame are skipped.
pub(super) fn footprint<'p>(
    raster: &Raster<'_>,
    paths: impl Iterator<Item = &'p DepositionPath>,
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
            let half = edge.width_mm * 0.5 + FOOTPRINT_JOIN_MM;
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
    current: Option<(&[PrintLayer], usize)>,
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
            Some((layers, owned_bytes)) => {
                let Some(layer) = layers.get(object_layer) else {
                    continue;
                };
                let ordinary = footprint(
                    raster,
                    layer
                        .paths
                        .iter()
                        .filter(|path| path.kind.is_model() && path.kind != DepositionKind::Bridge),
                    owned_bytes,
                )?;
                Cow::Owned(raster.intersection(cells, &ordinary)?)
            }
            None => Cow::Borrowed(cells),
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
        let mut anchors = AnchorWorkspace::default();
        let contact_tops = vec![None; layers.len()];
        let mut step = Step {
            raster,
            layers,
            contact_tops: &contact_tops,
            anchors: &mut anchors,
            work: &mut 0,
            owned_bytes: 0,
        };
        establishment
            .landing(&mut step, target, &mut |_| Cow::Owned(Bits::default()))
            .unwrap()
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
        // The lower bead ends at 10.3; the allowance is half a bead (0.3).
        assert!(raster.holds_point(&cells, [10.4, 0.0]));
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
        assert!(raster.holds_point(&cells, [1.4, 0.0]));
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
}
