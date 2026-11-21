// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Layer heights for stepped walls, decided once before planning.
//!
//! A wall line rests on the line below it when it steps out no more than
//! half a bead. Between two neighbouring layers, the part of the upper
//! section farther than that outside the lower one is wall that would hang.
//! A layer is halved when its midpoint section actually shrinks that excess
//! (a slope, which thinner layers anchor); a step whose excess stays the same
//! however thin the layers get (a ledge or ceiling) is left to support. The
//! schedule is settled here from sections alone, so the plan is constructed
//! once and never split and retried.

use super::{LayerElevation, native, region, tolerant::SectionStore};
use crate::{check_cancel, error};
use geo::{Area, BooleanOps, MultiPolygon};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;
use std::{cell::Cell, mem::size_of, rc::Rc};

/// The thinnest layer a step may ask for, as a fraction of the nominal
/// layer height. Steeper walls belong to support.
const STEP_FLOOR_FRACTION: f64 = 0.25;

/// Arithmetic roundoff of halving a millimetre-scale height, far below any
/// commanded Z step.
const FLOOR_ROUNDOFF_MM: f64 = 1e-9;

#[cfg(test)]
thread_local! {
    static CACHE_ENABLED: Cell<bool> = const { Cell::new(true) };
}

#[cfg(test)]
pub(super) fn without_cache<T>(operation: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            CACHE_ENABLED.set(self.0);
        }
    }
    let _restore = Restore(CACHE_ENABLED.replace(false));
    operation()
}

