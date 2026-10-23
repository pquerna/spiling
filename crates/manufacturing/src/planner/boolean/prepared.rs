// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Borrowed, immutable Boolean ADMISSION metadata, never geometry authority.
//! Kernel callers still use the exact full source operands and original order.
use super::*;
use std::mem::size_of;

pub(crate) struct Prepared<'a> {
    source: &'a MultiPolygon<f64>,
    inventory: Inventory,
    edges: Vec<EdgeBounds>,
}
impl<'a> Prepared<'a> {
    /// Optional retained storage declines before allocation. Raw validation,
    /// cumulative work, allocation and cancellation errors are not caught.
    pub(crate) fn new(
        source: &'a MultiPolygon<f64>,
        available_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let inventory = inspect(source.0.iter(), work, cancel)?;
        if inventory.count > MAX_RETAINED_GEOMETRY_UNITS {
            return Err(resource("Boolean broad-phase staging point limit exceeded"));
        }
        let bytes = inventory
            .count
            .checked_mul(size_of::<EdgeBounds>())
            .ok_or_else(|| resource("prepared Boolean metadata byte overflow"))?;
        if bytes > available_bytes {
            return Ok(None);
        }
        admit_host_sort(inventory.count, work)?;
        let mut edges = Vec::new();
        edges
            .try_reserve_exact(inventory.count)
            .map_err(|_| resource("prepared Boolean metadata allocation refused"))?;
        if edges.capacity() * size_of::<EdgeBounds>() > available_bytes {
            return Err(resource(
                "prepared Boolean metadata capacity exceeds byte budget",
            ));
        }
        build_edges(source, &mut edges, work, cancel)?;
        sort_by(&mut edges, |edge| edge.min[0], work, cancel)?;
        Ok(Some(Self {
            source,
            inventory,
            edges,
        }))
    }
    pub(crate) fn source(&self) -> &'a MultiPolygon<f64> {
        self.source
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.edges.capacity() * size_of::<EdgeBounds>()
    }
}

