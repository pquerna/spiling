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
//! anchored cells on every layer of its separation band. Both passes run on
//! the conservative cell grid of [`grid`](super::grid): seeds, occupancy,
//! voids and clearance round outward, landings and road footprints inward
//! (a layer's landings then close the seams between adjacent ledges), so
//! every cell kept is support space under the exact rules. Polygons are produced
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
    /// Per layer, whether it is some demand's contact top (where the next
    /// section's touching sheet was excluded).
    pub tops: Vec<bool>,
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
    /// Emitted edges the raster proved clear exactly in its clearance
    /// field, sorted by key.
    edges: Vec<CertifiedEdge>,
}

/// One emitted support edge the raster proved clear of the layer's exact
/// exclusions: its XYZ6 segment key and the largest bead radius (including
/// endpoint enclosure) whose capsule keeps the exclusions' full clearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CertifiedEdge {
    pub key: [u64; 4],
    pub radius: f64,
}

/// The order-free key of an emitted XY segment.
pub(super) fn segment_key([a, b]: [[f64; 2]; 2]) -> [u64; 4] {
    let (p, q) = if (a[0], a[1]) <= (b[0], b[1]) {
        (a, b)
    } else {
        (b, a)
    };
    [p[0], p[1], q[0], q[1]].map(f64::to_bits)
}

