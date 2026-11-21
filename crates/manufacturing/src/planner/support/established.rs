// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Established part material on the support grid, built bottom-up.
//!
//! Support may stand on the part only where the part is itself held up: by
//! the bed, by deposited support, or by established material below. That is
//! decided per cell, never per road, so one local gap removes only the cells
//! over it:
//!
//! - `E[0]` is the inward footprint of the first layer's model roads;
//! - above it, with `L = E[l − 1] ∪ S[l − 1] ∪ S[contact top]` (`S` is the
//!   deposited support footprint), a model road cell is established when it
//!   lies over `L`, or within the one-sided overhang allowance of `L` and the
//!   road edge over it has an exact capsule anchor on a lower established
//!   road or on deposited support. The dilation alone never establishes;
//! - an explicit Bridge road joins the chain only when both of its ends
//!   stand within the allowance of `L`: a printed bridge holds up the layer
//!   above it, but it never donates a landing itself.
//!
//! These cells are producer evidence for choosing landings, not proof; the
//! verifier keeps its own ancestry.

use super::grid::{Bits, Raster};
use super::*;
use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The one-sided nominal overhang a road may take past the material below
/// it and still be held up: half a bead, as the stroke analyzer tolerates.
pub(super) fn overhang_allowance(recipe: &PlanarPrintRecipe) -> f64 {
    recipe.bead_width_mm * 0.5
}

/// Adjacent roads planned tangent (row spacing equal to bead width) share
/// only a line; inflating each footprint by this much joins them so the
/// shared line is not mistaken for a boundary. It is far below a grid cell.
const FOOTPRINT_JOIN_MM: f64 = 1.0e-3;

/// Deposited support at one lower layer: its inward cells, and its paths
/// when they exist. Before support is drawn (the sweep) only the kept cells
/// exist; a road edge over them is then witnessed on the cells.
pub(super) struct LowerSupport<'s> {
    pub cells: Cow<'s, Bits>,
    pub paths: Vec<&'s DepositionPath>,
}

impl LowerSupport<'_> {
    pub(super) fn none() -> Self {
        Self {
            cells: Cow::Owned(Bits::default()),
            paths: Vec::new(),
        }
    }
}

/// Deposited support at layer `k` from its actual paths.
pub(super) fn deposited<'s>(
    raster: &Raster<'_>,
    layers: &'s [PrintLayer],
    k: usize,
    owned_bytes: usize,
) -> Result<LowerSupport<'s>, ManufacturingError> {
    let mut paths = Vec::new();
    let count = layers[k]
        .paths
        .iter()
        .filter(|path| support_role(path.kind))
        .count();
    reserve_owned(&mut paths, count, owned_bytes)?;
    paths.extend(
        layers[k]
            .paths
            .iter()
            .filter(|path| support_role(path.kind)),
    );
    let cells = footprint(raster, paths.iter().copied(), owned_bytes)?;
    Ok(LowerSupport {
        cells: Cow::Owned(cells),
        paths,
    })
}

/// Everything one establishment step borrows from its caller.
pub(super) struct Step<'r, 'a, 's> {
    pub raster: &'r Raster<'a>,
    pub layers: &'s [PrintLayer],
    pub contact_tops: &'r [Option<usize>],
    pub recipe: &'r PlanarPrintRecipe,
    pub anchors: &'r mut AnchorWorkspace,
    pub work: &'r mut usize,
    pub owned_bytes: usize,
}

/// The running established set: layers `[0, next)` are done and `last`
/// holds the chain set of layer `next − 1`. Only it is retained; its bytes
/// are held in the caller's live counter.
pub(super) struct Establishment<'l> {
    live: &'l AtomicUsize,
    next: usize,
    last: Bits,
}

impl Drop for Establishment<'_> {
    fn drop(&mut self) {
        self.set_last(Bits::default());
    }
}

impl<'l> Establishment<'l> {
    pub(super) fn new(live: &'l AtomicUsize) -> Self {
        Self {
            live,
            next: 0,
            last: Bits::default(),
        }
    }

    /// Bytes the running set retains.
    pub(super) fn bytes(&self) -> usize {
        self.last.bytes()
    }

