// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Actual object bead edges whose nominal capsule is not provably inside the
//! section material D they were planned from. Support placement already
//! excludes D at the requested clearance; only these remaining edges need their
//! own capsule exclusion. Variable-width walls and Bridge construction can put
//! a bead footprint slightly outside D. This screen grants no anchor, material
//! or collision authority: final actual capsule proofs stay independent.

use crate::check_cancel;
use geo::MultiPolygon;
use spiling_contracts::manufacturing::{DepositionPath, ManufacturingError};
use std::sync::atomic::AtomicBool;

/// One actual object bead edge in XY with its nominal half width.
#[derive(Clone, Copy, Debug)]
pub(super) struct BeadEdge {
    pub(super) a: [f64; 2],
    pub(super) b: [f64; 2],
    pub(super) half: f64,
}

/// Uniform grid of D boundary segments, registered in every cell their bounds
/// cross. Cell size keeps about one segment per cell and at least one bead.
struct BoundaryGrid {
    segments: Vec<[[f64; 2]; 2]>,
    /// Segment indexes sorted by cell; `starts[cell]..starts[cell + 1]`.
    entries: Vec<u32>,
    starts: Vec<usize>,
    origin: [f64; 2],
    cell: f64,
    dims: [usize; 2],
}

impl BoundaryGrid {
    fn new(
        material: &MultiPolygon<f64>,
        reach: f64,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let mut segments = Vec::new();
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for polygon in &material.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                let n = ring.0.len();
                for i in 0..n {
                    super::super::native::charge_support_clearance(work, cancel)?;
                    let p = ring.0[i];
                    let q = ring.0[(i + 1) % n];
                    if p == q {
                        continue;
                    }
                    lo = [lo[0].min(p.x), lo[1].min(p.y)];
                    hi = [hi[0].max(p.x), hi[1].max(p.y)];
                    segments.push([[p.x, p.y], [q.x, q.y]]);
                }
            }
        }
        if segments.is_empty() || lo.iter().chain(&hi).any(|v| !v.is_finite()) {
            return Ok(None);
        }
        let extent = [
            (hi[0] - lo[0]).max(f64::MIN_POSITIVE),
            (hi[1] - lo[1]).max(f64::MIN_POSITIVE),
        ];
        let cell = (extent[0] * extent[1] / segments.len() as f64)
            .sqrt()
            .max(2.0 * reach)
            .max(extent[0].max(extent[1]) / 4096.0);
        if !cell.is_finite() || cell <= 0.0 {
            return Ok(None);
        }
        let dims = extent.map(|e| ((e / cell).floor() as usize + 1).max(1));
        let mut grid = Self {
            segments,
            entries: Vec::new(),
            starts: vec![0; dims[0] * dims[1] + 1],
            origin: lo,
            cell,
            dims,
        };
        // Counting sort of (cell, segment) registrations.
        for pass in 0..2 {
            if pass == 1 {
                for index in 1..grid.starts.len() {
                    grid.starts[index] += grid.starts[index - 1];
                }
                grid.entries = vec![0; *grid.starts.last().expect("cell offsets")];
            }
            let mut fill = if pass == 1 {
                grid.starts.clone()
            } else {
                Vec::new()
            };
            for (index, segment) in grid.segments.iter().enumerate() {
                check_cancel(cancel)?;
                let [x0, y0] = grid.cell_of([
                    segment[0][0].min(segment[1][0]),
                    segment[0][1].min(segment[1][1]),
                ]);
                let [x1, y1] = grid.cell_of([
                    segment[0][0].max(segment[1][0]),
                    segment[0][1].max(segment[1][1]),
                ]);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        super::super::native::charge_support_clearance(work, cancel)?;
                        let cell = y * dims[0] + x;
                        if pass == 0 {
                            grid.starts[cell + 1] += 1;
                        } else {
                            grid.entries[fill[cell]] = index as u32;
                            fill[cell] += 1;
                        }
                    }
                }
            }
        }
        Ok(Some(grid))
    }

    fn cell_of(&self, point: [f64; 2]) -> [usize; 2] {
        std::array::from_fn(|axis| {
            let offset = ((point[axis] - self.origin[axis]) / self.cell).floor();
            if offset.is_nan() || offset < 0.0 {
                0
            } else {
                (offset as usize).min(self.dims[axis] - 1)
            }
        })
    }

    fn cell(&self, x: usize, y: usize) -> &[u32] {
        let cell = y * self.dims[0] + x;
        &self.entries[self.starts[cell]..self.starts[cell + 1]]
    }

    /// True when some boundary segment lies strictly within `reach` of `a`-`b`.
    fn near(
        &self,
        a: [f64; 2],
        b: [f64; 2],
        reach: f64,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        let [x0, y0] = self.cell_of([a[0].min(b[0]) - reach, a[1].min(b[1]) - reach]);
        let [x1, y1] = self.cell_of([a[0].max(b[0]) + reach, a[1].max(b[1]) + reach]);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for &index in self.cell(x, y) {
                    super::super::native::charge_support_clearance(work, cancel)?;
                    let [c, d] = self.segments[index as usize];
                    if segment_distance(a, b, c, d) < reach {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// Even-odd containment by a +X ray; each crossing is counted only in the
    /// cell whose X range holds it.
    fn contains(
        &self,
        point: [f64; 2],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        let [x0, y] = self.cell_of(point);
        if point[1] < self.origin[1] || point[1] > self.origin[1] + self.cell * self.dims[1] as f64
        {
            return Ok(false);
        }
        let mut inside = false;
        for x in x0..self.dims[0] {
            let left = self.origin[0] + self.cell * x as f64;
            let right = if x + 1 == self.dims[0] {
                f64::INFINITY
            } else {
                self.origin[0] + self.cell * (x + 1) as f64
            };
            let left = if x == 0 { f64::NEG_INFINITY } else { left };
            for &index in self.cell(x, y) {
                super::super::native::charge_support_clearance(work, cancel)?;
                let [c, d] = self.segments[index as usize];
                if (c[1] > point[1]) == (d[1] > point[1]) {
                    continue;
                }
                let crossing = c[0] + (point[1] - c[1]) / (d[1] - c[1]) * (d[0] - c[0]);
                if crossing > point[0] && crossing >= left && crossing < right {
                    inside = !inside;
                }
            }
        }
        Ok(inside)
    }
}

fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length2 = d[0] * d[0] + d[1] * d[1];
    let t = if length2 > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - (a[0] + t * d[0])).hypot(p[1] - (a[1] + t * d[1]))
}

