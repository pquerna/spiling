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

use super::grid::{Bits, CellFrame, Raster, SUPPORT_CELL_MM};
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
        let empty = OwnedMaterial::new(MultiPolygon(Vec::new())).into_shared();
        let mut support = vec![Rc::clone(&empty); layers.len()];
        let mut interfaces = vec![Rc::clone(&empty); layers.len()];
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
                support,
                interfaces,
                contact_tops,
                access,
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
        for l in (0..=max_top).rev() {
            check_cancel(cancel)?;
            let seeds: Vec<Option<Bits>> = channels
                .iter_mut()
                .map(|channel| channel.seeds[l].take())
                .collect();
            if carried.iter().all(Bits::is_empty) && seeds.iter().all(Option::is_none) {
                continue;
            }
            let slab = program_coordinates::project_xyz6_bead_height_mm(
                layers[l].z_mm,
                layers[l].height_mm,
            )?;
            self.load_pieces(l, &mut window, cell_bounds, distance, work, cancel)?;
            // A carry already excludes every piece that touched a layer above
            // since it was seeded, and a piece's height scope is contiguous,
            // so only pieces newly touching this slab can change it.
            let touches = |piece: &Piece| piece.scope[0] <= slab[1] && piece.scope[1] >= slab[0];
            if carried.iter().any(|carry| !carry.is_empty()) {
                let fresh: Vec<&Polygon<f64>> = window
                    .iter()
                    .flat_map(|(_, pieces)| pieces)
                    .filter(|piece| !piece.applied && touches(piece))
                    .filter(|piece| {
                        carried
                            .iter()
                            .any(|carry| carry.meets_rect(&cells, piece.reach))
                    })
                    .map(|piece| &piece.polygon)
                    .collect();
                if !fresh.is_empty() {
                    let occupancy = raster.outward(fresh, distance)?;
                    for carry in &mut carried {
                        carry.subtract(&occupancy);
                    }
                }
            }
            if seeds.iter().any(Option::is_some) {
                // The top layer's own material lies inside its occupancy
                // clearance; removing it first keeps the piece set local to
                // the seed's overhang band.
                let material = &self.sections[span_at(self.sections, l as u32)].material;
                let material = raster.outward(&material.0, 0.0)?;
                for (carry, seed) in carried.iter_mut().zip(&seeds) {
                    let Some(seed) = seed else {
                        continue;
                    };
                    let mut entered = seed.clone();
                    entered.subtract(&material);
                    let near: Vec<&Polygon<f64>> = window
                        .iter()
                        .flat_map(|(_, pieces)| pieces)
                        .filter(|piece| touches(piece) && entered.meets_rect(&cells, piece.reach))
                        .map(|piece| &piece.polygon)
                        .collect();
                    if !near.is_empty() {
                        entered.subtract(&raster.outward(near, distance)?);
                    }
                    *carry = raster.union(carry, &entered)?;
                }
            }
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
        for bits in voids.into_iter().flatten().flatten() {
            release(&bits);
        }
        timing.mark("down_pass");

        // Up pass: keep only grounded support, clipped by nominal clearance,
        // one layer at a time across every channel.
        let mut grounded: Vec<Bits> = channels.iter().map(|_| Bits::default()).collect();
        let mut part_bands: Vec<Vec<(f64, Bits)>> = channels.iter().map(|_| Vec::new()).collect();
        let mut clearance: Vec<Option<Bits>> = vec![None; self.nominal.len()];
        let mut footprint: Option<(usize, Bits)> = None;
        let mut regions: Vec<(usize, Bits, Bits)> = Vec::new();
        let mut relieved: Vec<(usize, usize, Bits)> = Vec::new();
        for l in 0..=max_top {
            check_cancel(cancel)?;
            if channels.iter().all(|channel| channel.active[l].is_none()) {
                grounded.iter_mut().for_each(|g| *g = Bits::default());
                part_bands.iter_mut().for_each(Vec::clear);
                continue;
            }
            let reaching =
                self.prepare_nominal(l, &raster, &mut clearance, &hold, &release, work)?;
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
                if channel.relieved.is_none() {
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
                        let landing = raster.inward(&self.landings[landing].0)?;
                        let mut accepted = raster.intersection(&active, &landing)?;
                        if accepted.is_empty() {
                            continue;
                        }
                        if footprint.as_ref().is_none_or(|(at, _)| *at != object_layer) {
                            footprint =
                                Some((object_layer, self.road_footprint(&raster, object_layer)?));
                        }
                        let roads = &footprint.as_ref().expect("road footprint").1;
                        accepted = raster.intersection(&accepted, roads)?;
                        if accepted.is_empty() {
                            continue;
                        }
                        ground = raster.union(&ground, &accepted)?;
                        part_bands[c].push((layers[l].lower_z_mm, accepted));
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
                reserve_owned(&mut regions, 1, self.owned_bytes)?;
                regions.push((l, layer_support, layer_interface));
            }
            charge(work)?;
        }
        drop(clearance);
        drop(footprint);
        drop(channels);
        timing.mark("up_pass");

        // Polygons only at the output, each inside its kept cells. Layers are
        // independent, so they convert in parallel and land in layer order.
        let contours = {
            use rayon::prelude::*;
            let working_set = crate::working_set::current();
            let raster = &raster;
            let convert = |bits: &Bits| -> Result<OwnedMaterial, ManufacturingError> {
                crate::working_set::scope(working_set, || {
                    let mut material = raster.contour(bits)?;
                    region::canonicalize(&mut material);
                    region::check_size(&material)?;
                    Ok(OwnedMaterial::new(material))
                })
            };
            let layers: Vec<Result<(OwnedMaterial, Option<OwnedMaterial>), ManufacturingError>> =
                regions
                    .par_iter()
                    .map(|(_, region, interface)| {
                        let region_polygons = convert(region)?;
                        let interface_polygons = if interface.is_empty() || interface == region {
                            None
                        } else {
                            Some(convert(interface)?)
                        };
                        Ok((region_polygons, interface_polygons))
                    })
                    .collect();
            let columns: Vec<Result<OwnedMaterial, ManufacturingError>> = relieved
                .par_iter()
                .map(|(_, _, column)| convert(column))
                .collect();
            (layers, columns)
        };
        charge(work)?;
        let (layer_contours, column_contours) = contours;
        let (mut units, mut bytes) = (0usize, 0usize);
        for ((l, _, interface), converted) in regions.iter().zip(layer_contours) {
            let (region_polygons, interface_polygons) = converted?;
            units = units.saturating_add(region_polygons.units);
            bytes = bytes.saturating_add(region_polygons.bytes);
            let region_polygons = region_polygons.into_shared();
            if !interface.is_empty() {
                interfaces[*l] = match interface_polygons {
                    // A layer that is interface throughout shares one owner
                    // with its support region: raster hatches it as whole
                    // interface.
                    None => Rc::clone(&region_polygons),
                    Some(polygons) => {
                        units = units.saturating_add(polygons.units);
                        bytes = bytes.saturating_add(polygons.bytes);
                        polygons.into_shared()
                    }
                };
            }
            support[*l] = region_polygons;
            super::super::remaining_geometry_units(
                retained.saturating_add(units),
                owned_bytes
                    .saturating_add(live.load(Ordering::Relaxed))
                    .saturating_add(bytes),
            )?;
        }
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
        Ok(SweepOutput {
            support,
            interfaces,
            contact_tops,
            access,
        })
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
    /// pass has left for good is released.
    fn prepare_nominal(
        &self,
        l: usize,
        raster: &Raster<'_>,
        clearance: &mut [Option<Bits>],
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
                }
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

    /// Cells lying entirely inside the nominal footprint of the object layer's
    /// model roads: each road's centreline dilated by half its bead width,
    /// unioned, then rasterized inward. Support may stand only over them.
    fn road_footprint(
        &self,
        raster: &Raster<'_>,
        object_layer: usize,
    ) -> Result<Bits, ManufacturingError> {
        let bounds = raster.frame.bounds();
        let mut capsules: Vec<Vec<[f64; 2]>> = Vec::new();
        let mut extent = bounds;
        for path in self.layers[object_layer]
            .paths
            .iter()
            .filter(|path| path.kind.is_model())
        {
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
                if !rects_meet(reach, bounds) || half.is_nan() || half <= 0.0 {
                    continue;
                }
                extent = union_frame(Some(extent), reach);
                reserve_owned(&mut capsules, 1, self.owned_bytes)?;
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
        let union: Vec<Polygon<f64>> = shapes
            .into_iter()
            .map(|shape| {
                let mut rings = shape.into_iter().map(|ring| {
                    let mut line =
                        geo::LineString(ring.into_iter().map(geo::Coord::from).collect());
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
}

/// Adjacent roads planned tangent (row spacing equal to bead width) share
/// only a line; inflating each footprint by this much joins them so the
/// shared line is not mistaken for a boundary. It is far below a grid cell.
const FOOTPRINT_JOIN_MM: f64 = 1.0e-3;

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