/// Split layers whose walls would hang past half a bead where thinner
/// layers anchor them. Only with support enabled, where the alternative is
/// scaffold; explicit Off prints authored overhangs as reported. Returns the
/// source sections that can move to span collection.
pub(super) fn split_for_slopes(
    input: &super::CompilerInput<'_>,
    admission: &super::preflight::Admission,
    schedule: &mut Vec<LayerElevation>,
    outside_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<SectionStore, ManufacturingError> {
    let intent = &input.intent;
    if matches!(intent.recipe.supports, SupportPolicy::Disabled) || schedule.len() < 2 {
        return Ok(SectionStore::default());
    }
    let width = intent.recipe.bead_width_mm;
    let nominal = intent.select_layer_height()?.height_mm;
    let floor = (nominal * STEP_FLOOR_FRACTION)
        .max(intent.effective_deposition_limits().min_bead_height_mm);
    let noise_mm2 = width * spiling_geometry::tolerant::TolerantPolicy::default().profile_mm;
    super::remaining_geometry_units(
        0,
        outside_bytes
            .saturating_add(
                schedule
                    .capacity()
                    .saturating_mul(size_of::<LayerElevation>()),
            )
            .saturating_add(size_of::<Live>() + 2 * size_of::<usize>()),
    )?;
    let mut steps = Steps {
        models: &admission.models,
        width,
        noise_mm2,
        cached: Vec::new(),
        sections: SectionStore::default(),
        active_row_bytes: 0,
        excess: Vec::new(),
        live: Rc::new(Live::default()),
        outside_bytes,
        state_bytes: 0,
        work,
        cancel,
        cache_enabled: {
            #[cfg(test)]
            {
                CACHE_ENABLED.get()
            }
            #[cfg(not(test))]
            {
                true
            }
        },
    };
    if steps.cache_enabled {
        // Hold at most one share for optional raw rows; active section pairs
        // and overlays keep the remaining allowance.
        let allowance = crate::working_set::bytes()
            .saturating_sub(outside_bytes)
            .saturating_sub(state_bytes(schedule, &Vec::new()))
            / 3;
        for wave in schedule.chunks(rayon::current_num_threads().max(1)) {
            check_cancel(cancel)?;
            steps
                .sections
                .prefetch(&admission.models, wave, allowance, cancel);
        }
    }
    split_where_thinner_anchors(
        schedule,
        floor,
        |height| intent.admits_deposition(width, height),
        &mut steps,
        noise_mm2,
        cancel,
    )?;
    steps.sections.keep_scheduled(schedule);
    Ok(steps.sections)
}

/// The samples of the halves `split_resolved_layer` makes.
fn half_samples(layer: &LayerElevation) -> [f64; 2] {
    let midpoint = layer.lower_z + (layer.z - layer.lower_z) * 0.5;
    [
        layer.lower_z + (midpoint - layer.lower_z) * 0.5,
        midpoint + (layer.z - midpoint) * 0.5,
    ]
}

/// Halve layers while it shrinks the hanging-wall excess between sampled
/// sections. `excess(lower_sample, upper_sample)` is that area; each pair
/// that cannot progress is settled once.
fn split_where_thinner_anchors(
    schedule: &mut Vec<LayerElevation>,
    floor: f64,
    admits: impl Fn(f64) -> bool,
    analysis: &mut impl Analysis,
    noise_mm2: f64,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let before = schedule.len();
    let least = floor - FLOOR_ROUNDOFF_MM;
    let mut settled = Vec::new();
    let mut index = 1;
    while index < schedule.len() {
        check_cancel(cancel)?;
        analysis.retain(state_bytes(schedule, &settled))?;
        let pair = (
            schedule[index - 1].sample.to_bits(),
            schedule[index].sample.to_bits(),
        );
        analysis.charge(lookup_work(settled.len()))?;
        if settled.binary_search(&pair).is_ok() {
            index += 1;
            continue;
        }
        let hanging = analysis.excess(schedule[index - 1].sample, schedule[index].sample)?;
        // The thicker eligible layer of the pair; the first rests on the bed.
        let height = |layer: &LayerElevation| layer.z - layer.lower_z;
        let target = [index - 1, index]
            .into_iter()
            .filter(|&k| {
                let half = height(&schedule[k]) * 0.5;
                k >= 1 && half >= least && admits(half)
            })
            .max_by(|&a, &b| {
                height(&schedule[a])
                    .total_cmp(&height(&schedule[b]))
                    .then(a.cmp(&b))
            });
        let Some(target) = target.filter(|_| hanging > noise_mm2) else {
            settle(&mut settled, pair, schedule, analysis)?;
            index += 1;
            continue;
        };
        // Excess over every pair the split replaces, before and after.
        let below = schedule[target - 1].sample;
        let above = schedule.get(target + 1).map(|layer| layer.sample);
        let middle = schedule[target].sample;
        let [lower, upper] = half_samples(&schedule[target]);
        let mut now = analysis.excess(below, middle)?;
        let mut split = analysis.excess(below, lower)? + analysis.excess(lower, upper)?;
        if let Some(above) = above {
            now += analysis.excess(middle, above)?;
            split += analysis.excess(upper, above)?;
        }
        if split < now - noise_mm2 {
            match native::split_resolved_layer_admitted(
                schedule,
                target as u32,
                least,
                "stepped wall layer height",
                |schedule| {
                    if schedule.len() == schedule.capacity() {
                        let capacity = schedule.capacity().saturating_mul(2).max(1);
                        analysis.retain(state_bytes(schedule, &settled).saturating_add(
                            capacity.saturating_mul(size_of::<LayerElevation>()),
                        ))?;
                        analysis.charge(schedule.len())?;
                        schedule
                            .try_reserve_exact(capacity - schedule.len())
                            .map_err(|_| resource("stepped-wall schedule allocation refused"))?;
                        analysis.retain(state_bytes(schedule, &settled))?;
                    }
                    analysis.charge(schedule.len() - target)
                },
            ) {
                // Recheck from the lowest changed pair.
                Ok(()) => index = target.max(1),
                Err(failure) if failure.code == ManufacturingErrorCode::ResourceLimit => {
                    return Err(failure);
                }
                // A piece the process cannot command is left to support.
                Err(_) => {
                    settle(&mut settled, pair, schedule, analysis)?;
                    index += 1;
                }
            }
        } else {
            settle(&mut settled, pair, schedule, analysis)?;
            index += 1;
        }
    }
    Ok(schedule.len() - before)
}

type Pair = (u64, u64);

fn lookup_work(entries: usize) -> usize {
    (usize::BITS - entries.leading_zeros()) as usize + 1
}

/// The pinned geo conversion holds both polygon results and, for the largest
/// ring, RDP indices plus disjoint pending subchains and old/new growth buffers.
pub(super) fn section_conversion(
    section: &spiling_geometry::tolerant::TolerantSection,
) -> Result<(usize, usize), ManufacturingError> {
    let rings = section.loops.len();
    let mut points = 0usize;
    let mut largest = 0usize;
    let mut work = 0usize;
    for ring in &section.loops {
        let count = ring.points.len().saturating_add(1);
        points = points.saturating_add(count);
        largest = largest.max(count);
        super::boolean::charge(
            &mut work,
            count
                .checked_mul(count)
                .ok_or_else(|| resource("stepped-wall section simplification work overflow"))?,
        )?;
    }
    super::boolean::charge(
        &mut work,
        rings
            .checked_mul(points)
            .ok_or_else(|| resource("stepped-wall hole ownership work overflow"))?,
    )?;
    super::boolean::charge(&mut work, points.saturating_mul(4))?;
    let bytes = super::geometry_bytes(&[
        points.saturating_mul(5 * size_of::<geo::Coord<f64>>()),
        rings.saturating_mul(
            2 * size_of::<geo::Polygon<f64>>()
                + 6 * size_of::<geo::LineString<f64>>()
                + 2 * size_of::<usize>(),
        ),
        largest.saturating_mul(7 * size_of::<(usize, geo::Coord<f64>)>()),
    ])?;
    Ok((bytes, work))
}

pub(super) fn section_bytes(section: &spiling_geometry::tolerant::TolerantSection) -> usize {
    section
        .loops
        .capacity()
        .saturating_mul(size_of::<spiling_geometry::tolerant::TolerantLoop>())
        .saturating_add(
            section
                .circles
                .capacity()
                .saturating_mul(size_of::<spiling_geometry::tolerant::SectionCircle>()),
        )
        .saturating_add(section.loops.iter().fold(0usize, |bytes, ring| {
            bytes
                .saturating_add(ring.points.capacity().saturating_mul(size_of::<[f64; 2]>()))
                .saturating_add(ring.faces.capacity().saturating_mul(size_of::<u32>()))
        }))
}

fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}

