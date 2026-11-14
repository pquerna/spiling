// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Exposed top skin. A mandatory top-skin path that the next scheduled
//! layer's material does not completely cover lies on the part's top surface
//! and is marked `exposed`; covered top skin is internal solid under that
//! surface (PrusaSlicer's top solid infill versus solid infill). The mark is
//! a speed declaration only: classification, ownership, hatch axis and every
//! construction check keep the TopSkin role.

use super::{SectionSpan, span_at};
use crate::{check_cancel, error};
use geo::MultiPolygon;
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) fn mark_exposed_top(
    layers: &mut [PrintLayer],
    sections: &[SectionSpan],
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let top_owner = Some(DepositionOwner::MandatorySkin {
        skin: ProcessSkinKind::TopSkin,
    });
    let mut grid: Option<(usize, BoundaryGrid)> = None;
    let count = layers.len();
    for (index, layer) in layers.iter_mut().enumerate() {
        check_cancel(cancel)?;
        if !layer
            .paths
            .iter()
            .any(|p| p.kind == DepositionKind::TopSkin && p.owner == top_owner)
        {
            continue;
        }
        let current = span_at(sections, index as u32);
        let next = (index + 1 < count).then(|| span_at(sections, index as u32 + 1));
        for path in &mut layer.paths {
            if path.kind != DepositionKind::TopSkin || path.owner != top_owner {
                continue;
            }
            check_cancel(cancel)?;
            path.exposed = match next {
                None => true,
                // The held section continues: the same material covers the
                // fill this skin lies in.
                Some(next) if next == current => false,
                Some(next) => {
                    if grid.as_ref().is_none_or(|(span, _)| *span != next) {
                        drop(grid.take());
                        grid = Some((next, BoundaryGrid::new(&sections[next].material)?));
                    }
                    let (_, grid) = grid.as_ref().expect("boundary grid built");
                    !grid.covers(&path.points_mm, cancel)?
                }
            };
        }
    }
    Ok(())
}

/// Boundary edges of one layer's material bucketed on a uniform grid, for
/// exact point-in-material and segment-crossing tests.
struct BoundaryGrid {
    min: [f64; 2],
    cell: f64,
    columns: usize,
    rows: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
    edges: Vec<[[f64; 2]; 2]>,
}

