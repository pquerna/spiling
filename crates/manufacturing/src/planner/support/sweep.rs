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
    interface_seeds: Vec<(usize, usize, MultiPolygon<f64>)>,
    tops: Vec<(usize, usize)>,
    active: Vec<Option<Rc<OwnedMaterial>>>,
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
        self.frame.union(a, b)
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
            channel
                .interface_seeds
                .push((top, demand.target, seed.clone()));
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
                ordinary
                    .interface_seeds
                    .push((top, demand.target, seed.clone()));
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

        let mut span_pieces = SpanPieces::default();
        // Down pass: carry every channel's candidate through occupancy and voids.
        let mut carried: Vec<MultiPolygon<f64>> =
            channels.iter().map(|_| MultiPolygon(Vec::new())).collect();
        for l in (0..=max_top).rev() {
            check_cancel(cancel)?;
            for (channel, carry) in channels.iter_mut().zip(&mut carried) {
                if let Some(seed) = channel.seeds[l].take() {
                    charge_boolean(carry, &seed, work, self.foreign_work, cancel)?;
                    *carry = self.union(carry, &seed);
                }
            }
            if carried.iter().all(|carry| carry.0.is_empty()) {
                continue;
            }
            let slab = program_coordinates::project_xyz6_bead_height_mm(
                layers[l].z_mm,
                layers[l].height_mm,
            )?;
            self.subtract_occupancy(l, slab, &mut span_pieces, &mut carried, work, cancel)?;
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
                    let clearance = offset_polygon_owned(
                        mask,
                        distance,
                        self.retained,
                        self.owned_bytes,
                        &support,
                        &interfaces,
                        &[self.protected],
                        cancel,
                    )?;
                    charge_boolean(carry, &clearance, work, self.foreign_work, cancel)?;
                    *carry = self.frame.difference(carry, &clearance);
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

        // Up pass: keep only grounded support, clipped by nominal clearance.
        let mut outside: Vec<Option<Vec<beads::BeadEdge>>> = vec![None; self.nominal.len()];
        let mut offsets: Vec<Option<Vec<MultiPolygon<f64>>>> = vec![None; self.nominal.len()];
        for channel in &mut channels {
            let mut grounded = MultiPolygon(Vec::new());
            let mut part_bands: Vec<(f64, MultiPolygon<f64>)> = Vec::new();
            for l in 0..=max_top {
                check_cancel(cancel)?;
                let Some(active) = channel.active[l].take() else {
                    grounded = MultiPolygon(Vec::new());
                    part_bands.clear();
                    continue;
                };
                let mut ground = if l == 0 {
                    active.material.clone()
                } else {
                    MultiPolygon(Vec::new())
                };
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
                        let landing = &self.landings[landing];
                        charge_boolean(&active, landing, work, self.foreign_work, cancel)?;
                        let candidate = self.frame.intersection(&active, landing);
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
                            part_bands.push((layers[l].lower_z_mm, accepted.clone()));
                            ground = self.union(&ground, &accepted);
                        }
                    }
                }
                let base = self.union(&grounded, &ground);
                charge_boolean(&active, &base, work, self.foreign_work, cancel)?;
                let mut column = OwnedMaterial::new(self.frame.intersection(&active, &base));
                drop(active);
                if !column.0.is_empty() {
                    column = self.subtract_nominal(
                        l,
                        column,
                        &mut outside,
                        &mut offsets,
                        &support,
                        &interfaces,
                        work,
                        cancel,
                    )?;
                }
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
                    column = self.subtract_sheet(l, column, &support, &interfaces, work, cancel)?;
                    drop_micro_edges(&mut column.material);
                    if column.0.is_empty() {
                        continue;
                    }
                }
                region::canonicalize(&mut column.material);
                let column = OwnedMaterial::new(column.material).into_shared();
                // As for a legacy column: a demand has its contact top once
                // any kept layer up to it reaches the demand's seed.
                for (top, target, seed) in &channel.interface_seeds {
                    if *top >= l
                        && contact_tops[*target].is_none()
                        && !self.frame.intersection(&column.material, seed).0.is_empty()
                    {
                        contact_tops[*target] = Some(*top);
                    }
                }
                let mut interface = MultiPolygon(Vec::new());
                for (top, _, seed) in &channel.interface_seeds {
                    if *top >= l && layers[l].z_mm > layers[*top].z_mm - self.interface_thickness {
                        interface = self
                            .union(&interface, &self.frame.intersection(&column.material, seed));
                    }
                }
                part_bands
                    .retain(|(floor, _)| layers[l].lower_z_mm < floor + self.interface_thickness);
                for (_, band) in &part_bands {
                    interface =
                        self.union(&interface, &self.frame.intersection(&column.material, band));
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

    /// Subtract every raw occupied piece touching this layer's bead slab from
    /// each carried candidate. A span's pieces are collected once; a layer
    /// whose piece set equals the previous layer's reuses its clearance.
    fn subtract_occupancy(
        &self,
        l: usize,
        slab: [f64; 2],
        cache: &mut SpanPieces,
        carried: &mut [MultiPolygon<f64>],
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
        // Only pieces whose clearance can reach a carried polygon matter.
        let reach = separation_reach(self.frame.distance);
        let mut boxes: Vec<[[f64; 2]; 2]> = carried
            .iter()
            .flat_map(|carry| carry.0.iter())
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
        let touches = |piece: &[[f64; 2]; 2]| {
            let from = boxes.partition_point(|b| b[0][0] < piece[0][0] - widest);
            boxes[from..]
                .iter()
                .take_while(|b| b[0][0] <= piece[1][0])
                .any(|b| b[1][0] >= piece[0][0] && b[0][1] <= piece[1][1] && b[1][1] >= piece[0][1])
        };
        let mut selected: Vec<PieceKey> = Vec::new();
        for (span, pieces) in &cache.spans {
            if !window.contains(span) {
                continue;
            }
            for (index, (_, scope, bounds)) in pieces.iter().enumerate() {
                if scope[0] <= slab[1] && scope[1] >= slab[0] && touches(bounds) {
                    selected.push((*span, index));
                }
            }
        }
        selected.sort_unstable();
        if selected.is_empty() {
            cache.last = None;
            return Ok(());
        }
        let clearance = if let Some((last, clearance)) = &cache.last
            && *last == selected
        {
            clearance.clone()
        } else {
            let mut merged = MultiPolygon(Vec::new());
            for chunk in selected.chunks(1 << 10) {
                check_cancel(cancel)?;
                let batch = MultiPolygon(
                    chunk
                        .iter()
                        .map(|&(span, index)| {
                            let pieces = &cache
                                .spans
                                .iter()
                                .find(|(cached, _)| *cached == span)
                                .expect("cached span pieces")
                                .1;
                            pieces[index].0.clone()
                        })
                        .collect(),
                );
                let part = union_pieces_in_frame(&batch, Some(self.frame));
                merged = self.union(&merged, &part);
            }
            let clearance = region::offset_admitted_in_frame(
                &merged,
                self.frame.distance,
                self.frame,
                |_, _, _| Ok(()),
                cancel,
            )?;
            cache.last = Some((selected, clearance.clone()));
            clearance
        };
        for carry in carried.iter_mut() {
            if carry.0.is_empty() {
                continue;
            }
            charge_boolean(carry, &clearance, work, self.foreign_work, cancel)?;
            *carry = self.frame.difference(carry, &clearance);
        }
        Ok(())
    }

    #[expect(clippy::too_many_arguments, reason = "shared admission owners")]
    fn subtract_nominal(
        &self,
        l: usize,
        mut column: OwnedMaterial,
        outside: &mut [Option<Vec<beads::BeadEdge>>],
        offsets: &mut [Option<Vec<MultiPolygon<f64>>>],
        support: &[Rc<OwnedMaterial>],
        interfaces: &[Rc<OwnedMaterial>],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<OwnedMaterial, ManufacturingError> {
        let layers = self.layers;
        let lower =
            program_coordinates::project_xyz6_bead_height_mm(layers[l].z_mm, layers[l].height_mm)?
                [0];
        let upper = program_coordinates::project_xyz6_segment_bead_height_mm(
            layers[l].z_mm,
            layers[l].z_mm,
            layers[l].z_mm - layers[l].lower_z_mm,
        )?[1];
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
            if offsets[at].is_none() {
                let mut built = Vec::new();
                for polygon in &self.sections[scope.index].material.0 {
                    let rect = polygon.bounding_rect().ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::NativeAnalysisIndeterminate,
                            "native outer operand has no finite bounds",
                        )
                    })?;
                    let distance = clearance_distance_in_frame(
                        [[rect.min().x, rect.min().y], [rect.max().x, rect.max().y]],
                        self.frame.bounds,
                        self.requested,
                    )?;
                    built.push(offset_polygon_owned(
                        polygon,
                        distance,
                        self.retained,
                        self.owned_bytes,
                        support,
                        interfaces,
                        &[self.protected, &column],
                        cancel,
                    )?);
                }
                offsets[at] = Some(built);
            }
            let column_bounds = material_bounds(&column);
            for clearance in offsets[at].as_deref().expect("cached nominal clearance") {
                let (Some(a), Some(b)) = (material_bounds(clearance), column_bounds) else {
                    continue;
                };
                if a[1][0] < b[0][0] || a[0][0] > b[1][0] || a[1][1] < b[0][1] || a[0][1] > b[1][1]
                {
                    continue;
                }
                charge_boolean(&column, clearance, work, self.foreign_work, cancel)?;
                column = OwnedMaterial::new(self.frame.difference(&column.material, clearance));
                region::check_size(&column)?;
                if column.0.is_empty() {
                    break;
                }
            }
            if let Some(beads) = scope.beads
                && !column.0.is_empty()
                && !ColumnScreen::new(&column, self.requested)?
                    .is_some_and(|screen| screen.rejects(beads))
            {
                if outside[at].is_none() {
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
                let edges = outside[at].as_deref().expect("cached outside bead edges");
                let proximity = index_column(
                    &column,
                    self.retained,
                    self.owned_bytes,
                    support,
                    interfaces,
                    &[self.protected, &column],
                )?;
                let mut batch = stage_object_beads(
                    edges,
                    &column,
                    &proximity,
                    self.requested,
                    &BeadStaging {
                        retained: self.retained,
                        owned_bytes: self.owned_bytes,
                        support,
                        interfaces,
                        protected: self.protected,
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
                        support,
                        interfaces,
                        self.protected,
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

    fn subtract_sheet(
        &self,
        top: usize,
        column: OwnedMaterial,
        support: &[Rc<OwnedMaterial>],
        interfaces: &[Rc<OwnedMaterial>],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<OwnedMaterial, ManufacturingError> {
        let proximity = index_column(
            &column,
            self.retained,
            self.owned_bytes,
            support,
            interfaces,
            &[self.protected, &column],
        )?;
        let admission = BeadStaging {
            retained: self.retained,
            owned_bytes: self.owned_bytes,
            support,
            interfaces,
            protected: self.protected,
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
            support,
            interfaces,
            self.protected,
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
