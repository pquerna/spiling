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
/// Finest inner-cover bitmap cell in millimetres (25 um: covers even 0.12 mm
/// layer solid-infill overlap with zero gaps).
const INNER_CELL_MIN_MM: f64 = 0.025;
/// Conservative inward safety margin for `InnerCover` cells so every point of
/// a set cell lies strictly inside some lower edge's capsule well beyond f64
/// rounding error.
const INNER_SAFETY_MM: f64 = 1e-4;
/// Query cell-boundary expansion in millimetres so line-cell supercover visits
/// every closed cell meeting the candidate segment.
const INNER_QUERY_EPS_MM: f64 = 1e-7;
/// Per-floor byte ceiling for an optional `InnerCover` bitmap. Larger extents
/// coarsen the cell size to stay bounded; if too coarse, `InnerCover` is skipped.
const INNER_MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub(super) enum CoverResult {
    /// Every closed cell touched by the candidate segment is strictly covered
    /// by eligible lower capsules, so the candidate has zero gaps.
    All,
    /// `[0, lo]` and `[hi, 1]` are strictly covered; only `[lo, hi]` needs
    /// exact lower-edge evaluation.
    Partial { lo: f64, hi: f64 },
    /// The whole segment requires exact lower-edge evaluation.
    Uncovered,
}

/// Conservative eroded-capsule bitmap for one query floor.
///
/// Every set cell `(col, row)` guarantees that EVERY point in its closed
/// `[ox + col*c, ox + (col+1)*c] x [oy + row*c, oy + (row+1)*c]` square lies
/// at distance `<= radius - INNER_SAFETY_MM` from at least one eligible lower
/// edge. It can only skip proven-covered interior spans and never drops a gap
/// or changes a gap endpoint.
pub(super) struct InnerCover {
    origin: [f64; 2],
    cell: f64,
    cols: usize,
    rows: usize,
    width: usize,
    bits: Vec<u64>,
}

impl InnerCover {
    fn retained_bytes(&self) -> usize {
        super::super::super::vec_bytes(&self.bits)
    }

