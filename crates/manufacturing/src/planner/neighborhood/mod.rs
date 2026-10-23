// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Job-local physical-horizon intersections of immutable final-D material.
use super::{MAX_BOOLEAN_PAIRS, MAX_RETAINED_GEOMETRY_UNITS, SectionSpan, classification, region};
use crate::{check_cancel, error};
use geo::{Coord, MultiPolygon};
use spiling_contracts::manufacturing::{
    MAX_MANUFACTURING_RETAINED_BYTES, MAX_PRINT_LAYERS, ManufacturingError, ManufacturingErrorCode,
};
use std::{
    cmp::Ordering,
    ops::Deref,
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(test)]
mod tests;

enum Cached {
    Missing,
    Alias(usize),
    Leaf(usize),
    Empty,
    Geometry(Arc<MultiPolygon<f64>>, usize, usize, usize),
}

enum Storage<'a> {
    Borrowed(&'a MultiPolygon<f64>),
    Cached(Arc<MultiPolygon<f64>>),
    Temporary(MultiPolygon<f64>),
}

#[derive(Clone, Copy, Default)]
struct Usage {
    points: usize,
    bytes: usize,
}
impl Usage {
    fn add(self, other: Self) -> Result<Self, ManufacturingError> {
        Ok(Self {
            points: self
                .points
                .checked_add(other.points)
                .ok_or_else(|| resource("skin neighborhood live point overflow"))?,
            bytes: self
                .bytes
                .checked_add(other.bytes)
                .ok_or_else(|| resource("skin neighborhood live byte overflow"))?,
        })
    }
}

/// Source lifetime, not a borrow of the mutable index: lower and upper may coexist.
pub(super) struct Material<'a> {
    storage: Storage<'a>,
    points: usize,
    owned_bytes: usize,
}
impl Material<'_> {
    pub(super) fn temporary_points(&self) -> usize {
        match self.storage {
            Storage::Temporary(_) => self.points,
            _ => 0,
        }
    }
    pub(super) fn temporary_bytes(&self) -> usize {
        match self.storage {
            Storage::Temporary(_) => self.owned_bytes,
            _ => 0,
        }
    }
    fn temporary_usage(&self) -> Usage {
        Usage {
            points: self.temporary_points(),
            bytes: self.temporary_bytes(),
        }
    }
    fn owned(mut material: MultiPolygon<f64>, points: usize) -> Self {
        let owned_bytes = super::native::material_bytes(&mut material);
        Self {
            storage: Storage::Temporary(material),
            points,
            owned_bytes,
        }
    }
    fn empty() -> Self {
        Self {
            storage: Storage::Temporary(MultiPolygon(Vec::new())),
            points: 0,
            owned_bytes: 0,
        }
    }
}
impl Deref for Material<'_> {
    type Target = MultiPolygon<f64>;
    fn deref(&self) -> &Self::Target {
        match &self.storage {
            Storage::Borrowed(material) => material,
            Storage::Cached(material) => material,
            Storage::Temporary(material) => material,
        }
    }
}