    fn set_last(&mut self, bits: Bits) {
        let old = self.last.bytes();
        self.live.fetch_sub(
            old.min(self.live.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        self.live.fetch_add(bits.bytes(), Ordering::Relaxed);
        self.last = bits;
    }

    /// The cells of `target` that support may land on: established, and
    /// wholly under its ordinary (non-Bridge) model roads. Advances from the last layer
    /// established; a lower target restarts from the bed.
    pub(super) fn landing<'s>(
        &mut self,
        step: &mut Step<'_, '_, 's>,
        target: usize,
        support: &mut dyn FnMut(&Raster<'_>, usize) -> Result<LowerSupport<'s>, ManufacturingError>,
    ) -> Result<Bits, ManufacturingError> {
        if target + 1 < self.next {
            self.next = 0;
            self.set_last(Bits::default());
        }
        while self.next <= target {
            check_cancel(step.raster.cancel)?;
            let bits = self.layer(step, self.next, support)?;
            self.set_last(bits);
            self.next += 1;
        }
        // Only cells wholly under ordinary roads donate: a bridge chains
        // the layer above it but is itself held only at its ends.
        let ordinary = footprint(
            step.raster,
            step.layers[target]
                .paths
                .iter()
                .filter(|path| path.kind.is_model() && path.kind != DepositionKind::Bridge),
            step.owned_bytes,
        )?;
        step.raster.intersection(&self.last, &ordinary)
    }

    /// The chain set of layer `l` from the retained set of `l − 1`.
    fn layer<'s>(
        &self,
        step: &mut Step<'_, '_, 's>,
        l: usize,
        support: &mut dyn FnMut(&Raster<'_>, usize) -> Result<LowerSupport<'s>, ManufacturingError>,
    ) -> Result<Bits, ManufacturingError> {
        let raster = step.raster;
        let layers = step.layers;
        let layer = &layers[l];
        let ordinary = || {
            layer
                .paths
                .iter()
                .filter(|path| path.kind.is_model() && path.kind != DepositionKind::Bridge)
        };
        let model = footprint(raster, ordinary(), step.owned_bytes)?;
        if l == 0 {
            // The bed holds every first-layer road, a bridge included.
            let bridges = footprint(
                raster,
                layer
                    .paths
                    .iter()
                    .filter(|path| path.kind == DepositionKind::Bridge),
                step.owned_bytes,
            )?;
            return raster.union(&model, &bridges);
        }
        let below = support(raster, l - 1)?;
        let contact = match step.contact_tops.get(l).copied().flatten() {
            Some(top) if top + 1 != l && top < l => Some(support(raster, top)?),
            _ => None,
        };
        let mut lower = raster.union(&self.last, &below.cells)?;
        if let Some(contact) = &contact {
            lower = raster.union(&lower, &contact.cells)?;
        }
        if lower.is_empty() {
            return Ok(Bits::default());
        }
        let allowance = overhang_allowance(step.recipe);
        let reach = raster.dilate_inward(&lower, allowance)?;
        let direct = raster.intersection(&model, &lower)?;
        let mut extra = raster.intersection(&model, &reach)?;
        extra.subtract(&lower);
        drop(model);
        let mut established = direct;
        if !extra.is_empty() {
            let witnessed = self.witnessed(step, l, &extra, &below, contact.as_ref())?;
            established = raster.union(&established, &witnessed)?;
        }
        // A bridge chains when both its ends stand within reach of `lower`.
        for path in layer
            .paths
            .iter()
            .filter(|path| path.kind == DepositionKind::Bridge)
        {
            check_cancel(raster.cancel)?;
            let (Some(first), Some(last)) = (path.points_mm.first(), path.points_mm.last()) else {
                continue;
            };
            raster.charge(1);
            if raster.holds_point(&reach, [first[0], first[1]])
                && raster.holds_point(&reach, [last[0], last[1]])
            {
                let cells = footprint(raster, std::iter::once(path), step.owned_bytes)?;
                established = raster.union(&established, &cells)?;
            }
        }
        Ok(established)
    }

    /// The overhang cells `extra` of layer `l` that lie under a road edge
    /// with an exact capsule anchor on a lower established road or on
    /// deposited support (or, before support paths exist, whose centreline
    /// meets the deposited support cells).
    fn witnessed(
        &self,
        step: &mut Step<'_, '_, '_>,
        l: usize,
        extra: &Bits,
        below: &LowerSupport<'_>,
        contact: Option<&LowerSupport<'_>>,
    ) -> Result<Bits, ManufacturingError> {
        let raster = step.raster;
        let layers = step.layers;
        // Lower roads with any established cell under their own bead.
        let mut donors: Vec<&DepositionPath> = Vec::new();
        for path in layers[l - 1]
            .paths
            .iter()
            .filter(|path| path.kind.is_model())
        {
            check_cancel(raster.cancel)?;
            let Ok(edges) = path.checked_edges() else {
                continue;
            };
            let mut touches = false;
            for edge in edges {
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                let half = edge.width_mm * 0.5;
                let rect = [
                    [p[0].min(q[0]) - half, p[1].min(q[1]) - half],
                    [p[0].max(q[0]) + half, p[1].max(q[1]) + half],
                ];
                raster.charge(1);
                if !self.last.meets_rect(&raster.frame, rect) {
                    continue;
                }
                let cells = raster.capsules([([p[0], p[1]], [q[0], q[1]], half)])?;
                if cells.intersects(&self.last) {
                    touches = true;
                    break;
                }
            }
            if touches {
                reserve_owned(&mut donors, 1, step.owned_bytes)?;
                donors.push(path);
            }
        }
        let previous: Vec<&DepositionPath> = {
            let mut previous = Vec::new();
            reserve_owned(
                &mut previous,
                donors.len() + below.paths.len(),
                step.owned_bytes,
            )?;
            previous.extend(donors.iter().copied().chain(below.paths.iter().copied()));
            previous
        };
        drop(donors);
        let contacts: &[&DepositionPath] = contact.map_or(&[], |contact| &contact.paths);
        let mut lower =
            strokes::LowerStrokes::new(previous.iter().copied(), contacts.iter().copied());
        let cell_witness: Vec<&Bits> = std::iter::once(below)
            .chain(contact)
            .filter(|support| support.paths.is_empty() && !support.cells.is_empty())
            .map(|support| support.cells.as_ref())
            .collect();
        let mut pieces = Vec::new();
        let mut witnessed = Bits::default();
        for path in layers[l]
            .paths
            .iter()
            .filter(|path| path.kind.is_model() && path.kind != DepositionKind::Bridge)
        {
            let Ok(edges) = path.checked_edges() else {
                continue;
            };
            for edge in edges {
                check_cancel(raster.cancel)?;
                let [p, q] = [edge.points_mm[0], edge.points_mm[1]];
                let half = edge.width_mm * 0.5 + FOOTPRINT_JOIN_MM;
                let rect = [
                    [p[0].min(q[0]) - half, p[1].min(q[1]) - half],
                    [p[0].max(q[0]) + half, p[1].max(q[1]) + half],
                ];
                raster.charge(1);
                if !extra.meets_rect(&raster.frame, rect) {
                    continue;
                }
                let cells = raster.capsules([([p[0], p[1]], [q[0], q[1]], half)])?;
                let over = raster.intersection(&cells, extra)?;
                if over.is_empty() {
                    continue;
                }
                let gaps = lower.gaps_supplied_contacts(
                    AnchorCandidate {
                        endpoints_mm: [p, q],
                        bead_width_mm: edge.width_mm,
                    },
                    path.physical_z_mm[0],
                    true,
                    step.anchors,
                    step.work,
                    raster.cancel,
                )?;
                let unanchored = matches!(
                    gaps,
                    [gap] if gap.start <= 0.0 && gap.end >= 1.0 && !gap.left && !gap.right
                );
                let mut anchored = !unanchored;
                for cells in &cell_witness {
                    if anchored {
                        break;
                    }
                    raster.along(cells, cells, [p[0], p[1]], [q[0], q[1]], &mut pieces)?;
                    anchored = pieces.iter().any(|piece| piece.1);
                }
                if anchored {
                    witnessed = raster.union(&witnessed, &over)?;
                }
            }
        }
        Ok(witnessed)
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

/// The established landing cells of every part root first supported at
/// `index`, from the actual support deposited on `step.layers` (the final
/// layers below `index`). Roots are taken in ascending object layer, so the
/// bottom-up establishment only advances.
pub(super) fn part_contacts(
    establishment: &mut Establishment<'_>,
    step: &mut Step<'_, '_, '_>,
    roots: &[Root],
    index: usize,
) -> Result<Bits, ManufacturingError> {
    let mut contacts = Bits::default();
    let owned_bytes = step.owned_bytes;
    let layers = step.layers;
    for root in roots.iter().rev() {
        let Root::Part {
            object_layer,
            first_support_layer,
            ..
        } = *root
        else {
            continue;
        };
        if first_support_layer != index || object_layer >= layers.len() {
            continue;
        }
        let cells = establishment.landing(step, object_layer, &mut |raster, k| {
            deposited(raster, layers, k, owned_bytes)
        })?;
        contacts = step.raster.union(&contacts, &cells)?;
    }
    Ok(contacts)
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
        let recipe = crate::planner::tests::recipe(1.0);
        let live = AtomicUsize::new(0);
        let mut establishment = Establishment::new(&live);
        let mut anchors = AnchorWorkspace::default();
        let contact_tops = vec![None; layers.len()];
        let mut step = Step {
            raster,
            layers,
            contact_tops: &contact_tops,
            recipe: &recipe,
            anchors: &mut anchors,
            work: &mut 0,
            owned_bytes: 0,
        };
        establishment
            .landing(&mut step, target, &mut |raster, k| {
                deposited(raster, layers, k, 0)
            })
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
        let live = AtomicUsize::new(0);
        let mut establishment = Establishment::new(&live);
        let mut anchors = AnchorWorkspace::default();
        anchors.set_budget(usize::MAX);
        let contact_tops = vec![None; layers.len()];
        let roots = [Root::Part {
            object_layer: object,
            first_support_layer: first,
            landing: 0,
        }];
        let cells = part_contacts(
            &mut establishment,
            &mut Step {
                raster: &raster,
                layers: &layers[..first],
                contact_tops: &contact_tops,
                recipe: &recipe,
                anchors: &mut anchors,
                work: &mut 0,
                owned_bytes: 0,
            },
            &roots,
            first,
        )
        .unwrap();
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
    fn the_dilation_alone_never_establishes_an_unanchored_road() {
        use DepositionKind::*;
        let cancel = AtomicBool::new(false);
        let raster = raster(&cancel);
        // Each upper bead's footprint comes within the allowance of the
        // lower one; only the one whose own bead reaches it is established.
        for (offset, anchored) in [(0.75, false), (0.5, true)] {
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
}
