// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Job-local intersections of aligned immutable final-D wall-center insets.
use super::{MAX_BOOLEAN_PAIRS, SectionSpan, classification, region};
use crate::{check_cancel, error};
use geo::{Coord, MultiPolygon};
use spiling_contracts::manufacturing::{
    MAX_DEPOSITION_SEGMENTS, MAX_PRINT_LAYERS, ManufacturingError, ManufacturingErrorCode,
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
    Geometry(Arc<MultiPolygon<f64>>, usize, usize),
}

enum Storage<'a> {
    Borrowed(&'a MultiPolygon<f64>),
    Cached(Arc<MultiPolygon<f64>>),
    Temporary(MultiPolygon<f64>),
}

/// Source lifetime, not a borrow of the mutable index: lower and upper may coexist.
pub(super) struct Material<'a> {
    storage: Storage<'a>,
    points: usize,
}
impl Material<'_> {
    pub(super) fn temporary_points(&self) -> usize {
        match self.storage {
            Storage::Temporary(_) => self.points,
            _ => 0,
        }
    }
    fn empty() -> Self {
        Self {
            storage: Storage::Temporary(MultiPolygon(Vec::new())),
            points: 0,
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
    // Fixed for the entire compile, independent of native M/C reuse identity.
    inset_mm: f64,
    // Complete-tree positions identify aligned half-open final-D span ranges.
    // Nonzero-inset leaves share the same owned/reclaimable pool as ranges.
    nodes: Vec<Cached>,
    base: usize,
    leaf_identities_ready: bool,
    retained: usize,
    retained_capacity: usize,
    work: usize,
}
impl<'a> SkinNeighborhoods<'a> {
    pub(super) fn new(
        sections: &'a [SectionSpan],
        inset_mm: f64,
        initial_work: usize,
    ) -> Result<Self, ManufacturingError> {
        if !inset_mm.is_finite() || inset_mm < 0.0 {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "skin neighborhood inset must be finite and nonnegative",
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
            .checked_mul(2)
            .ok_or_else(|| resource("skin neighborhood node count overflow"))?;
        count
            .checked_mul(std::mem::size_of::<Cached>())
            .ok_or_else(|| resource("skin neighborhood index byte overflow"))?;
        let mut work = initial_work;
        charge(&mut work, count)?;
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(count)
            .map_err(|_| resource("skin neighborhood index allocation refused"))?;
        nodes.resize_with(count, || Cached::Missing);
        Ok(Self {
            sections,
            inset_mm,
            nodes,
            base,
            leaf_identities_ready: false,
            retained: 0,
            retained_capacity: MAX_DEPOSITION_SEGMENTS as usize,
            work,
        })
    }
    pub(super) fn work_mut(&mut self) -> &mut usize {
        &mut self.work
    }
    pub(super) fn retained_points(&self) -> usize {
        self.retained
    }
    pub(super) fn freeze_retained_capacity(&mut self) -> usize {
        self.retained_capacity = self.retained;
        self.retained_capacity
    }
    /// Admit another stage's live geometry, reclaiming only optional cache nodes.
    pub(super) fn admit_retained(
        &mut self,
        external: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        self.admit(external, 0, cancel).map(|_| ())
    }
    pub(super) fn survival(
        &mut self,
        current: usize,
        neighbor: Option<usize>,
        retained_points: usize,
        cancel: &AtomicBool,
    ) -> Result<Material<'a>, ManufacturingError> {
        check_cancel(cancel)?;
        self.admit(retained_points, 0, cancel)?;
        let Some(neighbor) = neighbor else {
            return Ok(Material::empty());
        };
        if current >= self.sections.len() || neighbor >= self.sections.len() {
            return Err(resource("skin neighborhood span index out of range"));
        }
        self.intern_leaves(cancel)?;
        let end = current
            .max(neighbor)
            .checked_add(1)
            .ok_or_else(|| resource("skin neighborhood range overflow"))?;
        let mut result = None;
        self.query(
            1,
            0,
            self.base,
            current.min(neighbor),
            end,
            retained_points,
            &mut result,
            cancel,
        )?;
        Ok(result.unwrap_or_else(Material::empty))
    }
    fn admit(
        &mut self,
        external: usize,
        temporary: usize,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        check_cancel(cancel)?;
        let live = external
            .checked_add(temporary)
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| resource("retained skin neighborhood point limit exceeded"))?;
        loop {
            let total = live
                .checked_add(self.retained)
                .ok_or_else(|| resource("skin neighborhood retained point overflow"))?;
            if total <= MAX_DEPOSITION_SEGMENTS as usize {
                total
                    .checked_mul(std::mem::size_of::<Coord<f64>>())
                    .ok_or_else(|| resource("skin neighborhood geometry byte overflow"))?;
                return Ok(total);
            }
            if !self.evict_oldest(cancel)? {
                return Err(resource("retained skin neighborhood point limit exceeded"));
            }
        }
    }
    fn evict_oldest(&mut self, cancel: &AtomicBool) -> Result<bool, ManufacturingError> {
        let mut oldest: Option<(usize, usize)> = None;
        for (index, slot) in self.nodes.iter().enumerate() {
            check_cancel(cancel)?;
            charge(&mut self.work, 1)?;
            if let Cached::Geometry(material, _, used) = slot
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
        let Cached::Geometry(_, points, _) = &self.nodes[index] else {
            return Err(resource("skin neighborhood eviction identity changed"));
        };
        self.retained = self
            .retained
            .checked_sub(*points)
            .ok_or_else(|| resource("skin neighborhood eviction point underflow"))?;
        self.nodes[index] = Cached::Missing;
        Ok(true)
    }
    fn reserve_cache(
        &mut self,
        points: usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        if points > self.retained_capacity {
            return Ok(false);
        }
        loop {
            check_cancel(cancel)?;
            let next = self
                .retained
                .checked_add(points)
                .ok_or_else(|| resource("skin neighborhood retained point overflow"))?;
            if next <= self.retained_capacity {
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
    fn intern_leaves(&mut self, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
        if self.leaf_identities_ready || self.inset_mm == 0.0 {
            return Ok(());
        }
        let count = self.sections.len();
        let slots = count
            .checked_mul(2)
            .ok_or_else(|| resource("skin leaf identity count overflow"))?;
        let identity_bytes = slots
            .checked_mul(std::mem::size_of::<usize>())
            .ok_or_else(|| resource("skin leaf identity byte overflow"))?;
        self.nodes
            .len()
            .checked_mul(std::mem::size_of::<Cached>())
            .and_then(|bytes| bytes.checked_add(identity_bytes))
            .ok_or_else(|| resource("skin neighborhood index staging byte overflow"))?;
        charge(&mut self.work, slots)?;
        let mut order = Vec::new();
        let mut scratch = Vec::new();
        order
            .try_reserve_exact(count)
            .map_err(|_| resource("skin leaf identity allocation refused"))?;
        scratch
            .try_reserve_exact(count)
            .map_err(|_| resource("skin leaf identity scratch allocation refused"))?;
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
        for (index, canonical) in order.into_iter().zip(scratch) {
            if index != canonical {
                self.nodes[self.base + index] = Cached::Alias(self.base + canonical);
            }
        }
        self.leaf_identities_ready = true;
        Ok(())
    }
    fn build(
        &mut self,
        node: usize,
        start: usize,
        end: usize,
        external: usize,
        temporary: usize,
        cancel: &AtomicBool,
    ) -> Result<Material<'a>, ManufacturingError> {
        check_cancel(cancel)?;
        charge(&mut self.work, 1)?;
        self.admit(external, temporary, cancel)?;
        let (node, start, end) = match self.nodes.get(node) {
            Some(Cached::Alias(canonical)) => {
                let start = canonical
                    .checked_sub(self.base)
                    .filter(|index| *index < self.sections.len())
                    .ok_or_else(|| resource("skin canonical leaf index out of range"))?;
                (*canonical, start, start + 1)
            }
            _ => (node, start, end),
        };
        let used = self.work;
        let slot = self
            .nodes
            .get_mut(node)
            .ok_or_else(|| resource("skin neighborhood node index out of range"))?;
        match slot {
            Cached::Leaf(points) => {
                return Ok(Material {
                    storage: Storage::Borrowed(&self.sections[start].material),
                    points: *points,
                });
            }
            Cached::Empty => return Ok(Material::empty()),
            Cached::Geometry(material, points, last_use) => {
                *last_use = used;
                return Ok(Material {
                    storage: Storage::Cached(Arc::clone(material)),
                    points: *points,
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
            let points = checked_points(material, &mut self.work, cancel)?;
            if self.inset_mm == 0.0 {
                // The exact identity needs no owned geometry, including tests
                // and legitimate zero-distance range queries.
                self.nodes[node] = Cached::Leaf(points);
                return Ok(Material {
                    storage: Storage::Borrowed(material),
                    points,
                });
            }
            let material = region::offset_admitted(
                material,
                -self.inset_mm,
                |scratch, overlay| {
                    let staging = temporary
                        .checked_add(scratch)
                        .ok_or_else(|| resource("skin inset staging overflow"))?;
                    self.admit(external, staging, cancel)?;
                    if let Some((a, b)) = overlay {
                        super::boolean::budget_accumulated(a, b, &mut self.work, cancel)?;
                    }
                    Ok(())
                },
                cancel,
            )?;
            let points = checked_points(&material, &mut self.work, cancel)?;
            let staged = temporary
                .checked_add(points)
                .ok_or_else(|| resource("skin inset staging overflow"))?;
            self.admit(external, staged, cancel)?;
            return self.cache_material(node, material, points, cancel);
        }
        let (left, right, mid) = Self::children(node, start, end)?;
        let lower = self.build(left, start, mid, external, temporary, cancel)?;
        if lower.0.is_empty() {
            self.nodes[node] = Cached::Empty;
            return Ok(Material::empty());
        }
        let lower_live = temporary
            .checked_add(lower.temporary_points())
            .ok_or_else(|| resource("skin neighborhood operand staging overflow"))?;
        let upper = self.build(right, mid, end, external, lower_live, cancel)?;
        if upper.0.is_empty() {
            self.nodes[node] = Cached::Empty;
            return Ok(Material::empty());
        }
        // A frozen pool or pinned nodes may leave either recursive operand owned.
        // Keep both inputs and the query accumulator live through output admission.
        let operands = lower_live
            .checked_add(upper.temporary_points())
            .ok_or_else(|| resource("skin neighborhood operand staging overflow"))?;
        self.admit(external, operands, cancel)?;
        let material =
            classification::intersection_accumulated(&lower, &upper, &mut self.work, cancel)?;
        let points = checked_points(&material, &mut self.work, cancel)?;
        let staged = operands
            .checked_add(points)
            .ok_or_else(|| resource("skin neighborhood staging overflow"))?;
        self.admit(external, staged, cancel)?;
        check_cancel(cancel)?;
        if material.0.is_empty() {
            self.nodes[node] = Cached::Empty;
            return Ok(Material::empty());
        }
        drop(lower);
        drop(upper);
        self.cache_material(node, material, points, cancel)
    }
    fn cache_material(
        &mut self,
        node: usize,
        material: MultiPolygon<f64>,
        points: usize,
        cancel: &AtomicBool,
    ) -> Result<Material<'a>, ManufacturingError> {
        check_cancel(cancel)?;
        if material.0.is_empty() {
            self.nodes[node] = Cached::Empty;
            return Ok(Material::empty());
        }
        if !self.reserve_cache(points, cancel)? {
            return Ok(Material {
                storage: Storage::Temporary(material),
                points,
            });
        }
        let retained = self
            .retained
            .checked_add(points)
            .ok_or_else(|| resource("skin neighborhood retained point overflow"))?;
        let material = Arc::new(material);
        self.nodes[node] = Cached::Geometry(Arc::clone(&material), points, self.work);
        self.retained = retained;
        Ok(Material {
            storage: Storage::Cached(material),
            points,
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn query(
        &mut self,
        node: usize,
        start: usize,
        end: usize,
        first: usize,
        last: usize,
        external: usize,
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
        let temporary = result.as_ref().map_or(0, Material::temporary_points);
        self.admit(external, temporary, cancel)?;
        if first <= start && end <= last {
            let next = self.build(node, start, end, external, temporary, cancel)?;
            let Some(previous) = result.as_ref() else {
                *result = Some(next);
                return Ok(());
            };
            if next.0.is_empty() {
                *result = Some(Material::empty());
                return Ok(());
            }
            let operands = temporary
                .checked_add(next.temporary_points())
                .ok_or_else(|| resource("skin neighborhood query operand overflow"))?;
            self.admit(external, operands, cancel)?;
            let material =
                classification::intersection_accumulated(previous, &next, &mut self.work, cancel)?;
            let points = checked_points(&material, &mut self.work, cancel)?;
            let staged = operands
                .checked_add(points)
                .ok_or_else(|| resource("skin neighborhood query staging overflow"))?;
            self.admit(external, staged, cancel)?;
            check_cancel(cancel)?;
            *result = Some(Material {
                storage: Storage::Temporary(material),
                points,
            });
            return Ok(());
        }
        let (left, right, mid) = Self::children(node, start, end)?;
        // Ascending aligned disjoint nodes; no query-list allocation or sorting.
        if first < mid {
            self.query(left, start, mid, first, last, external, result, cancel)?;
        }
        if last > mid {
            self.query(right, mid, end, first, last, external, result, cancel)?;
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
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
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
