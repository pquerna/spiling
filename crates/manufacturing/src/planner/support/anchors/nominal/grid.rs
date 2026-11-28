// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Uniform edge grid over a nominal anchor inventory. It only selects which
//! borrowed edges reach the unchanged per-edge broad box and capsule proof;
//! every other z-eligible edge is still charged as skipped work. Paths whose
//! edges could raise a typed refusal after the broad box (malformed,
//! zero-length or out-of-range coordinates) are never indexed and are
//! enumerated whole in order.

use super::*;

/// Absolute cell-cover slack in millimetres. It only enlarges the indexed
/// footprint; it never enters a proof.
const COVER_SLACK_MM: f64 = 1e-6;
/// Coordinates and radii beyond this stay on the ordered full enumeration so
/// floating overflow refusals keep their original position.
pub(super) const INDEXED_MAGNITUDE_MM: f64 = 1e6;

#[derive(Clone, Copy)]
pub(in super::super) struct GridEdge {
    pub(in super::super) c: [f64; 2],
    pub(in super::super) d: [f64; 2],
    pub(in super::super) unit: [f64; 2],
    pub(in super::super) length: f64,
    pub(in super::super) width: f64,
    pub(in super::super) path: u32,
}

struct FloorCache {
    key: (u64, bool, bool),
    total_eligible: usize,
    has_whole: bool,
    prefix: Option<Vec<usize>>,
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
    /// True when every indexed edge stays within XYZ6 projection bounds and
    /// exceeds the zero-length projection screen, so endpoint projection
    /// cannot raise a typed refusal on any indexed edge.
    pub(super) all_xyz6_safe: bool,
    stamp: Vec<u32>,
    query: u32,
    pub(super) hits: Vec<u32>,
    floors: Vec<FloorCache>,
}

impl EdgeGrid {
    pub(super) fn retained_bytes(&self) -> usize {
        use super::super::super::vec_bytes;

        vec_bytes(&self.offsets)
            + vec_bytes(&self.entries)
            + vec_bytes(&self.edges)
            + vec_bytes(&self.first_edge)
            + vec_bytes(&self.whole)
            + vec_bytes(&self.stamp)
            + vec_bytes(&self.hits)
            + vec_bytes(&self.floors)
            + self
                .floors
                .iter()
                .map(|entry| entry.prefix.as_ref().map_or(0, vec_bytes))
                .sum::<usize>()
    }