    fn build(
        edges: &[GridEdge],
        nominal: &[super::NominalPath<'_>],
        floor: super::LowerFloor,
        whole: &[u32],
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        if whole.iter().any(|&p| nominal[p as usize].eligible(floor)) {
            return Ok(None);
        }
        let mut eligible = 0usize;
        let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
        let mut max_radius = 0.0_f64;
        for (idx, edge) in edges.iter().enumerate() {
            if idx & 1023 == 0 {
                check_cancel(cancel)?;
            }
            if edge.width.is_nan() || !nominal[edge.path as usize].eligible(floor) {
                continue;
            }
            let (c, d) = (edge.c, edge.d);
            // Any edge that could refuse during XYZ6 endpoint projection stays
            // off the fast path so ordered typed refusals are preserved.
            if c.iter().chain(&d).any(|x| x.abs() > MAX_XYZ6_COORDINATE_MM)
                || ((d[0] - c[0]).abs() <= XYZ6_PROJECTION_SCREEN_MM
                    && (d[1] - c[1]).abs() <= XYZ6_PROJECTION_SCREEN_MM)
            {
                return Ok(None);
            }
            eligible += 1;
            let radius = edge.width * 0.5;
            max_radius = max_radius.max(radius);
            bounds[0][0] = bounds[0][0].min(c[0].min(d[0]));
            bounds[0][1] = bounds[0][1].min(c[1].min(d[1]));
            bounds[1][0] = bounds[1][0].max(c[0].max(d[0]));
            bounds[1][1] = bounds[1][1].max(c[1].max(d[1]));
        }
        if eligible < 16 || max_radius <= INNER_SAFETY_MM {
            return Ok(None);
        }
        let pad = max_radius + INNER_CELL_MIN_MM;
        let origin = [bounds[0][0] - pad, bounds[0][1] - pad];
        let extent = [
            bounds[1][0] + pad - origin[0],
            bounds[1][1] + pad - origin[1],
        ];
        if !(extent[0].is_finite() && extent[1].is_finite() && extent[0] > 0.0 && extent[1] > 0.0) {
            return Ok(None);
        }
        let mut cell = INNER_CELL_MIN_MM;
        let dims_for = |cell: f64| -> Option<(usize, usize, usize)> {
            let cols = (extent[0] / cell).ceil();
            let rows = (extent[1] / cell).ceil();
            if !(cols.is_finite() && rows.is_finite() && cols >= 1.0 && rows >= 1.0)
                || cols > 1e7
                || rows > 1e7
            {
                return None;
            }
            let cols = cols as usize;
            let rows = rows as usize;
            let width = cols.div_ceil(64);
            Some((cols, rows, width))
        };
        let (cols, rows, width) = loop {
            if cell * std::f64::consts::FRAC_1_SQRT_2 + INNER_SAFETY_MM >= max_radius {
                return Ok(None);
            }
            if let Some((cols, rows, width)) = dims_for(cell) {
                let bytes = rows.saturating_mul(width).saturating_mul(8);
                if bytes <= INNER_MAX_BYTES {
                    break (cols, rows, width);
                }
            }
            cell *= 2.0;
        };
        let words = rows.saturating_mul(width);
        let mut bits = Vec::new();
        if bits.try_reserve_exact(words).is_err() {
            return Ok(None);
        }
        bits.resize(words, 0);
        let mut cover = Self {
            origin,
            cell,
            cols,
            rows,
            width,
            bits,
        };
        let half_diag = cell * std::f64::consts::FRAC_1_SQRT_2;
        for (idx, edge) in edges.iter().enumerate() {
            if idx & 1023 == 0 {
                check_cancel(cancel)?;
            }
            if edge.width.is_nan() || !nominal[edge.path as usize].eligible(floor) {
                continue;
            }
            cover.stamp_edge(edge.c, edge.d, edge.width * 0.5, half_diag);
        }
        Ok(Some(cover))
    }

    fn set_span(&mut self, row: usize, from: usize, to_inclusive: usize) {
        let row_slice = &mut self.bits[row * self.width..(row + 1) * self.width];
        let (first, last) = (from / 64, to_inclusive / 64);
        for (w, word) in row_slice.iter_mut().enumerate().take(last + 1).skip(first) {
            let mut mask = !0u64;
            if w == first {
                mask &= !0u64 << (from % 64);
            }
            if w == last {
                mask &= !0u64 >> (63 - to_inclusive % 64);
            }
            *word |= mask;
        }
    }

    fn stamp_rect(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let inv = 1.0 / self.cell;
        let c0 = ((x0 - self.origin[0]) * inv).ceil();
        let c1 = ((x1 - self.origin[0]) * inv).floor() - 1.0;
        let r0 = ((y0 - self.origin[1]) * inv).ceil();
        let r1 = ((y1 - self.origin[1]) * inv).floor() - 1.0;
        if c0 > c1 || r0 > r1 || c1 < 0.0 || r1 < 0.0 {
            return;
        }
        let c0 = (c0.max(0.0) as usize).min(self.cols - 1);
        let c1 = (c1 as usize).min(self.cols - 1);
        let r0 = (r0.max(0.0) as usize).min(self.rows - 1);
        let r1 = (r1 as usize).min(self.rows - 1);
        if c0 > c1 || r0 > r1 {
            return;
        }
        for row in r0..=r1 {
            self.set_span(row, c0, c1);
        }
    }

    fn stamp_disk(&mut self, center: [f64; 2], radius: f64) {
        if radius <= 0.0 {
            return;
        }
        let inv = 1.0 / self.cell;
        let r0 = ((center[1] - radius - self.origin[1]) * inv - 0.5).ceil();
        let r1 = ((center[1] + radius - self.origin[1]) * inv - 0.5).floor();
        if r0 > r1 || r1 < 0.0 {
            return;
        }
        let r0 = (r0.max(0.0) as usize).min(self.rows - 1);
        let r1 = (r1 as usize).min(self.rows - 1);
        let r2 = radius * radius;
        for row in r0..=r1 {
            let my = self.origin[1] + (row as f64 + 0.5) * self.cell;
            let dy = my - center[1];
            let rem = r2 - dy * dy;
            if rem < 0.0 {
                continue;
            }
            let dx = rem.sqrt();
            let c0 = ((center[0] - dx - self.origin[0]) * inv - 0.5).ceil();
            let c1 = ((center[0] + dx - self.origin[0]) * inv - 0.5).floor();
            if c0 > c1 || c1 < 0.0 {
                continue;
            }
            let c0 = (c0.max(0.0) as usize).min(self.cols - 1);
            let c1 = (c1 as usize).min(self.cols - 1);
            if c0 <= c1 {
                self.set_span(row, c0, c1);
            }
        }
    }

    fn stamp_edge(&mut self, c: [f64; 2], d: [f64; 2], radius: f64, half_diag: f64) {
        let r_axial = radius - INNER_SAFETY_MM;
        if r_axial <= 0.0 {
            return;
        }
        let r_diag = radius - half_diag - INNER_SAFETY_MM;
        if c[1] == d[1] {
            // Horizontal edge: closed cells wholly inside [min_x, max_x] x [y - r_axial, y + r_axial]
            // project onto the interior of the segment without diagonal erosion.
            self.stamp_rect(
                c[0].min(d[0]),
                c[1] - r_axial,
                c[0].max(d[0]),
                c[1] + r_axial,
            );
            if r_diag > 0.0 {
                self.stamp_disk(c, r_diag);
                self.stamp_disk(d, r_diag);
            }
            return;
        }
        if c[0] == d[0] {
            self.stamp_rect(
                c[0] - r_axial,
                c[1].min(d[1]),
                c[0] + r_axial,
                c[1].max(d[1]),
            );
            if r_diag > 0.0 {
                self.stamp_disk(c, r_diag);
                self.stamp_disk(d, r_diag);
            }
            return;
        }
        if r_diag <= 0.0 {
            return;
        }
        let (a, b) = if c[0] <= d[0] { (c, d) } else { (d, c) };
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = dx.hypot(dy);
        if length <= 0.0 {
            self.stamp_disk(a, r_diag);
            return;
        }
        let nx = (dy.abs() / length) * r_diag;
        let ny = (dx / length).copysign(dy) * r_diag;
        let left = [[a[0] - nx, a[1] + ny], [b[0] - nx, b[1] + ny]];
        let right = [[a[0] + nx, a[1] - ny], [b[0] + nx, b[1] - ny]];
        let r2 = r_diag * r_diag;
        let inv = 1.0 / self.cell;
        let ymin = a[1].min(b[1]) - r_diag;
        let ymax = a[1].max(b[1]) + r_diag;
        let r0 = ((ymin - self.origin[1]) * inv - 0.5).ceil();
        let r1 = ((ymax - self.origin[1]) * inv - 0.5).floor();
        if r0 > r1 || r1 < 0.0 {
            return;
        }
        let r0 = (r0.max(0.0) as usize).min(self.rows - 1);
        let r1 = (r1 as usize).min(self.rows - 1);
        for row in r0..=r1 {
            let my = self.origin[1] + (row as f64 + 0.5) * self.cell;
            let mut x_lo = f64::INFINITY;
            let mut x_hi = f64::NEG_INFINITY;
            for [px, py] in [a, b] {
                let d_y = (my - py).abs();
                if d_y <= r_diag {
                    let half = (r2 - d_y * d_y).max(0.0).sqrt();
                    x_lo = x_lo.min(px - half);
                    x_hi = x_hi.max(px + half);
                }
            }
            if let Some(x) = tangent_cross(left[0], left[1], my) {
                x_lo = x_lo.min(x);
            }
            if let Some(x) = tangent_cross(right[1], right[0], my) {
                x_hi = x_hi.max(x);
            }
            if x_lo > x_hi {
                continue;
            }
            let c0 = ((x_lo - self.origin[0]) * inv - 0.5).ceil();
            let c1 = ((x_hi - self.origin[0]) * inv - 0.5).floor();
            if c0 > c1 || c1 < 0.0 {
                continue;
            }
            let c0 = (c0.max(0.0) as usize).min(self.cols - 1);
            let c1 = (c1 as usize).min(self.cols - 1);
            if c0 <= c1 {
                self.set_span(row, c0, c1);
            }
        }
    }

    #[inline]
    fn bit(&self, row: usize, col: usize) -> bool {
        (self.bits[row * self.width + col / 64] >> (col % 64)) & 1 == 1
    }

    /// Check whether closed cells meeting segment `a..b` are set in this bitmap.
    pub(super) fn check_segment(&self, a: [f64; 2], b: [f64; 2]) -> CoverResult {
        let eps = INNER_QUERY_EPS_MM;
        let max_x = self.origin[0] + self.cols as f64 * self.cell - eps;
        let max_y = self.origin[1] + self.rows as f64 * self.cell - eps;
        let min_x = self.origin[0] + eps;
        let min_y = self.origin[1] + eps;
        if a[0] < min_x
            || b[0] < min_x
            || a[0] >= max_x
            || b[0] >= max_x
            || a[1] < min_y
            || b[1] < min_y
            || a[1] >= max_y
            || b[1] >= max_y
        {
            return CoverResult::Uncovered;
        }
        let inv = 1.0 / self.cell;
        let col_of = |x: f64| (((x - self.origin[0]) * inv).floor() as usize).min(self.cols - 1);
        let row_of = |y: f64| (((y - self.origin[1]) * inv).floor() as usize).min(self.rows - 1);
        if a[1] == b[1] {
            let r0 = row_of(a[1] - eps);
            let r1 = row_of(a[1] + eps);
            let (seg_lo, seg_hi) = (a[0].min(b[0]), a[0].max(b[0]));
            let c0 = col_of(seg_lo - eps);
            let c1 = col_of(seg_hi + eps);
            let (w0, w1) = (c0 / 64, c1 / 64);
            let mut first_miss = usize::MAX;
            let mut last_miss = 0usize;
            for w in w0..=w1 {
                let mut mask = !0u64;
                if w == w0 {
                    mask &= !0u64 << (c0 % 64);
                }
                if w == w1 {
                    mask &= !0u64 >> (63 - c1 % 64);
                }
                let mut combined = self.bits[r0 * self.width + w];
                if r1 != r0 {
                    combined &= self.bits[r1 * self.width + w];
                }
                let missing = mask & !combined;
                if missing != 0 {
                    let first = w * 64 + missing.trailing_zeros() as usize;
                    let last = w * 64 + 63 - missing.leading_zeros() as usize;
                    first_miss = first_miss.min(first);
                    last_miss = last_miss.max(last);
                }
            }
            if first_miss == usize::MAX {
                return CoverResult::All;
            }
            return self.partial_from_axis(a[0], b[0], 0, c0, c1, first_miss, last_miss);
        }
        if a[0] == b[0] {
            let c0 = col_of(a[0] - eps);
            let c1 = col_of(a[0] + eps);
            let (seg_lo, seg_hi) = (a[1].min(b[1]), a[1].max(b[1]));
            let r0 = row_of(seg_lo - eps);
            let r1 = row_of(seg_hi + eps);
            let mut first_miss = usize::MAX;
            let mut last_miss = 0usize;
            for row in r0..=r1 {
                let covered = self.bit(row, c0) && (c1 == c0 || self.bit(row, c1));
                if !covered {
                    first_miss = first_miss.min(row);
                    last_miss = last_miss.max(row);
                }
            }
            if first_miss == usize::MAX {
                return CoverResult::All;
            }
            return self.partial_from_axis(a[1], b[1], 1, r0, r1, first_miss, last_miss);
        }
        let (p, q) = if a[0] <= b[0] { (a, b) } else { (b, a) };
        let dx = q[0] - p[0];
        let [ymin, ymax] = [p[1].min(q[1]), p[1].max(q[1])];
        let c0 = col_of(p[0] - eps);
        let c1 = col_of(q[0] + eps);
        let slope = (q[1] - p[1]) / dx;
        let mut first_miss = usize::MAX;
        let mut last_miss = 0usize;
        for col in c0..=c1 {
            let slab_lo = self.origin[0] + col as f64 * self.cell - eps;
            let slab_hi = self.origin[0] + (col + 1) as f64 * self.cell + eps;
            let lo = slab_lo.clamp(p[0], q[0]);
            let hi = slab_hi.clamp(p[0], q[0]);
            let at = |x: f64| (p[1] + (x - p[0]) * slope).clamp(ymin, ymax);
            let (ya, yb) = (at(lo), at(hi));
            let r0 = row_of(ya.min(yb) - eps);
            let r1 = row_of(ya.max(yb) + eps);
            let mut missed = false;
            for row in r0..=r1 {
                if !self.bit(row, col) {
                    missed = true;
                    break;
                }
            }
            if missed {
                first_miss = first_miss.min(col);
                last_miss = last_miss.max(col);
            }
        }
        if first_miss == usize::MAX {
            return CoverResult::All;
        }
        self.partial_from_axis(a[0], b[0], 0, c0, c1, first_miss, last_miss)
    }

    #[allow(clippy::too_many_arguments)]
    fn partial_from_axis(
        &self,
        a_val: f64,
        b_val: f64,
        axis: usize,
        start_cell: usize,
        end_cell: usize,
        first_miss: usize,
        last_miss: usize,
    ) -> CoverResult {
        if first_miss <= start_cell && last_miss >= end_cell {
            return CoverResult::Uncovered;
        }
        let eps = INNER_QUERY_EPS_MM;
        let delta = b_val - a_val;
        if delta == 0.0 {
            return CoverResult::Uncovered;
        }
        let seg_lo = a_val.min(b_val);
        let seg_hi = a_val.max(b_val);
        let coord_lo = if first_miss > start_cell {
            (self.origin[axis] + first_miss as f64 * self.cell - eps).clamp(seg_lo, seg_hi)
        } else {
            seg_lo
        };
        let coord_hi = if last_miss < end_cell {
            (self.origin[axis] + (last_miss + 1) as f64 * self.cell + eps).clamp(seg_lo, seg_hi)
        } else {
            seg_hi
        };
        let (t0, t1) = if delta > 0.0 {
            ((coord_lo - a_val) / delta, (coord_hi - a_val) / delta)
        } else {
            ((coord_hi - a_val) / delta, (coord_lo - a_val) / delta)
        };
        let lo = t0.clamp(0.0, 1.0);
        let hi = t1.clamp(0.0, 1.0);
        if lo >= hi || (lo == 0.0 && hi == 1.0) {
            return CoverResult::Uncovered;
        }
        CoverResult::Partial { lo, hi }
    }
}

#[inline]
fn tangent_cross(from: [f64; 2], to: [f64; 2], y: f64) -> Option<f64> {
    if (from[1] < y && to[1] >= y) || (from[1] > y && to[1] <= y) {
        let fraction = (y - from[1]) / (to[1] - from[1]);
        Some(from[0] + (to[0] - from[0]) * fraction)
    } else if from[1] == y {
        Some(from[0])
    } else {
        None
    }
}

#[derive(Clone, Copy)]
pub(super) struct GridEdge {
    pub(super) c: [f64; 2],
    pub(super) d: [f64; 2],
    pub(super) width: f64,
    pub(super) path: u32,
}

struct FloorCache {
    key: (u64, bool, bool),
    prefix: Vec<usize>,
    inner: Option<InnerCover>,
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
    prefixes: Vec<FloorCache>,
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
            + vec_bytes(&self.prefixes)
            + self
                .prefixes
                .iter()
                .map(|entry| {
                    vec_bytes(&entry.prefix)
                        + entry.inner.as_ref().map_or(0, InnerCover::retained_bytes)
                })
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
                                .any(|x| !x.is_finite() || x.abs() > INDEXED_MAGNITUDE_MM)
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