fn state_bytes(schedule: &Vec<LayerElevation>, settled: &Vec<Pair>) -> usize {
    schedule
        .capacity()
        .saturating_mul(size_of::<LayerElevation>())
        .saturating_add(settled.capacity().saturating_mul(size_of::<Pair>()))
}

fn settle(
    settled: &mut Vec<Pair>,
    pair: Pair,
    schedule: &Vec<LayerElevation>,
    analysis: &mut impl Analysis,
) -> Result<(), ManufacturingError> {
    analysis.charge(lookup_work(settled.len()))?;
    let position = match settled.binary_search(&pair) {
        Ok(_) => return Ok(()),
        Err(position) => position,
    };
    if settled.len() == settled.capacity() {
        let capacity = settled.capacity().saturating_mul(2).max(1);
        analysis.retain(
            state_bytes(schedule, settled)
                .saturating_add(capacity.saturating_mul(size_of::<Pair>())),
        )?;
        analysis.charge(settled.len())?;
        settled
            .try_reserve_exact(capacity - settled.len())
            .map_err(|_| resource("stepped-wall settled-pair allocation refused"))?;
    }
    analysis.retain(state_bytes(schedule, settled))?;
    analysis.charge(settled.len() - position)?;
    settled.insert(position, pair);
    Ok(())
}

trait Analysis {
    fn retain(&mut self, bytes: usize) -> Result<(), ManufacturingError>;
    fn charge(&mut self, amount: usize) -> Result<(), ManufacturingError>;
    fn excess(&mut self, lower: f64, upper: f64) -> Result<f64, ManufacturingError>;
}

#[derive(Default)]
struct Live {
    bytes: Cell<usize>,
    points: Cell<usize>,
}

struct Material {
    region: MultiPolygon<f64>,
    bytes: usize,
    points: usize,
    live: Rc<Live>,
}

impl Drop for Material {
    fn drop(&mut self) {
        self.live.bytes.set(self.live.bytes.get() - self.bytes);
        self.live.points.set(self.live.points.get() - self.points);
    }
}

struct Steps<'a> {
    models: &'a std::collections::BTreeMap<
        spiling_contracts::geometry::OccurrenceId,
        std::sync::Arc<spiling_geometry::tolerant::TolerantModel>,
    >,
    width: f64,
    noise_mm2: f64,
    cached: Vec<(u64, Rc<Material>)>,
    sections: SectionStore,
    active_row_bytes: usize,
    excess: Vec<(Pair, f64)>,
    live: Rc<Live>,
    outside_bytes: usize,
    state_bytes: usize,
    work: &'a mut usize,
    cancel: &'a AtomicBool,
    cache_enabled: bool,
}