impl BoundaryGrid {
    fn new(material: &MultiPolygon<f64>) -> Result<Self, ManufacturingError> {
        let mut edges = Vec::new();
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        let edge_count: usize = material
            .0
            .iter()
            .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
            .map(|ring| ring.0.len().saturating_sub(1))
            .sum();
        admit(edge_count.saturating_mul(std::mem::size_of::<[[f64; 2]; 2]>() + 16))?;
        edges.reserve_exact(edge_count);
        for polygon in &material.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for pair in ring.0.windows(2) {
                    let (a, b) = ([pair[0].x, pair[0].y], [pair[1].x, pair[1].y]);
                    for axis in 0..2 {
                        min[axis] = min[axis].min(a[axis]).min(b[axis]);
                        max[axis] = max[axis].max(a[axis]).max(b[axis]);
                    }
                    edges.push([a, b]);
                }
            }
        }
        if edges.is_empty() {
            return Ok(Self {
                min: [0.0; 2],
                cell: 1.0,
                columns: 0,
                rows: 0,
                starts: vec![0],
                items: Vec::new(),
                edges,
            });
        }
        let extent = [(max[0] - min[0]).max(1e-9), (max[1] - min[1]).max(1e-9)];
        // About one edge per cell on average.
        let cell = ((extent[0] * extent[1]) / edges.len() as f64)
            .sqrt()
            .max(extent[0].max(extent[1]) / 4096.0)
            .max(1e-6);
        let columns = ((extent[0] / cell).floor() as usize + 1).max(1);
        let rows = ((extent[1] / cell).floor() as usize + 1).max(1);
        let cells = columns
            .checked_mul(rows)
            .filter(|&c| c < u32::MAX as usize)
            .ok_or_else(|| error(ManufacturingErrorCode::ResourceLimit, "exposure grid size"))?;
        let mut grid = Self {
            min,
            cell,
            columns,
            rows,
            starts: Vec::new(),
            items: Vec::new(),
            edges,
        };
        let mut counts = vec![0u32; cells + 1];
        let mut total = 0usize;
        for &[a, b] in &grid.edges {
            grid.each_cell(a, b, |cell| {
                counts[cell + 1] += 1;
                total += 1;
            });
        }
        if total >= u32::MAX as usize {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "exposure grid entry count",
            ));
        }
        admit(
            grid.edges.len() * std::mem::size_of::<[[f64; 2]; 2]>()
                + (cells + 1 + total) * std::mem::size_of::<u32>(),
        )?;
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let mut fill = counts.clone();
        let mut items = vec![0u32; total];
        for (index, &[a, b]) in grid.edges.iter().enumerate() {
            grid.each_cell(a, b, |cell| {
                let slot = &mut fill[cell];
                items[*slot as usize] = index as u32;
                *slot += 1;
            });
        }
        grid.starts = counts;
        grid.items = items;
        Ok(grid)
    }

    fn cell_of(&self, p: [f64; 2]) -> (usize, usize) {
        let c = ((p[0] - self.min[0]) / self.cell).floor();
        let r = ((p[1] - self.min[1]) / self.cell).floor();
        (
            (c.max(0.0) as usize).min(self.columns.saturating_sub(1)),
            (r.max(0.0) as usize).min(self.rows.saturating_sub(1)),
        )
    }

    /// Every cell a segment passes through: per grid row, the cells spanned
    /// by the segment's X extent within that row's band (padded by one cell
    /// so rounding never skips a crossed cell).
    fn each_cell(&self, a: [f64; 2], b: [f64; 2], mut visit: impl FnMut(usize)) {
        let (_, r0) = self.cell_of(a);
        let (_, r1) = self.cell_of(b);
        for row in r0.min(r1)..=r0.max(r1) {
            let band = [
                self.min[1] + row as f64 * self.cell,
                self.min[1] + (row + 1) as f64 * self.cell,
            ];
            let at = |y: f64| {
                if a[1] == b[1] {
                    a[0]
                } else {
                    a[0] + ((y - a[1]) / (b[1] - a[1])).clamp(0.0, 1.0) * (b[0] - a[0])
                }
            };
            let (x0, x1) = (at(band[0]), at(band[1]));
            let (lo, hi) = if a[1] == b[1] {
                (a[0].min(b[0]), a[0].max(b[0]))
            } else {
                (x0.min(x1), x0.max(x1))
            };
            let first = self.cell_of([lo, band[0]]).0.saturating_sub(1);
            let last = (self.cell_of([hi, band[0]]).0 + 1).min(self.columns - 1);
            for column in first..=last {
                visit(row * self.columns + column);
            }
        }
    }

    fn cell_edges(&self, column: usize, row: usize) -> &[u32] {
        let cell = row * self.columns + column;
        &self.items[self.starts[cell] as usize..self.starts[cell + 1] as usize]
    }

    /// Even-odd ray cast in +X; each crossing is counted in the one cell that
    /// contains it.
    fn inside(&self, p: [f64; 2]) -> bool {
        if self.edges.is_empty()
            || p[0] < self.min[0]
            || p[1] < self.min[1]
            || p[0] > self.min[0] + self.cell * self.columns as f64
            || p[1] > self.min[1] + self.cell * self.rows as f64
        {
            return false;
        }
        let (start, row) = self.cell_of(p);
        let mut crossings = 0usize;
        for column in start..self.columns {
            for &edge in self.cell_edges(column, row) {
                let [a, b] = self.edges[edge as usize];
                if (a[1] > p[1]) == (b[1] > p[1]) {
                    continue;
                }
                let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if x > p[0] && self.cell_of([x, p[1]]).0 == column {
                    crossings += 1;
                }
            }
        }
        crossings % 2 == 1
    }

    /// Every vertex lies inside the material and no edge of the path meets
    /// its boundary (touching counts as meeting, so exposure is conservative).
    fn covers(&self, points: &[[f64; 3]], cancel: &AtomicBool) -> Result<bool, ManufacturingError> {
        if points.iter().any(|p| !self.inside([p[0], p[1]])) {
            return Ok(false);
        }
        for pair in points.windows(2) {
            check_cancel(cancel)?;
            let (a, b) = ([pair[0][0], pair[0][1]], [pair[1][0], pair[1][1]]);
            let mut meets = false;
            self.each_cell(a, b, |cell| {
                if !meets {
                    meets = self.items[self.starts[cell] as usize..self.starts[cell + 1] as usize]
                        .iter()
                        .any(|&edge| {
                            let [p, q] = self.edges[edge as usize];
                            segments_meet(a, b, p, q)
                        });
                }
            });
            if meets {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn admit(bytes: usize) -> Result<(), ManufacturingError> {
    if bytes > crate::working_set::bytes() {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "exposed-top boundary index exceeds the working set",
        ));
    }
    Ok(())
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

fn segments_meet(a: [f64; 2], b: [f64; 2], p: [f64; 2], q: [f64; 2]) -> bool {
    let (d1, d2) = (orient(p, q, a), orient(p, q, b));
    let (d3, d4) = (orient(a, b, p), orient(a, b, q));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && on_segment(p, q, a))
        || (d2 == 0.0 && on_segment(p, q, b))
        || (d3 == 0.0 && on_segment(a, b, p))
        || (d4 == 0.0 && on_segment(a, b, q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{LineString, Polygon};

    fn square(x0: f64, x1: f64) -> Polygon<f64> {
        Polygon::new(
            LineString::from(vec![
                (x0, 0.0),
                (x1, 0.0),
                (x1, 10.0),
                (x0, 10.0),
                (x0, 0.0),
            ]),
            Vec::new(),
        )
    }

    #[test]
    fn covered_rows_lie_inside_without_meeting_the_boundary() {
        let cancel = AtomicBool::new(false);
        let mut ring = square(0.0, 10.0);
        ring.interiors_push(LineString::from(vec![
            (4.0, 4.0),
            (6.0, 4.0),
            (6.0, 6.0),
            (4.0, 6.0),
            (4.0, 4.0),
        ]));
        let grid = BoundaryGrid::new(&MultiPolygon(vec![ring, square(20.0, 30.0)])).unwrap();
        let row = |x0: f64, x1: f64, y: f64| [[x0, y, 1.0], [x1, y, 1.0]];
        assert!(grid.covers(&row(1.0, 9.0, 2.0), &cancel).unwrap());
        assert!(grid.covers(&row(21.0, 29.0, 9.0), &cancel).unwrap());
        // Crosses the hole, leaves the material, or bridges two islands.
        assert!(!grid.covers(&row(1.0, 9.0, 5.0), &cancel).unwrap());
        assert!(!grid.covers(&row(8.0, 12.0, 2.0), &cancel).unwrap());
        assert!(!grid.covers(&row(9.0, 21.0, 2.0), &cancel).unwrap());
        assert!(!grid.covers(&row(4.5, 5.5, 5.0), &cancel).unwrap());
        let empty = BoundaryGrid::new(&MultiPolygon(Vec::new())).unwrap();
        assert!(!empty.covers(&row(1.0, 9.0, 2.0), &cancel).unwrap());
    }
}