    /// Prefix of z-eligible inventory edges before each path for one query floor,
    /// along with its lazy conservative inner-cover bitmap.
    pub(super) fn prefix(
        &mut self,
        floor: super::LowerFloor,
        nominal: &[super::NominalPath<'_>],
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        let key = (floor.z.to_bits(), floor.contacts, floor.supplied);
        if let Some(position) = self.prefixes.iter().position(|entry| entry.key == key) {
            return Ok(position);
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
        let inner = InnerCover::build(&self.edges, nominal, floor, &self.whole, cancel)?;
        if self.prefixes.len() == 8 {
            self.prefixes.remove(0);
        }
        self.prefixes.push(FloorCache { key, prefix, inner });
        Ok(self.prefixes.len() - 1)
    }

    pub(super) fn prefix_at(&self, position: usize) -> &[usize] {
        &self.prefixes[position].prefix
    }

    pub(super) fn inner_at(&self, position: usize) -> Option<&InnerCover> {
        self.prefixes[position].inner.as_ref()
    }

    /// Ascending global edges whose unchanged per-edge broad box admits the
    /// candidate, restricted to z-eligible indexed paths.
    pub(super) fn collect(
        &mut self,
        candidate: AnchorCandidate,
        floor: super::LowerFloor,
        nominal: &[super::NominalPath<'_>],
    ) {
        self.collect_span(candidate, None, floor, nominal);
    }

    /// Like [`Self::collect`], optionally restricted to an uncovered sub-segment
    /// `span = Some((p_lo, p_hi))` of `candidate`.
    pub(super) fn collect_span(
        &mut self,
        candidate: AnchorCandidate,
        span: Option<([f64; 2], [f64; 2])>,
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
        let (a2, b2) = span.unwrap_or(([a[0], a[1]], [b[0], b[1]]));
        let radius = candidate.bead_width_mm * 0.5;
        if self.entries.is_empty() {
            return;
        }
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
                if a2[0].min(b2[0]) > c[0].max(d[0]) + endpoint_reach
                    || a2[0].max(b2[0]) < c[0].min(d[0]) - endpoint_reach
                    || a2[1].min(b2[1]) > c[1].max(d[1]) + endpoint_reach
                    || a2[1].max(b2[1]) < c[1].min(d[1]) - endpoint_reach
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