impl Steps<'_> {
    fn sample_one(
        &mut self,
        model: &spiling_geometry::tolerant::TolerantModel,
        z: f64,
    ) -> Result<Rc<Material>, ManufacturingError> {
        use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
        let convert_error = |failure: GeometryError| {
            let code = match failure.code {
                GeometryErrorCode::ResourceLimit => ManufacturingErrorCode::ResourceLimit,
                GeometryErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
                _ => ManufacturingErrorCode::UnsupportedGeometry,
            };
            error(
                code,
                format!("stepped-wall section at Z {z} mm: {}", failure.message),
            )
        };
        let demand = model.section_demand(z).map_err(convert_error)?;
        self.charge(demand.triangles)?;
        let mut reserved = super::geometry_bytes(&[demand.scratch_bytes, demand.output_bytes])?;
        self.admit(0, reserved)?;
        let section = model
            .section_admitted(z, self.cancel, &mut |bytes| {
                reserved = reserved.checked_add(bytes).ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "stepped-wall section byte overflow",
                    )
                })?;
                self.admit(0, reserved).map_err(|failure| {
                    GeometryError::new(
                        if failure.code == ManufacturingErrorCode::Cancelled {
                            GeometryErrorCode::Cancelled
                        } else {
                            GeometryErrorCode::ResourceLimit
                        },
                        failure.message,
                    )
                })
            })
            .map_err(convert_error)?;
        let (conversion_bytes, work) = section_conversion(&section)?;
        self.admit(
            0,
            super::geometry_bytes(&[section_bytes(&section), conversion_bytes])?,
        )?;
        self.charge(work)?;
        let material = super::tolerant::material_from_section(model, &section);
        check_cancel(self.cancel)?;
        drop(section);
        self.own(material)
    }

    fn used_bytes(&self) -> usize {
        self.outside_bytes
            .saturating_add(self.state_bytes)
            .saturating_add(self.sections.bytes())
            .saturating_add(self.active_row_bytes)
            .saturating_add(
                self.cached
                    .capacity()
                    .saturating_mul(size_of::<(u64, Rc<Material>)>()),
            )
            .saturating_add(
                self.excess
                    .capacity()
                    .saturating_mul(size_of::<(Pair, f64)>()),
            )
            .saturating_add(self.live.bytes.get())
            .saturating_add(size_of::<Live>() + 2 * size_of::<usize>())
    }

    fn clear(&mut self) {
        self.cached = Vec::new();
        self.sections = SectionStore::default();
        self.excess = Vec::new();
    }

    fn fits(&self, points: usize, bytes: usize) -> bool {
        self.used_bytes()
            .checked_add(bytes)
            .is_some_and(|total| total <= crate::working_set::bytes())
            && self
                .live
                .points
                .get()
                .checked_add(points)
                .is_some_and(|total| total <= crate::working_set::units())
    }

    fn admit(&mut self, points: usize, bytes: usize) -> Result<(), ManufacturingError> {
        check_cancel(self.cancel)?;
        if !self.fits(points, bytes) {
            self.clear();
        }
        if !self.fits(points, bytes) {
            return Err(resource(
                "stepped-wall live geometry exceeds the working set",
            ));
        }
        Ok(())
    }

    fn own(&mut self, mut region: MultiPolygon<f64>) -> Result<Rc<Material>, ManufacturingError> {
        let points = region::point_count(&region);
        let bytes = native::material_bytes(&mut region)
            .saturating_add(size_of::<Material>() + 2 * size_of::<usize>());
        self.admit(points, bytes)?;
        self.live.points.set(self.live.points.get() + points);
        self.live.bytes.set(self.live.bytes.get() + bytes);
        Ok(Rc::new(Material {
            region,
            bytes,
            points,
            live: Rc::clone(&self.live),
        }))
    }

    fn overlay(&mut self, a: &Material, b: &Material) -> Result<(), ManufacturingError> {
        self.budget(&a.region, &b.region, 0, 0)
    }

    fn budget(
        &mut self,
        a: &MultiPolygon<f64>,
        b: &MultiPolygon<f64>,
        points: usize,
        bytes: usize,
    ) -> Result<(), ManufacturingError> {
        self.admit(points, bytes)?;
        let available = crate::working_set::bytes() - self.used_bytes() - bytes;
        let result = super::boolean::budget_limited(a, b, self.work, available, self.cancel);
        if result
            .as_ref()
            .is_err_and(|failure| failure.code == ManufacturingErrorCode::ResourceLimit)
        {
            self.clear();
            let available = crate::working_set::bytes() - self.used_bytes() - bytes;
            return super::boolean::budget_limited(a, b, self.work, available, self.cancel);
        }
        result
    }

    fn cache_section(&mut self, z: u64, section: &Rc<Material>) -> Result<(), ManufacturingError> {
        if !self.cache_enabled {
            return Ok(());
        }
        self.charge(lookup_work(self.cached.len()))?;
        let position = match self.cached.binary_search_by_key(&z, |(key, _)| *key) {
            Ok(_) => return Ok(()),
            Err(position) => position,
        };
        if self.cached.len() == self.cached.capacity() {
            let capacity = self.cached.capacity().saturating_mul(2).max(1);
            if !self.fits(0, capacity.saturating_mul(size_of::<(u64, Rc<Material>)>())) {
                return Ok(());
            }
            self.charge(self.cached.len())?;
            if self
                .cached
                .try_reserve_exact(capacity - self.cached.len())
                .is_err()
            {
                return Ok(());
            }
        }
        if self.fits(0, 0) {
            self.charge(self.cached.len() - position)?;
            self.cached.insert(position, (z, Rc::clone(section)));
        }
        Ok(())
    }

    fn cache_excess(&mut self, key: Pair, area: f64) -> Result<(), ManufacturingError> {
        if !self.cache_enabled {
            return Ok(());
        }
        self.charge(lookup_work(self.excess.len()))?;
        let position = match self.excess.binary_search_by_key(&key, |(pair, _)| *pair) {
            Ok(_) => return Ok(()),
            Err(position) => position,
        };
        if self.excess.len() == self.excess.capacity() {
            let capacity = self.excess.capacity().saturating_mul(2).max(1);
            if !self.fits(0, capacity.saturating_mul(size_of::<(Pair, f64)>())) {
                return Ok(());
            }
            self.charge(self.excess.len())?;
            if self
                .excess
                .try_reserve_exact(capacity - self.excess.len())
                .is_err()
            {
                return Ok(());
            }
        }
        if self.fits(0, 0) {
            self.charge(self.excess.len() - position)?;
            self.excess.insert(position, (key, area));
        }
        Ok(())
    }

    fn union_row(
        &mut self,
        row: &super::tolerant::SampleRow,
    ) -> Result<Rc<Material>, ManufacturingError> {
        self.charge(row.work)?;
        let mut section = self.own(MultiPolygon(Vec::new()))?;
        for placed in &row.sections {
            check_cancel(self.cancel)?;
            let points = region::point_count(placed);
            self.charge(points)?;
            if section.region.0.is_empty() {
                self.admit(points, row.bytes)?;
                section = self.own(placed.clone())?;
            } else {
                self.budget(&section.region, placed, 0, 0)?;
                let merged = section.region.union(placed);
                check_cancel(self.cancel)?;
                section = self.own(merged)?;
            }
        }
        Ok(section)
    }

    // When carrying an all-occurrence row won't fit, don't reduce the set of
    // printable scenes: use the old one-occurrence-at-a-time slope reduction.
    fn union_sequential(&mut self, z: f64) -> Result<Rc<Material>, ManufacturingError> {
        let mut section = self.own(MultiPolygon(Vec::new()))?;
        for model in self.models.values() {
            let placed = self.sample_one(model, z)?;
            self.charge(placed.points)?;
            if section.region.0.is_empty() {
                section = placed;
            } else {
                self.overlay(&section, &placed)?;
                let merged = section.region.union(&placed.region);
                check_cancel(self.cancel)?;
                section = self.own(merged)?;
            }
        }
        Ok(section)
    }

    fn at(&mut self, z: f64) -> Result<Rc<Material>, ManufacturingError> {
        check_cancel(self.cancel)?;
        self.charge(lookup_work(self.cached.len()))?;
        if let Ok(position) = self
            .cached
            .binary_search_by_key(&z.to_bits(), |(key, _)| *key)
        {
            return Ok(Rc::clone(&self.cached[position].1));
        }
        let mut section = if !self.cache_enabled {
            self.union_sequential(z)?
        } else {
            let available = |steps: &Self| {
                crate::working_set::bytes()
                    .saturating_sub(steps.used_bytes().saturating_sub(steps.sections.bytes()))
            };
            let headroom = available(self);
            let mut row = self.sections.sample(self.models, z, headroom, self.cancel);
            if row
                .as_ref()
                .is_err_and(|e| e.code == ManufacturingErrorCode::ResourceLimit)
            {
                self.clear();
                let headroom = available(self);
                row = self.sections.sample(self.models, z, headroom, self.cancel);
            }
            match row {
                Ok(row) => {
                    self.active_row_bytes = row.bytes;
                    let combined = self.union_row(&row);
                    self.active_row_bytes = 0;
                    match combined {
                        Ok(combined) => {
                            if self.cache_enabled {
                                let allowance = crate::working_set::bytes()
                                    .saturating_sub(self.outside_bytes)
                                    .min(available(self))
                                    / 3;
                                self.sections.remember(z, row, allowance);
                            }
                            combined
                        }
                        Err(failure) if failure.code == ManufacturingErrorCode::ResourceLimit => {
                            drop(row);
                            self.clear();
                            self.union_sequential(z)?
                        }
                        Err(failure) => return Err(failure),
                    }
                }
                Err(failure) if failure.code == ManufacturingErrorCode::ResourceLimit => {
                    self.union_sequential(z)?
                }
                Err(failure) => return Err(failure),
            }
        };
        region::canonicalize(&mut Rc::get_mut(&mut section).unwrap().region);
        self.cache_section(z.to_bits(), &section)?;
        Ok(section)
    }
}