pub(super) struct SkinNeighborhoods<'a> {
    sections: &'a [SectionSpan],
    // Raw final-D survival is independent of every neighbor's wall allocation.
    // Raw and cleared complete-tree slots share one allocation and retention pool.
    fill_clearance_mm: f64,
    nodes: Vec<Cached>,
    base: usize,
    leaf_identities_ready: bool,
    retained: usize,
    retained_capacity: usize,
    retained_owned_bytes: usize,
    retained_owned_byte_capacity: usize,
    work: usize,
}
impl<'a> SkinNeighborhoods<'a> {
    pub(super) fn new(
        sections: &'a [SectionSpan],
        fill_clearance_mm: f64,
        initial_work: usize,
    ) -> Result<Self, ManufacturingError> {
        if !fill_clearance_mm.is_finite() || fill_clearance_mm < 0.0 {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "fill-bead clearance must be finite and nonnegative",
            ));
        }
        if sections.len() > MAX_PRINT_LAYERS as usize {
            return Err(resource("skin neighborhood span limit exceeded"));
        }
        let base = sections
            .len()
            .max(1)
            .checked_next_power_of_two()
            .ok_or_else(|| resource("skin neighborhood index overflow"))?;
        let count = base
            .checked_mul(if fill_clearance_mm == 0.0 { 2 } else { 4 })
            .ok_or_else(|| resource("skin neighborhood node count overflow"))?;
        count
            .checked_mul(std::mem::size_of::<Cached>())
            .filter(|bytes| *bytes <= MAX_MANUFACTURING_RETAINED_BYTES as usize)
            .ok_or_else(|| resource("skin neighborhood index byte limit exceeded"))?;
        let mut work = initial_work;
        charge(&mut work, count)?;
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(count)
            .map_err(|_| resource("skin neighborhood index allocation refused"))?;
        if nodes
            .capacity()
            .checked_mul(std::mem::size_of::<Cached>())
            .is_none_or(|bytes| bytes > MAX_MANUFACTURING_RETAINED_BYTES as usize)
        {
            return Err(resource(
                "skin neighborhood index allocation byte limit exceeded",
            ));
        }
        nodes.resize_with(count, || Cached::Missing);
        Ok(Self {
            sections,
            fill_clearance_mm,
            nodes,
            base,
            leaf_identities_ready: false,
            retained: 0,
            retained_capacity: MAX_RETAINED_GEOMETRY_UNITS,
            retained_owned_bytes: 0,
            retained_owned_byte_capacity: MAX_MANUFACTURING_RETAINED_BYTES as usize,
            work,
        })
    }
    pub(super) fn work_mut(&mut self) -> &mut usize {
        &mut self.work
    }
    pub(super) fn retained_points(&self) -> usize {
        self.retained
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.nodes
            .capacity()
            .saturating_mul(std::mem::size_of::<Cached>())
            .saturating_add(self.retained_owned_bytes)
    }
    pub(super) fn freeze_retained_capacity(&mut self) -> usize {
        self.retained_capacity = self.retained;
        self.retained_owned_byte_capacity = self.retained_owned_bytes;
        self.retained_capacity
    }
    /// Admit another stage's live geometry, reclaiming only optional cache nodes.
    pub(super) fn admit_retained(
        &mut self,
        external: usize,
        external_owned_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        self.admit(
            Usage {
                points: external,
                bytes: external_owned_bytes,
            },
            Usage::default(),
            cancel,
        )
        .map(|_| ())
    }
    pub(super) fn survival(
        &mut self,
        current: usize,
        neighbor: Option<usize>,
        retained_points: usize,
        external_owned_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Material<'a>, ManufacturingError> {
        check_cancel(cancel)?;

        let external = Usage {
            points: retained_points,
            bytes: external_owned_bytes,
        };
        self.admit(external, Usage::default(), cancel)?;
        let Some(neighbor) = neighbor else {
            return Ok(Material::empty());
        };
        if current >= self.sections.len() || neighbor >= self.sections.len() {
            return Err(resource("skin neighborhood span index out of range"));
        }
        self.intern_leaves(external, cancel)?;
        let end = current
            .max(neighbor)
            .checked_add(1)
            .ok_or_else(|| resource("skin neighborhood range overflow"))?;
        let mut result = None;
        self.query(
            false,
            1,
            0,
            self.base,
            current.min(neighbor),
            end,
            external,
            &mut result,
            cancel,
        )?;
        let material = result.unwrap_or_else(Material::empty);

        if material.0.is_empty() || self.fill_clearance_mm == 0.0 {
            return Ok(material);
        }
        // Raw physical coverage is established first. Clearance by one fixed
        // structuring disk distributes over intersection, so lazily intersect
        // cleared full-D leaves instead of offsetting the fragmented horizon.
        // Never substitute neighbor wall/fill ownership for these source leaves.
        let temporary = material.temporary_usage();
        let cleared_external = external.add(temporary)?;

        let mut cleared = None;
        self.query(
            true,
            1,
            0,
            self.base,
            current.min(neighbor),
            end,
            cleared_external,
            &mut cleared,
            cancel,
        )?;
        let cleared = cleared.unwrap_or_else(Material::empty);
        self.admit(cleared_external, cleared.temporary_usage(), cancel)?;

        Ok(cleared)
    }
    fn admit(
        &mut self,
        external: Usage,
        temporary: Usage,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        self.try_admit(external, temporary, cancel)?
            .ok_or_else(|| resource("retained skin neighborhood geometry/byte limit exceeded"))
    }
    fn try_admit(
        &mut self,
        external: Usage,
        temporary: Usage,
        cancel: &AtomicBool,
    ) -> Result<Option<usize>, ManufacturingError> {
        check_cancel(cancel)?;
        let live = external.add(temporary)?;
        if live.points > MAX_RETAINED_GEOMETRY_UNITS
            || live.bytes > MAX_MANUFACTURING_RETAINED_BYTES as usize
        {
            return Ok(None);
        }
        loop {
            let total = live
                .points
                .checked_add(self.retained)
                .ok_or_else(|| resource("skin neighborhood retained point overflow"))?;
            let bytes = live
                .bytes
                .checked_add(self.retained_bytes())
                .ok_or_else(|| resource("skin neighborhood retained byte overflow"))?;
            if total <= MAX_RETAINED_GEOMETRY_UNITS
                && bytes <= MAX_MANUFACTURING_RETAINED_BYTES as usize
            {
                return Ok(Some(total));
            }
            if !self.evict_oldest(cancel)? {
                return Ok(None);
            }
        }
    }
    fn evict_oldest(&mut self, cancel: &AtomicBool) -> Result<bool, ManufacturingError> {
        let mut oldest: Option<(usize, usize)> = None;
        for (index, slot) in self.nodes.iter().enumerate() {
            check_cancel(cancel)?;
            charge(&mut self.work, 1)?;
            if let Cached::Geometry(material, _, used, _) = slot
                && Arc::strong_count(material) == 1
            {
                let candidate = (*used, index);
                if oldest.is_none_or(|previous| candidate < previous) {
                    oldest = Some(candidate);
                }
            }
        }
        check_cancel(cancel)?;
        let Some((_, index)) = oldest else {
            return Ok(false);
        };
        let Cached::Geometry(_, points, _, bytes) = &self.nodes[index] else {
            return Err(resource("skin neighborhood eviction identity changed"));
        };
        self.retained = self
            .retained
            .checked_sub(*points)
            .ok_or_else(|| resource("skin neighborhood eviction point underflow"))?;
        self.retained_owned_bytes = self
            .retained_owned_bytes
            .checked_sub(*bytes)
            .ok_or_else(|| resource("skin neighborhood owned-byte underflow"))?;
        self.nodes[index] = Cached::Missing;
        Ok(true)
    }
    fn reserve_cache(
        &mut self,
        points: usize,
        owned_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        if points > self.retained_capacity || owned_bytes > self.retained_owned_byte_capacity {
            return Ok(false);
        }
        loop {
            check_cancel(cancel)?;
            let next = self
                .retained
                .checked_add(points)
                .ok_or_else(|| resource("skin neighborhood retained point overflow"))?;
            let next_bytes = self
                .retained_owned_bytes
                .checked_add(owned_bytes)
                .ok_or_else(|| resource("skin neighborhood retained-byte overflow"))?;
            if next <= self.retained_capacity && next_bytes <= self.retained_owned_byte_capacity {
                return Ok(true);
            }
            if !self.evict_oldest(cancel)? {
                return Ok(false);
            }
        }
    }
    fn children(
        node: usize,
        start: usize,
        end: usize,
    ) -> Result<(usize, usize, usize), ManufacturingError> {
        let width = end
            .checked_sub(start)
            .ok_or_else(|| resource("skin neighborhood range underflow"))?;
        let mid = start
            .checked_add(width / 2)
            .ok_or_else(|| resource("skin neighborhood midpoint overflow"))?;
        let left = node
            .checked_mul(2)
            .ok_or_else(|| resource("skin neighborhood child index overflow"))?;
        let right = left
            .checked_add(1)
            .ok_or_else(|| resource("skin neighborhood child index overflow"))?;
        Ok((left, right, mid))
    }
    /// Only exact ordered final-D contents share a derived leaf. Heights and
    /// physical range positions remain distinct; this grants no M/C reuse.
    /// A fallible merge sort bounds even adversarial identities by O(n log n)
    /// comparisons, with every inspected coordinate charged and cancellable.
    fn intern_leaves(
        &mut self,
        external: Usage,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        if self.leaf_identities_ready {
            return Ok(());
        }
        let count = self.sections.len();
        let slots = count
            .checked_mul(2)
            .ok_or_else(|| resource("skin leaf identity count overflow"))?;
        let identity_bytes = slots
            .checked_mul(std::mem::size_of::<usize>())
            .ok_or_else(|| resource("skin leaf identity byte overflow"))?;
        self.admit(
            external,
            Usage {
                points: 0,
                bytes: identity_bytes,
            },
            cancel,
        )?;
        charge(&mut self.work, slots)?;
        let mut order = Vec::new();
        let mut scratch = Vec::new();
        order
            .try_reserve_exact(count)
            .map_err(|_| resource("skin leaf identity allocation refused"))?;
        scratch
            .try_reserve_exact(count)
            .map_err(|_| resource("skin leaf identity scratch allocation refused"))?;
        let allocated_bytes = order
            .capacity()
            .checked_add(scratch.capacity())
            .and_then(|slots| slots.checked_mul(std::mem::size_of::<usize>()))
            .ok_or_else(|| resource("skin leaf identity allocation byte overflow"))?;
        self.admit(
            external,
            Usage {
                points: 0,
                bytes: allocated_bytes,
            },
            cancel,
        )?;
        for index in 0..count {
            check_cancel(cancel)?;
            // Validate even a unique input, which sorting need not inspect.
            checked_points(&self.sections[index].material, &mut self.work, cancel)?;
            order.push(index);
            scratch.push(0);
        }
        let mut width = 1;
        while width < count {
            for start in (0..count).step_by(width * 2) {
                let mid = (start + width).min(count);
                let end = (mid + width).min(count);
                let (mut left, mut right) = (start, mid);
                for target in &mut scratch[start..end] {
                    check_cancel(cancel)?;
                    charge(&mut self.work, 1)?;
                    let take_left = right == end
                        || (left < mid
                            && compare_material(
                                &self.sections[order[left]].material,
                                &self.sections[order[right]].material,
                                &mut self.work,
                                cancel,
                            )? != Ordering::Greater);
                    *target = if take_left {
                        let index = order[left];
                        left += 1;
                        index
                    } else {
                        let index = order[right];
                        right += 1;
                        index
                    };
                }
            }
            std::mem::swap(&mut order, &mut scratch);
            width *= 2;
        }
        // Finish all fallible comparisons before publishing any aliases, so a
        // cancelled/refused identity pass cannot leave half-interned leaves.
        let mut canonical: Option<usize> = None;
        for (position, index) in order.iter().copied().enumerate() {
            let equal = if let Some(previous) = canonical {
                compare_material(
                    &self.sections[previous].material,
                    &self.sections[index].material,
                    &mut self.work,
                    cancel,
                )? == Ordering::Equal
            } else {
                false
            };
            if !equal {
                canonical = Some(index);
            }
            scratch[position] = canonical.unwrap_or(index);
        }
        check_cancel(cancel)?;
        charge(&mut self.work, count)?;
        if self.fill_clearance_mm != 0.0 {
            charge(&mut self.work, count)?;
        }
        for (index, canonical) in order.into_iter().zip(scratch) {
            if index != canonical {
                self.nodes[self.base + index] = Cached::Alias(self.base + canonical);
                if self.fill_clearance_mm != 0.0 {
                    self.nodes[3 * self.base + index] = Cached::Alias(3 * self.base + canonical);
                }
            }
        }
        self.leaf_identities_ready = true;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        cleared: bool,
        node: usize,
        start: usize,
        end: usize,
        external: Usage,
        temporary: Usage,
        cancel: &AtomicBool,
    ) -> Result<Material<'a>, ManufacturingError> {
        check_cancel(cancel)?;
        charge(&mut self.work, 1)?;
        self.admit(external, temporary, cancel)?;
        let tree_offset = if cleared { 2 * self.base } else { 0 };
        let cache_node = tree_offset + node;
        let (cache_node, start, end) = match self.nodes.get(cache_node) {
            Some(Cached::Alias(canonical)) => {
                let start = canonical
                    .checked_sub(tree_offset + self.base)
                    .filter(|index| *index < self.sections.len())
                    .ok_or_else(|| resource("skin canonical leaf index out of range"))?;
                (*canonical, start, start + 1)
            }
            _ => (cache_node, start, end),
        };
        let used = self.work;
        let slot = self
            .nodes
            .get_mut(cache_node)
            .ok_or_else(|| resource("skin neighborhood node index out of range"))?;
        match slot {
            Cached::Leaf(points) => {
                return Ok(Material {
                    storage: Storage::Borrowed(&self.sections[start].material),
                    points: *points,
                    owned_bytes: 0,
                });
            }
            Cached::Empty => return Ok(Material::empty()),
            Cached::Geometry(material, points, last_use, _) => {
                *last_use = used;
                return Ok(Material {
                    storage: Storage::Cached(Arc::clone(material)),
                    points: *points,
                    owned_bytes: 0,
                });
            }
            Cached::Missing => {}
            Cached::Alias(_) => return Err(resource("skin canonical leaf alias chain")),
        }
        if end.checked_sub(start) == Some(1) {
            let sections = self.sections;
            let material = &sections
                .get(start)
                .ok_or_else(|| resource("skin neighborhood leaf index out of range"))?
                .material;
            if cleared {
                let offset = region::offset_admitted(
                    material,
                    -self.fill_clearance_mm,
                    |scratch, scratch_owned_bytes, overlay| {
                        let live = temporary.add(Usage {
                            points: scratch,
                            bytes: scratch_owned_bytes,
                        })?;
                        self.admit(external, live, cancel)?;
                        if let Some((a, b)) = overlay {
                            super::boolean::budget_accumulated(a, b, &mut self.work, cancel)?;
                        }
                        Ok(())
                    },
                    cancel,
                )?;
                let points = checked_points(&offset, &mut self.work, cancel)?;
                let material = Material::owned(offset, points);
                self.admit(external, temporary.add(material.temporary_usage())?, cancel)?;
                return self.cache_material(cache_node, material, external, temporary, cancel);
            }
            let points = checked_points(material, &mut self.work, cancel)?;
            self.nodes[cache_node] = Cached::Leaf(points);
            return Ok(Material {
                storage: Storage::Borrowed(material),
                points,
                owned_bytes: 0,
            });
        }
        let (left, right, mid) = Self::children(node, start, end)?;
        let lower = self.build(cleared, left, start, mid, external, temporary, cancel)?;
        if lower.0.is_empty() {
            self.nodes[cache_node] = Cached::Empty;
            return Ok(Material::empty());
        }
        let lower_live = temporary.add(lower.temporary_usage())?;
        let upper = self.build(cleared, right, mid, end, external, lower_live, cancel)?;
        if upper.0.is_empty() {
            self.nodes[cache_node] = Cached::Empty;
            return Ok(Material::empty());
        }
        // A frozen pool or pinned nodes may leave either recursive operand owned.
        // Keep both inputs and the query accumulator live through output admission.
        let operands = lower_live.add(upper.temporary_usage())?;
        self.admit(external, operands, cancel)?;
        let material =
            classification::intersection_accumulated(&lower, &upper, &mut self.work, cancel)?;
        let points = checked_points(&material, &mut self.work, cancel)?;
        let material = Material::owned(material, points);
        self.admit(external, operands.add(material.temporary_usage())?, cancel)?;
        check_cancel(cancel)?;
        if material.0.is_empty() {
            self.nodes[cache_node] = Cached::Empty;
            return Ok(Material::empty());
        }
        drop(lower);
        drop(upper);
        self.cache_material(cache_node, material, external, temporary, cancel)
    }
    fn cache_material(
        &mut self,
        node: usize,
        material: Material<'a>,
        external: Usage,
        temporary: Usage,
        cancel: &AtomicBool,
    ) -> Result<Material<'a>, ManufacturingError> {
        check_cancel(cancel)?;
        if material.0.is_empty() {
            self.nodes[node] = Cached::Empty;
            return Ok(Material::empty());
        }
        let points = material.points;
        let payload_bytes = material.owned_bytes;
        let Storage::Temporary(material) = material.storage else {
            return Err(resource("skin neighborhood lost owned derived material"));
        };
        let owned_bytes = payload_bytes
            .checked_add(super::native::MATERIAL_ARC_BYTES)
            .ok_or_else(|| resource("skin neighborhood allocation-byte overflow"))?;
        if !self.reserve_cache(points, owned_bytes, cancel)? {
            return Ok(Material {
                storage: Storage::Temporary(material),
                points,
                owned_bytes: payload_bytes,
            });
        }
        let candidate = temporary.add(Usage {
            points,
            bytes: owned_bytes,
        })?;
        if self.try_admit(external, candidate, cancel)?.is_none() {
            return Ok(Material {
                storage: Storage::Temporary(material),
                points,
                owned_bytes: payload_bytes,
            });
        }
        let retained = self
            .retained
            .checked_add(points)
            .ok_or_else(|| resource("skin neighborhood retained point overflow"))?;
        let retained_owned_bytes = self
            .retained_owned_bytes
            .checked_add(owned_bytes)
            .ok_or_else(|| resource("skin neighborhood retained-byte overflow"))?;
        let material = Arc::new(material);
        self.nodes[node] = Cached::Geometry(Arc::clone(&material), points, self.work, owned_bytes);
        self.retained = retained;
        self.retained_owned_bytes = retained_owned_bytes;
        Ok(Material {
            storage: Storage::Cached(material),
            points,
            owned_bytes: 0,
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn query(
        &mut self,
        cleared: bool,
        node: usize,
        start: usize,
        end: usize,
        first: usize,
        last: usize,
        external: Usage,
        result: &mut Option<Material<'a>>,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        charge(&mut self.work, 1)?;
        if result
            .as_ref()
            .is_some_and(|material| material.0.is_empty())
        {
            return Ok(());
        }
        let temporary = result
            .as_ref()
            .map_or(Usage::default(), Material::temporary_usage);
        self.admit(external, temporary, cancel)?;
        if first <= start && end <= last {
            let next = self.build(cleared, node, start, end, external, temporary, cancel)?;
            let Some(previous) = result.as_ref() else {
                *result = Some(next);
                return Ok(());
            };
            if next.0.is_empty() {
                *result = Some(Material::empty());
                return Ok(());
            }
            let operands = temporary.add(next.temporary_usage())?;
            self.admit(external, operands, cancel)?;
            let material =
                classification::intersection_accumulated(previous, &next, &mut self.work, cancel)?;
            let points = checked_points(&material, &mut self.work, cancel)?;
            let material = Material::owned(material, points);
            self.admit(external, operands.add(material.temporary_usage())?, cancel)?;
            check_cancel(cancel)?;
            *result = Some(material);
            return Ok(());
        }
        let (left, right, mid) = Self::children(node, start, end)?;
        // Ascending aligned disjoint nodes; no query-list allocation or sorting.
        if first < mid {
            self.query(
                cleared, left, start, mid, first, last, external, result, cancel,
            )?;
        }
        if last > mid {
            self.query(
                cleared, right, mid, end, first, last, external, result, cancel,
            )?;
        }
        Ok(())
    }
}
/// Structural, bit-exact ordering in the existing final-D coordinate frame.
/// Polygon/ring order, holes and signed coordinate bits are never normalized.
fn compare_material(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Ordering, ManufacturingError> {
    check_cancel(cancel)?;
    charge(work, 1)?;
    let order = a.0.len().cmp(&b.0.len());
    if order != Ordering::Equal {
        return Ok(order);
    }
    for (a, b) in a.0.iter().zip(&b.0) {
        check_cancel(cancel)?;
        charge(work, 1)?;
        let order = a.interiors().len().cmp(&b.interiors().len());
        if order != Ordering::Equal {
            return Ok(order);
        }
        for (a, b) in std::iter::once(a.exterior())
            .chain(a.interiors())
            .zip(std::iter::once(b.exterior()).chain(b.interiors()))
        {
            check_cancel(cancel)?;
            charge(work, 1)?;
            let order = a.0.len().cmp(&b.0.len());
            if order != Ordering::Equal {
                return Ok(order);
            }
            for (a, b) in a.0.iter().zip(&b.0) {
                check_cancel(cancel)?;
                charge(work, 1)?;
                let order = (a.x.to_bits(), a.y.to_bits()).cmp(&(b.x.to_bits(), b.y.to_bits()));
                if order != Ordering::Equal {
                    return Ok(order);
                }
            }
        }
    }
    Ok(Ordering::Equal)
}
fn checked_points(
    material: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let mut points = 0usize;
    for polygon in &material.0 {
        check_cancel(cancel)?;
        charge(work, 1)?;
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            check_cancel(cancel)?;
            charge(work, 1)?;
            points = points
                .checked_add(ring.0.len())
                .filter(|n| *n <= MAX_RETAINED_GEOMETRY_UNITS)
                .ok_or_else(|| resource("skin neighborhood geometry point limit exceeded"))?;
            for point in &ring.0 {
                check_cancel(cancel)?;
                charge(work, 1)?;
                if !point.x.is_finite() || !point.y.is_finite() {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        "skin neighborhood material is nonfinite",
                    ));
                }
            }
        }
    }
    points
        .checked_mul(std::mem::size_of::<Coord<f64>>())
        .ok_or_else(|| resource("skin neighborhood geometry byte overflow"))?;
    Ok(points)
}
fn charge(work: &mut usize, amount: usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(amount)
        .filter(|n| *n <= MAX_BOOLEAN_PAIRS)
        .ok_or_else(|| resource("skin neighborhood work budget exceeded"))?;
    Ok(())
}
fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}
