// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Exact, job-local intersections of aligned immutable realized-D span ranges.
use super::{MAX_BOOLEAN_PAIRS, SectionSpan, classification};
use crate::{check_cancel, error};
use geo::{Coord, MultiPolygon};
use spiling_contracts::manufacturing::{
    MAX_DEPOSITION_SEGMENTS, MAX_PRINT_LAYERS, ManufacturingError, ManufacturingErrorCode,
};
use std::{
    ops::Deref,
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(test)]
mod tests;

enum Cached {
    Missing,
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
    // Complete-tree positions identify exact aligned half-open D span ranges.
    // Leaves retain only validated counts, never copied material coordinates.
    nodes: Vec<Cached>,
    base: usize,
    retained: usize,
    retained_capacity: usize,
    work: usize,
}
impl<'a> SkinNeighborhoods<'a> {
    pub(super) fn new(
        sections: &'a [SectionSpan],
        initial_work: usize,
    ) -> Result<Self, ManufacturingError> {
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
            nodes,
            base,
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
        }
        if end.checked_sub(start) == Some(1) {
            let material = &self
                .sections
                .get(start)
                .ok_or_else(|| resource("skin neighborhood leaf index out of range"))?
                .material;
            let points = checked_points(material, &mut self.work, cancel)?;
            self.nodes[node] = Cached::Leaf(points);
            return Ok(Material {
                storage: Storage::Borrowed(material),
                points,
            });
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
