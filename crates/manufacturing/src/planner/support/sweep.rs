// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Layer-major support construction on the support grid.
//!
//! A legacy column is one (demand, root) prism: the demand layer's buffered
//! material, minus all occupancy from the root's first layer up to the
//! contact top, minus the nominal clearance it meets on the way up. The same
//! sets come from two shared passes over the layers:
//!
//! - down: `A[l] = (A[l + 1] ∪ seeds[l]) − occupancy[l] − voids[l]`;
//! - up: `G[l] = A[l] ∩ (G[l − 1] ∪ ground[l]) − nominal[l]`,
//!
//! where ground is the bed at the first layer and each part landing's
//! anchored cells at its first support layer. Both passes run on the
//! conservative cell grid of [`grid`](super::grid): seeds, occupancy, voids
//! and clearance round outward, landings and road footprints inward, so every
//! cell kept is support space under the exact rules. Polygons are produced
//! only at the output, inside the kept cells. Each layer's occupancy, void
//! and nominal clearance is built once and shared by every demand.

use super::grid::{Bits, CellFrame, Escape, Raster, SUPPORT_CELL_MM};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct Sweep<'a> {
    pub layers: &'a [PrintLayer],
    pub sections: &'a [super::super::SectionSpan],
    pub schedule: &'a [super::super::LayerElevation],
    pub recipe: &'a PlanarPrintRecipe,
    pub frame: &'a projection::Frame,
    pub demands: &'a [Demand],
    pub roots: &'a [Root],
    pub landings: &'a [OwnedMaterial],
    pub protected: &'a OwnedMaterial,
    pub nominal: &'a [NominalScope],
    pub gap: f64,
    pub requested: f64,
    pub interface_thickness: f64,
    pub retained: usize,
    pub owned_bytes: usize,
    pub foreign_work: usize,
}

/// One layer's kept support cells and the interface cells among them.
pub(super) struct LayerGrid {
    pub support: Bits,
    pub interface: Bits,
}

impl LayerGrid {
    pub(super) fn bytes(&self) -> usize {
        self.support.bytes().saturating_add(self.interface.bytes())
    }
}

pub(super) struct SweepOutput {
    /// The cell frame every grid below is in; `None` without demand.
    pub frame: Option<CellFrame>,
    /// Per layer, its kept support cells when it has any.
    pub grids: Vec<Option<LayerGrid>>,
    pub contact_tops: Vec<Option<usize>>,
    /// (protected ordinal, layer, grounded relieved support at that layer).
    pub access: Vec<(usize, usize, Rc<OwnedMaterial>)>,
    /// The kept cells final clearance certifies emitted support against.
    pub kept: KeptSupport,
    /// Per first support layer, the common proposal and its original roots.
    pub part_contacts: Vec<Option<PartContact>>,
}

pub(super) struct PartContact {
    pub cells: Bits,
    pub roots: Vec<usize>,
}

impl PartContact {
    pub(super) fn bytes(&self) -> usize {
        self.cells.bytes().saturating_add(vec_bytes(&self.roots))
    }
}

/// What a part landing proposal reads, shared across layer tasks.
struct LandingInputs<'a> {
    roots: &'a [Root],
    /// Per landing, its exposed ledge.
    ledges: Vec<&'a MultiPolygon<f64>>,
    layers: &'a [PrintLayer],
    radius: f64,
    owned_bytes: usize,
}

struct LandingProposal<'a> {
    ledge: &'a MultiPolygon<f64>,
    roads: MultiPolygon<f64>,
}

