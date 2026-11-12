// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Uniform edge grid over a nominal anchor inventory. It only selects which
//! borrowed edges reach the unchanged per-edge broad box and capsule proof, in
//! their original inventory order; every other z-eligible edge is still charged
//! as skipped work at the same position. Paths whose edges could raise a typed
//! refusal after the broad box (malformed, zero-length or out-of-range
//! coordinates) are never indexed and are enumerated whole in order.

use super::*;

/// Absolute cell-cover slack in millimetres. It only enlarges the indexed
/// footprint; it never enters a proof.
const COVER_SLACK_MM: f64 = 1e-6;
/// Coordinates and radii beyond this stay on the ordered full enumeration so
/// floating overflow refusals keep their original position.
pub(super) const INDEXED_MAGNITUDE_MM: f64 = 1e6;

#[derive(Clone, Copy)]
pub(super) struct GridEdge {
    pub(super) c: [f64; 2],
    pub(super) d: [f64; 2],
    pub(super) width: f64,
    pub(super) path: u32,
}

pub(super) struct EdgeGrid {
    origin: [f64; 2],
    cell: f64,
    dims: [usize; 2],
    offsets: Vec<u32>,
    entries: Vec<u32>,
    /// Global inventory edges in original path/edge order.
    pub(super) edges: Vec<GridEdge>,
    /// First global edge of each inventory path; one extra terminal entry.
    pub(super) first_edge: Vec<u32>,
    /// Paths that are enumerated whole instead of indexed, ascending.
    pub(super) whole: Vec<u32>,
    stamp: Vec<u32>,
    query: u32,
    pub(super) hits: Vec<u32>,
    prefixes: Vec<((u64, bool), Vec<usize>)>,
}