pub(crate) struct Workspace {
    raw: Vec<EdgeBounds>,
    active: Vec<EdgeBounds>,
}
impl Workspace {
    pub(crate) fn new() -> Self {
        Self {
            raw: Vec::new(),
            active: Vec::new(),
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        (self.raw.capacity() + self.active.capacity()) * size_of::<EdgeBounds>()
    }

    pub(crate) fn budget_pair(
        &mut self,
        a: &Prepared<'_>,
        b: &Prepared<'_>,
        available_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        if self.retained_bytes() > available_bytes {
            return Err(resource(
                "prepared Boolean retained workspace exceeds byte budget",
            ));
        }
        let unary = std::ptr::eq(a.source, b.source);
        let other = if unary {
            Inventory::empty()
        } else {
            b.inventory
        };
        let Some((count, padding)) = admit_query(a.inventory, other, work, cancel)? else {
            return Ok(());
        };
        let b_edges = if unary { &[][..] } else { b.edges.as_slice() };
        reserve(
            &mut self.active,
            count,
            self.raw.capacity() * size_of::<EdgeBounds>(),
            available_bytes,
        )?;
        sweep(&a.edges, b_edges, padding, &mut self.active, work, cancel)
    }

    /// The changing side is typically one actual strip. Reuse its small box
    /// buffer too; never retain a pointer/key for changing raw operands.
    pub(crate) fn budget_raw(
        &mut self,
        a: &MultiPolygon<f64>,
        b: &Prepared<'_>,
        available_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        if self.retained_bytes() > available_bytes {
            return Err(resource(
                "prepared Boolean retained workspace exceeds byte budget",
            ));
        }
        if std::ptr::eq(a, b.source) {
            return self.budget_pair(b, b, available_bytes, work, cancel);
        }
        let inventory = inspect(a.0.iter(), work, cancel)?;
        let Some((count, padding)) = admit_query(inventory, b.inventory, work, cancel)? else {
            return Ok(());
        };
        admit_host_sort(inventory.count, work)?;
        reserve(
            &mut self.raw,
            inventory.count,
            self.active.capacity() * size_of::<EdgeBounds>(),
            available_bytes,
        )?;
        self.raw.clear();
        build_edges(a, &mut self.raw, work, cancel)?;
        sort_by(&mut self.raw, |edge| edge.min[0], work, cancel)?;
        reserve(
            &mut self.active,
            count,
            self.raw.capacity() * size_of::<EdgeBounds>(),
            available_bytes,
        )?;
        sweep(&self.raw, &b.edges, padding, &mut self.active, work, cancel)
    }
}

fn admit_host_sort(count: usize, work: &usize) -> Result<(), ManufacturingError> {
    let bound = sort_work(count)?;
    if work
        .checked_add(bound)
        .is_none_or(|v| v > MAX_BOOLEAN_PAIRS)
    {
        return Err(resource(
            "Boolean broad-phase construction/sort work limit exceeded",
        ));
    }
    Ok(())
}

fn admit_query(
    a: Inventory,
    b: Inventory,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<(usize, f64)>, ManufacturingError> {
    check_cancel(cancel)?;
    let count = a
        .count
        .checked_add(b.count)
        .ok_or_else(|| resource("Boolean boundary count overflow"))?;
    // The STOCK kernel still consumes/converts these inputs every call, even
    // though immutable host metadata no longer needs rescanning or sorting.
    charge(
        work,
        a.points
            .checked_add(b.points)
            .ok_or_else(|| resource("Boolean point count overflow"))?,
    )?;
    if count == 0 {
        return Ok(None);
    }
    let min = [a.min[0].min(b.min[0]), a.min[1].min(b.min[1])];
    let max = [a.max[0].max(b.max[0]), a.max[1].max(b.max[1])];
    let padding = overlay_padding(min, max)?;
    if count > MAX_RETAINED_GEOMETRY_UNITS {
        return Err(resource("Boolean broad-phase staging point limit exceeded"));
    }
    // Explicit repeated kernel construction/NlogN obligation, including
    // small coarse-safe queries. Cached HOST order never prepares the kernel.
    charge(work, sort_work(count)?)?;
    if let Some(pairs) = count.checked_mul(count).filter(|pairs| {
        *pairs <= 256
            && work
                .checked_add(*pairs)
                .is_some_and(|total| total <= MAX_BOOLEAN_PAIRS)
    }) {
        charge(work, pairs)?;
        return Ok(None);
    }
    Ok(Some((count, padding)))
}

fn build_edges(
    source: &MultiPolygon<f64>,
    edges: &mut Vec<EdgeBounds>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for polygon in &source.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            for edge in ring.0.windows(2) {
                check_cancel(cancel)?;
                charge(work, 1)?;
                edges.push(EdgeBounds {
                    min: [edge[0].x.min(edge[1].x), edge[0].y.min(edge[1].y)],
                    max: [edge[0].x.max(edge[1].x), edge[0].y.max(edge[1].y)],
                });
            }
        }
    }
    Ok(())
}

fn reserve(
    values: &mut Vec<EdgeBounds>,
    needed: usize,
    other: usize,
    available: usize,
) -> Result<(), ManufacturingError> {
    if needed <= values.capacity() {
        return Ok(());
    }
    let next = needed.max(
        values
            .capacity()
            .max(4)
            .checked_mul(2)
            .ok_or_else(|| resource("prepared Boolean scratch capacity overflow"))?,
    );
    let peak = values
        .capacity()
        .checked_add(next)
        .and_then(|n| n.checked_mul(size_of::<EdgeBounds>()))
        .and_then(|n| n.checked_add(other))
        .ok_or_else(|| resource("prepared Boolean scratch byte overflow"))?;
    if peak > available {
        return Err(resource("prepared Boolean scratch byte budget exceeded"));
    }
    values
        .try_reserve_exact(next - values.len())
        .map_err(|_| resource("prepared Boolean scratch allocation refused"))?;
    if values
        .capacity()
        .checked_mul(size_of::<EdgeBounds>())
        .and_then(|n| n.checked_add(other))
        .is_none_or(|n| n > available)
    {
        return Err(resource(
            "prepared Boolean scratch retained byte budget exceeded",
        ));
    }
    Ok(())
}