fn joined_landing(
    raster: &Raster<'_>,
    proposals: &[LandingProposal<'_>],
) -> Result<Bits, ManufacturingError> {
    use geo::algorithm::unary_union;

    let points = proposals.iter().fold(0usize, |total, proposal| {
        total
            .saturating_add(region::point_count(proposal.ledge))
            .saturating_add(region::point_count(&proposal.roads))
    });
    (raster.admit)(points.saturating_mul(128))?;
    raster.charge(points.saturating_mul(points.max(1).ilog2() as usize + 1));
    check_cancel(raster.cancel)?;
    let ledges = unary_union(proposals.iter().map(|proposal| proposal.ledge));
    check_cancel(raster.cancel)?;
    let roads = unary_union(proposals.iter().map(|proposal| &proposal.roads));
    boolean_budget(&ledges, &roads, raster.cancel)?;
    let ledge_points = region::point_count(&ledges);
    let road_points = region::point_count(&roads);
    (raster.admit)(
        points
            .saturating_add(ledge_points)
            .saturating_add(road_points)
            .saturating_mul(128),
    )?;
    raster.charge(ledge_points.saturating_mul(road_points));
    let joined = ledges.intersection(&roads);
    drop((ledges, roads));
    raster.inward(&joined.0)
}

/// What the sweep built clear, for the final clearance assertion: each
/// layer's kept support cells, the height band its exclusions hold for, and
/// the object bead edges each nominal scope excluded. Every point of a kept
/// cell is at least the XY clearance from the occupancy, protected voids
/// (other than the void a relieved channel enters, whose lease is proved
/// separately) and object beads that reach that band, so an emitted bead
/// whose outward cells are all kept is clear by construction.
#[derive(Default)]
pub(in crate::planner) struct KeptSupport {
    frame: Option<CellFrame>,
    /// Ascending by layer.
    layers: Vec<KeptLayer>,
    /// Ascending by section.
    scopes: Vec<NominalRecord>,
}

struct KeptLayer {
    layer: usize,
    band: [f64; 2],
    bits: Bits,
}

/// One nominal scope as the sweep excluded it: its section, the object
/// height band it was applied for, and the exact bead edges outside its
/// material that it excluded, as sorted bit keys.
struct NominalRecord {
    section: usize,
    floor: f64,
    ceiling: f64,
    edges: Vec<[u64; 5]>,
}

fn edge_key(edge: &beads::BeadEdge) -> [u64; 5] {
    [edge.a[0], edge.a[1], edge.b[0], edge.b[1], edge.half].map(f64::to_bits)
}

/// An emitted bead's cells that the kept grid does not certify.
pub(in crate::planner) struct BeadEscape {
    pub cells: usize,
    pub bounds: [[f64; 2]; 2],
}

impl KeptSupport {
    /// Bytes this record retains.
    pub(in crate::planner) fn bytes(&self) -> usize {
        vec_bytes(&self.layers)
            .saturating_add(vec_bytes(&self.scopes))
            .saturating_add(
                self.layers
                    .iter()
                    .map(|layer| layer.bits.bytes())
                    .fold(0, usize::saturating_add),
            )
            .saturating_add(
                self.scopes
                    .iter()
                    .map(|scope| vec_bytes(&scope.edges))
                    .fold(0, usize::saturating_add),
            )
    }

    fn layer(&self, layer: usize) -> Option<&KeptLayer> {
        self.layers
            .binary_search_by_key(&layer, |kept| kept.layer)
            .ok()
            .map(|at| &self.layers[at])
    }

    /// The cells of the closed emitted capsule `segment ⊕ radius` with
    /// vertical extent `band` on `layer` that the kept grid does not
    /// certify, and the cells and words inspected. A bead outside its
    /// layer's certified band, or on a layer without kept cells, escapes
    /// whole.
    pub(in crate::planner) fn escaped(
        &self,
        layer: usize,
        segment: [[f64; 2]; 2],
        radius: f64,
        band: [f64; 2],
    ) -> Result<(Option<BeadEscape>, usize), ManufacturingError> {
        let [a, b] = segment;
        let Some(frame) = &self.frame else {
            let bounds = [
                [
                    (a[0].min(b[0]) - radius).next_down(),
                    (a[1].min(b[1]) - radius).next_down(),
                ],
                [
                    (a[0].max(b[0]) + radius).next_up(),
                    (a[1].max(b[1]) + radius).next_up(),
                ],
            ];
            if !(radius >= 0.0 && bounds.iter().flatten().all(|v| v.is_finite())) {
                return Err(error(
                    ManufacturingErrorCode::NativeConditioningFailure,
                    "support grid capsule has non-finite coordinates",
                ));
            }
            return Ok((Some(BeadEscape { cells: 0, bounds }), 1));
        };
        let empty = Bits::default();
        let kept = self
            .layer(layer)
            .filter(|kept| kept.band[0] <= band[0] && band[1] <= kept.band[1])
            .map_or(&empty, |kept| &kept.bits);
        let (escape, work) = frame.escaped(kept, a, b, radius)?;
        Ok((
            escape.map(|Escape { cells, bounds }| BeadEscape { cells, bounds }),
            work,
        ))
    }

    /// Visits each final object bead edge whose clearance no kept layer
    /// within `z_gap` of it already holds: the edges outside their section's
    /// material that its nominal scope did not exclude, or every object edge
    /// of a section whose final height band leaves the band its scope was
    /// applied for. Object roads regenerated after the sweep are therefore
    /// never trusted to the sweep's copy of them.
    pub(in crate::planner) fn visit_unseen_object_edges(
        &self,
        layers: &[PrintLayer],
        sections: &[super::super::SectionSpan],
        z_gap: f64,
        work: &mut usize,
        cancel: &AtomicBool,
        mut visit: impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        if self.layers.is_empty() {
            return Ok(());
        }
        for (index, section) in sections.iter().enumerate() {
            check_cancel(cancel)?;
            let objects = layers
                .get(section.start as usize..(section.end as usize).min(layers.len()))
                .unwrap_or(&[]);
            let mut band: Option<[f64; 2]> = None;
            for path in objects.iter().flat_map(|layer| &layer.paths) {
                super::super::native::charge_support_clearance(work, cancel)?;
                if support_role(path.kind) {
                    continue;
                }
                let mut take = |[lower, upper]: [f64; 2]| {
                    band = Some(band.map_or([lower, upper], |[l, u]| [l.min(lower), u.max(upper)]));
                };
                take([
                    program_coordinates::project_xyz6_bead_height_mm(
                        path.physical_z_mm[1],
                        path.physical_z_mm[1] - path.physical_z_mm[0],
                    )?[0],
                    program_coordinates::project_xyz6_bead_height_mm(
                        path.physical_z_mm[1],
                        path.height_mm(),
                    )?[1],
                ]);
                for point in &path.points_mm {
                    super::super::native::charge_support_clearance(work, cancel)?;
                    take(program_coordinates::project_xyz6_bead_height_mm(
                        point[2],
                        path.height_mm(),
                    )?);
                }
            }
            let Some([floor, ceiling]) = band else {
                continue;
            };
            // Kept bands rise with the layer: the first whose top comes within
            // the gap of the floor is the lowest that can meet the section.
            let mut failure = None;
            let first = self.layers.partition_point(|kept| {
                match program_coordinates::enclosed_vertical_gap_mm(floor, kept.band[1]) {
                    Ok(gap) => gap >= z_gap,
                    Err(error) => {
                        failure.get_or_insert(error);
                        false
                    }
                }
            });
            if let Some(failure) = failure {
                return Err(failure);
            }
            let Some(kept) = self.layers.get(first) else {
                continue;
            };
            if program_coordinates::enclosed_vertical_gap_mm(kept.band[0], ceiling)? >= z_gap {
                continue;
            }
            let record = self
                .scopes
                .binary_search_by_key(&index, |scope| scope.section)
                .ok()
                .map(|at| &self.scopes[at])
                .filter(|scope| scope.floor <= floor && ceiling <= scope.ceiling);
            let start = section.start as usize;
            let tagged = objects.iter().enumerate().flat_map(|(offset, layer)| {
                layer
                    .paths
                    .iter()
                    .enumerate()
                    .map(move |(path, value)| ((start + offset, path), value))
            });
            if let Some(record) = record {
                beads::visit_outside_material(
                    &section.material,
                    tagged,
                    BEAD_OUTSIDE_ALLOWANCE_MM,
                    work,
                    cancel,
                    |(layer, path), edge, bead| {
                        if record.edges.binary_search(&edge_key(&bead)).is_ok() {
                            return Ok(());
                        }
                        visit(layer, path, edge)
                    },
                )?;
            } else {
                for ((layer, path), value) in tagged {
                    super::super::native::charge_support_clearance(work, cancel)?;
                    if support_role(value.kind) {
                        continue;
                    }
                    for edge in 0..value.points_mm.len().saturating_sub(1) {
                        visit(layer, path, edge)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// A record whose layers keep the cells entirely inside `kept`.
    #[cfg(test)]
    pub(in crate::planner) fn for_test(
        bounds: [[f64; 2]; 2],
        layers: impl IntoIterator<Item = (usize, [f64; 2], MultiPolygon<f64>)>,
    ) -> Self {
        let cancel = AtomicBool::new(false);
        let frame = CellFrame::new(bounds, SUPPORT_CELL_MM).unwrap();
        let admit = |_: usize| Ok(());
        let raster = Raster {
            frame,
            cancel: &cancel,
            admit: &admit,
            work: AtomicUsize::new(0),
        };
        Self {
            frame: Some(frame),
            layers: layers
                .into_iter()
                .map(|(layer, band, kept)| KeptLayer {
                    layer,
                    band,
                    bits: raster.inward(&kept.0).unwrap(),
                })
                .collect(),
            scopes: Vec::new(),
        }
    }

    /// Adds the record of a nominal scope that excluded `edges`.
    #[cfg(test)]
    pub(in crate::planner) fn with_test_scope(
        mut self,
        section: usize,
        [floor, ceiling]: [f64; 2],
        edges: &[([f64; 2], [f64; 2], f64)],
    ) -> Self {
        let mut keys: Vec<[u64; 5]> = edges
            .iter()
            .map(|&(a, b, half)| edge_key(&beads::BeadEdge { a, b, half }))
            .collect();
        keys.sort_unstable();
        self.scopes.push(NominalRecord {
            section,
            floor,
            ceiling,
            edges: keys,
        });
        self.scopes.sort_unstable_by_key(|scope| scope.section);
        self
    }
}

/// One independently grounded set of demands: the ordinary channel, or the
/// bed-only demands a single open-below void relieves.
struct Channel {
    relieved: Option<usize>,
    seeds: Vec<Option<Bits>>,
    /// Per demand target, in first-seen order: its contact tops in demand
    /// order and its seed. Every demand on one target shares that target's
    /// buffered section as its seed.
    interface_seeds: Vec<InterfaceSeed>,
    tops: Vec<usize>,
    active: Vec<Option<Bits>>,
}

struct InterfaceSeed {
    target: usize,
    tops: Vec<usize>,
    seed: Rc<Bits>,
}

impl Channel {
    fn new(relieved: Option<usize>, layers: usize) -> Self {
        Self {
            relieved,
            seeds: vec![None; layers],
            interface_seeds: Vec::new(),
            tops: Vec::new(),
            active: vec![None; layers],
        }
    }

    fn add_seed(
        &mut self,
        raster: &Raster<'_>,
        top: usize,
        target: usize,
        seed: &Rc<Bits>,
    ) -> Result<(), ManufacturingError> {
        let slot = &mut self.seeds[top];
        *slot = Some(match slot.take() {
            Some(old) => raster.union(&old, seed)?,
            None => Bits::clone(seed),
        });
        self.tops.push(top);
        if let Some(entry) = self.interface_seeds.iter_mut().find(|e| e.target == target) {
            if !entry.tops.contains(&top) {
                entry.tops.push(top);
            }
        } else {
            self.interface_seeds.push(InterfaceSeed {
                target,
                tops: vec![top],
                seed: Rc::clone(seed),
            });
        }
        Ok(())
    }
}

/// A raw occupied piece, its height scope clipped to its span, the bounds
/// its clearance can reach, and whether every live carry already excludes it.
struct Piece {
    polygon: Polygon<f64>,
    scope: [f64; 2],
    reach: [[f64; 2]; 2],
    applied: bool,
}

/// One demand before rasterization: its contact top, target and relief.
struct Entry {
    top: usize,
    target: usize,
    relieved: Option<usize>,
}

fn expand(bounds: [[f64; 2]; 2], by: f64) -> [[f64; 2]; 2] {
    [
        bounds[0].map(|v| (v - by).next_down()),
        bounds[1].map(|v| (v + by).next_up()),
    ]
}

fn rects_meet(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

impl Sweep<'_> {
    #[expect(
        clippy::too_many_lines,
        reason = "two ordered passes share local state"
    )]
    pub(super) fn run(
        &self,
        scopes: &[VoidScope],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<SweepOutput, ManufacturingError> {
        let mut timing = crate::timing::Clock::new("support.sweep");
        let layers = self.layers;
        let width = self.recipe.bead_width_mm;
        let mut grids: Vec<Option<LayerGrid>> = Vec::new();
        reserve_owned(&mut grids, layers.len(), self.owned_bytes)?;
        grids.resize_with(layers.len(), || None);
        let mut contact_tops = vec![None; layers.len()];
        let mut access = Vec::new();

        // Each demand's contact top and relief; the seed is its target's
        // section material buffered by half a bead.
        let mut entries: Vec<Entry> = Vec::new();
        let mut bounds: Option<[[f64; 2]; 2]> = None;
        for demand in self.demands {
            check_cancel(cancel)?;
            let path = &layers[demand.target].paths[demand.failure.path];
            let Some(top) = contact_layer(layers, demand.target, path, self.gap, work, cancel)?
            else {
                continue;
            };
            let material = &self.sections[span_at(self.sections, demand.target as u32)].material;
            let Some(seed_bounds) = material_bounds(material).map(|b| expand(b, width / 2.0))
            else {
                continue;
            };
            bounds = Some(bounds.map_or(seed_bounds, |b| union_frame(Some(b), seed_bounds)));
            // A bed-rooted demand may enter one open-below void whose roof it
            // supports; that relief is per demand, so it gets its own channel.
            let corridor_floor = layers[0].lower_z_mm;
            let mut relieved = None;
            for (ordinal, (mask, scope)) in self.protected.0.iter().zip(scopes).enumerate() {
                if !void_reaches_corridor(&scope.height, corridor_floor, layers[top].z_mm) {
                    continue;
                }
                if candidate_clearance_distance_in_bounds(mask, Some(seed_bounds), self.requested)?
                    .is_none()
                {
                    continue;
                }
                if scope.allows_roof_access(
                    Root::Bed,
                    &layers[demand.target],
                    &layers[top],
                    path,
                    &demand.failure,
                    width,
                    work,
                    cancel,
                )? {
                    relieved = Some(ordinal);
                    break;
                }
            }
            reserve_owned(&mut entries, 1, self.owned_bytes)?;
            entries.push(Entry {
                top,
                target: demand.target,
                relieved,
            });
        }
        let (Some(bounds), Some(max_top)) = (bounds, entries.iter().map(|e| e.top).max()) else {
            return Ok(SweepOutput {
                frame: None,
                grids,
                contact_tops,
                access,
                kept: KeptSupport::default(),
                part_contacts: Vec::new(),
            });
        };
        let cells = CellFrame::new(bounds, SUPPORT_CELL_MM)?;
        let cell_bounds = cells.bounds();
        // Live grid bytes held across steps; every new allocation is admitted
        // against them and the planner's retained inventory.
        let live = AtomicUsize::new(0);
        let (retained, owned_bytes) = (self.retained, self.owned_bytes);
        let admit = |extra: usize| -> Result<(), ManufacturingError> {
            super::super::remaining_geometry_units(
                retained,
                owned_bytes
                    .saturating_add(live.load(Ordering::Relaxed))
                    .saturating_add(extra),
            )
            .map(|_| ())
        };
        admit(cells.dense_bytes().saturating_mul(4))?;
        let raster = Raster {
            frame: cells,
            cancel,
            admit: &admit,
            work: AtomicUsize::new(0),
        };
        let hold = |bits: &Bits| -> Result<(), ManufacturingError> {
            admit(bits.bytes())?;
            live.fetch_add(bits.bytes(), Ordering::Relaxed);
            Ok(())
        };
        let release = |bits: &Bits| {
            live.fetch_sub(
                bits.bytes().min(live.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        };
        let charge = |work: &mut usize| {
            charge_owned_work(work, self.foreign_work, raster.take_work(), cancel)
        };

        let mut channels = vec![Channel::new(None, layers.len())];
        let mut seed: Option<(usize, Rc<Bits>)> = None;
        for entry in &entries {
            check_cancel(cancel)?;
            if seed
                .as_ref()
                .is_none_or(|(target, _)| *target != entry.target)
            {
                let material = &self.sections[span_at(self.sections, entry.target as u32)].material;
                let bits = raster.outward(&material.0, width / 2.0)?;
                hold(&bits)?;
                seed = Some((entry.target, Rc::new(bits)));
            }
            let bits = &seed.as_ref().expect("rasterized seed").1;
            let at = if entry.relieved.is_none() {
                0
            } else if let Some(at) = channels.iter().position(|c| c.relieved == entry.relieved) {
                at
            } else {
                reserve_owned(&mut channels, 1, self.owned_bytes)?;
                channels.push(Channel::new(entry.relieved, layers.len()));
                channels.len() - 1
            };
            channels[at].add_seed(&raster, entry.top, entry.target, bits)?;
            if at != 0 {
                // The ordinary channel still owns this demand's part roots.
                channels[0].add_seed(&raster, entry.top, entry.target, bits)?;
            }
            charge(work)?;
        }
        drop(seed);
        timing.mark("seeds");

        // Down pass: carry every channel's candidate through occupancy and voids.
        let distance = self.frame.distance;
        let mut window: Vec<(usize, Vec<Piece>)> = Vec::new();
        let mut carried: Vec<Bits> = channels.iter().map(|_| Bits::default()).collect();
        let mut voids: Vec<Option<Option<Bits>>> = vec![None; self.protected.0.len()];
        let mut material_cells: Option<(usize, Bits)> = None;
        let (mut down_load, mut down_fresh, mut down_seed, mut down_rest): (
            std::time::Duration,
            std::time::Duration,
            std::time::Duration,
            std::time::Duration,
        ) = Default::default();
        for l in (0..=max_top).rev() {
            check_cancel(cancel)?;
            let seeds: Vec<Option<Bits>> = channels
                .iter_mut()
                .map(|channel| channel.seeds[l].take())
                .collect();
            if carried.iter().all(Bits::is_empty) && seeds.iter().all(Option::is_none) {
                continue;
            }
            let slab = bead_slab(&layers[l])?;
            let t0 = std::time::Instant::now();
            self.load_pieces(l, &mut window, cell_bounds, distance, work, cancel)?;
            down_load += t0.elapsed();
            let t0 = std::time::Instant::now();
            // A carry already excludes every piece that touched a layer above
            // since it was seeded, and a piece's height scope is contiguous,
            // so only pieces newly touching this slab can change it.
            let touches = |piece: &Piece| piece.scope[0] <= slab[1] && piece.scope[1] >= slab[0];
            if carried.iter().any(|carry| !carry.is_empty()) {
                let fresh: Vec<&Polygon<f64>> = {
                    use rayon::prelude::*;
                    let candidates: Vec<&Piece> = window
                        .iter()
                        .flat_map(|(_, pieces)| pieces)
                        .filter(|piece| !piece.applied && touches(piece))
                        .collect();
                    candidates
                        .par_iter()
                        .filter(|piece| {
                            carried
                                .iter()
                                .any(|carry| carry.meets_rect(&cells, piece.reach))
                        })
                        .map(|piece| &piece.polygon)
                        .collect()
                };
                if !fresh.is_empty() {
                    let occupancy = raster.outward_parallel(&fresh, distance)?;
                    for carry in &mut carried {
                        carry.subtract(&occupancy);
                    }
                }
            }
            down_fresh += t0.elapsed();
            let t0 = std::time::Instant::now();
            if seeds.iter().any(Option::is_some) {
                // The top layer's own material lies inside its occupancy
                // clearance; removing it first keeps the piece set local to
                // the seed's overhang band.
                // Every layer of a section shares its material: rasterize it
                // once per section.
                let span = span_at(self.sections, l as u32);
                if material_cells.as_ref().is_none_or(|(at, _)| *at != span) {
                    if let Some((_, old)) = material_cells.take() {
                        release(&old);
                    }
                    let bits = raster.outward(&self.sections[span].material.0, 0.0)?;
                    hold(&bits)?;
                    material_cells = Some((span, bits));
                }
                let material = &material_cells.as_ref().expect("section material cells").1;
                for (carry, seed) in carried.iter_mut().zip(&seeds) {
                    let Some(seed) = seed else {
                        continue;
                    };
                    let mut entered = seed.clone();
                    entered.subtract(material);
                    let near: Vec<&Polygon<f64>> = {
                        use rayon::prelude::*;
                        let candidates: Vec<&Piece> = window
                            .iter()
                            .flat_map(|(_, pieces)| pieces)
                            .filter(|piece| touches(piece))
                            .collect();
                        candidates
                            .par_iter()
                            .filter(|piece| entered.meets_rect(&cells, piece.reach))
                            .map(|piece| &piece.polygon)
                            .collect()
                    };
                    if !near.is_empty() {
                        entered.subtract(&raster.outward_parallel(&near, distance)?);
                    }
                    *carry = raster.union(carry, &entered)?;
                }
            }
            down_seed += t0.elapsed();
            let t0 = std::time::Instant::now();
            for piece in window.iter_mut().flat_map(|(_, pieces)| pieces) {
                if touches(piece) {
                    piece.applied = true;
                }
            }
            drop(seeds);
            for (ordinal, (mask, scope)) in self.protected.0.iter().zip(scopes).enumerate() {
                if !void_reaches_corridor(&scope.height, slab[0], slab[1]) {
                    continue;
                }
                if channels
                    .iter()
                    .zip(&carried)
                    .all(|(channel, carry)| carry.is_empty() || channel.relieved == Some(ordinal))
                {
                    continue;
                }
                // A void's clearance depends only on the void; build it once.
                if voids[ordinal].is_none() {
                    let clearance = match candidate_clearance_distance_in_bounds(
                        mask,
                        Some(cell_bounds),
                        self.requested,
                    )? {
                        Some(distance) => Some(raster.outward([mask], distance)?),
                        None => None,
                    };
                    if let Some(bits) = &clearance {
                        hold(bits)?;
                    }
                    voids[ordinal] = Some(clearance);
                }
                let Some(Some(clearance)) = &voids[ordinal] else {
                    continue;
                };
                for (channel, carry) in channels.iter().zip(&mut carried) {
                    if channel.relieved != Some(ordinal) {
                        carry.subtract(clearance);
                    }
                }
            }
            down_rest += t0.elapsed();
            for (channel, carry) in channels.iter_mut().zip(&carried) {
                if !carry.is_empty() {
                    hold(carry)?;
                    channel.active[l] = Some(carry.clone());
                }
            }
            charge(work)?;
        }
        drop(carried);
        drop(window);
        if let Some((_, bits)) = material_cells.take() {
            release(&bits);
        }
        for bits in voids.into_iter().flatten().flatten() {
            release(&bits);
        }
        timing.mark("down_pass");
        if std::env::var_os("SPILING_TRACE_SLICE_PHASES").is_some() {
            eprintln!(
                "{}",
                serde_json::json!({"event": "support_down_detail", "load": down_load.as_secs_f64(), "fresh": down_fresh.as_secs_f64(), "seed": down_seed.as_secs_f64(), "rest": down_rest.as_secs_f64()})
            );
        }

        // Up pass: keep only grounded support, clipped by nominal clearance,
        // one layer at a time across every channel.
        let mut grounded: Vec<Bits> = channels.iter().map(|_| Bits::default()).collect();
        let mut part_bands: Vec<Vec<(f64, Bits)>> = channels.iter().map(|_| Vec::new()).collect();
        let mut clearance: Vec<Option<Bits>> = vec![None; self.nominal.len()];
        let mut records: Vec<NominalRecord> = Vec::new();
        let mut relieved: Vec<(usize, usize, Bits)> = Vec::new();
        let mut up_e = std::time::Duration::ZERO;
        let mut up_nominal = std::time::Duration::ZERO;
        let mut part_contacts = Vec::new();
        reserve_owned(&mut part_contacts, max_top + 1, self.owned_bytes)?;
        part_contacts.resize_with(max_top + 1, || None);
        let te = std::time::Instant::now();
        let mut proposed = self.propose_landings(max_top, &raster, &channels[0].active)?;
        for landing in proposed.iter().flatten() {
            admit(landing.bytes())?;
            live.fetch_add(landing.bytes(), Ordering::Relaxed);
        }
        up_e += te.elapsed();
        for l in 0..=max_top {
            check_cancel(cancel)?;
            if channels.iter().all(|channel| channel.active[l].is_none()) {
                grounded.iter_mut().for_each(|g| *g = Bits::default());
                part_bands.iter_mut().for_each(Vec::clear);
                continue;
            }
            let tn = std::time::Instant::now();
            let reaching = self.prepare_nominal(
                l,
                &raster,
                &mut clearance,
                &mut records,
                &hold,
                &release,
                work,
            )?;
            up_nominal += tn.elapsed();
            let landing = proposed[l].take();
            let mut layer_support = Bits::default();
            let mut layer_interface = Bits::default();
            for (c, channel) in channels.iter_mut().enumerate() {
                let Some(active) = channel.active[l].take() else {
                    grounded[c] = Bits::default();
                    part_bands[c].clear();
                    continue;
                };
                release(&active);
                let mut ground = Bits::default();
                if channel.relieved.is_none()
                    && let Some(landing) = &landing
                {
                    ground = raster.intersection(&active, &landing.cells)?;
                    if !ground.is_empty() {
                        part_bands[c].push((layers[l].lower_z_mm, ground.clone()));
                    }
                }
                let mut kept = active;
                for &at in &reaching {
                    if let Some(bits) = &clearance[at] {
                        kept.subtract(bits);
                    }
                }
                // The bed grounds all of layer 0.
                let mut column = if l == 0 {
                    kept
                } else {
                    let base = raster.union(&grounded[c], &ground)?;
                    raster.intersection(&kept, &base)?
                };
                // Higher support stands on this layer before the touching
                // sheet trims a column ending here: the sheet sits on that
                // column's top only, as in its own fixed-XY prism.
                grounded[c] = column.clone();
                if column.is_empty() {
                    part_bands[c].clear();
                    continue;
                }
                if channel.tops.contains(&l) {
                    column.subtract(&self.touching_sheet(l, &column, &raster, distance, cancel)?);
                    if column.is_empty() {
                        continue;
                    }
                }
                let mut interface = Bits::default();
                for entry in &channel.interface_seeds {
                    let contact = if contact_tops[entry.target].is_none() {
                        entry.tops.iter().copied().find(|top| *top >= l)
                    } else {
                        None
                    };
                    let band = entry.tops.iter().any(|top| {
                        *top >= l && layers[l].z_mm > layers[*top].z_mm - self.interface_thickness
                    });
                    if (contact.is_none() && !band) || !column.intersects(&entry.seed) {
                        continue;
                    }
                    // As for a legacy column: a demand has its contact top once
                    // any kept layer up to it reaches the demand's seed.
                    if let Some(top) = contact {
                        contact_tops[entry.target] = Some(top);
                    }
                    if band {
                        let meet = raster.intersection(&column, &entry.seed)?;
                        interface = raster.union(&interface, &meet)?;
                    }
                }
                part_bands[c]
                    .retain(|(floor, _)| layers[l].lower_z_mm < floor + self.interface_thickness);
                for (_, band) in &part_bands[c] {
                    let meet = raster.intersection(&column, band)?;
                    interface = raster.union(&interface, &meet)?;
                }
                layer_support = raster.union(&layer_support, &column)?;
                layer_interface = raster.union(&layer_interface, &interface)?;
                if let Some(ordinal) = channel.relieved {
                    hold(&column)?;
                    reserve_owned(&mut relieved, 1, self.owned_bytes)?;
                    relieved.push((ordinal, l, column));
                }
            }
            if !layer_support.is_empty() {
                hold(&layer_support)?;
                hold(&layer_interface)?;
                grids[l] = Some(LayerGrid {
                    support: layer_support,
                    interface: layer_interface,
                });
            }
            // Raster retention and final reconciliation anchor interface on
            // these same cells; they stay held until the output is dropped.
            part_contacts[l] = landing;
            charge(work)?;
        }
        if std::env::var_os("SPILING_TRACE_SLICE_PHASES").is_some() {
            eprintln!(
                "{}",
                serde_json::json!({"event": "support_sweep_detail", "landing_seconds": up_e.as_secs_f64(), "nominal_seconds": up_nominal.as_secs_f64()})
            );
        }
        drop(clearance);
        drop(channels);
        timing.mark("up_pass");

        // Support layers leave as cells: raster draws rows and walls from
        // them. Only the relieved roof-access columns become polygons, each
        // inside its kept cells, converted in parallel and kept in order.
        let column_contours: Vec<Result<OwnedMaterial, ManufacturingError>> = {
            use rayon::prelude::*;
            let working_set = crate::working_set::current();
            let raster = &raster;
            relieved
                .par_iter()
                .map(|(_, _, column)| {
                    crate::working_set::scope(working_set, || {
                        let mut material = raster.contour(column)?;
                        region::canonicalize(&mut material);
                        region::check_size(&material)?;
                        Ok(OwnedMaterial::new(material))
                    })
                })
                .collect()
        };
        charge(work)?;
        let (mut units, mut bytes) = (0usize, 0usize);
        for ((ordinal, l, _), converted) in relieved.iter().zip(column_contours) {
            let column = converted?;
            units = units.saturating_add(column.units);
            bytes = bytes.saturating_add(column.bytes);
            super::super::remaining_geometry_units(
                retained.saturating_add(units),
                owned_bytes
                    .saturating_add(live.load(Ordering::Relaxed))
                    .saturating_add(bytes),
            )?;
            access.push((*ordinal, *l, column.into_shared()));
        }
        timing.mark("contours");
        drop(relieved);
        let mut kept = KeptSupport {
            frame: Some(cells),
            layers: Vec::new(),
            scopes: records,
        };
        kept.scopes.sort_unstable_by_key(|scope| scope.section);
        // Raster consumes the support cells; final clearance keeps its own
        // admitted copy until it asserts against them.
        let kept_layers = grids.iter().filter(|grid| grid.is_some()).count();
        reserve_owned(&mut kept.layers, kept_layers, self.owned_bytes)?;
        for (l, grid) in grids.iter().enumerate() {
            let Some(grid) = grid else {
                continue;
            };
            let bits = grid.support.clone();
            hold(&bits)?;
            let slab = bead_slab(&layers[l])?;
            let nominal = nominal_band(layers, l)?;
            kept.layers.push(KeptLayer {
                layer: l,
                band: [slab[0].max(nominal[0]), slab[1].min(nominal[1])],
                bits,
            });
        }
        Ok(SweepOutput {
            frame: Some(raster.frame),
            grids,
            contact_tops,
            access,
            kept,
            part_contacts,
        })
    }

    /// Propose every layer's part landing before the up pass. A proposal
    /// reads only the ordinary channel's down-pass candidate at that layer
    /// and the object layers' roads, never the up pass, so layers are
    /// proposed in parallel; each is the same proposal the up pass would
    /// make at that layer.
    fn propose_landings(
        &self,
        max_top: usize,
        raster: &Raster<'_>,
        active: &[Option<Bits>],
    ) -> Result<Vec<Option<PartContact>>, ManufacturingError> {
        use rayon::prelude::*;
        let mut ledges = Vec::new();
        reserve_owned(&mut ledges, self.landings.len(), self.owned_bytes)?;
        ledges.extend(self.landings.iter().map(|landing| &landing.material));
        let inputs = LandingInputs {
            roots: self.roots,
            ledges,
            layers: self.layers,
            radius: self.recipe.bead_width_mm * 0.5,
            owned_bytes: self.owned_bytes,
        };
        let working_set = crate::working_set::current();
        (0..=max_top)
            .into_par_iter()
            .map(|l| {
                crate::working_set::scope(working_set, || {
                    Self::part_landing(&inputs, l, raster, active[l].as_ref())
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect()
    }

    /// Propose each reachable part root on ordinary model roads. Reconciliation
    /// qualifies their final deposited intervals before granting contact.
    fn part_landing(
        inputs: &LandingInputs<'_>,
        l: usize,
        raster: &Raster<'_>,
        active: Option<&Bits>,
    ) -> Result<Option<PartContact>, ManufacturingError> {
        let Some(active) = active else {
            return Ok(None);
        };
        let mut roots = Vec::new();
        for (index, root) in inputs.roots.iter().enumerate().rev() {
            let Root::Part {
                first_support_layer,
                landing,
                ..
            } = *root
            else {
                continue;
            };
            if first_support_layer != l {
                continue;
            }
            check_cancel(raster.cancel)?;
            if material_bounds(inputs.ledges[landing])
                .is_some_and(|bounds| active.meets_rect(&raster.frame, bounds))
            {
                reserve_owned(&mut roots, 1, inputs.owned_bytes)?;
                roots.push(index);
            }
        }
        let footprint = |object_layer: usize, live_bytes| {
            established::proposal_geometry(
                raster,
                inputs.layers[object_layer]
                    .paths
                    .iter()
                    .filter(|path| established::ordinary(path)),
                inputs.radius,
                live_bytes,
            )
        };
        let root_geometry = |index| match inputs.roots[index] {
            Root::Part {
                object_layer,
                landing,
                ..
            } => (object_layer, inputs.ledges[landing]),
            Root::Bed => unreachable!("part landing root"),
        };
        match roots.as_slice() {
            [] => Ok(None),
            &[index] => {
                let (object_layer, ledge) = root_geometry(index);
                let ledge = raster.inward(&ledge.0)?;
                if !active.intersects(&ledge) {
                    return Ok(None);
                }
                let geometry = footprint(object_layer, ledge.bytes())?;
                let roads = raster.inward(&geometry.0)?;
                let cells = raster.intersection(&ledge, &roads)?;
                Ok((!cells.is_empty()).then_some(PartContact { cells, roots }))
            }
            _ => {
                let mut proposals = Vec::new();
                let mut points = 0usize;
                for &index in &roots {
                    let (object_layer, ledge) = root_geometry(index);
                    let roads = footprint(object_layer, points.saturating_mul(64))?;
                    points = points.saturating_add(region::point_count(&roads));
                    raster.reserve(&mut proposals, 1)?;
                    proposals.push(LandingProposal { ledge, roads });
                }
                let joined = joined_landing(raster, &proposals)?;
                let cells = raster.intersection(active, &joined)?;
                Ok((!cells.is_empty()).then_some(PartContact { cells, roots }))
            }
        }
    }

    /// Keep the raw occupied pieces of the spans a bead slab at layer `l` can
    /// touch (its own and both neighbours), each span collected once while
    /// the descending pass is inside it. Pieces whose clearance cannot reach
    /// the grid are never collected.
    fn load_pieces(
        &self,
        l: usize,
        window: &mut Vec<(usize, Vec<Piece>)>,
        cell_bounds: [[f64; 2]; 2],
        distance: f64,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let here = span_at(self.sections, l as u32);
        // The pass descends: spans above the next one up are finished.
        window.retain(|(span, _)| *span <= here + 1);
        for span in here.saturating_sub(1)..=(here + 1).min(self.sections.len() - 1) {
            if window.iter().any(|(cached, _)| *cached == span) {
                continue;
            }
            let section = &self.sections[span];
            let interval = self.span_interval(span);
            let mut pieces = Vec::new();
            let mut failure = None;
            section.visit_occupied_screened(
                cancel,
                |bounds, _| Ok(rects_meet(expand(bounds, distance), cell_bounds)),
                |piece, height| {
                    let scope = height.map_or(interval, |height| {
                        [height[0].max(interval[0]), height[1].min(interval[1])]
                    });
                    if scope[0] <= scope[1]
                        && let Some(bounds) = piece.bounding_rect()
                    {
                        if let Err(error) = reserve_owned(&mut pieces, 1, self.owned_bytes) {
                            failure = Some(error);
                            return Ok(false);
                        }
                        pieces.push(Piece {
                            polygon: piece.clone(),
                            scope,
                            reach: expand(
                                [
                                    [bounds.min().x, bounds.min().y],
                                    [bounds.max().x, bounds.max().y],
                                ],
                                distance,
                            ),
                            applied: false,
                        });
                    }
                    Ok(true)
                },
            )?;
            if let Some(error) = failure {
                return Err(error);
            }
            charge_owned_work(work, self.foreign_work, pieces.len(), cancel)?;
            window.push((span, pieces));
        }
        Ok(())
    }

    /// The closed Z interval a section span's layers occupy.
    fn span_interval(&self, span: usize) -> [f64; 2] {
        let section = &self.sections[span];
        [
            self.schedule[section.start as usize].lower_z,
            self.schedule[(section.end - 1) as usize].z,
        ]
    }

    /// Nominal clearance of every scope reaching layer `l`: each section
    /// polygon dilated by its clearance distance, plus the actual object bead
    /// edges outside it. Each scope's grid is built once; a scope the rising
    /// pass has left for good is released. Each built scope is recorded for
    /// final clearance.
    #[expect(clippy::too_many_arguments)]
    fn prepare_nominal(
        &self,
        l: usize,
        raster: &Raster<'_>,
        clearance: &mut [Option<Bits>],
        records: &mut Vec<NominalRecord>,
        hold: &impl Fn(&Bits) -> Result<(), ManufacturingError>,
        release: &impl Fn(&Bits),
        work: &mut usize,
    ) -> Result<Vec<usize>, ManufacturingError> {
        let cancel = raster.cancel;
        let [lower, upper] = nominal_band(self.layers, l)?;
        let mut reaching = Vec::new();
        for (at, scope) in self.nominal.iter().enumerate() {
            check_cancel(cancel)?;
            if program_coordinates::enclosed_vertical_gap_mm(scope.floor, upper)? >= self.gap {
                break;
            }
            if program_coordinates::enclosed_vertical_gap_mm(lower, scope.ceiling)? >= self.gap {
                // Lower bands only rise, so this scope never reaches again.
                if let Some(bits) = clearance[at].take() {
                    release(&bits);
                }
                continue;
            }
            if clearance[at].is_none() {
                let section = &self.sections[scope.index];
                let distances = section
                    .material
                    .0
                    .iter()
                    .map(|polygon| {
                        let rect = polygon.bounding_rect().ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                                "native outer operand has no finite bounds",
                            )
                        })?;
                        clearance_distance_in_frame(
                            [[rect.min().x, rect.min().y], [rect.max().x, rect.max().y]],
                            self.frame.bounds,
                            self.requested,
                        )
                    })
                    .collect::<Result<Vec<f64>, ManufacturingError>>()?;
                let mut bits = raster.dilated(section.material.0.iter().zip(distances))?;
                let mut keys = Vec::new();
                if scope.beads.is_some() {
                    let objects = self
                        .layers
                        .get(section.start as usize..(section.end as usize).min(self.layers.len()))
                        .unwrap_or(&[]);
                    let edges = beads::outside_material(
                        &section.material,
                        objects.iter().flat_map(|layer| &layer.paths),
                        BEAD_OUTSIDE_ALLOWANCE_MM,
                        work,
                        cancel,
                    )?;
                    let distance = self.frame.distance;
                    let beads = raster.capsules(
                        edges
                            .iter()
                            .map(|edge| (edge.a, edge.b, (edge.half + distance).next_up())),
                    )?;
                    bits = raster.union(&bits, &beads)?;
                    reserve_owned(&mut keys, edges.len(), self.owned_bytes)?;
                    keys.extend(edges.iter().map(edge_key));
                    keys.sort_unstable();
                    keys.dedup();
                }
                reserve_owned(records, 1, self.owned_bytes)?;
                records.push(NominalRecord {
                    section: scope.index,
                    floor: scope.floor,
                    ceiling: scope.ceiling,
                    edges: keys,
                });
                hold(&bits)?;
                clearance[at] = Some(bits);
            }
            reaching.push(at);
        }
        Ok(reaching)
    }

    /// The next section's continuous occupancy pieces whose closed height
    /// scope reaches the top support bead's actual upper height, dilated by
    /// the clearance distance. Unscoped invariant D spans its whole height and
    /// stays with the height-aware nominal exclusion.
    fn touching_sheet(
        &self,
        top: usize,
        column: &Bits,
        raster: &Raster<'_>,
        distance: f64,
        cancel: &AtomicBool,
    ) -> Result<Bits, ManufacturingError> {
        let next = span_at(self.sections, top as u32) + 1;
        let Some(section) = self.sections.get(next) else {
            return Ok(Bits::default());
        };
        let layer = &self.layers[top];
        let upper = (program_coordinates::project_xyz6_segment_bead_height_mm(
            layer.z_mm,
            layer.z_mm,
            layer.z_mm - layer.lower_z_mm,
        )?[1]
            + super::super::native::XYZ6_SCREEN_SLACK_MM)
            .next_up();
        let lower = layer.lower_z_mm;
        let touches = |height: Option<[f64; 2]>| {
            height.is_some_and(|[from, to]| from <= upper && to >= lower)
        };
        let frame = &raster.frame;
        let mut pieces = Vec::new();
        let mut failure = None;
        section.visit_occupied_screened(
            cancel,
            |bounds, height| {
                Ok(touches(height) && column.meets_rect(frame, expand(bounds, distance)))
            },
            |piece, height| {
                if touches(height) {
                    if let Err(error) = reserve_owned(&mut pieces, 1, self.owned_bytes) {
                        failure = Some(error);
                        return Ok(false);
                    }
                    pieces.push(piece.clone());
                }
                Ok(true)
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        raster.outward(&pieces, distance)
    }
}

/// The closed full-bead slab of a layer's beads. A road's height is its
/// physical extent `z − lower_z`, which can differ from the layer's recorded
/// height in the last bit; the slab encloses both, so every road on the
/// layer lies inside the band its exclusions are built for.
fn bead_slab(layer: &PrintLayer) -> Result<[f64; 2], ManufacturingError> {
    let recorded = program_coordinates::project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm)?;
    let physical = program_coordinates::project_xyz6_bead_height_mm(
        layer.z_mm,
        layer.z_mm - layer.lower_z_mm,
    )?;
    Ok([recorded[0].min(physical[0]), recorded[1].max(physical[1])])
}

/// Lower bead floor and upper segment ceiling of layer `l`'s physical slab.
fn nominal_band(layers: &[PrintLayer], l: usize) -> Result<[f64; 2], ManufacturingError> {
    let lower = bead_slab(&layers[l])?[0];
    let upper = program_coordinates::project_xyz6_segment_bead_height_mm(
        layers[l].z_mm,
        layers[l].z_mm,
        layers[l].z_mm - layers[l].lower_z_mm,
    )?[1];
    Ok([lower, upper])
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Rect, coord};

    #[test]
    fn same_layer_landing_union_keeps_only_the_covered_seam() {
        let cancel = AtomicBool::new(false);
        let raster = Raster {
            frame: CellFrame::new([[0.0, 0.0], [1.0, 1.0]], SUPPORT_CELL_MM).unwrap(),
            cancel: &cancel,
            admit: &|_| Ok(()),
            work: AtomicUsize::new(0),
        };
        let strip = |lo, hi| {
            let polygon =
                Rect::new(coord! { x: lo, y: 0.1 }, coord! { x: hi, y: 0.9 }).to_polygon();
            MultiPolygon::new(vec![polygon])
        };
        let left = strip(0.19, 0.225);
        let right = strip(0.225, 0.26);
        assert!(raster.inward(&left.0).unwrap().is_empty());
        assert!(raster.inward(&right.0).unwrap().is_empty());
        let join = |right: &MultiPolygon<f64>, right_edge| {
            joined_landing(
                &raster,
                &[
                    LandingProposal {
                        ledge: &left,
                        roads: strip(0.18, right_edge),
                    },
                    LandingProposal {
                        ledge: right,
                        roads: strip(0.24, 0.27),
                    },
                ],
            )
            .unwrap()
        };
        assert_eq!(
            join(&right, 0.28).count(),
            raster.inward(&strip(0.19, 0.26).0).unwrap().count()
        );
        assert!(join(&right, 0.23).is_empty());
        assert!(join(&strip(0.226, 0.26), 0.28).is_empty());
    }
}
