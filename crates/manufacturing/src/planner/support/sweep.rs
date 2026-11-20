// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Layer-major support construction.
//!
//! A legacy column is one (demand, root) prism: the demand layer's buffered
//! material, minus all occupancy from the root's first layer up to the
//! contact top, minus the nominal clearance it meets on the way up. The same
//! sets come from two shared passes over the layers:
//!
//! - down: `A[l] = (A[l + 1] ∪ seeds[l]) − occupancy[l] − voids[l]`;
//! - up: `G[l] = A[l] ∩ (G[l − 1] ∪ ground[l]) − nominal[l]`,
//!
//! where ground is the bed at the first layer and each accepted part landing
//! at its first support layer. For one demand the passes reproduce its bed and
//! landing prisms exactly. With several demands support may also stand on
//! lower grounded support, which is physically valid. Each layer's occupancy,
//! void and nominal clearance is built once and shared by every demand.

use super::*;

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

pub(super) struct SweepOutput {
    pub support: Vec<Rc<OwnedMaterial>>,
    pub interfaces: Vec<Rc<OwnedMaterial>>,
    pub contact_tops: Vec<Option<usize>>,
    /// (protected ordinal, layer, grounded relieved support at that layer).
    pub access: Vec<(usize, usize, Rc<OwnedMaterial>)>,
}

/// One independently grounded set of demands: the ordinary channel, or the
/// bed-only demands a single open-below void relieves.
struct Channel {
    relieved: Option<usize>,
    seeds: Vec<Option<MultiPolygon<f64>>>,
    /// Per demand target, in first-seen order: its contact tops in demand
    /// order, its seed and the seed's bounds. Every demand on one target
    /// shares that target's buffered section as its seed.
    interface_seeds: Vec<InterfaceSeed>,
    tops: Vec<(usize, usize)>,
    active: Vec<Option<Rc<OwnedMaterial>>>,
}

struct InterfaceSeed {
    target: usize,
    tops: Vec<usize>,
    seed: MultiPolygon<f64>,
    bounds: Option<[[f64; 2]; 2]>,
}

impl Channel {
    fn add_interface_seed(&mut self, top: usize, target: usize, seed: &MultiPolygon<f64>) {
        if let Some(entry) = self.interface_seeds.iter_mut().find(|e| e.target == target) {
            if !entry.tops.contains(&top) {
                entry.tops.push(top);
            }
            return;
        }
        self.interface_seeds.push(InterfaceSeed {
            target,
            tops: vec![top],
            seed: seed.clone(),
            bounds: material_bounds(seed),
        });
    }
}

fn bounds_meet(a: Option<[[f64; 2]; 2]>, b: Option<[[f64; 2]; 2]>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
        }
        _ => false,
    }
}

/// Chained fixed-frame differences leave runs of nearly collinear vertices
/// where nearly equal clearance arcs from successive layers cross. A
/// Douglas–Peucker pass at this tolerance moves a boundary far inside the
/// offset tolerance already reserved in every clearance distance; actual bead
/// clearance is still proven independently at the end.
const COLLINEAR_MM: f64 = 1.0e-4;

fn drop_micro_edges(material: &mut MultiPolygon<f64>) {
    use geo::Simplify;
    let polygons = std::mem::take(&mut material.0);
    for polygon in polygons {
        let simplified = polygon.simplify(COLLINEAR_MM);
        if simplified.exterior().0.len() < 4 {
            continue;
        }
        let (exterior, interiors) = simplified.into_inner();
        let holes = interiors
            .into_iter()
            .filter(|hole| hole.0.len() >= 4)
            .collect();
        material.0.push(Polygon::new(exterior, holes));
    }
}

/// Union disjoint-or-overlapping parts pairwise in a fixed index order, in
/// parallel, so the result does not depend on scheduling.
fn merge_pairwise(
    frame: &projection::Frame,
    mut parts: Vec<MultiPolygon<f64>>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    use rayon::prelude::*;
    while parts.len() > 1 {
        check_cancel(cancel)?;
        parts = parts
            .par_chunks(2)
            .map(|pair| match pair {
                [left, right] => {
                    if left.0.is_empty() {
                        right.clone()
                    } else if right.0.is_empty() {
                        left.clone()
                    } else {
                        frame.union(left, right)
                    }
                }
                [single] => single.clone(),
                _ => unreachable!("chunks of at most two"),
            })
            .collect();
    }
    Ok(parts.pop().unwrap_or_else(|| MultiPolygon(Vec::new())))
}

/// Occupied pieces of the spans the down pass is currently inside, each
/// collected once. A bead slab may touch an adjacent span's pieces.
#[derive(Default)]
struct SpanPieces {
    spans: Vec<(usize, Vec<Piece>)>,
    last: Option<(Vec<PieceKey>, MultiPolygon<f64>)>,
}