impl Analysis for Steps<'_> {
    fn retain(&mut self, bytes: usize) -> Result<(), ManufacturingError> {
        self.state_bytes = bytes;
        self.admit(0, 0)
    }

    fn charge(&mut self, amount: usize) -> Result<(), ManufacturingError> {
        check_cancel(self.cancel)?;
        super::boolean::charge(self.work, amount)
    }

    fn excess(&mut self, lower: f64, upper: f64) -> Result<f64, ManufacturingError> {
        check_cancel(self.cancel)?;
        let key = (lower.to_bits(), upper.to_bits());
        self.charge(lookup_work(self.excess.len()))?;
        if let Ok(position) = self.excess.binary_search_by_key(&key, |(pair, _)| *pair) {
            return Ok(self.excess[position].1);
        }
        let below = self.at(lower)?;
        let above = self.at(upper)?;
        let area = if above.region.0.is_empty() {
            0.0
        } else {
            let cancel = self.cancel;
            let carried = region::offset_admitted(
                &below.region,
                self.width * 0.5,
                |points, bytes, overlay| {
                    if let Some((a, b)) = overlay {
                        self.budget(a, b, points, bytes)?;
                    } else {
                        self.admit(points, bytes)?;
                    }
                    Ok(())
                },
                cancel,
            )?;
            let carried = self.own(carried)?;
            self.overlay(&above, &carried)?;
            let difference = self.own(above.region.difference(&carried.region))?;
            self.charge(difference.points)?;
            difference.region.unsigned_area()
        };
        let area = if area > self.noise_mm2 { area } else { 0.0 };
        self.cache_excess(key, area)?;
        Ok(area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;

    struct Scalar<F>(F);

    impl<F: FnMut(f64, f64) -> Result<f64, ManufacturingError>> Analysis for Scalar<F> {
        fn retain(&mut self, bytes: usize) -> Result<(), ManufacturingError> {
            super::super::remaining_geometry_units(0, bytes).map(|_| ())
        }

        fn charge(&mut self, _: usize) -> Result<(), ManufacturingError> {
            Ok(())
        }

        fn excess(&mut self, lower: f64, upper: f64) -> Result<f64, ManufacturingError> {
            self.0(lower, upper)
        }
    }

    fn steps<'a>(
        models: &'a std::collections::BTreeMap<
            spiling_contracts::geometry::OccurrenceId,
            std::sync::Arc<spiling_geometry::tolerant::TolerantModel>,
        >,
        work: &'a mut usize,
        cancel: &'a AtomicBool,
        outside_bytes: usize,
    ) -> Steps<'a> {
        Steps {
            models,
            work,
            cancel,
            outside_bytes,
            width: 0.45,
            noise_mm2: 0.0045,
            state_bytes: 0,
            cached: Vec::new(),
            sections: SectionStore::default(),
            active_row_bytes: 0,
            excess: Vec::new(),
            live: Rc::new(Live::default()),
            cache_enabled: true,
        }
    }

    #[test]
    fn shared_sections_count_once_and_pinned_owners_survive_eviction() {
        let models = Default::default();
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut steps = steps(&models, &mut work, &cancel, 4096);
        let polygon = polygon![(x: 0., y: 0.), (x: 1., y: 0.), (x: 1., y: 1.), (x: 0., y: 1.)];
        let material = steps.own(MultiPolygon(vec![polygon])).unwrap();
        let bytes = steps.live.bytes.get();
        assert!(bytes > material.points * size_of::<geo::Coord<f64>>());
        steps.cache_section(1, &material).unwrap();
        let alias = Rc::clone(&material);
        steps.clear();
        assert_eq!(steps.live.bytes.get(), bytes);
        drop(material);
        assert_eq!(steps.live.bytes.get(), bytes);
        drop(alias);
        assert_eq!(steps.live.bytes.get(), 0);
        assert_eq!(steps.live.points.get(), 0);
    }

    #[test]
    fn section_conversion_receipt_covers_both_polygon_owners() {
        use spiling_geometry::tolerant::{TolerantLoop, TolerantSection};
        let section = TolerantSection {
            z_mm: 0.,
            joined_gaps: 0,
            max_gap_mm: 0.,
            dropped_slivers: 0,
            circles: Vec::new(),
            loops: (0..32)
                .map(|index| {
                    let x = f64::from(index) * 4.;
                    TolerantLoop {
                        points: vec![[x, 0.], [x + 3., 0.], [x + 3., 3.], [x, 3.]],
                        faces: vec![0; 4],
                        area: 9.,
                    }
                })
                .chain((0..32).map(|index| {
                    let x = f64::from(index) * 4.;
                    TolerantLoop {
                        points: vec![[x + 1., 1.], [x + 1., 2.], [x + 2., 2.], [x + 2., 1.]],
                        faces: vec![0; 4],
                        area: -1.,
                    }
                }))
                .collect(),
        };
        let (bytes, work) = section_conversion(&section).unwrap();
        let mut original = super::super::tolerant::polygons(&section);
        let mut simplified = geo::Simplify::simplify(&original, 0.0025);
        let held = native::material_bytes(&mut original) + native::material_bytes(&mut simplified);
        assert!(held <= bytes);
        assert!(work >= region::point_count(&original));
        assert!(section_bytes(&section) > 64 * 4 * size_of::<[f64; 2]>());
        assert_eq!(original, simplified);
        assert!(
            original
                .0
                .iter()
                .all(|polygon| polygon.interiors().len() == 1)
        );
    }

    #[test]
    fn outside_state_and_live_operands_share_the_scratch_allowance() {
        let models = Default::default();
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut steps = steps(&models, &mut work, &cancel, 4096);
        let material = steps.own(MultiPolygon(Vec::new())).unwrap();
        let used = steps.used_bytes();
        crate::working_set::scope(crate::WorkingSet::new(used + 127).unwrap(), || {
            assert!(steps.admit(0, 127).is_ok());
            assert_eq!(
                steps.admit(0, 128).unwrap_err().code,
                ManufacturingErrorCode::ResourceLimit
            );
            assert_eq!(
                steps.retain(128).unwrap_err().code,
                ManufacturingErrorCode::ResourceLimit
            );
        });
        drop(material);
    }

    #[test]
    fn optional_caches_decline_and_evict_before_mandatory_scratch_refuses() {
        let models = Default::default();
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut steps = steps(&models, &mut work, &cancel, 4096);
        let baseline = steps.used_bytes();
        crate::working_set::scope(crate::WorkingSet::new(baseline + 1).unwrap(), || {
            steps.cache_excess((1, 2), 3.).unwrap();
            assert_eq!(steps.excess.capacity(), 0);
        });
        steps.cache_excess((1, 2), 3.).unwrap();
        assert!(!steps.excess.is_empty());
        crate::working_set::scope(crate::WorkingSet::new(baseline + 1).unwrap(), || {
            steps.admit(0, 1).unwrap();
            assert_eq!(steps.excess.capacity(), 0);
        });
    }

    #[test]
    fn memo_hits_still_observe_cancellation_and_work_does_not_reset() {
        let models = Default::default();
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut steps = steps(&models, &mut work, &cancel, 0);
        assert_eq!(steps.excess(1., 2.).unwrap(), 0.);
        let before = *steps.work;
        assert_eq!(steps.excess(1., 2.).unwrap(), 0.);
        assert!(*steps.work > before);
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            steps.excess(1., 2.).unwrap_err().code,
            ManufacturingErrorCode::Cancelled
        );
    }

    #[test]
    fn large_constant_schedule_and_retained_indexes_have_logarithmic_lookups() {
        struct Constant {
            work: usize,
        }
        impl Analysis for Constant {
            fn retain(&mut self, _: usize) -> Result<(), ManufacturingError> {
                Ok(())
            }
            fn charge(&mut self, amount: usize) -> Result<(), ManufacturingError> {
                super::super::boolean::charge(&mut self.work, amount)
            }
            fn excess(&mut self, _: f64, _: f64) -> Result<f64, ManufacturingError> {
                Ok(0.)
            }
        }
        let count = 100_000;
        let cancel = AtomicBool::new(false);
        let mut layers = schedule(count, 0.2);
        let mut constant = Constant { work: 0 };
        assert_eq!(
            split_where_thinner_anchors(&mut layers, 0.05, |_| true, &mut constant, 1e-6, &cancel)
                .unwrap(),
            0
        );
        let bound = count as usize * (2 * lookup_work(count as usize) + 4);
        assert!(constant.work <= bound, "{} > {bound}", constant.work);

        let models = Default::default();
        let mut work = 0;
        let mut steps = steps(&models, &mut work, &cancel, 0);
        let material = steps.own(MultiPolygon(Vec::new())).unwrap();
        for index in 0..count {
            let lower = f64::from(index);
            let upper = lower + 0.5;
            steps.cache_section(lower.to_bits(), &material).unwrap();
            steps
                .cache_excess((lower.to_bits(), upper.to_bits()), 0.)
                .unwrap();
        }
        let before = *steps.work;
        for index in (0..count).rev() {
            let lower = f64::from(index);
            assert!(Rc::ptr_eq(&steps.at(lower).unwrap(), &material));
            assert_eq!(steps.excess(lower, lower + 0.5).unwrap(), 0.);
        }
        assert!(*steps.work - before <= 2 * count as usize * lookup_work(count as usize));
    }

    #[test]
    fn split_growth_is_admitted_only_after_command_height_eligibility() {
        let mut layers = schedule(2, 1e-7);
        assert!(
            native::split_resolved_layer_admitted(&mut layers, 1, 0., "test", |_| {
                panic!("uncommandable children must not allocate schedule growth")
            })
            .is_err()
        );
        let mut layers = schedule(2, 0.2);
        let before = layers
            .iter()
            .map(|layer| (layer.lower_z, layer.z))
            .collect::<Vec<_>>();
        assert_eq!(
            native::split_resolved_layer_admitted(&mut layers, 1, 0., "test", |_| {
                Err(resource("forced growth refusal"))
            })
            .unwrap_err()
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
        assert_eq!(
            layers
                .iter()
                .map(|layer| (layer.lower_z, layer.z))
                .collect::<Vec<_>>(),
            before
        );
    }

    fn schedule(layers: u32, height: f64) -> Vec<LayerElevation> {
        (0..layers)
            .map(|index| LayerElevation {
                lower_z: f64::from(index) * height,
                z: f64::from(index + 1) * height,
                sample: (f64::from(index) + 0.5) * height,
                height,
                course: index,
            })
            .collect()
    }

    /// A wall whose outline sits at `edge(z)` over a unit perimeter: the
    /// excess past half a bead between two samples.
    fn hanging(
        edge: impl Fn(f64) -> f64,
        width: f64,
    ) -> impl FnMut(f64, f64) -> Result<f64, ManufacturingError> {
        move |lower, upper| Ok((edge(upper) - edge(lower) - width * 0.5).max(0.0))
    }

    fn split(
        layers: &mut Vec<LayerElevation>,
        edge: impl Fn(f64) -> f64,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        split_where_thinner_anchors(
            layers,
            0.05,
            |_| true,
            &mut Scalar(hanging(edge, 0.45)),
            1e-6,
            cancel,
        )
    }

    #[test]
    fn a_slope_is_thinned_until_its_walls_rest_on_the_layer_below() {
        let cancel = AtomicBool::new(false);
        // Steps out 1.5 mm per mm between z 0.8 and 1.2: 0.3 mm per 0.2 mm
        // layer, more than half a bead.
        let edge = |z: f64| 1.5 * (z.clamp(0.8, 1.2) - 0.8);
        let mut layers = schedule(10, 0.2);
        assert!(split(&mut layers, edge, &cancel).unwrap() > 0);
        for pair in layers.windows(2) {
            let step = edge(pair[1].sample) - edge(pair[0].sample);
            assert!(step <= 0.225 + 1e-9, "{step} at {}", pair[1].sample);
        }
        // Layers away from the slope keep their height; every boundary stays.
        assert!((layers[1].z - layers[1].lower_z - 0.2).abs() < 1e-9);
        for z in 1..=10 {
            assert!(
                layers
                    .iter()
                    .any(|layer| (layer.z - f64::from(z) * 0.2).abs() < 1e-9)
            );
        }
    }

    #[test]
    fn a_ledge_and_the_floor_are_left_to_support() {
        let cancel = AtomicBool::new(false);
        // A 5 mm ledge at z 1.05: thinner layers never shrink it.
        let mut layers = schedule(10, 0.2);
        let ledge = |z: f64| if z > 1.05 { 5.0 } else { 0.0 };
        assert_eq!(split(&mut layers, ledge, &cancel).unwrap(), 0);
        // A slope too steep for the floor stops at the floor.
        let mut layers = schedule(10, 0.2);
        let steep = |z: f64| 20.0 * (z.clamp(0.8, 1.2) - 0.8);
        split(&mut layers, steep, &cancel).unwrap();
        assert!(
            layers
                .iter()
                .all(|layer| layer.z - layer.lower_z >= 0.05 - 1e-9)
        );
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(split(&mut layers, steep, &cancel).is_err());
    }
}