impl EdgeGrid {
    pub(super) fn new<'a>(
        paths: impl ExactSizeIterator<Item = &'a DepositionPath>,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let count = paths.len();
        if count >= u32::MAX as usize {
            return Ok(None);
        }
        let mut edges = Vec::new();
        let mut first_edge = Vec::with_capacity(count + 1);
        let mut whole = Vec::new();
        let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
        let mut max_radius = 0.0_f64;
        for (index, path) in paths.enumerate() {
            check_cancel(cancel)?;
            first_edge.push(edges.len() as u32);
            let start = edges.len();
            let mut indexed = true;
            match path.checked_edges() {
                Ok(view) => {
                    for edge in view {
                        let (p, q) = (edge.points_mm[0], edge.points_mm[1]);
                        let c = [p[0], p[1]];
                        let d = [q[0], q[1]];
                        let radius = edge.width_mm * 0.5;
                        if !(radius.is_finite() && radius > 0.0 && radius <= INDEXED_MAGNITUDE_MM)
                            || c.iter()
                                .chain(&d)
                                .any(|x| !(x.abs() <= INDEXED_MAGNITUDE_MM))
                            || (c[0] == d[0] && c[1] == d[1])
                        {
                            indexed = false;
                            break;
                        }
                        edges.push(GridEdge {
                            c,
                            d,
                            width: edge.width_mm,
                            path: index as u32,
                        });
                    }
                }
                Err(_) => indexed = false,
            }
            if indexed && edges.len() - start != path.points_mm.len().saturating_sub(1) {
                indexed = false;
            }
            if !indexed {
                edges.truncate(start);
                whole.push(index as u32);
                // Keep global positions aligned with original per-path edge counts.
                let span = path.points_mm.len().saturating_sub(1);
                edges.extend(std::iter::repeat_n(
                    GridEdge {
                        c: [f64::NAN; 2],
                        d: [f64::NAN; 2],
                        width: f64::NAN,
                        path: index as u32,
                    },
                    span,
                ));
                continue;
            }
            for edge in &edges[start..] {
                max_radius = max_radius.max(edge.width * 0.5);
                for point in [edge.c, edge.d] {
                    for axis in 0..2 {
                        bounds[0][axis] = bounds[0][axis].min(point[axis]);
                        bounds[1][axis] = bounds[1][axis].max(point[axis]);
                    }
                }
            }
            if edges.len() >= u32::MAX as usize {
                return Ok(None);
            }
        }
        first_edge.push(edges.len() as u32);
        let indexed = edges.iter().filter(|edge| !edge.width.is_nan()).count();
        let mut grid = Self {
            origin: [0.0; 2],
            cell: 1.0,
            dims: [1, 1],
            offsets: Vec::new(),
            entries: Vec::new(),
            stamp: vec![0; edges.len()],
            edges,
            first_edge,
            whole,
            query: 0,
            hits: Vec::new(),
            prefixes: Vec::new(),
        };
        let reach = max_radius + 2.0 * COVER_SLACK_MM;
        if indexed > 0 {
            let lo = [bounds[0][0] - reach, bounds[0][1] - reach];
            let hi = [bounds[1][0] + reach, bounds[1][1] + reach];
            let extent = [hi[0] - lo[0], hi[1] - lo[1]];
            // Roughly one indexed edge per cell; never finer than a bead.
            let mut cell = ((extent[0] * extent[1]) / indexed as f64)
                .sqrt()
                .max(2.0 * reach);
            let cells = |cell: f64| {
                ((extent[0] / cell).floor() as usize + 1)
                    .saturating_mul((extent[1] / cell).floor() as usize + 1)
            };
            while cells(cell) > 4 * indexed + 64 {
                cell *= 2.0;
            }
            grid.origin = lo;
            grid.cell = cell;
            grid.dims = [
                (extent[0] / cell).floor() as usize + 1,
                (extent[1] / cell).floor() as usize + 1,
            ];
        }
        let cell_count = grid.dims[0] * grid.dims[1];
        let mut counts = vec![0u32; cell_count + 1];
        for edge in grid.edges.iter().filter(|edge| !edge.width.is_nan()) {
            check_cancel(cancel)?;
            grid.cover(edge.c, edge.d, edge.width * 0.5, |cell| {
                counts[cell + 1] += 1
            });
        }
        for cell in 0..cell_count {
            counts[cell + 1] = counts[cell + 1].checked_add(counts[cell]).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "anchor edge grid exceeds index range",
                )
            })?;
        }
        let mut fill = counts.clone();
        let mut entries = vec![0u32; counts[cell_count] as usize];
        for (index, edge) in grid.edges.iter().enumerate() {
            if edge.width.is_nan() {
                continue;
            }
            grid.cover(edge.c, edge.d, edge.width * 0.5, |cell| {
                entries[fill[cell] as usize] = index as u32;
                fill[cell] += 1;
            });
        }
        grid.offsets = counts;
        grid.entries = entries;
        Ok(Some(grid))
    }

    fn index(&self, value: f64, axis: usize) -> usize {
        let cell = ((value - self.origin[axis]) / self.cell).floor();
        if cell <= 0.0 {
            0
        } else {
            (cell as usize).min(self.dims[axis] - 1)
        }
    }

    /// Visit every cell meeting the square dilation of segment `c..d` by
    /// `radius` plus slack. That square dilation contains the round capsule.
    fn cover(&self, c: [f64; 2], d: [f64; 2], radius: f64, mut visit: impl FnMut(usize)) {
        let e = radius + COVER_SLACK_MM;
        let (p, q) = if c[0] <= d[0] { (c, d) } else { (d, c) };
        let dx = q[0] - p[0];
        let [ymin, ymax] = [p[1].min(q[1]), p[1].max(q[1])];
        let first = self.index(p[0] - e, 0);
        let last = self.index(q[0] + e, 0);
        for column in first..=last {
            let (y0, y1) = if dx > 0.0 {
                let slab_lo = self.origin[0] + column as f64 * self.cell - e;
                let slab_hi = self.origin[0] + (column + 1) as f64 * self.cell + e;
                let lo = slab_lo.clamp(p[0], q[0]);
                let hi = slab_hi.clamp(p[0], q[0]);
                let at = |x: f64| (p[1] + (x - p[0]) * ((q[1] - p[1]) / dx)).clamp(ymin, ymax);
                let (a, b) = (at(lo), at(hi));
                (a.min(b), a.max(b))
            } else {
                (ymin, ymax)
            };
            let row_first = self.index(y0 - e, 1);
            let row_last = self.index(y1 + e, 1);
            for row in row_first..=row_last {
                visit(row * self.dims[0] + column);
            }
        }
    }

    /// Prefix of z-eligible inventory edges before each path for one query floor.
    pub(super) fn prefix(
        &mut self,
        floor: super::LowerFloor,
        nominal: &[super::NominalPath<'_>],
    ) -> usize {
        let key = (floor.z.to_bits(), floor.contacts);
        if let Some(position) = self.prefixes.iter().position(|(bits, _)| *bits == key) {
            return position;
        }
        let mut prefix = Vec::with_capacity(nominal.len() + 1);
        let mut total = 0usize;
        prefix.push(0);
        for (index, path) in nominal.iter().enumerate() {
            if path.eligible(floor) {
                total += (self.first_edge[index + 1] - self.first_edge[index]) as usize;
            }
            prefix.push(total);
        }
        if self.prefixes.len() == 8 {
            self.prefixes.remove(0);
        }
        self.prefixes.push((key, prefix));
        self.prefixes.len() - 1
    }

    pub(super) fn prefix_at(&self, position: usize) -> &[usize] {
        &self.prefixes[position].1
    }

    /// Ascending global edges whose unchanged per-edge broad box admits the
    /// candidate, restricted to z-eligible indexed paths.
    pub(super) fn collect(
        &mut self,
        candidate: AnchorCandidate,
        floor: super::LowerFloor,
        nominal: &[super::NominalPath<'_>],
    ) {
        self.hits.clear();
        self.query = self.query.wrapping_add(1);
        if self.query == 0 {
            self.stamp.fill(0);
            self.query = 1;
        }
        let [a, b] = candidate.endpoints_mm;
        let radius = candidate.bead_width_mm * 0.5;
        if self.entries.is_empty() {
            return;
        }
        let (a2, b2) = ([a[0], a[1]], [b[0], b[1]]);
        // Unit direction and the candidate's own extent in that frame:
        // (start along, across, end along).
        let delta = [b2[0] - a2[0], b2[1] - a2[1]];
        let length = delta[0].hypot(delta[1]);
        let unit = [delta[0] / length, delta[1] / length];
        let frame = ([dot(unit, a2), cross(unit, a2), dot(unit, b2)], unit);
        let mut hits = std::mem::take(&mut self.hits);
        let query = self.query;
        let mut stamp = std::mem::take(&mut self.stamp);
        self.cover(a2, b2, radius, |cell| {
            let range = self.offsets[cell] as usize..self.offsets[cell + 1] as usize;
            for &index in &self.entries[range] {
                let slot = &mut stamp[index as usize];
                if *slot == query {
                    continue;
                }
                *slot = query;
                let edge = &self.edges[index as usize];
                if !nominal[edge.path as usize].eligible(floor) {
                    continue;
                }
                let (c, d) = (edge.c, edge.d);
                let endpoint_reach = radius + edge.width * 0.5;
                if a[0].min(b[0]) > c[0].max(d[0]) + endpoint_reach
                    || a[0].max(b[0]) < c[0].min(d[0]) - endpoint_reach
                    || a[1].min(b[1]) > c[1].max(d[1]) + endpoint_reach
                    || a[1].max(b[1]) < c[1].min(d[1]) - endpoint_reach
                {
                    continue;
                }
                // Conservative oriented slab: an edge wholly beyond the
                // candidate's reach on one side cannot meet either footprint.
                let reach = endpoint_reach + COVER_SLACK_MM;
                let across = [cross(frame.1, c), cross(frame.1, d)];
                let along = [dot(frame.1, c), dot(frame.1, d)];
                if across[0].min(across[1]) > frame.0[1] + reach
                    || across[0].max(across[1]) < frame.0[1] - reach
                    || along[0].min(along[1]) > frame.0[2] + reach
                    || along[0].max(along[1]) < frame.0[0] - reach
                {
                    continue;
                }
                hits.push(index);
            }
        });
        self.stamp = stamp;
        hits.sort_unstable();
        self.hits = hits;
    }
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