/// (span, piece index) of a piece selected for one layer's clearance.
type PieceKey = (usize, usize);

/// A raw occupied piece, its height scope clipped to its span, and its bounds.
type Piece = (Polygon<f64>, [f64; 2], [[f64; 2]; 2]);

impl Sweep<'_> {
    fn union(&self, a: &MultiPolygon<f64>, b: &MultiPolygon<f64>) -> MultiPolygon<f64> {
        if a.0.is_empty() {
            return b.clone();
        }
        if b.0.is_empty() {
            return a.clone();
        }
        self.frame.union_local(a, b)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "two ordered passes share local state"
    )]
    pub(super) fn run(
        &self,
        scopes: &[VoidScope],
        segments: &mut usize,
        anchors: &mut AnchorWorkspace,
        anchor_work: &mut usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<SweepOutput, ManufacturingError> {
        let mut timing = crate::timing::Clock::new("support.sweep");
        let layers = self.layers;
        let width = self.recipe.bead_width_mm;
        let empty = OwnedMaterial::new(MultiPolygon(Vec::new())).into_shared();
        let mut support = vec![Rc::clone(&empty); layers.len()];
        let mut interfaces = vec![Rc::clone(&empty); layers.len()];
        let mut contact_tops = vec![None; layers.len()];
        let mut access = Vec::new();

        // Seeds: each demand's buffered target material at its contact top.
        let mut channels: Vec<Channel> = Vec::new();
        let new_channel = |relieved| Channel {
            relieved,
            seeds: vec![None; layers.len()],
            interface_seeds: Vec::new(),
            tops: Vec::new(),
            active: vec![None; layers.len()],
        };
        channels.push(new_channel(None));
        let mut buffered: Option<(usize, MultiPolygon<f64>)> = None;
        let mut max_top = None::<usize>;
        for demand in self.demands {
            check_cancel(cancel)?;
            let path = &layers[demand.target].paths[demand.failure.path];
            let Some(top) = contact_layer(layers, demand.target, path, self.gap, work, cancel)?
            else {
                continue;
            };
            if buffered.as_ref().is_none_or(|(t, _)| *t != demand.target) {
                let candidate =
                    &self.sections[span_at(self.sections, demand.target as u32)].material;
                charge_boolean(candidate, candidate, work, self.foreign_work, cancel)?;
                buffered = Some((
                    demand.target,
                    candidate.buffer_with_style(
                        BufferStyle::new(width / 2.0).line_join(LineJoin::Bevel),
                    ),
                ));
            }
            let seed = &buffered.as_ref().expect("buffered demand").1;
            // A bed-rooted demand may enter one open-below void whose roof it
            // supports; that relief is per demand, so it gets its own channel.
            let corridor_floor = layers[0].lower_z_mm;
            let mut relieved = None;
            for (ordinal, (mask, scope)) in self.protected.0.iter().zip(scopes).enumerate() {
                if !void_reaches_corridor(&scope.height, corridor_floor, layers[top].z_mm) {
                    continue;
                }
                if candidate_clearance_distance(mask, seed, self.requested)?.is_none() {
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
            let at = if relieved.is_none() {
                0
            } else if let Some(at) = channels.iter().position(|c| c.relieved == relieved) {
                at
            } else {
                channels.push(new_channel(relieved));
                channels.len() - 1
            };
            let channel = &mut channels[at];
            let slot = &mut channel.seeds[top];
            *slot = Some(
                slot.take()
                    .map_or_else(|| seed.clone(), |old| self.union(&old, seed)),
            );
            channel.add_interface_seed(top, demand.target, seed);
            channel.tops.push((top, demand.target));
            max_top = Some(max_top.map_or(top, |m| m.max(top)));
            if relieved.is_some() {
                // The ordinary channel still owns this demand's part roots.
                let ordinary = &mut channels[0];
                let slot = &mut ordinary.seeds[top];
                *slot = Some(
                    slot.take()
                        .map_or_else(|| seed.clone(), |old| self.union(&old, seed)),
                );
                ordinary.tops.push((top, demand.target));
                ordinary.add_interface_seed(top, demand.target, seed);
            }
        }
        drop(buffered);
        let Some(max_top) = max_top else {
            return Ok(SweepOutput {
                support,
                interfaces,
                contact_tops,
                access,
            });
        };

        timing.mark("seeds");
        let mut span_pieces = SpanPieces::default();
        // Down pass: carry every channel's candidate through occupancy and voids.
        let mut carried: Vec<MultiPolygon<f64>> =
            channels.iter().map(|_| MultiPolygon(Vec::new())).collect();
        let mut applied: Vec<std::collections::BTreeSet<PieceKey>> =
            channels.iter().map(|_| Default::default()).collect();
        let mut void_clearance: Vec<Option<(u64, MultiPolygon<f64>)>> =
            vec![None; self.protected.0.len()];
        for l in (0..=max_top).rev() {
            check_cancel(cancel)?;
            let seeds: Vec<Option<MultiPolygon<f64>>> = channels
                .iter_mut()
                .map(|channel| channel.seeds[l].take())
                .collect();
            if carried.iter().all(|carry| carry.0.is_empty()) && seeds.iter().all(Option::is_none) {
                continue;
            }
            let slab = program_coordinates::project_xyz6_bead_height_mm(
                layers[l].z_mm,
                layers[l].height_mm,
            )?;
            self.subtract_occupancy(
                l,
                slab,
                &mut span_pieces,
                &mut carried,
                seeds,
                &mut applied,
                work,
                cancel,
            )?;
            for (ordinal, (mask, scope)) in self.protected.0.iter().zip(scopes).enumerate() {
                if !void_reaches_corridor(&scope.height, slab[0], slab[1]) {
                    continue;
                }
                for (channel, carry) in channels.iter().zip(&mut carried) {
                    if carry.0.is_empty() || channel.relieved == Some(ordinal) {
                        continue;
                    }
                    let Some(distance) = candidate_clearance_distance(mask, carry, self.requested)?
                    else {
                        continue;
                    };
                    // A void's clearance depends only on the void and the
                    // distance; most layers reuse the previous one.
                    let cached = &mut void_clearance[ordinal];
                    if cached
                        .as_ref()
                        .is_none_or(|(bits, _)| *bits != distance.to_bits())
                    {
                        *cached = Some((
                            distance.to_bits(),
                            offset_polygon_owned(
                                mask,
                                distance,
                                self.retained,
                                self.owned_bytes,
                                &support,
                                &interfaces,
                                &[self.protected],
                                cancel,
                            )?,
                        ));
                    }
                    let clearance = &cached.as_ref().expect("cached void clearance").1;
                    charge_boolean(carry, clearance, work, self.foreign_work, cancel)?;
                    *carry = self.frame.difference_local(carry, clearance);
                }
            }
            for carry in &mut carried {
                drop_micro_edges(carry);
            }
            for (channel, carry) in channels.iter_mut().zip(&carried) {
                if !carry.0.is_empty() {
                    channel.active[l] = Some(OwnedMaterial::new(carry.clone()).into_shared());
                }
            }
        }
        drop(carried);
        timing.mark("down_pass");

        // Up pass: keep only grounded support, clipped by nominal clearance.
        // Part-root acceptance and the per-scope clearance caches come first.
        // Nominal clearance distributes over the grounded intersection, so it
        // runs on every active layer in parallel; the grounded chain then
        // stays in layer order.
        let live = Live::of(&support, &interfaces, &[self.protected]);
        let mut outside: Vec<Option<Vec<beads::BeadEdge>>> = vec![None; self.nominal.len()];
        let mut offsets: Vec<Option<MultiPolygon<f64>>> = vec![None; self.nominal.len()];
        let mut landed: Vec<Vec<Vec<MultiPolygon<f64>>>> = Vec::new();
        for channel in &channels {
            let mut per_layer = vec![Vec::new(); layers.len()];
            for (l, active) in channel.active.iter().enumerate() {
                let Some(active) = active.as_ref().filter(|_| channel.relieved.is_none()) else {
                    continue;
                };
                for root in self.roots {
                    let Root::Part {
                        object_layer,
                        first_support_layer,
                        landing,
                    } = *root
                    else {
                        continue;
                    };
                    if first_support_layer != l {
                        continue;
                    }
                    check_cancel(cancel)?;
                    let landing = &self.landings[landing];
                    charge_boolean(active, landing, work, self.foreign_work, cancel)?;
                    let candidate = self.frame.intersection_local(active, landing);
                    if candidate.0.is_empty() {
                        continue;
                    }
                    let accepted = self.anchored_on(
                        &candidate,
                        l,
                        object_layer,
                        &support,
                        &interfaces,
                        segments,
                        anchors,
                        anchor_work,
                        work,
                        cancel,
                    )?;
                    if !accepted.0.is_empty() {
                        per_layer[l].push(accepted);
                    }
                }
            }
            landed.push(per_layer);
        }
        for l in 0..=max_top {
            if channels.iter().any(|channel| channel.active[l].is_some()) {
                self.prepare_nominal(l, &mut outside, &mut offsets, live, work, cancel)?;
            }
        }
        let mut cleared: Vec<Vec<Option<MultiPolygon<f64>>>> = {
            use rayon::prelude::*;
            let mut tasks: Vec<(usize, usize, MultiPolygon<f64>)> = Vec::new();
            for (c, channel) in channels.iter_mut().enumerate() {
                for (l, slot) in channel.active.iter_mut().enumerate() {
                    if let Some(active) = slot.take() {
                        let material = match Rc::try_unwrap(active) {
                            Ok(owned) => owned.material,
                            Err(shared) => shared.material.clone(),
                        };
                        tasks.push((c, l, material));
                    }
                }
            }
            let clearance = NominalClearance {
                layers,
                nominal: self.nominal,
                frame: self.frame,
                offsets: &offsets,
                outside: &outside,
                gap: self.gap,
                requested: self.requested,
                retained: self.retained,
                owned_bytes: self.owned_bytes,
                // Each task counts from zero; its total is charged in order below.
                foreign_work: 0,
                live,
            };
            let working_set = crate::working_set::current();
            type Cleared = (usize, usize, MultiPolygon<f64>, usize);
            let results: Vec<Result<Cleared, ManufacturingError>> = tasks
                .into_par_iter()
                .map(|(c, l, material)| {
                    crate::working_set::scope(working_set, || {
                        let mut local = 0usize;
                        clearance
                            .apply(l, OwnedMaterial::new(material), &mut local, cancel)
                            .map(|kept| (c, l, kept.material, local))
                    })
                })
                .collect();
            let mut cleared = vec![vec![None; layers.len()]; channels.len()];
            let (mut units, mut bytes) = (0usize, 0usize);
            for result in results {
                let (c, l, kept, local) = result?;
                charge_owned_work(work, self.foreign_work, local, cancel)?;
                let owned = OwnedMaterial::new(kept);
                units = units.saturating_add(owned.units);
                bytes = bytes.saturating_add(owned.bytes);
                cleared[c][l] = Some(owned.material);
            }
            live.budget(
                self.retained.saturating_add(units),
                self.owned_bytes.saturating_add(bytes),
                &[],
            )?;
            cleared
        };
        for (c, channel) in channels.iter_mut().enumerate() {
            let mut grounded = MultiPolygon(Vec::new());
            let mut part_bands: Vec<(f64, MultiPolygon<f64>)> = Vec::new();
            for l in 0..=max_top {
                check_cancel(cancel)?;
                let Some(kept) = cleared[c][l].take() else {
                    grounded = MultiPolygon(Vec::new());
                    part_bands.clear();
                    continue;
                };
                let mut ground = MultiPolygon(Vec::new());
                for accepted in std::mem::take(&mut landed[c][l]) {
                    ground = self.union(&ground, &accepted);
                    part_bands.push((layers[l].lower_z_mm, accepted));
                }
                // The bed grounds all of layer 0.
                let mut column = OwnedMaterial::new(if l == 0 {
                    kept
                } else {
                    let base = self.union(&grounded, &ground);
                    charge_boolean(&kept, &base, work, self.foreign_work, cancel)?;
                    self.frame.intersection_local(&kept, &base)
                });
                drop_micro_edges(&mut column.material);
                // Higher support stands on this layer before the touching
                // sheet trims a column ending here: the sheet sits on that
                // column's top only, as in its own fixed-XY prism.
                grounded = column.material.clone();
                if column.0.is_empty() {
                    part_bands.clear();
                    continue;
                }
                if channel.tops.iter().any(|(top, _)| *top == l) {
                    let live = Live::of(&support, &interfaces, &[self.protected]);
                    column = self.subtract_sheet(l, column, live, work, cancel)?;
                    drop_micro_edges(&mut column.material);
                    if column.0.is_empty() {
                        continue;
                    }
                }
                region::canonicalize(&mut column.material);
                let column = OwnedMaterial::new(column.material).into_shared();
                // A target's seed is the same for all its demands, so one
                // intersection per target serves its contact top and its
                // interface band; disjoint bounds mean an empty intersection.
                let column_bounds = material_bounds(&column.material);
                let mut interface = MultiPolygon(Vec::new());
                for entry in &channel.interface_seeds {
                    let contact = if contact_tops[entry.target].is_none() {
                        entry.tops.iter().copied().find(|top| *top >= l)
                    } else {
                        None
                    };
                    let band = entry.tops.iter().any(|top| {
                        *top >= l && layers[l].z_mm > layers[*top].z_mm - self.interface_thickness
                    });
                    if (contact.is_none() && !band) || !bounds_meet(entry.bounds, column_bounds) {
                        continue;
                    }
                    let meet = self.frame.intersection_local(&column.material, &entry.seed);
                    if meet.0.is_empty() {
                        continue;
                    }
                    // As for a legacy column: a demand has its contact top once
                    // any kept layer up to it reaches the demand's seed.
                    if let Some(top) = contact {
                        contact_tops[entry.target] = Some(top);
                    }
                    if band {
                        interface = self.union(&interface, &meet);
                    }
                }
                part_bands
                    .retain(|(floor, _)| layers[l].lower_z_mm < floor + self.interface_thickness);
                for (_, band) in &part_bands {
                    interface = self.union(
                        &interface,
                        &self.frame.intersection_local(&column.material, band),
                    );
                }
                let mut cache = None;
                let (units, bytes) = geometry_receipt(&support, &interfaces, &[&column]);
                merge_shared(
                    &mut support[l],
                    &column,
                    &mut cache,
                    self.retained.saturating_add(units),
                    self.owned_bytes.saturating_add(bytes),
                    work,
                    self.foreign_work,
                    cancel,
                )?;
                if !interface.0.is_empty() {
                    let interface = OwnedMaterial::new(interface).into_shared();
                    let mut cache = None;
                    let (units, bytes) = geometry_receipt(&support, &interfaces, &[&interface]);
                    merge_shared(
                        &mut interfaces[l],
                        &interface,
                        &mut cache,
                        self.retained.saturating_add(units),
                        self.owned_bytes.saturating_add(bytes),
                        work,
                        self.foreign_work,
                        cancel,
                    )?;
                }
                if let Some(ordinal) = channel.relieved {
                    access.push((ordinal, l, Rc::clone(&column)));
                }
            }
        }
        timing.mark("up_pass");
        // A layer that is interface throughout shares one owner with its
        // support region: raster hatches it as whole interface.
        for (region, interface) in support.iter().zip(&mut interfaces) {
            check_cancel(cancel)?;
            if interface.0.is_empty() || Rc::ptr_eq(region, interface) {
                continue;
            }
            charge_boolean(region, interface, work, self.foreign_work, cancel)?;
            if self.frame.difference(region, interface).0.is_empty() {
                *interface = Rc::clone(region);
            }
        }
        Ok(SweepOutput {
            support,
            interfaces,
            contact_tops,
            access,
        })
    }

    /// Subtract every raw occupied piece touching this layer's bead slab.
    ///
    /// A carried region already excludes every piece applied at a higher
    /// layer, and a piece's height scope is contiguous, so only pieces newly
    /// touching this slab can change it. A seed entering here takes the full
    /// set. A span's pieces are collected once; adjacent spans may touch a
    /// slab. Pieces whose clearance cannot reach the operand are skipped.
    #[expect(clippy::too_many_arguments, reason = "per-channel down-pass state")]
    fn subtract_occupancy(
        &self,
        l: usize,
        slab: [f64; 2],
        cache: &mut SpanPieces,
        carried: &mut [MultiPolygon<f64>],
        seeds: Vec<Option<MultiPolygon<f64>>>,
        applied: &mut [std::collections::BTreeSet<PieceKey>],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let here = span_at(self.sections, l as u32);
        // The pass descends: spans above the next one up are finished.
        cache.spans.retain(|(span, _)| *span <= here + 1);
        let window = here.saturating_sub(1)..=(here + 1).min(self.sections.len() - 1);
        for span in window.clone() {
            if cache.spans.iter().any(|(cached, _)| *cached == span) {
                continue;
            }
            let section = &self.sections[span];
            let interval = [
                self.schedule[section.start as usize].lower_z,
                self.schedule[(section.end - 1) as usize].z,
            ];
            let mut pieces = Vec::new();
            section.visit_occupied_screened(
                cancel,
                |_, _| Ok(true),
                |piece, height| {
                    let scope = height.map_or(interval, |height| {
                        [height[0].max(interval[0]), height[1].min(interval[1])]
                    });
                    if scope[0] <= scope[1]
                        && let Some(bounds) = material_bounds(&MultiPolygon(vec![piece.clone()]))
                    {
                        pieces.push((piece.clone(), scope, bounds));
                    }
                    Ok(true)
                },
            )?;
            *work = work.saturating_add(pieces.len());
            cache.spans.push((span, pieces));
        }
        for set in applied.iter_mut() {
            set.retain(|(span, _)| window.contains(span));
        }
        let touching: Vec<(PieceKey, [[f64; 2]; 2])> = cache
            .spans
            .iter()
            .filter(|(span, _)| window.contains(span))
            .flat_map(|(span, pieces)| {
                pieces
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, scope, _))| scope[0] <= slab[1] && scope[1] >= slab[0])
                    .map(move |(index, (_, _, bounds))| ((*span, index), *bounds))
            })
            .collect();
        for ((carry, seed), applied) in carried.iter_mut().zip(seeds).zip(applied.iter_mut()) {
            if !carry.0.is_empty() {
                let near = self.screen(carry);
                let fresh: Vec<PieceKey> = touching
                    .iter()
                    .filter(|(key, bounds)| !applied.contains(key) && near(bounds))
                    .map(|(key, _)| *key)
                    .collect();
                if !fresh.is_empty() {
                    let clearance = self.clearance(cache, &fresh, cancel)?;
                    charge_boolean(carry, &clearance, work, self.foreign_work, cancel)?;
                    *carry = self.frame.difference_local(carry, &clearance);
                    applied.extend(fresh);
                }
            }
            if let Some(seed) = seed {
                // The top layer's own material lies inside its occupancy
                // clearance, so only the overhang band of a seed can survive
                // it; screening against that band keeps the piece set local.
                let material = &self.sections[span_at(self.sections, l as u32)].material;
                charge_boolean(&seed, material, work, self.foreign_work, cancel)?;
                let seed = self.frame.difference_local(&seed, material);
                let near = self.screen(&seed);
                let mut full: Vec<PieceKey> = touching
                    .iter()
                    .filter(|(_, bounds)| near(bounds))
                    .map(|(key, _)| *key)
                    .collect();
                full.sort_unstable();
                let entered = if full.is_empty() {
                    seed
                } else {
                    let clearance = if let Some((last, clearance)) = &cache.last
                        && *last == full
                    {
                        clearance.clone()
                    } else {
                        let clearance = self.clearance(cache, &full, cancel)?;
                        cache.last = Some((full.clone(), clearance.clone()));
                        clearance
                    };
                    charge_boolean(&seed, &clearance, work, self.foreign_work, cancel)?;
                    self.frame.difference_local(&seed, &clearance)
                };
                applied.extend(full);
                charge_boolean(carry, &entered, work, self.foreign_work, cancel)?;
                *carry = self.union(carry, &entered);
            }
            if carry.0.is_empty() {
                applied.clear();
            }
        }
        *work = work.saturating_add(touching.len());
        Ok(())
    }

    /// Bounds screen: whether a piece's clearance can reach `material`.
    fn screen(&self, material: &MultiPolygon<f64>) -> impl Fn(&[[f64; 2]; 2]) -> bool + use<> {
        let reach = separation_reach(self.frame.distance);
        let mut boxes: Vec<[[f64; 2]; 2]> = material
            .0
            .iter()
            .filter_map(|polygon| material_bounds(&MultiPolygon(vec![polygon.clone()])))
            .map(|b| {
                [
                    [b[0][0] - reach, b[0][1] - reach],
                    [b[1][0] + reach, b[1][1] + reach],
                ]
            })
            .collect();
        boxes.sort_unstable_by(|a, b| a[0][0].total_cmp(&b[0][0]));
        let widest = boxes
            .iter()
            .map(|b| b[1][0] - b[0][0])
            .fold(0.0f64, f64::max);
        move |piece: &[[f64; 2]; 2]| {
            let from = boxes.partition_point(|b| b[0][0] < piece[0][0] - widest);
            boxes[from..]
                .iter()
                .take_while(|b| b[0][0] <= piece[1][0])
                .any(|b| b[1][0] >= piece[0][0] && b[0][1] <= piece[1][1] && b[1][1] >= piece[0][1])
        }
    }

    /// Offset union of the given cached pieces at the shared clearance reach.
    fn clearance(
        &self,
        cache: &SpanPieces,
        keys: &[PieceKey],
        cancel: &AtomicBool,
    ) -> Result<MultiPolygon<f64>, ManufacturingError> {
        // Union bounded chunks in parallel, then merge results pairwise in a
        // fixed index order, so output does not depend on scheduling.
        use rayon::prelude::*;
        check_cancel(cancel)?;
        let frame = self.frame;
        let pieces: Vec<&Polygon<f64>> = keys
            .iter()
            .map(|&(span, index)| {
                &cache
                    .spans
                    .iter()
                    .find(|(cached, _)| *cached == span)
                    .expect("cached span pieces")
                    .1[index]
                    .0
            })
            .collect();
        let parts: Vec<MultiPolygon<f64>> = pieces
            .par_chunks(1 << 7)
            .map(|chunk| {
                let batch = MultiPolygon(chunk.iter().map(|piece| (*piece).clone()).collect());
                union_pieces_in_frame(&batch, Some(frame))
            })
            .collect();
        drop(pieces);
        let mut merged = merge_pairwise(frame, parts, cancel)?;
        // Unions of many padded facet pieces carry the same near-collinear
        // noise as chained differences; a convolution offset of it is
        // pathologically slow.
        drop_micro_edges(&mut merged);
        region::offset_admitted_in_frame(
            &merged,
            self.frame.distance,
            self.frame,
            |_, _, _| Ok(()),
            cancel,
        )
    }

    /// Build the per-scope clearance and outside-bead caches every scope
    /// reaching layer `l` needs, so `NominalClearance::apply` only reads them.
    fn prepare_nominal(
        &self,
        l: usize,
        outside: &mut [Option<Vec<beads::BeadEdge>>],
        offsets: &mut [Option<MultiPolygon<f64>>],
        live: Live,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let layers = self.layers;
        let [lower, upper] = nominal_band(layers, l)?;
        for (at, scope) in self.nominal.iter().enumerate() {
            check_cancel(cancel)?;
            if program_coordinates::enclosed_vertical_gap_mm(scope.floor, upper)? >= self.gap {
                break;
            }
            if program_coordinates::enclosed_vertical_gap_mm(lower, scope.ceiling)? >= self.gap {
                continue;
            }
            if offsets[at].is_none() {
                use rayon::prelude::*;
                let polygons = &self.sections[scope.index].material.0;
                let distances = polygons
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
                // Each polygon's clearance is independent; offsets run in
                // parallel against one inventory and are kept in input order.
                let retained = self.retained.saturating_add(live.units);
                let owned_bytes = self.owned_bytes.saturating_add(live.bytes);
                let working_set = crate::working_set::current();
                let built = polygons
                    .par_iter()
                    .zip(&distances)
                    .map(|(polygon, &distance)| {
                        crate::working_set::scope(working_set, || {
                            offset_polygon_admitted_in(
                                polygon,
                                distance,
                                retained,
                                owned_bytes,
                                cancel,
                            )
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .collect::<Result<Vec<MultiPolygon<f64>>, ManufacturingError>>()?;
                // One clearance per scope: every layer it reaches subtracts it
                // with a single overlay instead of one per section polygon.
                offsets[at] = Some(merge_pairwise(self.frame, built, cancel)?);
            }
            if scope.beads.is_some() && outside[at].is_none() {
                let section = &self.sections[scope.index];
                let objects = layers
                    .get(section.start as usize..(section.end as usize).min(layers.len()))
                    .unwrap_or(&[]);
                outside[at] = Some(beads::outside_material(
                    &section.material,
                    objects.iter().flat_map(|layer| &layer.paths),
                    BEAD_OUTSIDE_ALLOWANCE_MM,
                    work,
                    cancel,
                )?);
            }
        }
        Ok(())
    }

    fn subtract_sheet(
        &self,
        top: usize,
        column: OwnedMaterial,
        live: Live,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<OwnedMaterial, ManufacturingError> {
        let proximity = index_column(&column, self.retained, self.owned_bytes, live, &[&column])?;
        let admission = BeadStaging {
            retained: self.retained,
            owned_bytes: self.owned_bytes,
            live,
        };
        let mut batch = stage_touching_sheet(
            self.layers,
            self.sections,
            top,
            &column,
            &proximity,
            self.requested,
            &admission,
            work,
            cancel,
        )?;
        Ok(subtract_staged(
            &mut batch,
            &column,
            self.requested,
            self.retained,
            self.owned_bytes,
            live,
            work,
            self.foreign_work,
            cancel,
            None,
        )?
        .unwrap_or(column))
    }

    /// The legacy part-root rule: keep each landing polygon only if a
    /// support hatch on it anchors to the landing layer's model roads.
    #[expect(clippy::too_many_arguments, reason = "shared admission owners")]
    fn anchored_on(
        &self,
        candidate: &MultiPolygon<f64>,
        l: usize,
        object_layer: usize,
        support: &[Rc<OwnedMaterial>],
        interfaces: &[Rc<OwnedMaterial>],
        segments: &mut usize,
        anchors: &mut AnchorWorkspace,
        anchor_work: &mut usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<MultiPolygon<f64>, ManufacturingError> {
        let layers = self.layers;
        let width = self.recipe.bead_width_mm;
        let mut accepted = MultiPolygon(Vec::new());
        for polygon in &candidate.0 {
            check_cancel(cancel)?;
            let single = OwnedMaterial::new(MultiPolygon(vec![polygon.clone()]));
            let (units, bytes) = geometry_receipt(support, interfaces, &[self.protected, &single]);
            let domains = raster::Domains::new(
                &single,
                &single,
                true,
                width,
                self.retained + units,
                self.owned_bytes + bytes,
                anchors.retained_bytes(),
                work,
                self.foreign_work,
                cancel,
            )?;
            let preferred = region::HatchAxis::for_policy(
                RectilinearAxisPolicy::AlternatingXY,
                super::super::hatch_key(self.schedule, l),
            );
            for axis in [preferred, preferred.orthogonal()] {
                let hatch = region::HatchLayer {
                    roof: None,
                    width_mm: width,
                    z: layers[l].z_mm,
                    lower_z: layers[l].lower_z_mm,
                    height: layers[l].height_mm,
                    axis,
                    top_axis: axis,
                    recipe: self.recipe,
                };
                let (mut paths, receipt) = domains.paths(
                    &single,
                    1.0,
                    &hatch,
                    *segments,
                    work,
                    self.foreign_work,
                    cancel,
                )?;
                anchors.set_budget(domains.anchor_budget(&paths)?);
                raster::retain_anchored(
                    &mut paths,
                    receipt.paths,
                    std::iter::empty(),
                    layers[object_layer]
                        .paths
                        .iter()
                        .filter(|path| path.kind.is_model()),
                    self.recipe,
                    self.retained + units,
                    domains.owned_bytes(),
                    anchors,
                    anchor_work,
                    cancel,
                )?;
                if !paths.is_empty() {
                    accepted.0.push(polygon.clone());
                    break;
                }
            }
            anchors.set_budget(0);
        }
        Ok(accepted)
    }
}

/// Lower bead floor and upper segment ceiling of layer `l`'s physical slab.
fn nominal_band(layers: &[PrintLayer], l: usize) -> Result<[f64; 2], ManufacturingError> {
    let lower =
        program_coordinates::project_xyz6_bead_height_mm(layers[l].z_mm, layers[l].height_mm)?[0];
    let upper = program_coordinates::project_xyz6_segment_bead_height_mm(
        layers[l].z_mm,
        layers[l].z_mm,
        layers[l].z_mm - layers[l].lower_z_mm,
    )?[1];
    Ok([lower, upper])
}

/// Read-only inputs of nominal clearance, shareable across worker threads.
struct NominalClearance<'a> {
    layers: &'a [PrintLayer],
    nominal: &'a [NominalScope],
    frame: &'a projection::Frame,
    offsets: &'a [Option<MultiPolygon<f64>>],
    outside: &'a [Option<Vec<beads::BeadEdge>>],
    gap: f64,
    requested: f64,
    retained: usize,
    owned_bytes: usize,
    foreign_work: usize,
    live: Live,
}

impl NominalClearance<'_> {
    /// Remove every reaching nominal scope's clearance and its actual
    /// outside-bead clearance from `column`. Clearance distributes over
    /// intersection, so applying it before the grounded intersection keeps
    /// the same set.
    fn apply(
        &self,
        l: usize,
        mut column: OwnedMaterial,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<OwnedMaterial, ManufacturingError> {
        let [lower, upper] = nominal_band(self.layers, l)?;
        for (at, scope) in self.nominal.iter().enumerate() {
            check_cancel(cancel)?;
            if column.0.is_empty() {
                break;
            }
            if program_coordinates::enclosed_vertical_gap_mm(scope.floor, upper)? >= self.gap {
                break;
            }
            if program_coordinates::enclosed_vertical_gap_mm(lower, scope.ceiling)? >= self.gap {
                continue;
            }
            let clearance = self.offsets[at]
                .as_ref()
                .expect("prepared nominal clearance");
            if let (Some(a), Some(b)) = (material_bounds(clearance), material_bounds(&column))
                && !(a[1][0] < b[0][0]
                    || a[0][0] > b[1][0]
                    || a[1][1] < b[0][1]
                    || a[0][1] > b[1][1])
            {
                charge_boolean(&column, clearance, work, self.foreign_work, cancel)?;
                column =
                    OwnedMaterial::new(self.frame.difference_local(&column.material, clearance));
                region::check_size(&column)?;
            }
            if let Some(beads) = scope.beads
                && !column.0.is_empty()
                && !ColumnScreen::new(&column, self.requested)?
                    .is_some_and(|screen| screen.rejects(beads))
            {
                let edges = self.outside[at]
                    .as_deref()
                    .expect("prepared outside bead edges");
                let proximity = index_column(
                    &column,
                    self.retained,
                    self.owned_bytes,
                    self.live,
                    &[&column],
                )?;
                let mut batch = stage_object_beads(
                    edges,
                    &column,
                    &proximity,
                    self.requested,
                    &BeadStaging {
                        retained: self.retained,
                        owned_bytes: self.owned_bytes,
                        live: self.live,
                    },
                    work,
                    cancel,
                )?;
                if !batch.pieces.0.is_empty()
                    && let Some(next) = subtract_staged(
                        &mut batch,
                        &column,
                        self.requested,
                        self.retained,
                        self.owned_bytes,
                        self.live,
                        work,
                        self.foreign_work,
                        cancel,
                        None,
                    )?
                {
                    column = next;
                }
            }
        }
        Ok(column)
    }
}