/// One nominal scope as the sweep excluded it: its section, the object
/// height band it was applied for, and the exact bead edges outside its
/// material that it excluded, as sorted bit keys.
struct NominalRecord {
    section: usize,
    floor: f64,
    ceiling: f64,
    /// Digest of every object bead edge the scope's clearance excluded.
    beads: beads::BeadDigest,
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
                self.layers
                    .iter()
                    .map(|layer| vec_bytes(&layer.edges))
                    .fold(0, usize::saturating_add),
            )
    }

    /// Records the edges `layer`'s raster proved clear of the same
    /// exclusions exactly; returns the bytes now retained for them.
    pub(super) fn certify(&mut self, layer: usize, mut edges: Vec<CertifiedEdge>) -> usize {
        let Ok(at) = self.layers.binary_search_by_key(&layer, |kept| kept.layer) else {
            return 0;
        };
        edges.sort_unstable_by(|a, b| a.key.cmp(&b.key).then(b.radius.total_cmp(&a.radius)));
        edges.dedup_by(|later, first| later.key == first.key);
        let kept = &mut self.layers[at];
        kept.edges = edges;
        vec_bytes(&kept.edges)
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
        let applicable = self
            .layer(layer)
            .filter(|kept| kept.band[0] <= band[0] && band[1] <= kept.band[1]);
        let kept = applicable.map_or(&empty, |kept| &kept.bits);
        let (escape, work) = frame.escaped(kept, a, b, radius)?;
        // Cells outside the kept set are clear when the raster proved this
        // very emitted capsule against the layer's exact exclusions.
        if escape.is_some()
            && let Some(kept) = applicable
            && let Ok(at) = kept
                .edges
                .binary_search_by(|edge| edge.key.cmp(&segment_key(segment)))
            && radius <= kept.edges[at].radius
        {
            return Ok((None, work.saturating_add(1)));
        }
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
    /// Sections are independent, so they are visited in parallel, each with
    /// its own `init()` state; the states return in section order and the
    /// first error in that order is the one reported.
    #[expect(clippy::too_many_arguments)]
    pub(in crate::planner) fn visit_unseen_object_edges<S: Send>(
        &self,
        layers: &[PrintLayer],
        sections: &[super::super::SectionSpan],
        z_gap: f64,
        work: &mut usize,
        cancel: &AtomicBool,
        init: impl Fn() -> S + Sync,
        visit: impl Fn(&mut S, usize, usize, usize) -> Result<(), ManufacturingError> + Sync,
    ) -> Result<Vec<S>, ManufacturingError> {
        use rayon::prelude::*;
        if self.layers.is_empty() {
            return Ok(Vec::new());
        }
        let working_set = crate::working_set::current();
        let visited = sections
            .par_iter()
            .enumerate()
            .map(|(index, section)| {
                crate::working_set::scope(working_set, || {
                    let mut state = init();
                    let mut section_work = 0usize;
                    let result = self.visit_unseen_section(
                        layers,
                        (index, section),
                        z_gap,
                        &mut section_work,
                        cancel,
                        &mut |layer, path, edge| visit(&mut state, layer, path, edge),
                    );
                    (result, section_work, state)
                })
            })
            .collect::<Vec<_>>();
        let mut states = Vec::new();
        reserve_owned(&mut states, visited.len(), 0)?;
        for (result, section_work, state) in visited {
            super::super::native::charge_support_clearance_range(work, section_work, cancel)?;
            result?;
            states.push(state);
        }
        Ok(states)
    }

    fn visit_unseen_section(
        &self,
        layers: &[PrintLayer],
        (index, section): (usize, &super::super::SectionSpan),
        z_gap: f64,
        work: &mut usize,
        cancel: &AtomicBool,
        visit: &mut dyn FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
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
            return Ok(());
        };
        // Kept bands rise with the layer: the first whose top comes within
        // the gap of the floor is the lowest that can meet the section.
        let mut failure = None;
        let first =
            self.layers.partition_point(
                |kept| match program_coordinates::enclosed_vertical_gap_mm(floor, kept.band[1]) {
                    Ok(gap) => gap >= z_gap,
                    Err(error) => {
                        failure.get_or_insert(error);
                        false
                    }
                },
            );
        if let Some(failure) = failure {
            return Err(failure);
        }
        let Some(kept) = self.layers.get(first) else {
            return Ok(());
        };
        if program_coordinates::enclosed_vertical_gap_mm(kept.band[0], ceiling)? >= z_gap {
            return Ok(());
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
        // The scope excluded every bead it saw; the same beads are seen.
        if let Some(record) = record {
            let mut digest = beads::BeadDigest::default();
            beads::visit_object_beads(
                objects.iter().flat_map(|layer| &layer.paths),
                work,
                cancel,
                |bead| {
                    digest.add(&bead);
                    Ok(())
                },
            )?;
            if digest == record.beads {
                return Ok(());
            }
        }
        for ((layer, path), value) in tagged {
            super::super::native::charge_support_clearance(work, cancel)?;
            if support_role(value.kind) {
                continue;
            }
            for edge in 0..value.points_mm.len().saturating_sub(1) {
                visit(layer, path, edge)?;
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
                    edges: Vec::new(),
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
        let mut digest = beads::BeadDigest::default();
        for &(a, b, half) in edges {
            digest.add(&beads::BeadEdge { a, b, half });
        }
        self.scopes.push(NominalRecord {
            section,
            floor,
            ceiling,
            beads: digest,
        });
        self.scopes.sort_unstable_by_key(|scope| scope.section);
        self
    }
}

/// One independently grounded set of demands: the ordinary channel, or the
/// bed-only demands a single open-below void relieves.
struct Channel {
    relieved: Option<usize>,
    /// Branching support: its candidate is the tree slice of each layer,
    /// not a carried prism, and it stands on the lower layer grown by the
    /// branch lean.
    tree: bool,
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
            tree: false,
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
/// its clearance can reach, and the highest swept layer whose bead slab it
/// meets: the layer where the descending carry first excludes it.
struct Piece {
    polygon: Polygon<f64>,
    scope: [f64; 2],
    reach: [[f64; 2]; 2],
    first: Option<usize>,
}

/// One demand before rasterization: its contact top, target and relief.
struct Entry {
    top: usize,
    target: usize,
    /// Open-below voids whose roof this demand holds.
    relieved: Vec<usize>,
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
            // A bed-rooted demand may enter every open-below void whose roof
            // it supports; each such void gets its own relieved channel.
            let corridor_floor = layers[0].lower_z_mm;
            let mut relieved = Vec::new();
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
                    mask,
                    &material.0,
                    Root::Bed,
                    &layers[demand.target],
                    &layers[top],
                    path,
                    &demand.failure,
                    width,
                    work,
                    cancel,
                )? {
                    reserve_owned(&mut relieved, 1, self.owned_bytes)?;
                    relieved.push(ordinal);
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
                tops: Vec::new(),
            });
        };
        let params = super::tree::Params::new(width);
        // Branches walk around the part to the bed, so the frame covers the
        // part below every contact top, not only the seeds.
        let bounds = {
            let mut reach = bounds;
            for span in self.sections {
                if span.start as usize <= max_top
                    && let Some(b) = material_bounds(&span.material)
                {
                    reach = union_frame(Some(reach), b);
                }
            }
            expand(reach, params.base + 2.0 * params.max_radius)
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
            // The ordinary channel owns every demand's part roots; each void
            // whose roof the demand holds also gets its own bed-only channel.
            channels[0].add_seed(&raster, entry.top, entry.target, bits)?;
            for &ordinal in &entry.relieved {
                let at = if let Some(at) = channels.iter().position(|c| c.relieved == Some(ordinal))
                {
                    at
                } else {
                    reserve_owned(&mut channels, 1, self.owned_bytes)?;
                    channels.push(Channel::new(Some(ordinal), layers.len()));
                    channels.len() - 1
                };
                channels[at].add_seed(&raster, entry.top, entry.target, bits)?;
            }
            charge(work)?;
        }
        drop(seed);
        let tree_at = {
            let interface_seeds = channels[0]
                .interface_seeds
                .iter()
                .map(|entry| InterfaceSeed {
                    target: entry.target,
                    tops: entry.tops.clone(),
                    seed: Rc::clone(&entry.seed),
                })
                .collect();
            let mut channel = Channel::new(None, layers.len());
            channel.tree = true;
            channel.interface_seeds = interface_seeds;
            channel.tops = channels[0].tops.clone();
            channels.push(channel);
            channels.len() - 1
        };
        timing.mark("seeds");

        // Down pass: carry every channel's candidate through occupancy and
        // voids, A[l] = (A[l+1] ∪ S[l]) − O[l] − V[l]. Occupancy O[l] depends
        // only on the layer, so each chunk of layers rasterizes it in
        // parallel and the carry stays cheap set algebra in layer order.
        // Subtracting a piece again at every layer it occupies changes no
        // cell: the carry already lacks it, and each entering seed needs it.
        let distance = self.frame.distance;
        let guide = {
            {
                let mut moves = Vec::new();
                reserve_owned(&mut moves, max_top + 1, self.owned_bytes)?;
                for layer in &layers[..=max_top] {
                    moves.push(params.lean(layer.z_mm - layer.lower_z_mm));
                }
                let sections = self.sections;
                let protected = &self.protected.0;
                let obstacles = |l: usize| {
                    let slab = bead_slab(&layers[l])?;
                    // The layer's own section only: the next section up is the
                    // overhang a tree supports, not an obstacle under it.
                    let span = span_at(sections, l as u32);
                    let mut key = span as u64;
                    let mut polygons = Vec::new();
                    polygons.extend(sections[span].material.0.iter());
                    for (ordinal, (mask, scope)) in protected.iter().zip(scopes).enumerate() {
                        if void_reaches_corridor(&scope.height, slab[0], slab[1]) {
                            key = key.rotate_left(7)
                                ^ (ordinal as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                            polygons.push(mask);
                        }
                    }
                    Ok((key, polygons))
                };
                let guide = super::tree::Guide::build(
                    cell_bounds,
                    &moves,
                    obstacles,
                    distance,
                    &admit,
                    cancel,
                    work,
                )?;
                admit(guide.bytes())?;
                live.fetch_add(guide.bytes(), Ordering::Relaxed);
                guide
            }
        };
        timing.mark("tree_guide");
        let mut roofs: Vec<(usize, Bits)> = Vec::new();
        let mut forest = super::tree::Forest::default();
        let mut bridge = Vec::new();
        reserve_owned(&mut bridge, max_top + 1, self.owned_bytes)?;
        bridge.resize(max_top + 1, false);
        let mut tree_near: Option<(Vec<(usize, usize)>, Bits)> = None;
        let mut tree_time = [std::time::Duration::ZERO; 4];
        // The carry at layer `l` lies inside the seeds at or above `l`, so a
        // piece outside their running envelope cannot change any cell.
        let mut envelope: Vec<Option<[[f64; 2]; 2]>> = Vec::new();
        reserve_owned(&mut envelope, max_top + 1, self.owned_bytes)?;
        envelope.resize(max_top + 1, None);
        let mut running: Option<[[f64; 2]; 2]> = None;
        for l in (0..=max_top).rev() {
            for channel in &channels {
                if let Some(seed) = &channel.seeds[l]
                    && let (Some(x), Some(y)) = (seed.extent(&cells, 0), seed.extent(&cells, 1))
                {
                    let rect = [[x[0], y[0]], [x[1], y[1]]];
                    running = Some(union_frame(running, rect));
                }
            }
            envelope[l] = running;
        }
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
        let mut slabs = Vec::new();
        reserve_owned(&mut slabs, max_top + 1, self.owned_bytes)?;
        for layer in &layers[..=max_top] {
            slabs.push(bead_slab(layer)?);
        }
        let working_set = crate::working_set::current();
        const CHUNK: usize = 32;
        let mut next = Some(max_top);
        while let Some(hi) = next {
            let lo = hi.saturating_sub(CHUNK - 1);
            next = lo.checked_sub(1);
            check_cancel(cancel)?;
            let t0 = std::time::Instant::now();
            self.load_spans(
                lo,
                hi,
                &mut window,
                &slabs,
                cell_bounds,
                distance,
                work,
                cancel,
            )?;
            down_load += t0.elapsed();
            // Each piece is rasterized once, in the batch of the layer where
            // the carry first meets it; batches build in parallel.
            let t0 = std::time::Instant::now();
            let mut fresh: Vec<Option<Bits>> = {
                use rayon::prelude::*;
                let mut batches: Vec<Vec<&Polygon<f64>>> = vec![Vec::new(); hi - lo + 1];
                for piece in window.iter().flat_map(|(_, pieces)| pieces) {
                    if let Some(first) = piece.first
                        && (lo..=hi).contains(&first)
                        && envelope[first].is_some_and(|reach| rects_meet(piece.reach, reach))
                    {
                        raster.reserve(&mut batches[first - lo], 1)?;
                        batches[first - lo].push(&piece.polygon);
                    }
                }
                let raster = &raster;
                let built: Vec<Result<Option<Bits>, ManufacturingError>> = batches
                    .par_iter()
                    .map(|batch| {
                        crate::working_set::scope(working_set, || {
                            if batch.is_empty() {
                                return Ok(None);
                            }
                            raster.outward_parallel(batch, distance).map(Some)
                        })
                    })
                    .collect();
                built.into_iter().collect::<Result<_, _>>()?
            };
            for bits in fresh.iter().flatten() {
                hold(bits)?;
            }
            down_fresh += t0.elapsed();
            for l in (lo..=hi).rev() {
                check_cancel(cancel)?;
                let fresh = fresh[l - lo].take();
                let mut seeds: Vec<Option<Bits>> = channels
                    .iter_mut()
                    .map(|channel| channel.seeds[l].take())
                    .collect();
                let skip = carried.iter().all(Bits::is_empty)
                    && seeds.iter().all(Option::is_none)
                    && roofs.is_empty()
                    && forest.is_empty();
                if let Some(bits) = &fresh {
                    // Carries already exclude every earlier piece.
                    for carry in &mut carried {
                        carry.subtract(bits);
                    }
                    for (_, roof) in &mut roofs {
                        roof.subtract(bits);
                    }
                    release(bits);
                }
                if skip {
                    continue;
                }
                let slab = slabs[l];
                let t0 = std::time::Instant::now();
                if seeds.iter().any(Option::is_some) {
                    // Every layer in the same section shares the material used to
                    // trim its seed before sizing the occupancy query.
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
                    for seed in seeds.iter_mut().flatten() {
                        seed.subtract(material);
                    }
                    // An entering seed needs every piece of this slab.
                    let spans = slab_spans(self.sections, l);
                    let mut near = Vec::new();
                    for piece in window
                        .iter()
                        .filter(|(span, _)| spans.contains(span))
                        .flat_map(|(_, pieces)| pieces)
                    {
                        if piece.scope[0] <= slab[1]
                            && piece.scope[1] >= slab[0]
                            && seeds
                                .iter()
                                .flatten()
                                .any(|seed| seed.meets_rect(&cells, piece.reach))
                        {
                            raster.reserve(&mut near, 1)?;
                            near.push(&piece.polygon);
                        }
                    }
                    if !near.is_empty() {
                        let clearance = raster.outward_parallel(&near, distance)?;
                        for seed in seeds.iter_mut().flatten() {
                            seed.subtract(&clearance);
                        }
                    }
                }
                // A tree overhang leaves the ordinary channel at its contact
                // top: it carries as a roof through the interface band, then
                // as branches.
                if layers[l].z_mm > TREE_MIN_HEIGHT_MM
                    && let Some(seed) = seeds[0].as_mut()
                {
                    let mut moved = false;
                    for component in raster.components(seed)? {
                        if prefers_tree(&raster, &guide, &params, &component, l)? {
                            // Only cells whose branches reach the bed leave the
                            // column: the rest still lands on the part.
                            let component = raster.select(&component, |c, r| {
                                guide.reach(l, cells.centre(c, r)) >= params.tip
                            })?;
                            if component.is_empty() {
                                continue;
                            }
                            seed.subtract(&component);
                            hold(&component)?;
                            reserve_owned(&mut roofs, 1, self.owned_bytes)?;
                            roofs.push((self.band_bottom(l), component));
                            moved = true;
                        }
                    }
                    if moved && seed.is_empty() {
                        seeds[0] = None;
                    }
                }
                for (carry, seed) in carried.iter_mut().zip(seeds) {
                    if let Some(seed) = seed {
                        *carry = raster.union(carry, &seed)?;
                    }
                }
                down_seed += t0.elapsed();
                let t0 = std::time::Instant::now();
                for (ordinal, (mask, scope)) in self.protected.0.iter().zip(scopes).enumerate() {
                    if !void_reaches_corridor(&scope.height, slab[0], slab[1]) {
                        continue;
                    }
                    if channels.iter().zip(&carried).all(|(channel, carry)| {
                        carry.is_empty() || channel.relieved == Some(ordinal)
                    }) {
                        continue;
                    }
                    // A void's clearance depends only on the void; build it once.
                    if voids[ordinal].is_none() {
                        let clearance =
                            projection::void_placement_distance(mask, cell_bounds, self.requested)?
                                .map(|distance| raster.outward([mask], distance))
                                .transpose()?;
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
                if !(roofs.is_empty() && forest.is_empty()) {
                    let mut cuts = Vec::new();
                    for (ordinal, (mask, scope)) in self.protected.0.iter().zip(scopes).enumerate()
                    {
                        if !void_reaches_corridor(&scope.height, slab[0], slab[1]) {
                            continue;
                        }
                        if voids[ordinal].is_none() {
                            let clearance = projection::void_placement_distance(
                                mask,
                                cell_bounds,
                                self.requested,
                            )?
                            .map(|distance| raster.outward([mask], distance))
                            .transpose()?;
                            if let Some(bits) = &clearance {
                                hold(bits)?;
                            }
                            voids[ordinal] = Some(clearance);
                        }
                        if let Some(Some(clearance)) = &voids[ordinal] {
                            cuts.push(ordinal);
                            let _ = clearance;
                        }
                    }
                    let void_bits = |ordinal: usize| match &voids[ordinal] {
                        Some(Some(bits)) => Some(bits),
                        _ => None,
                    };
                    let layer = &layers[l];
                    let tt = std::time::Instant::now();
                    forest.descend(
                        &guide,
                        l,
                        layer.z_mm - layer.lower_z_mm,
                        layer.lower_z_mm,
                        &params,
                        cancel,
                        work,
                    )?;
                    let mut at = 0;
                    while at < roofs.len() {
                        if roofs[at].0 > l {
                            let (bottom, roof) = roofs.remove(at);
                            forest.plant(&raster, &roof, &params, self.roof_rows(bottom))?;
                            release(&roof);
                        } else {
                            at += 1;
                        }
                    }
                    let mut roof_cells = Bits::default();
                    for (bottom, roof) in &mut roofs {
                        for &ordinal in &cuts {
                            if let Some(bits) = void_bits(ordinal) {
                                roof.subtract(bits);
                            }
                        }
                        roof_cells = raster.union(&roof_cells, roof)?;
                        if *bottom == l {
                            bridge[l] = true;
                        }
                    }
                    tree_time[0] += tt.elapsed();
                    let tt = std::time::Instant::now();
                    let mut slice = if forest.is_empty() {
                        Bits::default()
                    } else {
                        raster.capsules(forest.discs().map(|(p, r)| (p, p, r)))?
                    };
                    tree_time[1] += tt.elapsed();
                    let tt = std::time::Instant::now();
                    if !slice.is_empty() {
                        let span = span_at(self.sections, l as u32);
                        if material_cells.as_ref().is_none_or(|(at, _)| *at != span) {
                            if let Some((_, old)) = material_cells.take() {
                                release(&old);
                            }
                            let bits = raster.outward(&self.sections[span].material.0, 0.0)?;
                            hold(&bits)?;
                            material_cells = Some((span, bits));
                        }
                        slice.subtract(&material_cells.as_ref().expect("section material cells").1);
                        let spans = slab_spans(self.sections, l);
                        let mut key = Vec::new();
                        let mut near = Vec::new();
                        for (span, pieces) in window.iter().filter(|(span, _)| spans.contains(span))
                        {
                            for (k, piece) in pieces.iter().enumerate() {
                                // Only pieces whose clearance reaches the slice
                                // can remove a cell of it.
                                if piece.scope[0] <= slab[1]
                                    && piece.scope[1] >= slab[0]
                                    && slice.meets_rect(&cells, piece.reach)
                                {
                                    reserve_owned(&mut key, 1, self.owned_bytes)?;
                                    key.push((*span, k));
                                    raster.reserve(&mut near, 1)?;
                                    near.push(&piece.polygon);
                                }
                            }
                        }
                        if tree_near.as_ref().is_none_or(|(k, _)| *k != key) {
                            if let Some((_, old)) = tree_near.take() {
                                release(&old);
                            }
                            let bits = if near.is_empty() {
                                Bits::default()
                            } else {
                                raster.outward_parallel(&near, distance)?
                            };
                            hold(&bits)?;
                            tree_near = Some((key, bits));
                        }
                        slice.subtract(&tree_near.as_ref().expect("tree occupancy").1);
                        for &ordinal in &cuts {
                            if let Some(bits) = void_bits(ordinal) {
                                slice.subtract(bits);
                            }
                        }
                    }
                    tree_time[2] += tt.elapsed();
                    let active = raster.union(&roof_cells, &slice)?;
                    if !active.is_empty() {
                        hold(&active)?;
                        channels[tree_at].active[l] = Some(active);
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
        }
        drop(carried);
        drop(window);
        for (_, roof) in roofs.drain(..) {
            release(&roof);
        }
        if let Some((_, bits)) = tree_near.take() {
            release(&bits);
        }
        live.fetch_sub(
            guide.bytes().min(live.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        drop(guide);
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
                serde_json::json!({"event": "support_down_detail", "tree_descend": tree_time[0].as_secs_f64(), "tree_slice": tree_time[1].as_secs_f64(), "tree_clip": tree_time[2].as_secs_f64(), "load": down_load.as_secs_f64(), "fresh": down_fresh.as_secs_f64(), "seed": down_seed.as_secs_f64(), "rest": down_rest.as_secs_f64()})
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
        // Branches may land on the part as ordinary columns do.
        let mut joined = Vec::new();
        reserve_owned(&mut joined, max_top + 1, self.owned_bytes)?;
        for l in 0..=max_top {
            joined.push(
                match (&channels[0].active[l], &channels[tree_at].active[l]) {
                    (Some(a), Some(b)) => Some(raster.union(a, b)?),
                    (Some(a), None) | (None, Some(a)) => Some(a.clone()),
                    (None, None) => None,
                },
            );
        }
        let mut proposed = self.propose_landings(max_top, &raster, &joined)?;
        drop(joined);
        for landing in proposed.iter().flatten() {
            admit(landing.bytes())?;
            live.fetch_add(landing.bytes(), Ordering::Relaxed);
        }
        up_e += te.elapsed();
        // Nominal scope grids depend only on their scope, so the scopes the
        // next chunk of swept layers will first reach are built in parallel
        // ahead of the serial pass, which still records them in its order.
        let mut prebuilt: Vec<Option<(Bits, NominalRecord)>> = Vec::new();
        reserve_owned(&mut prebuilt, self.nominal.len(), self.owned_bytes)?;
        prebuilt.resize_with(self.nominal.len(), || None);
        let mut scheduled = vec![false; self.nominal.len()];
        let mut prebuilt_to = 0;
        for l in 0..=max_top {
            check_cancel(cancel)?;
            if channels.iter().all(|channel| channel.active[l].is_none()) {
                grounded.iter_mut().for_each(|g| *g = Bits::default());
                part_bands.iter_mut().for_each(Vec::clear);
                continue;
            }
            let tn = std::time::Instant::now();
            if l >= prebuilt_to {
                const CHUNK: usize = 32;
                prebuilt_to = (l + CHUNK).min(max_top + 1);
                let mut wanted = Vec::new();
                for ahead in l..prebuilt_to {
                    if channels
                        .iter()
                        .all(|channel| channel.active[ahead].is_none())
                    {
                        continue;
                    }
                    for at in self.nominal_reach(ahead)?.0 {
                        if !scheduled[at] {
                            scheduled[at] = true;
                            reserve_owned(&mut wanted, 1, self.owned_bytes)?;
                            wanted.push(at);
                        }
                    }
                }
                let built = {
                    use rayon::prelude::*;
                    let builder = NominalBuilder {
                        layers: self.layers,
                        sections: self.sections,
                        nominal: self.nominal,
                        distance,
                        owned_bytes: self.owned_bytes,
                        raster: &raster,
                    };
                    let working_set = crate::working_set::current();
                    wanted
                        .par_iter()
                        .map(|&at| crate::working_set::scope(working_set, || builder.build(at)))
                        .collect::<Vec<_>>()
                };
                for (&at, result) in wanted.iter().zip(built) {
                    let (bits, record, spent) = result?;
                    charge_owned_work(work, self.foreign_work, spent, cancel)?;
                    hold(&bits)?;
                    prebuilt[at] = Some((bits, record));
                }
            }
            let reaching =
                self.prepare_nominal(l, &mut clearance, &mut prebuilt, &mut records, &release)?;
            up_nominal += tn.elapsed();
            let landing = proposed[l].take();
            let mut layer_support = Bits::default();
            let mut layer_interface = Bits::default();
            // Columns and branches print together: either stands on what the
            // other kept below. Relieved roof-access channels stay apart.
            let shared = if grounded[tree_at].is_empty() {
                None
            } else {
                Some(raster.union(&grounded[0], &grounded[tree_at])?)
            };
            for (c, channel) in channels.iter_mut().enumerate() {
                let Some(active) = channel.active[l].take() else {
                    grounded[c] = Bits::default();
                    part_bands[c].clear();
                    continue;
                };
                release(&active);
                // Every layer of a root's separation band lands on its cells.
                let mut ground = Bits::default();
                if channel.relieved.is_none()
                    && let Some(landing) = &landing
                {
                    ground = raster.intersection(&active, &landing.cells)?;
                }
                let mut kept = active;
                for &at in &reaching {
                    if let Some(bits) = &clearance[at] {
                        kept.subtract(bits);
                    }
                }
                if !ground.is_empty() {
                    part_bands[c].push((layers[l].lower_z_mm, ground.clone()));
                }
                // The bed grounds all of layer 0.
                let mut column = if l == 0 {
                    kept
                } else {
                    let below = match &shared {
                        Some(shared) if channel.relieved.is_none() => shared,
                        None if channel.tree => &grounded[0],
                        _ => &grounded[c],
                    };
                    let dilated;
                    let lower = if channel.tree {
                        // A branch leans by its layer's move; the lowest roof
                        // layer bridges between the tips under it.
                        let layer = &layers[l];
                        let reach = if bridge[l] {
                            params.spacing
                        } else {
                            params.lean(layer.z_mm - layer.lower_z_mm) + 2.0 * SUPPORT_CELL_MM
                        };
                        let near = raster.near_window(below, &kept, reach)?;
                        dilated = raster.dilate(&near, reach)?;
                        &dilated
                    } else {
                        below
                    };
                    let base = raster.union(lower, &ground)?;
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
        // Every demand's top is an ordinary-channel top.
        let mut tops = Vec::new();
        reserve_owned(&mut tops, layers.len(), self.owned_bytes)?;
        tops.resize(layers.len(), false);
        for &top in &channels[0].tops {
            tops[top] = true;
        }
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
                edges: Vec::new(),
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
            tops,
        })
    }

    /// Propose every layer's part landing before the up pass. A proposal
    /// reads only the ordinary channel's down-pass candidate at that layer
    /// and the object layers' roads, never the up pass, so layers are
    /// proposed in parallel; each is the same proposal the up pass would
    /// make at that layer.
    /// Row lattice of interface layer `l`, as raster draws it: full-density
    /// pitch on the layer's alternating axis.
    fn roof_rows(&self, l: usize) -> super::tree::Rows {
        let width = self.recipe.bead_width_mm;
        let h = self.recipe.layer_height_mm;
        let pitch = ((width - h) * h + std::f64::consts::PI * (h * 0.5).powi(2)) / h;
        let axis = super::super::region::HatchAxis::for_policy(
            RectilinearAxisPolicy::AlternatingXY,
            super::super::hatch_key(l),
        );
        super::tree::Rows {
            fixed: usize::from(axis == super::super::region::HatchAxis::X),
            pitch,
        }
    }

    /// Lowest layer of a tree roof under contact top `top`: its interface
    /// band. A deeper roof carried as a column is eaten by the part's own
    /// occupancy before its tips are planted.
    fn band_bottom(&self, top: usize) -> usize {
        let layers = self.layers;
        let depth = self.interface_thickness;
        let mut bottom = top;
        while bottom > 0 && layers[bottom - 1].z_mm > layers[top].z_mm - depth {
            bottom -= 1;
        }
        bottom
    }

    fn propose_landings(
        &self,
        max_top: usize,
        raster: &Raster<'_>,
        active: &[Option<Bits>],
    ) -> Result<Vec<Option<PartContact>>, ManufacturingError> {
        use rayon::prelude::*;
        let working_set = crate::working_set::current();
        let radius = self.recipe.bead_width_mm * 0.5;
        let layers = self.layers;
        let mut ledges = Vec::new();
        reserve_owned(&mut ledges, self.landings.len(), self.owned_bytes)?;
        ledges.extend(self.landings.iter().map(|landing| &landing.material));
        // A root proposes on every layer of its separation band, so its
        // landing cells are built once: its inward ledge over its object
        // layer's ordinary road footprint.
        let built: Vec<Result<Option<Bits>, ManufacturingError>> = self
            .roots
            .par_iter()
            .map(|root| {
                crate::working_set::scope(working_set, || {
                    let Root::Part {
                        object_layer,
                        landing,
                        ..
                    } = *root
                    else {
                        return Ok(None);
                    };
                    let ledge = raster.inward(&ledges[landing].0)?;
                    if ledge.is_empty() {
                        return Ok(None);
                    }
                    let roads = established::proposal_geometry(
                        raster,
                        layers[object_layer]
                            .paths
                            .iter()
                            .filter(|path| established::ordinary(path)),
                        radius,
                        ledge.bytes(),
                    )?;
                    let cells = raster.landing(&ledge, &roads.0)?;
                    Ok((!cells.is_empty()).then_some(cells))
                })
            })
            .collect();
        let mut cells = Vec::new();
        reserve_owned(&mut cells, built.len(), self.owned_bytes)?;
        let mut cell_bytes = 0usize;
        for root_cells in built {
            let root_cells = root_cells?;
            cell_bytes = cell_bytes.saturating_add(root_cells.as_ref().map_or(0, Bits::bytes));
            (raster.admit)(cell_bytes)?;
            cells.push(root_cells);
        }
        let roots = self.roots;
        let owned_bytes = self.owned_bytes;
        let cells = &cells;
        (0..=max_top)
            .into_par_iter()
            .map(|l| {
                crate::working_set::scope(working_set, || {
                    Self::part_landing(roots, cells, owned_bytes, l, raster, active[l].as_ref())
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect()
    }

    /// Propose each reachable part root on ordinary model roads. Reconciliation
    /// qualifies their final deposited intervals before granting contact.
    fn part_landing(
        roots: &[Root],
        cells: &[Option<Bits>],
        owned_bytes: usize,
        l: usize,
        raster: &Raster<'_>,
        active: Option<&Bits>,
    ) -> Result<Option<PartContact>, ManufacturingError> {
        let Some(active) = active else {
            return Ok(None);
        };
        let mut meeting = Vec::new();
        let mut joined = Bits::default();
        for (index, root) in roots.iter().enumerate().rev() {
            let Root::Part {
                first_support_layer,
                last_support_layer,
                ..
            } = *root
            else {
                continue;
            };
            check_cancel(raster.cancel)?;
            let Some(root_cells) = cells[index].as_ref() else {
                continue;
            };
            if !(first_support_layer..=last_support_layer).contains(&l)
                || !active.intersects(root_cells)
            {
                continue;
            }
            reserve_owned(&mut meeting, 1, owned_bytes)?;
            meeting.push(index);
            joined = raster.union(&joined, root_cells)?;
        }
        // The band's ledges tile the part top it stands on; their seams too.
        let joined = raster.close_seams(&joined)?;
        Ok((!joined.is_empty()).then_some(PartContact {
            cells: joined,
            roots: meeting,
        }))
    }

    /// Keep the raw occupied pieces of every span the layers `lo..=hi` can
    /// touch, each span collected once while the descending pass is inside
    /// it and loaded in parallel. Pieces whose clearance cannot reach the
    /// grid are never collected.
    #[expect(clippy::too_many_arguments)]
    fn load_spans(
        &self,
        lo: usize,
        hi: usize,
        window: &mut Vec<(usize, Vec<Piece>)>,
        slabs: &[[f64; 2]],
        cell_bounds: [[f64; 2]; 2],
        distance: f64,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        use rayon::prelude::*;
        let sections = self.sections;
        let needed = *slab_spans(sections, lo).start()..=*slab_spans(sections, hi).end();
        // The pass descends: spans above the chunk's reach are finished.
        window.retain(|(span, _)| needed.contains(span));
        let missing: Vec<usize> = needed
            .filter(|span| !window.iter().any(|(cached, _)| cached == span))
            .collect();
        let working_set = crate::working_set::current();
        let (schedule, owned_bytes) = (self.schedule, self.owned_bytes);
        let loaded: Vec<Result<Vec<Piece>, ManufacturingError>> = missing
            .par_iter()
            .map(|&span| {
                crate::working_set::scope(working_set, || {
                    let mut pieces = span_pieces(
                        sections,
                        schedule,
                        owned_bytes,
                        span,
                        cell_bounds,
                        distance,
                        cancel,
                    )?;
                    // Layers reading this span: those whose own span is it or
                    // a neighbour. Slabs rise with the layer, so the highest
                    // one meeting a piece is found by bisection.
                    let from = sections[span.saturating_sub(1)].start as usize;
                    let to = (sections[(span + 1).min(sections.len() - 1)].end as usize)
                        .min(slabs.len());
                    for piece in &mut pieces {
                        let below = slabs.get(from..to).map_or(0, |run| {
                            run.partition_point(|slab| slab[0] <= piece.scope[1])
                        });
                        piece.first = below
                            .checked_sub(1)
                            .map(|at| from + at)
                            .filter(|&l| slabs[l][1] >= piece.scope[0]);
                    }
                    Ok(pieces)
                })
            })
            .collect();
        for (span, pieces) in missing.into_iter().zip(loaded) {
            let pieces = pieces?;
            charge_owned_work(work, self.foreign_work, pieces.len(), cancel)?;
            window.push((span, pieces));
        }
        Ok(())
    }

    /// The nominal scopes reaching layer `l`, and those whose ceiling the
    /// rising pass has left for good. Scopes rise in floor order, so the
    /// first scope above the band ends the search.
    fn nominal_reach(&self, l: usize) -> Result<(Vec<usize>, Vec<usize>), ManufacturingError> {
        let [lower, upper] = nominal_band(self.layers, l)?;
        let (mut reaching, mut passed) = (Vec::new(), Vec::new());
        for (at, scope) in self.nominal.iter().enumerate() {
            if program_coordinates::enclosed_vertical_gap_mm(scope.floor, upper)? >= self.gap {
                break;
            }
            if program_coordinates::enclosed_vertical_gap_mm(lower, scope.ceiling)? >= self.gap {
                reserve_owned(&mut passed, 1, self.owned_bytes)?;
                passed.push(at);
                continue;
            }
            reserve_owned(&mut reaching, 1, self.owned_bytes)?;
            reaching.push(at);
        }
        Ok((reaching, passed))
    }

    /// Nominal clearance of every scope reaching layer `l`: each section
    /// polygon dilated by the frame's enclosed distance, plus the actual object bead
    /// edges outside it. Each scope's grid is built once, ahead of the pass;
    /// a scope the rising pass has left for good is released. Each scope is
    /// recorded for final clearance when the pass first reaches it.
    fn prepare_nominal(
        &self,
        l: usize,
        clearance: &mut [Option<Bits>],
        prebuilt: &mut [Option<(Bits, NominalRecord)>],
        records: &mut Vec<NominalRecord>,
        release: &impl Fn(&Bits),
    ) -> Result<Vec<usize>, ManufacturingError> {
        let (reaching, passed) = self.nominal_reach(l)?;
        for at in passed {
            // Lower bands only rise, so this scope never reaches again.
            if let Some(bits) = clearance[at].take() {
                release(&bits);
            }
        }
        for &at in &reaching {
            if clearance[at].is_none() {
                let (bits, record) = prebuilt[at].take().ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "support nominal scope was reached before it was built",
                    )
                })?;
                reserve_owned(records, 1, self.owned_bytes)?;
                records.push(record);
                clearance[at] = Some(bits);
            }
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
        let touches = sheet_touches(&self.layers[top])?;
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

/// Whether a next-section piece's height scope reaches the top support
/// bead's actual upper height on `layer`. Unscoped invariant D spans its
/// whole height and stays with the height-aware nominal exclusion.
fn sheet_touches(
    layer: &PrintLayer,
) -> Result<impl Fn(Option<[f64; 2]>) -> bool + use<>, ManufacturingError> {
    let upper = (program_coordinates::project_xyz6_segment_bead_height_mm(
        layer.z_mm,
        layer.z_mm,
        layer.z_mm - layer.lower_z_mm,
    )?[1]
        + super::super::native::XYZ6_SCREEN_SLACK_MM)
        .next_up();
    let lower = layer.lower_z_mm;
    Ok(move |height: Option<[f64; 2]>| {
        height.is_some_and(|[from, to]: [f64; 2]| from <= upper && to >= lower)
    })
}

/// A piece's height scope clipped to its span's interval, when nonempty.
fn piece_scope(height: Option<[f64; 2]>, interval: [f64; 2]) -> Option<[f64; 2]> {
    let scope = height.map_or(interval, |height| {
        [height[0].max(interval[0]), height[1].min(interval[1])]
    });
    (scope[0] <= scope[1]).then_some(scope)
}

/// The exact exclusions the sweep subtracted directly on each layer, for the
/// layer's clearance field: occupancy pieces touching its bead slab (own and
/// adjacent spans) at the clearance distance, protected voids reaching it at
/// their candidate distance, every nominal scope within the Z gap (section
/// material and excluded bead edges at the frame's enclosed distance), and
/// at a contact top the next section's touching sheet. Exclusions carried
/// down from higher layers are not clearance and are not fed. A layer that
/// holds relieved roof-access support (which enters its void) takes no
/// field.
pub(super) struct Exclusions<'a> {
    pub layers: &'a [PrintLayer],
    pub sections: &'a [super::super::SectionSpan],
    pub schedule: &'a [super::super::LayerElevation],
    pub kept: &'a KeptSupport,
    pub voids: &'a [(&'a Polygon<f64>, spiling_geometry::ProtectedVoidHeightScope)],
    pub tops: &'a [bool],
    pub relieved: &'a [bool],
    pub distance: f64,
    pub requested: f64,
    pub gap: f64,
    /// Live units and bytes span loading is admitted against.
    pub retained: usize,
    pub owned_bytes: usize,
}

/// One section span's occupied pieces near some support, with their raw
/// height scopes, loaded once for every layer that reads them.
pub(super) struct SpanPieces {
    span: usize,
    pieces: Vec<(Polygon<f64>, Option<[f64; 2]>)>,
}

impl SpanPieces {
    pub(super) fn bytes(&self) -> usize {
        vec_bytes(&self.pieces).saturating_add(
            self.pieces
                .iter()
                .map(|(piece, _)| {
                    std::iter::once(piece.exterior())
                        .chain(piece.interiors())
                        .map(|ring| ring.0.len())
                        .sum::<usize>()
                        .saturating_mul(std::mem::size_of::<geo::Coord<f64>>())
                })
                .fold(0, usize::saturating_add),
        )
    }
}

impl Exclusions<'_> {
    /// Feeds layer `l`'s exclusions into `builder`; `false` when the layer
    /// takes no field.
    pub(super) fn feed(
        &self,
        l: usize,
        builder: &mut super::field::FieldBuilder<'_, '_>,
        spans: &[std::sync::Arc<SpanPieces>],
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        let Some(frame) = &self.kept.frame else {
            return Ok(false);
        };
        if self.relieved.get(l).is_none_or(|&relieved| relieved) || self.sections.is_empty() {
            return Ok(false);
        }
        let layer = &self.layers[l];
        let slab = bead_slab(layer)?;
        let distance = self.distance;
        let here = span_at(self.sections, l as u32);
        let sheet = sheet_touches(layer)?;
        let last = self.sections.len() - 1;
        for span in here.saturating_sub(1)..=(here + 1).min(last) {
            let section = &self.sections[span];
            let interval = [
                self.schedule[section.start as usize].lower_z,
                self.schedule[(section.end - 1) as usize].z,
            ];
            let top_sheet = span == here + 1 && self.tops.get(l).is_some_and(|&top| top);
            let applies = |height: Option<[f64; 2]>| {
                piece_scope(height, interval)
                    .is_some_and(|scope| scope[0] <= slab[1] && scope[1] >= slab[0])
                    || (top_sheet && sheet(height))
            };
            let Ok(at) = spans.binary_search_by_key(&span, |cached| cached.span) else {
                return Err(error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "support clearance field lost a loaded occupancy span",
                ));
            };
            for (piece, height) in &spans[at].pieces {
                check_cancel(cancel)?;
                if applies(*height) {
                    builder.polygon(piece, distance)?;
                }
            }
        }
        for &(mask, height) in self.voids {
            check_cancel(cancel)?;
            if !void_reaches_corridor(&height, slab[0], slab[1]) {
                continue;
            }
            if let Some(distance) =
                projection::void_placement_distance(mask, frame.bounds(), self.requested)?
            {
                builder.polygon(mask, distance)?;
            }
        }
        let [lower, upper] = nominal_band(self.layers, l)?;
        for record in &self.kept.scopes {
            check_cancel(cancel)?;
            if program_coordinates::enclosed_vertical_gap_mm(record.floor, upper)? >= self.gap
                || program_coordinates::enclosed_vertical_gap_mm(lower, record.ceiling)? >= self.gap
            {
                continue;
            }
            let section = &self.sections[record.section];
            let objects = self
                .layers
                .get(section.start as usize..(section.end as usize).min(self.layers.len()))
                .unwrap_or(&[]);
            let mut work = 0;
            beads::visit_object_beads(
                objects.iter().flat_map(|layer| &layer.paths),
                &mut work,
                cancel,
                |bead| builder.segment(bead.a, bead.b, (bead.half + distance).next_up()),
            )?;
        }
        Ok(true)
    }
}

/// Loads each section span's occupied pieces once, on first read, for the
/// layers staging in parallel, and releases them after their last reader.
/// A span's content does not depend on which layer loads it.
pub(super) struct SpanCache<'e, 'a> {
    exclusions: &'e Exclusions<'a>,
    screen: [[f64; 2]; 2],
    slots: Vec<std::sync::Mutex<SpanSlot>>,
    /// Per span, the staging layers that have yet to release it.
    readers: Vec<AtomicUsize>,
    live: AtomicUsize,
}

enum SpanSlot {
    Unread,
    Loaded(std::sync::Arc<SpanPieces>),
    Released,
    Failed(ManufacturingError),
}

impl<'e, 'a> SpanCache<'e, 'a> {
    /// A cache for the staging layers `layers`, whose field queries lie
    /// within `bounds` widened by `reach`.
    pub(super) fn new(
        exclusions: &'e Exclusions<'a>,
        layers: impl Iterator<Item = usize>,
        bounds: [[f64; 2]; 2],
        reach: f64,
    ) -> Result<Self, ManufacturingError> {
        let count = exclusions.sections.len();
        let mut slots = Vec::new();
        reserve_owned(&mut slots, count, exclusions.owned_bytes)?;
        slots.resize_with(count, || std::sync::Mutex::new(SpanSlot::Unread));
        let mut readers = Vec::new();
        reserve_owned(&mut readers, count, exclusions.owned_bytes)?;
        readers.resize_with(count, || AtomicUsize::new(0));
        let mut cache = Self {
            exclusions,
            screen: expand(bounds, exclusions.distance + reach),
            slots,
            readers,
            live: AtomicUsize::new(0),
        };
        for l in layers {
            for span in cache.spans(l) {
                *cache.readers[span].get_mut() += 1;
            }
        }
        Ok(cache)
    }

    /// The spans layer `l` reads, its own and both neighbours.
    fn spans(&self, l: usize) -> std::ops::Range<usize> {
        let sections = self.exclusions.sections;
        if sections.is_empty() {
            return 0..0;
        }
        let here = span_at(sections, l as u32);
        here.saturating_sub(1)..(here + 2).min(sections.len())
    }

    /// The loaded pieces of every span layer `l` reads, in span order.
    pub(super) fn acquire(
        &self,
        l: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<std::sync::Arc<SpanPieces>>, ManufacturingError> {
        let mut out = Vec::new();
        reserve_owned(&mut out, 3, self.exclusions.owned_bytes)?;
        for span in self.spans(l) {
            let mut slot = self.slots[span].lock().map_err(|_| {
                error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "support clearance span cache was poisoned",
                )
            })?;
            if matches!(*slot, SpanSlot::Unread) {
                *slot = match self.load(span, cancel) {
                    Ok(pieces) => SpanSlot::Loaded(std::sync::Arc::new(pieces)),
                    Err(failure) => SpanSlot::Failed(failure),
                };
            }
            match &*slot {
                SpanSlot::Loaded(pieces) => out.push(std::sync::Arc::clone(pieces)),
                SpanSlot::Failed(failure) => return Err(failure.clone()),
                SpanSlot::Unread | SpanSlot::Released => {
                    return Err(error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "support clearance span was released before its last reader",
                    ));
                }
            }
        }
        Ok(out)
    }

    /// Layer `l` has staged: drops each span it read once no staging layer
    /// still needs it.
    pub(super) fn release(&self, l: usize) {
        for span in self.spans(l) {
            if self.readers[span].fetch_sub(1, Ordering::AcqRel) != 1 {
                continue;
            }
            if let Ok(mut slot) = self.slots[span].lock() {
                if let SpanSlot::Loaded(pieces) = &*slot {
                    let bytes = pieces.bytes();
                    let _ = self
                        .live
                        .try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                            Some(live.saturating_sub(bytes))
                        });
                }
                *slot = SpanSlot::Released;
            }
        }
    }

    fn load(&self, span: usize, cancel: &AtomicBool) -> Result<SpanPieces, ManufacturingError> {
        let exclusions = self.exclusions;
        let mut pieces = Vec::new();
        let mut failure = None;
        let live = self.live.load(Ordering::Relaxed);
        exclusions.sections[span].visit_occupied_screened(
            cancel,
            |piece, _| Ok(rects_meet(expand(piece, exclusions.distance), self.screen)),
            |piece, height| {
                if let Err(error) =
                    reserve_owned(&mut pieces, 1, exclusions.owned_bytes.saturating_add(live))
                {
                    failure = Some(error);
                    return Ok(false);
                }
                pieces.push((piece.clone(), height));
                Ok(true)
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        let loaded = SpanPieces { span, pieces };
        let bytes = loaded.bytes();
        super::super::remaining_geometry_units(
            exclusions.retained,
            exclusions
                .owned_bytes
                .saturating_add(live)
                .saturating_add(bytes),
        )?;
        self.live.fetch_add(bytes, Ordering::Relaxed);
        Ok(loaded)
    }
}

/// Builds one nominal scope's clearance grid; shared by parallel workers.
struct NominalBuilder<'r, 'a> {
    layers: &'r [PrintLayer],
    sections: &'r [super::super::SectionSpan],
    nominal: &'r [NominalScope],
    distance: f64,
    owned_bytes: usize,
    raster: &'r Raster<'a>,
}

impl NominalBuilder<'_, '_> {
    fn build(&self, at: usize) -> Result<(Bits, NominalRecord, usize), ManufacturingError> {
        let raster = self.raster;
        let cancel = raster.cancel;
        let scope = &self.nominal[at];
        let section = &self.sections[scope.index];
        // The clearance is what the scope deposits: its actual bead
        // capsules, not its section material, which can hold unprintable
        // slivers the walls omit. Capsules are unioned in bounded chunks.
        const CHUNK: usize = 4096;
        let mut bits = Bits::default();
        let mut digest = beads::BeadDigest::default();
        let mut work = 0;
        let objects = self
            .layers
            .get(section.start as usize..(section.end as usize).min(self.layers.len()))
            .unwrap_or(&[]);
        let mut chunk = Vec::new();
        reserve_owned(&mut chunk, CHUNK, self.owned_bytes)?;
        let mut flush = |chunk: &mut Vec<([f64; 2], [f64; 2], f64)>| {
            let beads = raster.capsules(chunk.drain(..))?;
            bits = raster.union(&bits, &beads)?;
            Ok::<(), ManufacturingError>(())
        };
        beads::visit_object_beads(
            objects.iter().flat_map(|layer| &layer.paths),
            &mut work,
            cancel,
            |bead| {
                digest.add(&bead);
                chunk.push((bead.a, bead.b, (bead.half + self.distance).next_up()));
                if chunk.len() == CHUNK {
                    flush(&mut chunk)?;
                }
                Ok(())
            },
        )?;
        flush(&mut chunk)?;
        let record = NominalRecord {
            section: scope.index,
            floor: scope.floor,
            ceiling: scope.ceiling,
            beads: digest,
        };
        Ok((bits, record, work))
    }
}

/// The section spans a bead slab at layer `l` can touch: its own and both
/// neighbours.
fn slab_spans(sections: &[super::super::SectionSpan], l: usize) -> std::ops::RangeInclusive<usize> {
    let here = span_at(sections, l as u32);
    here.saturating_sub(1)..=(here + 1).min(sections.len() - 1)
}

/// One section span's occupied pieces whose clearance can reach the grid.
fn span_pieces(
    sections: &[super::super::SectionSpan],
    schedule: &[super::super::LayerElevation],
    owned_bytes: usize,
    span: usize,
    cell_bounds: [[f64; 2]; 2],
    distance: f64,
    cancel: &AtomicBool,
) -> Result<Vec<Piece>, ManufacturingError> {
    let section = &sections[span];
    let interval = [
        schedule[section.start as usize].lower_z,
        schedule[(section.end - 1) as usize].z,
    ];
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
                if let Err(error) = reserve_owned(&mut pieces, 1, owned_bytes) {
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
                    first: None,
                });
            }
            Ok(true)
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(pieces)
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

/// Contact tops at or below this height keep columns: near the bed a
/// branch saves little and a column is sturdier (Bambu's vertical zone).
const TREE_MIN_HEIGHT_MM: f64 = 5.0;

/// Whether an overhang component goes to branches: it is wide enough that
/// its tips merge into fewer, thinner trunks than its column (a lip or ledge
/// narrower than the tip spacing is a cheap thin column but a row of full
/// branches), and a branch from most of it can walk around the part to the
/// bed. The strip beside the part's wall, inside the clearance and a tip
/// radius, never reaches, so the bar is 70%.
fn prefers_tree(
    raster: &Raster<'_>,
    guide: &super::tree::Guide,
    params: &super::tree::Params,
    component: &Bits,
    l: usize,
) -> Result<bool, ManufacturingError> {
    let cells = &raster.frame;
    let (mut over, mut reaching, mut total) = (0usize, 0usize, 0usize);
    component.for_each_cell(|c, r| {
        let p = cells.centre(c, r);
        total += 1;
        over += usize::from(guide.shadowed(l, p));
        reaching += usize::from(guide.reach(l, p) >= params.tip);
    });
    if total == 0 || 10 * reaching < 7 * total {
        return Ok(false);
    }
    let inner = raster.erode(component, cells.cell())?.count();
    let boundary = total.saturating_sub(inner).max(1);
    // Mean width 2A/P, with the boundary cells measuring the perimeter.
    let width = 2.0 * total as f64 * cells.cell() / boundary as f64;
    Ok(width >= params.spacing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Rect, coord};

    fn object_layer(paths: Vec<DepositionPath>) -> PrintLayer {
        use spiling_contracts::manufacturing::PlanarRegionCoverage;
        PrintLayer {
            index: 0,
            hatch_course: 0,
            z_mm: 0.2,
            lower_z_mm: 0.0,
            height_mm: 0.2,
            section_z_mm: 0.1,
            paths,
            skin_command_fits: Vec::new(),
            region_coverage: PlanarRegionCoverage {
                section_area_mm2: 0.0,
                source_section_area_mm2: 0.0,
                surface_added_area_mm2: 0.0,
                wall_area_mm2: 0.0,
                bottom_skin_area_mm2: 0.0,
                top_skin_area_mm2: 0.0,
                solid_interior_area_mm2: 0.0,
                sparse_interior_area_mm2: 0.0,
                intentional_void_area_mm2: 0.0,
                protected_hole_area_mm2: 0.0,
                nominal_deposition_volume_mm3: 0.0,
                support_area_mm2: 0.0,
                support_nominal_deposition_volume_mm3: 0.0,
                unsupported_overhang_mm: 0.0,
                support_omitted_wall_mm: 0.0,
                thin_widened_mm: 0.0,
                thin_omitted_area_mm2: 0.0,
                process_regions: Vec::new(),
                process_interfaces: Vec::new(),
                separator_area_mm2: 0.0,
                process_diaphragms: Vec::new(),
                diaphragm_area_mm2: 0.0,
                skin_bridge_domains: Vec::new(),
                web_roof_domains: Vec::new(),
            },
        }
    }

    fn bead(kind: DepositionKind, from: [f64; 2], to: [f64; 2], width_mm: f64) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind,
            owner: None,
            width: spiling_contracts::manufacturing::WidthProfile::Constant { width_mm },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![[from[0], from[1], 0.2], [to[0], to[1], 0.2]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }
    }

    /// An open-below pocket without a stitched roof cap is still the roof of
    /// the layer whose own material closes over it: bed-rooted support may
    /// enter it. A void the layer leaves open, a bounded void, or a contact
    /// top above the pocket's top grants nothing.
    #[test]
    fn roofless_open_below_pocket_is_held_by_the_layer_closing_over_it() {
        use geo::{LineString, polygon};
        let cancel = AtomicBool::new(false);
        let pocket = polygon![(x: 10., y: 0.), (x: 20., y: 0.), (x: 20., y: 4.), (x: 10., y: 4.)];
        let open = VoidScope::new(
            spiling_geometry::ProtectedVoidHeightScope::OpenBelow {
                rim_mm: 1.0,
                upper_mm: 5.0,
            },
            None,
        );
        let layer = |lower: f64, z: f64| {
            let mut layer = object_layer(vec![bead(
                DepositionKind::BottomSkin,
                [12.0, 2.0],
                [18.0, 2.0],
                0.45,
            )]);
            layer.lower_z_mm = lower;
            layer.z_mm = z;
            layer
        };
        let target = layer(4.95, 5.1);
        let top = layer(4.6, 4.75);
        let failure = super::super::strokes::Failure {
            path: 0,
            first: 0,
            last: 1,
            start: 0.0,
            end: 1.0,
            span: 6.0,
        };
        let closed =
            vec![polygon![(x: 0., y: -5.), (x: 30., y: -5.), (x: 30., y: 9.), (x: 0., y: 9.)]];
        let around = vec![Polygon::new(
            closed[0].exterior().clone(),
            vec![LineString::from(vec![
                (10., 0.),
                (20., 0.),
                (20., 4.),
                (10., 4.),
                (10., 0.),
            ])],
        )];
        let access = |scope: &VoidScope, ceiling: &[Polygon<f64>], top: &PrintLayer| {
            scope
                .allows_roof_access(
                    &pocket,
                    ceiling,
                    super::super::Root::Bed,
                    &target,
                    top,
                    &target.paths[0],
                    &failure,
                    0.45,
                    &mut 0,
                    &cancel,
                )
                .unwrap()
        };
        assert!(access(&open, &closed, &top));
        assert!(
            !access(&open, &around, &top),
            "the layer leaves the pocket open"
        );
        assert!(
            !access(&open, &closed, &target),
            "contact top above the pocket"
        );
        let bounded = VoidScope::new(
            spiling_geometry::ProtectedVoidHeightScope::Bounded {
                lower_mm: 1.0,
                upper_mm: 5.0,
            },
            None,
        );
        assert!(!access(&bounded, &closed, &top));
    }

    /// A scope excludes its deposited bead capsules at the decoded
    /// enclosure, and nothing of its section material that deposits no bead
    /// (an unprintable sliver the walls omit).
    #[test]
    fn nominal_scope_excludes_deposited_beads_not_omitted_material() {
        let cancel = AtomicBool::new(false);
        let bounds = [[0.0, 0.0], [2.0, 2.0]];
        let raster = Raster {
            frame: CellFrame::new(bounds, SUPPORT_CELL_MM).unwrap(),
            cancel: &cancel,
            admit: &|_| Ok(()),
            work: AtomicUsize::new(0),
        };
        // The bead covers y 0.2..0.68; the sliver at y 1.6 deposits nothing.
        let footprint =
            Rect::new(coord! { x: 0.2, y: 0.2 }, coord! { x: 1.8, y: 0.68 }).to_polygon();
        let sliver = Rect::new(coord! { x: 0.2, y: 1.6 }, coord! { x: 1.8, y: 1.65 }).to_polygon();
        let nominal_distance = clearance_distance_in_frame(bounds, bounds, 0.3).unwrap();
        let probe = [0.975, 1.025];
        let nominal_only = raster
            .outward(std::slice::from_ref(&footprint), nominal_distance)
            .unwrap();
        assert!(!raster.holds_all(&nominal_only, probe));

        let sections = [super::super::super::SectionSpan {
            start: 0,
            end: 1,
            material: std::sync::Arc::new(MultiPolygon::new(vec![footprint, sliver])),
            process_domains: Vec::new(),
            occupied: None,
            source_area_mm2: 0.0,
        }];
        let layers = [object_layer(vec![
            bead(DepositionKind::Perimeter, [0.2, 0.44], [1.8, 0.44], 0.48),
            bead(DepositionKind::Support, [0.2, 1.9], [1.8, 1.9], 0.4),
        ])];
        let builder = NominalBuilder {
            layers: &layers,
            sections: &sections,
            nominal: &[NominalScope {
                index: 0,
                floor: 0.0,
                ceiling: 0.2,
            }],
            distance: (nominal_distance + raster::exact::VERIFIER_ENCLOSURE_MM).next_up(),
            owned_bytes: 0,
            raster: &raster,
        };
        let (excluded, _, _) = builder.build(0).unwrap();
        assert!(raster.holds_all(&excluded, probe));
        assert!(!raster.holds_all(&excluded, [probe[0], 1.075]));
        assert!(!raster.holds_all(&excluded, [probe[0], 1.625]));
    }

    #[test]
    fn void_boundary_screen_includes_exactly_one_decoded_enclosure() {
        let mask = Rect::new(coord! { x: 0.0, y: 0.0 }, coord! { x: 1.0, y: 1.0 }).to_polygon();
        let requested = 0.3;
        let enclosure = raster::exact::VERIFIER_ENCLOSURE_MM;
        let mask_bounds = [[0.0, 0.0], [1.0, 1.0]];
        let near = [[1.0 + requested + enclosure * 0.5, 0.0], [2.0, 1.0]];
        assert_eq!(
            candidate_clearance_distance_in_bounds(&mask, Some(near), requested).unwrap(),
            None
        );
        let distance = projection::void_placement_distance(&mask, near, requested)
            .unwrap()
            .unwrap();
        let base = clearance_distance_in_frame(mask_bounds, near, requested).unwrap();
        assert!((distance - base - enclosure).abs() < 1.0e-7);
    }

    #[test]
    fn landing_recovers_only_centred_donor_seams_inside_full_ledge_cells() {
        let cancel = AtomicBool::new(false);
        let raster = Raster {
            frame: CellFrame::new([[0.0, 0.0], [4.0, 4.0]], SUPPORT_CELL_MM).unwrap(),
            cancel: &cancel,
            admit: &|_| Ok(()),
            work: AtomicUsize::new(0),
        };
        for transpose in [false, true] {
            let strip = |low, high| {
                let point = |along, across| {
                    let [x, y] = if transpose {
                        [across, along]
                    } else {
                        [along, across]
                    };
                    coord! { x: x, y: y }
                };
                Rect::new(point(low, 3.05), point(high, 3.60)).to_polygon()
            };
            let ledge = raster.inward(&[strip(3.0, 3.55)]).unwrap();
            let pair = [strip(3.0, 3.201), strip(3.202, 3.55)];
            let base = raster.inward(&pair).unwrap();
            let repaired = raster.landing(&ledge, &pair).unwrap();
            assert!(repaired.count() > base.count());
            assert!(raster.difference(&base, &repaired).unwrap().is_empty());
            assert!(raster.difference(&repaired, &ledge).unwrap().is_empty());

            let no_ledge = raster.inward(&pair).unwrap();
            assert_eq!(raster.landing(&no_ledge, &pair).unwrap(), base);
            for split in [(3.224, 3.227), (3.201, 3.252)] {
                let pair = [strip(3.0, split.0), strip(split.1, 3.55)];
                assert_eq!(
                    raster.landing(&ledge, &pair).unwrap(),
                    raster.inward(&pair).unwrap()
                );
            }
        }
    }
}