fn padded(edge: EdgeBounds, padding: f64) -> Result<EdgeBounds, ManufacturingError> {
    let min = [
        (edge.min[0] - padding).next_down(),
        (edge.min[1] - padding).next_down(),
    ];
    let max = [
        (edge.max[0] + padding).next_up(),
        (edge.max[1] + padding).next_up(),
    ];
    if min.into_iter().chain(max).any(|v| !v.is_finite()) {
        return Err(resource("Boolean broad-phase box overflow"));
    }
    Ok(EdgeBounds { min, max })
}

fn sweep(
    a: &[EdgeBounds],
    b: &[EdgeBounds],
    padding: f64,
    active: &mut Vec<EdgeBounds>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    active.clear();
    let (mut ai, mut bi) = (0, 0);
    while ai < a.len() || bi < b.len() {
        check_cancel(cancel)?;
        charge(work, 3)?; // Merge comparison, padded box and active append.
        let raw = if bi == b.len() || ai < a.len() && a[ai].min[0].total_cmp(&b[bi].min[0]).is_le()
        {
            let edge = a[ai];
            ai += 1;
            edge
        } else {
            let edge = b[bi];
            bi += 1;
            edge
        };
        // Uniform query padding is monotone, so unpadded order stays valid.
        let edge = padded(raw, padding)?;
        let mut position = 0;
        while position < active.len() {
            check_cancel(cancel)?;
            charge(work, 1)?;
            let previous = &active[position];
            if previous.max[0] < edge.min[0] {
                charge(work, 1)?;
                active.swap_remove(position);
                continue;
            }
            if previous.min[1] <= edge.max[1] && edge.min[1] <= previous.max[1] {
                charge(work, 1)?; // ALL repeated intra- AND cross-operand candidates.
            }
            position += 1;
        }
        active.push(edge);
    }
    check_cancel(cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Coord, LineString};

    #[test]
    fn prepared_queries_keep_cumulative_refusal_and_cancellation_after_warmup() {
        let source = MultiPolygon(vec![Polygon::new(
            LineString(
                (0..=64)
                    .map(|i| {
                        let angle = std::f64::consts::TAU * (i % 64) as f64 / 64.0;
                        Coord {
                            x: angle.cos(),
                            y: angle.sin(),
                        }
                    })
                    .collect(),
            ),
            Vec::new(),
        )]);
        let clip = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                [-0.5, -0.5],
                [0.5, -0.5],
                [0.5, 0.5],
                [-0.5, 0.5],
                [-0.5, -0.5],
            ]),
            Vec::new(),
        )]);
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let prepared = Prepared::new(&source, 1_000_000, &mut work, &cancel)
            .unwrap()
            .unwrap();
        let other = Prepared::new(&clip, 1_000_000, &mut work, &cancel)
            .unwrap()
            .unwrap();
        let mut workspace = Workspace::new();
        workspace
            .budget_pair(&prepared, &other, 1_000_000, &mut work, &cancel)
            .unwrap();
        let failure = workspace
            .budget_pair(
                &prepared,
                &other,
                1_000_000,
                &mut (MAX_BOOLEAN_PAIRS - 1),
                &cancel,
            )
            .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
        assert_eq!(
            workspace
                .budget_pair(&prepared, &other, 1_000_000, &mut 0, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            ManufacturingErrorCode::Cancelled
        );
    }
}