    pub(super) fn new<'a>(
        paths: impl ExactSizeIterator<Item = &'a DepositionPath>,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        Self::new_bounded(paths, None, cancel)
    }

    /// Optional preparation owns one share of the wave's live byte allowance.
    /// All buffers, including the grid build's temporary offsets, fit that share.
    pub(super) fn new_bounded<'a>(
        paths: impl ExactSizeIterator<Item = &'a DepositionPath>,
        budget: Option<(usize, usize)>,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let count = paths.len();
        if count >= u32::MAX as usize {
            return Ok(None);
        }
        let fixed = if let Some((edges, bytes)) = budget {
            let fixed = edges
                .saturating_mul(size_of::<GridEdge>() + 2 * size_of::<u32>())
                .saturating_add((2 * count + 1).saturating_mul(size_of::<u32>()));
            if fixed > bytes {
                return Ok(None);
            }
            fixed
        } else {
            0
        };
        let mut edges = Vec::new();
        let mut first_edge = Vec::with_capacity(count + 1);
        let mut whole = Vec::new();
        if let Some((count_edges, _)) = budget
            && (edges.try_reserve_exact(count_edges).is_err()
                || whole.try_reserve_exact(count).is_err())
        {
            return Ok(None);
        }
        let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
        let mut max_radius = 0.0_f64;
        let mut all_xyz6_safe = true;
        for (index, path) in paths.enumerate() {
            check_cancel(cancel)?;
            first_edge.push(edges.len() as u32);
            let start = edges.len();
            let mut indexed = true;
            let mut path_xyz6_safe = true;
            match path.checked_edges() {
                Ok(view) => {
                    for edge in view {
                        let (p, q) = (edge.points_mm[0], edge.points_mm[1]);
                        let c = [p[0], p[1]];
                        let d = [q[0], q[1]];
                        let radius = edge.width_mm * 0.5;
                        let delta = [d[0] - c[0], d[1] - c[1]];
                        let length = delta[0].hypot(delta[1]);
                        if !(radius.is_finite()
                            && radius > 0.0
                            && radius <= INDEXED_MAGNITUDE_MM
                            && length.is_finite()
                            && length > 0.0)
                            || c.iter()
                                .chain(&d)
                                .any(|x| !x.is_finite() || x.abs() > INDEXED_MAGNITUDE_MM)
                        {
                            indexed = false;
                            break;
                        }
                        if c.iter().chain(&d).any(|x| x.abs() > MAX_XYZ6_COORDINATE_MM)
                            || (delta[0].abs() <= XYZ6_PROJECTION_SCREEN_MM
                                && delta[1].abs() <= XYZ6_PROJECTION_SCREEN_MM)
                        {
                            path_xyz6_safe = false;
                        }
                        edges.push(GridEdge {
                            c,
                            d,
                            unit: [delta[0] / length, delta[1] / length],
                            length,
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
                        unit: [f64::NAN; 2],
                        length: f64::NAN,
                        width: f64::NAN,
                        path: index as u32,
                    },
                    span,
                ));
                continue;
            }
            all_xyz6_safe &= path_xyz6_safe;
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
            all_xyz6_safe,
            query: 0,
            hits: Vec::new(),
            floors: Vec::new(),
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
        let offset_bytes = (cell_count + 1).saturating_mul(2 * size_of::<u32>());
        if budget.is_some_and(|(_, bytes)| fixed.saturating_add(offset_bytes) > bytes) {
            return Ok(None);
        }
        let mut counts = vec![0u32; cell_count + 1];
        for edge in grid.edges.iter().filter(|edge| !edge.width.is_nan()) {
            check_cancel(cancel)?;
            grid.cover(edge.c, edge.d, edge.width * 0.5, |cell| {
                counts[cell + 1] = counts[cell + 1].saturating_add(1)
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
        if budget.is_some_and(|(_, bytes)| {
            fixed
                .saturating_add(offset_bytes)
                .saturating_add(counts[cell_count] as usize * size_of::<u32>())
                > bytes
        }) {
            return Ok(None);
        }
        if let Some((count_edges, _)) = budget
            && grid.hits.try_reserve_exact(count_edges).is_err()
        {
            return Ok(None);
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
        if dx == 0.0 || ymin == ymax {
            let row_first = self.index(ymin - e, 1);
            let row_last = self.index(ymax + e, 1);
            for row in row_first..=row_last {
                let row_offset = row * self.dims[0];
                for cell in row_offset + first..=row_offset + last {
                    visit(cell);
                }
            }
            return;
        }
        let slope = (q[1] - p[1]) / dx;
        for column in first..=last {
            let slab_lo = self.origin[0] + column as f64 * self.cell - e;
            let slab_hi = self.origin[0] + (column + 1) as f64 * self.cell + e;
            let lo = slab_lo.clamp(p[0], q[0]);
            let hi = slab_hi.clamp(p[0], q[0]);
            let at = |x: f64| (p[1] + (x - p[0]) * slope).clamp(ymin, ymax);
            let (a, b) = (at(lo), at(hi));
            let (y0, y1) = (a.min(b), a.max(b));
            let row_first = self.index(y0 - e, 1);
            let row_last = self.index(y1 + e, 1);
            for row in row_first..=row_last {
                visit(row * self.dims[0] + column);
            }
        }
    }

    /// Cached eligibility summary for one query floor without allocating a
    /// prefix array unless the ordered fallback path is needed.
    pub(super) fn floor_info(
        &mut self,
        floor: super::LowerFloor,
        nominal: &[super::NominalPath<'_>],
    ) -> (usize, usize, bool) {
        let key = (floor.z.to_bits(), floor.contacts, floor.supplied);
        if let Some(position) = self.floors.iter().position(|entry| entry.key == key) {
            let entry = &self.floors[position];
            return (position, entry.total_eligible, entry.has_whole);
        }
        let mut total_eligible = 0usize;
        for (index, path) in nominal.iter().enumerate() {
            if path.eligible(floor) {
                total_eligible += (self.first_edge[index + 1] - self.first_edge[index]) as usize;
            }
        }
        let has_whole = self
            .whole
            .iter()
            .any(|&path| nominal[path as usize].eligible(floor));
        if self.floors.len() == 8 {
            self.floors.remove(0);
        }
        self.floors.push(FloorCache {
            key,
            total_eligible,
            has_whole,
            prefix: None,
        });
        let position = self.floors.len() - 1;
        (position, total_eligible, has_whole)
    }

    /// Lazily build and return the z-eligible edge prefix array for the ordered
    /// fallback path.
    pub(super) fn ensure_prefix(
        &mut self,
        position: usize,
        floor: super::LowerFloor,
        nominal: &[super::NominalPath<'_>],
    ) -> &[usize] {
        let first_edge = &self.first_edge;
        self.floors[position].prefix.get_or_insert_with(|| {
            let mut prefix = Vec::with_capacity(nominal.len() + 1);
            let mut total = 0usize;
            prefix.push(0);
            for (index, path) in nominal.iter().enumerate() {
                if path.eligible(floor) {
                    total += (first_edge[index + 1] - first_edge[index]) as usize;
                }
                prefix.push(total);
            }
            prefix
        })
    }

    pub(super) fn prefix_at(&self, position: usize) -> &[usize] {
        self.floors[position]
            .prefix
            .as_deref()
            .expect("prefix ensured")
    }

    /// Collect global edges whose unchanged per-edge broad box admits the
    /// candidate, restricted to z-eligible indexed paths, without sorting.
    pub(super) fn collect_unsorted(
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
        let a2 = [a[0], a[1]];
        let b2 = [b[0], b[1]];
        let radius = candidate.bead_width_mm * 0.5;
        if self.entries.is_empty() {
            return;
        }
        let [ax_min, ax_max] = [a2[0].min(b2[0]), a2[0].max(b2[0])];
        let [ay_min, ay_max] = [a2[1].min(b2[1]), a2[1].max(b2[1])];
        // Unit direction and the query span's own extent in that frame:
        // (start along, across, end along).
        let delta = [b2[0] - a2[0], b2[1] - a2[1]];
        // An axial stroke already has the tighter world-axis box below.
        let frame = (delta[0] != 0.0 && delta[1] != 0.0).then(|| {
            let length = delta[0].hypot(delta[1]);
            let unit = [delta[0] / length, delta[1] / length];
            ([dot(unit, a2), cross(unit, a2), dot(unit, b2)], unit)
        });
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
                if ax_min > c[0].max(d[0]) + endpoint_reach
                    || ax_max < c[0].min(d[0]) - endpoint_reach
                    || ay_min > c[1].max(d[1]) + endpoint_reach
                    || ay_max < c[1].min(d[1]) - endpoint_reach
                {
                    continue;
                }
                // Conservative oriented slab: an edge wholly beyond the
                // candidate's reach on one side cannot meet either footprint.
                if let Some((extent, unit)) = frame {
                    let reach = endpoint_reach + COVER_SLACK_MM;
                    let across = [cross(unit, c), cross(unit, d)];
                    let along = [dot(unit, c), dot(unit, d)];
                    if across[0].min(across[1]) > extent[1] + reach
                        || across[0].max(across[1]) < extent[1] - reach
                        || along[0].min(along[1]) > extent[2] + reach
                        || along[0].max(along[1]) < extent[0] - reach
                    {
                        continue;
                    }
                }
                hits.push(index);
            }
        });
        self.stamp = stamp;
        self.hits = hits;
    }

    /// Ascending global edges whose unchanged per-edge broad box admits the
    /// candidate, restricted to z-eligible indexed paths.
    pub(super) fn collect(
        &mut self,
        candidate: AnchorCandidate,
        floor: super::LowerFloor,
        nominal: &[super::NominalPath<'_>],
    ) {
        self.collect_unsorted(candidate, floor, nominal);
        self.hits.sort_unstable();
    }
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
