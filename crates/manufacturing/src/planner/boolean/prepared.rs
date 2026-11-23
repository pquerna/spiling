// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Borrowed, immutable Boolean ADMISSION metadata, never geometry authority.
//! Kernel callers still use the exact full source operands and original order.
use super::*;

pub(crate) struct Prepared<'a> {
    source: &'a MultiPolygon<f64>,
    inventory: Inventory,
}
impl<'a> Prepared<'a> {
    /// Inventory once; raw validation, cumulative work and cancellation
    /// errors are not caught. Retains no scratch.
    pub(crate) fn new(
        source: &'a MultiPolygon<f64>,
        _available_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let inventory = inspect(source.0.iter(), work, cancel)?;
        Ok(Some(Self { source, inventory }))
    }
    pub(crate) fn source(&self) -> &'a MultiPolygon<f64> {
        self.source
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        0
    }
}

pub(crate) struct Workspace;
impl Workspace {
    pub(crate) fn new() -> Self {
        Self
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        0
    }

    pub(crate) fn budget_pair(
        &mut self,
        a: &Prepared<'_>,
        b: &Prepared<'_>,
        _available_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let other = if std::ptr::eq(a.source, b.source) {
            Inventory::empty()
        } else {
            b.inventory
        };
        admit_query(a.inventory, other, work, cancel)
    }

    pub(crate) fn budget_raw(
        &mut self,
        a: &MultiPolygon<f64>,
        b: &Prepared<'_>,
        available_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        if std::ptr::eq(a, b.source) {
            return self.budget_pair(b, b, available_bytes, work, cancel);
        }
        let inventory = inspect(a.0.iter(), work, cancel)?;
        admit_query(inventory, b.inventory, work, cancel)
    }
}

/// The kernel still converts both operands on every call: charge their points
/// and refuse an unconditioned coordinate frame.
fn admit_query(
    a: Inventory,
    b: Inventory,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    charge(
        work,
        a.points
            .checked_add(b.points)
            .ok_or_else(|| resource("Boolean point count overflow"))?,
    )?;
    if a.count == 0 && b.count == 0 {
        return Ok(());
    }
    let min = [a.min[0].min(b.min[0]), a.min[1].min(b.min[1])];
    let max = [a.max[0].max(b.max[0]), a.max[1].max(b.max[1])];
    overlay_padding(min, max).map(|_| ())
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
            .budget_pair(&prepared, &other, 1_000_000, &mut (usize::MAX - 1), &cancel)
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