fn segment_distance(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (o1, o2) = (orient(a, b, c), orient(a, b, d));
    let (o3, o4) = (orient(c, d, a), orient(c, d, b));
    if ((o1 > 0.0) != (o2 > 0.0) || o1 == 0.0 || o2 == 0.0)
        && ((o3 > 0.0) != (o4 > 0.0) || o3 == 0.0 || o4 == 0.0)
    {
        // Crossing or collinear contact: treat as touching.
        return 0.0;
    }
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}

/// Every actual non-support bead edge of `objects` whose capsule reaches more
/// than `allowance` outside `material`: it crosses or nears D's boundary, or a
/// boundary-free run of the path lies outside D. The allowance must stay within
/// the reserve the caller's D exclusion already adds. Edges are returned once
/// per layer in path order.
pub(super) fn outside_material<'a>(
    material: &MultiPolygon<f64>,
    objects: impl Iterator<Item = &'a DepositionPath> + Clone,
    allowance: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<BeadEdge>, ManufacturingError> {
    let mut result = Vec::new();
    let mut widest = 0.0f64;
    for path in objects.clone() {
        if super::support_role(path.kind) {
            continue;
        }
        if let Ok(edges) = path.checked_edges() {
            for edge in edges {
                widest = widest.max(edge.width_mm);
            }
        }
    }
    let grid = BoundaryGrid::new(material, widest * 0.5, work, cancel)?;
    for path in objects {
        {
            check_cancel(cancel)?;
            if super::support_role(path.kind) || path.points_mm.len() < 2 {
                continue;
            }
            let Ok(edges) = path.checked_edges() else {
                continue;
            };
            // Containment of the current boundary-free run, decided once.
            let mut run_inside: Option<bool> = None;
            for edge in edges {
                super::super::native::charge_support_clearance(work, cancel)?;
                let &[p, q] = edge.points_mm else {
                    continue;
                };
                let (a, b) = ([p[0], p[1]], [q[0], q[1]]);
                let half = edge.width_mm * 0.5;
                let bead = BeadEdge { a, b, half };
                let Some(grid) = &grid else {
                    result.push(bead);
                    continue;
                };
                if grid.near(a, b, half - allowance, work, cancel)? {
                    run_inside = None;
                    result.push(bead);
                    continue;
                }
                let inside = match run_inside {
                    Some(inside) => inside,
                    None => *run_inside.insert(grid.contains(a, work, cancel)?),
                };
                if !inside {
                    result.push(bead);
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;
    use spiling_contracts::manufacturing::{DepositionKind, WidthProfile};

    fn paths(paths: Vec<(Vec<[f64; 2]>, f64)>) -> Vec<DepositionPath> {
        paths
            .into_iter()
            .map(|(points, width_mm)| DepositionPath {
                material_domain: None,
                kind: DepositionKind::Perimeter,
                owner: None,
                width: WidthProfile::Constant { width_mm },
                physical_z_mm: [0.0, 0.2],
                points_mm: points.into_iter().map(|[x, y]| [x, y, 0.2]).collect(),
                arcs: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn only_beads_reaching_outside_material_are_reported() {
        let material = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0), (x: 10.0, y: 10.0), (x: 0.0, y: 10.0),
        ]]);
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let objects = paths(vec![
            // Outer wall exactly half a bead inside: its footprint stays in D.
            (vec![[0.2, 0.2], [9.8, 0.2]], 0.4),
            // Wall poking 0.05 mm outside D.
            (vec![[0.15, 5.0], [0.15, 9.0]], 0.4),
            // Interior infill far from the boundary.
            (vec![[2.0, 2.0], [8.0, 8.0]], 0.4),
            // A stray stroke entirely outside D.
            (vec![[20.0, 20.0], [25.0, 20.0]], 0.4),
        ]);
        let edges = outside_material(&material, objects.iter(), 0.002, &mut work, &cancel).unwrap();
        let starts: Vec<[f64; 2]> = edges.iter().map(|edge| edge.a).collect();
        assert_eq!(starts, vec![[0.15, 5.0], [20.0, 20.0]]);
        assert!(work > 0);
    }
}
