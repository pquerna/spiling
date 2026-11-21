// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Conservative integer grid for support area questions.
//!
//! Support asks set questions (where can a column stand, what does occupancy
//! exclude) that exact polygon Booleans answer slowly. Here every region is a
//! set of square cells in one fixed frame, stored as rows of `u64` words over
//! a tight row/word window. Every conversion rounds to a stated side:
//!
//! - [`Raster::outward`] marks every cell the region (or its dilation by an
//!   exact distance) touches: material, occupancy and voids;
//! - [`Raster::inward`] keeps only cells entirely inside: support space;
//! - [`Raster::contour`] returns polygons inside the cell set.
//!
//! Cells are closed squares. "Touches" includes boundary contact, so the
//! outward set is a superset of the exact cover and the inward set a subset.

use crate::{check_cancel, error};
use geo::{LineString, MultiPolygon, Polygon};
use spiling_contracts::manufacturing::{ManufacturingError, ManufacturingErrorCode};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Support grid cell edge in millimetres: the recorded ADR012 support
/// tolerance. Support domain, clearance and landing answers are computed on
/// cells of this size and always rounded to the safe side, so a support
/// boundary may sit up to a few cells inside the exact answer, never outside.
pub(super) const SUPPORT_CELL_MM: f64 = spiling_contracts::manufacturing::SUPPORT_GRID_CELL_MM;

/// Absolute guard against floating rounding in raster arithmetic. Outward
/// rasters widen by it and inward rasters lose it; it is far below a cell.
const GUARD_MM: f64 = 1.0e-7;

/// Cells of margin kept around the requested bounds.
const MARGIN_CELLS: f64 = 2.0;

/// Inward clip of every grid run end, and the distance within which a line
/// counts as lying on a cell boundary. It covers XYZ6 projection, so a
/// projected centreline point stays inside the closed cells it came from.
const RUN_CLIP_MM: f64 =
    2.0 * spiling_contracts::manufacturing::program_coordinates::XYZ6_QUANTUM_MM;

/// Fixed cell frame: cell `(col, row)` covers
/// `[ox + col·c, ox + (col + 1)·c] × [oy + row·c, oy + (row + 1)·c]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CellFrame {
    origin: [f64; 2],
    cell: f64,
    cols: usize,
    rows: usize,
}

impl CellFrame {
    /// A frame over `bounds` plus a small margin, aligned to whole cells.
    pub(super) fn new(bounds: [[f64; 2]; 2], cell: f64) -> Result<Self, ManufacturingError> {
        let invalid = || {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support grid frame exceeds its integer cell range",
            )
        };
        if !(cell.is_finite() && cell > 0.0)
            || !bounds.iter().flatten().all(|v| v.is_finite())
            || bounds[0][0] > bounds[1][0]
            || bounds[0][1] > bounds[1][1]
        {
            return Err(error(
                ManufacturingErrorCode::NativeConditioningFailure,
                "support grid frame has non-finite or empty bounds",
            ));
        }
        let origin = [0, 1].map(|axis| ((bounds[0][axis] / cell).floor() - MARGIN_CELLS) * cell);
        let count = |axis: usize| -> Result<usize, ManufacturingError> {
            let span = ((bounds[1][axis] - origin[axis]) / cell).ceil() + MARGIN_CELLS;
            // Corner keys pack (col, row) into 32 bits each.
            if !(span.is_finite() && span >= 1.0 && span < f64::from(i32::MAX) - 2.0) {
                return Err(invalid());
            }
            Ok(span as usize)
        };
        Ok(Self {
            origin,
            cell,
            cols: count(0)?,
            rows: count(1)?,
        })
    }

    pub(super) fn bounds(&self) -> [[f64; 2]; 2] {
        [
            self.origin,
            [
                self.origin[0] + self.cols as f64 * self.cell,
                self.origin[1] + self.rows as f64 * self.cell,
            ],
        ]
    }

    /// Bytes of one dense layer over the whole frame.
    pub(super) fn dense_bytes(&self) -> usize {
        self.cols
            .div_ceil(64)
            .saturating_mul(self.rows)
            .saturating_mul(8)
    }

    fn x(&self, col: f64) -> f64 {
        self.origin[0] + col * self.cell
    }

    fn y(&self, row: f64) -> f64 {
        self.origin[1] + row * self.cell
    }

    /// Inclusive cell range whose closed cells meet `[lo, hi]` on one axis.
    fn touching(&self, axis: usize, lo: f64, hi: f64) -> Option<(usize, usize)> {
        let limit = if axis == 0 { self.cols } else { self.rows };
        let first = ((lo - GUARD_MM - self.origin[axis]) / self.cell).floor();
        let last = ((hi + GUARD_MM - self.origin[axis]) / self.cell).floor();
        if !(first.is_finite() && last.is_finite()) || last < 0.0 || first >= limit as f64 {
            return None;
        }
        Some((first.max(0.0) as usize, (last as usize).min(limit - 1)))
    }
}

/// A cell set over a tight `[row0, row0 + rows) × [word0, word0 + width)`
/// window. The empty set has no window, so equal sets compare equal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Bits {
    row0: usize,
    word0: usize,
    width: usize,
    data: Vec<u64>,
}

impl Bits {
    pub(super) fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub(super) fn bytes(&self) -> usize {
        self.data.capacity().saturating_mul(8)
    }

    fn rows(&self) -> usize {
        self.data.len().checked_div(self.width).unwrap_or(0)
    }

    fn row(&self, row: usize) -> Option<&[u64]> {
        let local = row.checked_sub(self.row0)?;
        (local < self.rows()).then(|| &self.data[local * self.width..(local + 1) * self.width])
    }

    /// Word `word` (frame index) of `row`, zero outside the window.
    fn word(&self, row: usize, word: usize) -> u64 {
        match (self.row(row), word.checked_sub(self.word0)) {
            (Some(bits), Some(local)) if local < self.width => bits[local],
            _ => 0,
        }
    }

    #[cfg(test)]
    pub(super) fn contains(&self, col: usize, row: usize) -> bool {
        self.word(row, col / 64) >> (col % 64) & 1 == 1
    }

    /// Millimetre extent of the window on `axis` (0: x, 1: y), or `None`
    /// when empty.
    pub(super) fn extent(&self, frame: &CellFrame, axis: usize) -> Option<[f64; 2]> {
        if self.is_empty() {
            return None;
        }
        Some(if axis == 0 {
            [
                frame.x((self.word0 * 64) as f64),
                frame.x(((self.word0 + self.width) * 64) as f64),
            ]
        } else {
            [
                frame.y(self.row0 as f64),
                frame.y((self.row0 + self.rows()) as f64),
            ]
        })
    }

    /// Number of set cells.
    #[cfg(test)]
    pub(super) fn count(&self) -> usize {
        self.data.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Remove every cell of `other`.
    pub(super) fn subtract(&mut self, other: &Self) {
        if self.is_empty() || other.is_empty() {
            return;
        }
        let rows =
            self.row0.max(other.row0)..(self.row0 + self.rows()).min(other.row0 + other.rows());
        let words =
            self.word0.max(other.word0)..(self.word0 + self.width).min(other.word0 + other.width);
        if rows.is_empty() || words.is_empty() {
            return;
        }
        for row in rows {
            let from = (row - self.row0) * self.width;
            let theirs = (row - other.row0) * other.width;
            for word in words.clone() {
                self.data[from + word - self.word0] &= !other.data[theirs + word - other.word0];
            }
        }
        self.shrink();
    }

    /// Whether the two sets share a cell.
    pub(super) fn intersects(&self, other: &Self) -> bool {
        let rows =
            self.row0.max(other.row0)..(self.row0 + self.rows()).min(other.row0 + other.rows());
        let words =
            self.word0.max(other.word0)..(self.word0 + self.width).min(other.word0 + other.width);
        rows.into_iter().any(|row| {
            words
                .clone()
                .any(|word| self.word(row, word) & other.word(row, word) != 0)
        })
    }

    /// Whether any cell meets the closed millimetre rectangle.
    pub(super) fn meets_rect(&self, frame: &CellFrame, rect: [[f64; 2]; 2]) -> bool {
        if self.is_empty() {
            return false;
        }
        let (Some((c0, c1)), Some((r0, r1))) = (
            frame.touching(0, rect[0][0], rect[1][0]),
            frame.touching(1, rect[0][1], rect[1][1]),
        ) else {
            return false;
        };
        let rows = r0.max(self.row0)..=r1.min(self.row0 + self.rows() - 1);
        let (w0, w1) = (c0 / 64, c1 / 64);
        for row in rows {
            for word in w0.max(self.word0)..=w1.min(self.word0 + self.width - 1) {
                let mut mask = !0u64;
                if word == w0 {
                    mask &= !0u64 << (c0 % 64);
                }
                if word == w1 {
                    mask &= !0u64 >> (63 - c1 % 64);
                }
                if self.word(row, word) & mask != 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Restore the tight window after cells were removed.
    fn shrink(&mut self) {
        let rows = self.rows();
        let width = self.width;
        let live_row = |r: usize| {
            self.data[r * width..(r + 1) * width]
                .iter()
                .any(|w| *w != 0)
        };
        let Some(first) = (0..rows).find(|&r| live_row(r)) else {
            *self = Self::default();
            return;
        };
        let last = (first..rows).rev().find(|&r| live_row(r)).unwrap_or(first);
        let column = |w: usize| (first..=last).any(|r| self.data[r * width + w] != 0);
        let left = (0..width).find(|&w| column(w)).unwrap_or(0);
        let right = (left..width).rev().find(|&w| column(w)).unwrap_or(left);
        let new_width = right - left + 1;
        if first == 0 && last + 1 == rows && new_width == width {
            return;
        }
        let mut at = 0;
        for r in first..=last {
            for w in left..=right {
                self.data[at] = self.data[r * width + w];
                at += 1;
            }
        }
        self.data.truncate(at);
        self.data.shrink_to_fit();
        self.row0 += first;
        self.word0 += left;
        self.width = new_width;
    }
}

/// Which coarse blocks of `BLOCK_CELLS × 64` cells hold any cell of a set:
/// a screen only, never an answer. A rectangle meeting no marked block
/// meets no set cell.
pub(super) struct Coarse {
    row0: usize,
    word0: usize,
    rows: usize,
    words: usize,
    marked: Vec<bool>,
}

const BLOCK_CELLS: usize = 16;

impl Coarse {
    /// Whether the closed millimetre rectangle can meet a set cell.
    pub(super) fn meets(&self, frame: &CellFrame, rect: [[f64; 2]; 2]) -> bool {
        if self.marked.is_empty() {
            return false;
        }
        let (Some((c0, c1)), Some((r0, r1))) = (
            frame.touching(0, rect[0][0], rect[1][0]),
            frame.touching(1, rect[0][1], rect[1][1]),
        ) else {
            return false;
        };
        let rows =
            (r0 / BLOCK_CELLS).max(self.row0)..=(r1 / BLOCK_CELLS).min(self.row0 + self.rows - 1);
        let words = (c0 / 64).max(self.word0)..=(c1 / 64).min(self.word0 + self.words - 1);
        rows.into_iter().any(|row| {
            words
                .clone()
                .any(|word| self.marked[(row - self.row0) * self.words + word - self.word0])
        })
    }
}

/// Cells of a closed capsule that lie outside a kept set: how many and the
/// closed millimetre box around them (widened by the raster guard). Cells
/// beyond the frame are never kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Escape {
    pub(super) cells: usize,
    pub(super) bounds: [[f64; 2]; 2],
}

impl CellFrame {
    /// Unclamped inclusive cell indexes whose closed cells meet `[lo, hi]`.
    fn cell_span(&self, axis: usize, lo: f64, hi: f64) -> Option<(i64, i64)> {
        let first = ((lo - GUARD_MM - self.origin[axis]) / self.cell).floor();
        let last = ((hi + GUARD_MM - self.origin[axis]) / self.cell).floor();
        let limit = f64::from(i32::MAX);
        (first.is_finite() && last.is_finite() && first.abs() < limit && last.abs() < limit)
            .then_some((first as i64, last as i64))
    }

    /// Every cell meeting the closed capsule of `ab` and `radius` that is not
    /// in `kept`, rasterized outward exactly as [`Raster::capsules`] does but
    /// without clipping to the frame. Returns the escape, if any, and the
    /// rows and words inspected.
    pub(super) fn escaped(
        &self,
        kept: &Bits,
        a: [f64; 2],
        b: [f64; 2],
        radius: f64,
    ) -> Result<(Option<Escape>, usize), ManufacturingError> {
        let invalid = || {
            error(
                ManufacturingErrorCode::NativeConditioningFailure,
                "support grid capsule has non-finite coordinates",
            )
        };
        if !(a.iter().chain(&b).all(|v| v.is_finite()) && radius.is_finite() && radius >= 0.0) {
            return Err(invalid());
        }
        let (r0, r1) = self
            .cell_span(1, a[1].min(b[1]) - radius, a[1].max(b[1]) + radius)
            .ok_or_else(invalid)?;
        let (cols, rows) = (self.cols as i64, self.rows as i64);
        let mut cells = 0usize;
        let mut work = 0usize;
        let mut lo = [i64::MAX; 2];
        let mut hi = [i64::MIN; 2];
        let mut mark = |count: usize, from: i64, to: i64, row: i64| {
            cells = cells.saturating_add(count);
            lo = [lo[0].min(from), lo[1].min(row)];
            hi = [hi[0].max(to), hi[1].max(row)];
        };
        for row in r0..=r1 {
            work = work.saturating_add(1);
            let band = [
                self.y(row as f64) - GUARD_MM,
                self.y(row as f64 + 1.0) + GUARD_MM,
            ];
            let Some((x0, x1)) = capsule_band(a, b, radius, band) else {
                continue;
            };
            let (c0, c1) = self.cell_span(0, x0, x1).ok_or_else(invalid)?;
            if row < 0 || row >= rows {
                mark((c1 - c0 + 1) as usize, c0, c1, row);
                continue;
            }
            if c0 < 0 {
                let to = c1.min(-1);
                mark((to - c0 + 1) as usize, c0, to, row);
            }
            if c1 >= cols {
                let from = c0.max(cols);
                mark((c1 - from + 1) as usize, from, c1, row);
            }
            let (from, to) = (c0.max(0), c1.min(cols - 1));
            if from > to {
                continue;
            }
            let (from, to) = (from as usize, to as usize);
            for word in from / 64..=to / 64 {
                work = work.saturating_add(1);
                let mut mask = !0u64;
                if word == from / 64 {
                    mask &= !0u64 << (from % 64);
                }
                if word == to / 64 {
                    mask &= !0u64 >> (63 - to % 64);
                }
                let missing = mask & !kept.word(row as usize, word);
                if missing != 0 {
                    let first = (word * 64) as i64 + i64::from(missing.trailing_zeros());
                    let last = (word * 64 + 63) as i64 - i64::from(missing.leading_zeros());
                    mark(missing.count_ones() as usize, first, last, row);
                }
            }
        }
        let escape = (cells > 0).then(|| Escape {
            cells,
            bounds: [
                [
                    (self.x(lo[0] as f64) - GUARD_MM).next_down(),
                    (self.y(lo[1] as f64) - GUARD_MM).next_down(),
                ],
                [
                    (self.x(hi[0] as f64 + 1.0) + GUARD_MM).next_up(),
                    (self.y(hi[1] as f64 + 1.0) + GUARD_MM).next_up(),
                ],
            ],
        });
        Ok((escape, work))
    }
}

/// One horizontal run of cells, `to` exclusive.
#[derive(Clone, Copy)]
struct Span {
    row: u32,
    from: u32,
    to: u32,
}

/// Rasterizer and set algebra in one frame, with shared byte admission,
/// work counting and cancellation.
pub(super) struct Raster<'a> {
    pub(super) frame: CellFrame,
    pub(super) cancel: &'a AtomicBool,
    /// Admits `bytes` of additional live grid allocation or refuses.
    pub(super) admit: &'a (dyn Fn(usize) -> Result<(), ManufacturingError> + Sync),
    pub(super) work: AtomicUsize,
}

impl Raster<'_> {
    pub(super) fn charge(&self, amount: usize) {
        self.work.fetch_add(amount, Ordering::Relaxed);
    }

    /// Work counted since the last call.
    pub(super) fn take_work(&self) -> usize {
        self.work.swap(0, Ordering::Relaxed)
    }

    fn reserve<T>(&self, values: &mut Vec<T>, additional: usize) -> Result<(), ManufacturingError> {
        let needed = values.len().saturating_add(additional);
        if needed <= values.capacity() {
            return Ok(());
        }
        let capacity = needed.max(values.capacity().saturating_mul(2)).max(16);
        (self.admit)(capacity.saturating_mul(std::mem::size_of::<T>()))?;
        values
            .try_reserve_exact(capacity - values.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "support grid allocation refused",
                )
            })
    }

    fn zeroed(&self, words: usize) -> Result<Vec<u64>, ManufacturingError> {
        let mut data = Vec::new();
        self.reserve(&mut data, words)?;
        data.resize(words, 0);
        self.charge(words);
        Ok(data)
    }

    pub(super) fn union(&self, a: &Bits, b: &Bits) -> Result<Bits, ManufacturingError> {
        if a.is_empty() {
            return Ok(b.clone());
        }
        if b.is_empty() {
            return Ok(a.clone());
        }
        let row0 = a.row0.min(b.row0);
        let row1 = (a.row0 + a.rows()).max(b.row0 + b.rows());
        let word0 = a.word0.min(b.word0);
        let width = (a.word0 + a.width).max(b.word0 + b.width) - word0;
        let mut data = self.zeroed((row1 - row0) * width)?;
        for source in [a, b] {
            for r in 0..source.rows() {
                let to = (source.row0 + r - row0) * width + source.word0 - word0;
                for (dst, src) in data[to..to + source.width]
                    .iter_mut()
                    .zip(&source.data[r * source.width..(r + 1) * source.width])
                {
                    *dst |= src;
                }
            }
        }
        Ok(Bits {
            row0,
            word0,
            width,
            data,
        })
    }

    pub(super) fn intersection(&self, a: &Bits, b: &Bits) -> Result<Bits, ManufacturingError> {
        let row0 = a.row0.max(b.row0);
        let row1 = (a.row0 + a.rows()).min(b.row0 + b.rows());
        let word0 = a.word0.max(b.word0);
        let word1 = (a.word0 + a.width).min(b.word0 + b.width);
        if a.is_empty() || b.is_empty() || row0 >= row1 || word0 >= word1 {
            return Ok(Bits::default());
        }
        let width = word1 - word0;
        let mut data = self.zeroed((row1 - row0) * width)?;
        for row in row0..row1 {
            for word in word0..word1 {
                data[(row - row0) * width + word - word0] = a.word(row, word) & b.word(row, word);
            }
        }
        let mut bits = Bits {
            row0,
            word0,
            width,
            data,
        };
        bits.shrink();
        Ok(bits)
    }

    /// Cells of `a` not in `b`.
    pub(super) fn difference(&self, a: &Bits, b: &Bits) -> Result<Bits, ManufacturingError> {
        let mut data = self.zeroed(a.data.len())?;
        data.copy_from_slice(&a.data);
        let mut bits = Bits { data, ..*a };
        bits.subtract(b);
        Ok(bits)
    }

    fn bits_of(&self, spans: &[Span]) -> Result<Bits, ManufacturingError> {
        let Some(first) = spans.first() else {
            return Ok(Bits::default());
        };
        let (mut row0, mut row1, mut col0, mut col1) = (first.row, first.row, first.from, first.to);
        for span in spans {
            row0 = row0.min(span.row);
            row1 = row1.max(span.row);
            col0 = col0.min(span.from);
            col1 = col1.max(span.to);
        }
        let (row0, rows) = (row0 as usize, (row1 - row0) as usize + 1);
        let word0 = col0 as usize / 64;
        let width = (col1 as usize - 1) / 64 + 1 - word0;
        let mut data = self.zeroed(rows * width)?;
        for span in spans {
            let row = &mut data[(span.row as usize - row0) * width..][..width];
            set_range(
                row,
                span.from as usize - word0 * 64,
                span.to as usize - word0 * 64,
            );
        }
        self.charge(spans.len());
        let mut bits = Bits {
            row0,
            word0,
            width,
            data,
        };
        bits.shrink();
        Ok(bits)
    }

    /// Every cell touching any polygon or within `distance` of it.
    pub(super) fn outward<'p>(
        &self,
        polygons: impl IntoIterator<Item = &'p Polygon<f64>>,
        distance: f64,
    ) -> Result<Bits, ManufacturingError> {
        self.dilated(polygons.into_iter().map(|polygon| (polygon, distance)))
    }

    /// [`Self::outward`] with a distance of its own for each polygon.
    pub(super) fn dilated<'p>(
        &self,
        polygons: impl IntoIterator<Item = (&'p Polygon<f64>, f64)>,
    ) -> Result<Bits, ManufacturingError> {
        let mut spans = Vec::new();
        let mut crossings = Vec::new();
        for (polygon, distance) in polygons {
            check_cancel(self.cancel)?;
            self.interior(polygon, &mut crossings, &mut spans)?;
            for [a, b] in edges(polygon) {
                self.capsule(a, b, distance, &mut spans)?;
            }
        }
        self.bits_of(&spans)
    }

    /// Every cell within `radius` of a segment: union of closed capsules.
    pub(super) fn capsules(
        &self,
        segments: impl IntoIterator<Item = ([f64; 2], [f64; 2], f64)>,
    ) -> Result<Bits, ManufacturingError> {
        let mut spans = Vec::new();
        for (a, b, radius) in segments {
            check_cancel(self.cancel)?;
            self.capsule(a, b, radius, &mut spans)?;
        }
        self.bits_of(&spans)
    }

    /// Cells entirely inside the polygons' union is not attempted: each kept
    /// cell lies entirely inside one polygon.
    pub(super) fn inward<'p>(
        &self,
        polygons: impl IntoIterator<Item = &'p Polygon<f64>>,
    ) -> Result<Bits, ManufacturingError> {
        let mut inside = Vec::new();
        let mut boundary = Vec::new();
        let mut crossings = Vec::new();
        for polygon in polygons {
            check_cancel(self.cancel)?;
            self.interior(polygon, &mut crossings, &mut inside)?;
            for [a, b] in edges(polygon) {
                self.capsule(a, b, 0.0, &mut boundary)?;
            }
        }
        let mut bits = self.bits_of(&inside)?;
        drop(inside);
        bits.subtract(&self.bits_of(&boundary)?);
        Ok(bits)
    }

    /// Cells whose centre lies inside one polygon (even-odd over its own
    /// exterior and holes), by midline scanline crossings.
    fn interior(
        &self,
        polygon: &Polygon<f64>,
        crossings: &mut Vec<(u32, f64)>,
        spans: &mut Vec<Span>,
    ) -> Result<(), ManufacturingError> {
        let frame = &self.frame;
        crossings.clear();
        for [a, b] in edges(polygon) {
            if a[1] == b[1] {
                continue;
            }
            let (lo, hi) = (a[1].min(b[1]), a[1].max(b[1]));
            // Midlines y = oy + (row + ½)c with lo <= y < hi; one spare row
            // each side absorbs rounding, the exact test below decides.
            let first = ((lo - frame.origin[1]) / frame.cell - 1.5).ceil().max(0.0);
            let last = ((hi - frame.origin[1]) / frame.cell + 0.5)
                .ceil()
                .min(frame.rows as f64);
            if !(first.is_finite() && last.is_finite()) {
                return Err(error(
                    ManufacturingErrorCode::NativeConditioningFailure,
                    "support grid polygon has non-finite coordinates",
                ));
            }
            let (first, last) = (first as usize, last.max(0.0) as usize);
            if first >= last {
                continue;
            }
            self.reserve(crossings, last - first)?;
            self.charge(last - first);
            for row in first..last {
                let y = frame.y(row as f64 + 0.5);
                if (a[1] <= y) == (b[1] <= y) {
                    continue;
                }
                let x = a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                crossings.push((row as u32, x));
            }
        }
        crossings.sort_unstable_by(|p, q| p.0.cmp(&q.0).then(p.1.total_cmp(&q.1)));
        let centre = |x: f64| ((x - frame.origin[0]) / frame.cell - 0.5).ceil();
        let (pairs, odd) = crossings.as_chunks::<2>();
        for &[(row, from), (other, to)] in pairs {
            if row != other || !odd.is_empty() {
                return Err(error(
                    ManufacturingErrorCode::NativeConditioningFailure,
                    "support grid polygon ring is not closed",
                ));
            }
            let from = centre(from).clamp(0.0, frame.cols as f64) as u32;
            let to = centre(to).clamp(0.0, frame.cols as f64) as u32;
            if from < to {
                self.reserve(spans, 1)?;
                spans.push(Span { row, from, to });
            }
        }
        Ok(())
    }

    /// Every cell meeting the closed capsule of segment `ab` and `radius`.
    fn capsule(
        &self,
        a: [f64; 2],
        b: [f64; 2],
        radius: f64,
        spans: &mut Vec<Span>,
    ) -> Result<(), ManufacturingError> {
        let frame = &self.frame;
        if !(a.iter().chain(&b).all(|v| v.is_finite()) && radius.is_finite() && radius >= 0.0) {
            return Err(error(
                ManufacturingErrorCode::NativeConditioningFailure,
                "support grid capsule has non-finite coordinates",
            ));
        }
        let Some((r0, r1)) = frame.touching(1, a[1].min(b[1]) - radius, a[1].max(b[1]) + radius)
        else {
            return Ok(());
        };
        self.reserve(spans, r1 - r0 + 1)?;
        self.charge(r1 - r0 + 1);
        for row in r0..=r1 {
            let band = [
                frame.y(row as f64) - GUARD_MM,
                frame.y(row as f64 + 1.0) + GUARD_MM,
            ];
            let Some((lo, hi)) = capsule_band(a, b, radius, band) else {
                continue;
            };
            if let Some((c0, c1)) = frame.touching(0, lo, hi) {
                spans.push(Span {
                    row: row as u32,
                    from: c0 as u32,
                    to: c1 as u32 + 1,
                });
            }
        }
        Ok(())
    }

    /// The cell lines (rows when `fixed` is 1, columns when 0) holding the
    /// line at `coordinate`: one, or both neighbours when it lies within
    /// [`RUN_CLIP_MM`] of their shared boundary, so that a projected point
    /// on it stays inside whichever it lands in.
    fn lines(&self, fixed: usize, coordinate: f64) -> Option<[usize; 2]> {
        let frame = &self.frame;
        let limit = if fixed == 0 { frame.cols } else { frame.rows };
        let t = (coordinate - frame.origin[fixed]) / frame.cell;
        if !t.is_finite() {
            return None;
        }
        let nearest = t.round();
        let boundary = frame.origin[fixed] + nearest * frame.cell;
        let [lo, hi] = if (coordinate - boundary).abs() <= RUN_CLIP_MM {
            [nearest - 1.0, nearest]
        } else {
            [t.floor(), t.floor()]
        };
        (lo >= 0.0 && hi < limit as f64).then_some([lo as usize, hi as usize])
    }

    /// Runs of set cells along the line at `coordinate` on the `fixed`
    /// axis (1: constant y, runs along x; 0: constant x, runs along y), as
    /// millimetre `[start, end]` pairs pushed onto `hits` in increasing
    /// order. Run ends are cell edges clipped inward by [`RUN_CLIP_MM`].
    pub(super) fn line_runs(
        &self,
        bits: &Bits,
        fixed: usize,
        coordinate: f64,
        hits: &mut Vec<f64>,
    ) -> Result<(), ManufacturingError> {
        if bits.is_empty() {
            return Ok(());
        }
        let Some([first, second]) = self.lines(fixed, coordinate) else {
            return Ok(());
        };
        let frame = &self.frame;
        let mut push = |from: usize, to: usize| -> Result<(), ManufacturingError> {
            let (from, to) = if fixed == 1 {
                (frame.x(from as f64), frame.x(to as f64))
            } else {
                (frame.y(from as f64), frame.y(to as f64))
            };
            self.reserve(hits, 2)?;
            hits.push(from + RUN_CLIP_MM);
            hits.push(to - RUN_CLIP_MM);
            Ok(())
        };
        let mut start = None;
        if fixed == 1 {
            self.charge(bits.width);
            for w in bits.word0..bits.word0 + bits.width {
                let word = bits.word(first, w) & bits.word(second, w);
                let base = w * 64;
                let mut pos = 0u32;
                while pos < 64 {
                    let rest = word >> pos;
                    match start {
                        None => {
                            if rest == 0 {
                                break;
                            }
                            pos += rest.trailing_zeros();
                            start = Some(base + pos as usize);
                        }
                        Some(from) => {
                            let ones = rest.trailing_ones();
                            if pos + ones >= 64 {
                                break;
                            }
                            pos += ones;
                            push(from, base + pos as usize)?;
                            start = None;
                        }
                    }
                }
            }
            if let Some(from) = start {
                push(from, (bits.word0 + bits.width) * 64)?;
            }
        } else {
            self.charge(bits.rows());
            let set = |row: usize, col: usize| bits.word(row, col / 64) >> (col % 64) & 1 == 1;
            for row in bits.row0..bits.row0 + bits.rows() {
                match (start, set(row, first) && set(row, second)) {
                    (None, true) => start = Some(row),
                    (Some(from), false) => {
                        push(from, row)?;
                        start = None;
                    }
                    _ => {}
                }
            }
            if let Some(from) = start {
                push(from, bits.row0 + bits.rows())?;
            }
        }
        Ok(())
    }

    /// Membership of `bits` along segment `ab` as consecutive pieces
    /// `(end parameter, inside)`, equal neighbours merged, the last ending
    /// at 1. Pieces split at cell boundaries; each is judged by the cells
    /// holding its midpoint that lie in `within`.
    pub(super) fn along(
        &self,
        bits: &Bits,
        within: &Bits,
        a: [f64; 2],
        b: [f64; 2],
        pieces: &mut Vec<(f64, bool)>,
    ) -> Result<(), ManufacturingError> {
        pieces.clear();
        let frame = &self.frame;
        for axis in 0..2 {
            let (p, q) = (a[axis], b[axis]);
            if p == q {
                continue;
            }
            let first = ((p.min(q) - frame.origin[axis]) / frame.cell).ceil();
            let last = ((p.max(q) - frame.origin[axis]) / frame.cell).floor();
            if !(first.is_finite() && last.is_finite()) {
                return Err(error(
                    ManufacturingErrorCode::NativeConditioningFailure,
                    "support grid segment has non-finite coordinates",
                ));
            }
            if last < first {
                continue;
            }
            let count = (last - first) as usize + 1;
            self.reserve(pieces, count)?;
            self.charge(count);
            for m in 0..count {
                let line = frame.origin[axis] + (first + m as f64) * frame.cell;
                let t = (line - p) / (q - p);
                if t > 0.0 && t < 1.0 {
                    pieces.push((t, false));
                }
            }
        }
        pieces.sort_unstable_by(|l, r| l.0.total_cmp(&r.0));
        pieces.dedup_by(|l, r| l.0 == r.0);
        self.reserve(pieces, 1)?;
        pieces.push((1.0, false));
        let mut previous = 0.0;
        let mut out = 0;
        for k in 0..pieces.len() {
            let t = pieces[k].0;
            let mid = (previous + t) * 0.5;
            let point = [a[0] + (b[0] - a[0]) * mid, a[1] + (b[1] - a[1]) * mid];
            // A piece on a cell line belongs to the cells on both sides;
            // judge it by those inside `within`, the set it runs along.
            let [cols, rows] = [0, 1].map(|axis| {
                let u = (point[axis] - frame.origin[axis]) / frame.cell;
                let near = u.round();
                if (u - near).abs() <= GUARD_MM / frame.cell {
                    [near - 1.0, near]
                } else {
                    [u.floor(), u.floor()]
                }
            });
            let set = |bits: &Bits, col: f64, row: f64| {
                col >= 0.0
                    && row >= 0.0
                    && bits.word(row as usize, col as usize / 64) >> (col as usize % 64) & 1 == 1
            };
            let cells = cols
                .iter()
                .flat_map(|&col| rows.iter().map(move |&row| (col, row)));
            let owned = cells.clone().any(|(col, row)| set(within, col, row));
            let inside = cells
                .filter(|&(col, row)| !owned || set(within, col, row))
                .any(|(col, row)| set(bits, col, row));
            if out > 0 && pieces[out - 1].1 == inside {
                pieces[out - 1].0 = t;
            } else {
                pieces[out] = (t, inside);
                out += 1;
            }
            previous = t;
        }
        pieces.truncate(out);
        Ok(())
    }

    /// Cells whose closed distance to every cell outside the set exceeds
    /// `radius`: the complement of the complement's dilation. The closed
    /// distance disk is a union of horizontal runs per row offset, so the
    /// erosion is the intersection of row-shifted horizontal erosions.
    pub(super) fn erode(&self, bits: &Bits, radius: f64) -> Result<Bits, ManufacturingError> {
        if bits.is_empty() {
            return Ok(Bits::default());
        }
        let reach = structuring(radius / self.frame.cell);
        let (width, rows) = (bits.width, bits.rows());
        // Horizontal erosion of every row at each distinct extent.
        let mut extents: Vec<usize> = reach.clone();
        extents.dedup();
        let mut horizontal: Vec<Vec<u64>> = Vec::new();
        for &extent in &extents {
            check_cancel(self.cancel)?;
            let mut eroded = self.zeroed(bits.data.len())?;
            for r in 0..rows {
                let source = &bits.data[r * width..(r + 1) * width];
                for (w, slot) in eroded[r * width..(r + 1) * width].iter_mut().enumerate() {
                    let mut word = !0u64;
                    for shift in -(extent as i64)..=extent as i64 {
                        word &= shifted(source, w, shift);
                    }
                    *slot = word;
                }
            }
            self.charge(bits.data.len() * (2 * extent + 1));
            horizontal.push(eroded);
        }
        let mut data = self.zeroed(bits.data.len())?;
        for r in 0..rows {
            check_cancel(self.cancel)?;
            let out = &mut data[r * width..(r + 1) * width];
            out.iter_mut().for_each(|w| *w = !0);
            for (k, extent) in reach.iter().enumerate() {
                let eroded = &horizontal[extents.iter().position(|e| e == extent).unwrap_or(0)];
                for neighbour in [r.checked_sub(k), (k > 0).then_some(r + k)] {
                    let Some(n) = neighbour.filter(|&n| n < rows) else {
                        if neighbour.is_some() || k > 0 {
                            out.iter_mut().for_each(|w| *w = 0);
                        }
                        continue;
                    };
                    for (o, s) in out.iter_mut().zip(&eroded[n * width..(n + 1) * width]) {
                        *o &= s;
                    }
                }
            }
            self.charge(width * (2 * reach.len()));
        }
        drop(horizontal);
        let mut result = Bits {
            row0: bits.row0,
            word0: bits.word0,
            width,
            data,
        };
        result.shrink();
        Ok(result)
    }

    /// Whether the point lies in a closed set cell. A point within
    /// [`RUN_CLIP_MM`] of a cell line belongs to every cell it lies on, so a
    /// projected point stays with the closed cells it came from.
    pub(super) fn holds_point(&self, bits: &Bits, point: [f64; 2]) -> bool {
        let [cols, rows] = [0, 1].map(|axis| {
            let u = (point[axis] - self.frame.origin[axis]) / self.frame.cell;
            let near = u.round();
            if (u - near).abs() <= RUN_CLIP_MM / self.frame.cell {
                [near - 1.0, near]
            } else {
                [u.floor(), u.floor()]
            }
        });
        cols.iter().any(|&col| {
            rows.iter().any(|&row| {
                col.is_finite()
                    && row.is_finite()
                    && col >= 0.0
                    && row >= 0.0
                    && bits.word(row as usize, col as usize / 64) >> (col as usize % 64) & 1 == 1
            })
        })
    }

    /// The coarse block screen of `bits`.
    pub(super) fn coarse(&self, bits: &Bits) -> Result<Coarse, ManufacturingError> {
        if bits.is_empty() {
            return Ok(Coarse {
                row0: 0,
                word0: 0,
                rows: 0,
                words: 0,
                marked: Vec::new(),
            });
        }
        let row0 = bits.row0 / BLOCK_CELLS;
        let rows = (bits.row0 + bits.rows() - 1) / BLOCK_CELLS + 1 - row0;
        let words = bits.width;
        let mut marked = Vec::new();
        self.reserve(&mut marked, rows * words)?;
        marked.resize(rows * words, false);
        for r in 0..bits.rows() {
            let block = (bits.row0 + r) / BLOCK_CELLS - row0;
            for w in 0..words {
                if bits.data[r * words + w] != 0 {
                    marked[block * words + w] = true;
                }
            }
        }
        self.charge(bits.data.len());
        Ok(Coarse {
            row0,
            word0: bits.word0,
            rows,
            words,
            marked,
        })
    }

    /// Polygons inside the cell set with few vertices: erode by one cell,
    /// trace cell-edge boundaries, Douglas–Peucker at one cell, and resolve
    /// any simplification crossing by even-odd overlay. Every output point is
    /// within one cell of the eroded set, hence inside the original set.
    pub(super) fn contour(&self, bits: &Bits) -> Result<MultiPolygon<f64>, ManufacturingError> {
        let core = self.erode(bits, self.frame.cell)?;
        self.polygons(&core, false)
    }

    /// Polygons on the boundary of the cell set itself: trace cell-edge
    /// boundaries and Douglas–Peucker at one cell, keeping only chords that
    /// stay inside the closed set cells, then resolve crossings by even-odd
    /// overlay. Every output point lies in a closed set cell.
    pub(super) fn boundary(&self, bits: &Bits) -> Result<MultiPolygon<f64>, ManufacturingError> {
        self.polygons(bits, true)
    }

    fn polygons(
        &self,
        core: &Bits,
        constrained: bool,
    ) -> Result<MultiPolygon<f64>, ManufacturingError> {
        if core.is_empty() {
            return Ok(MultiPolygon(Vec::new()));
        }
        let rings = self.trace(core)?;
        let frame = &self.frame;
        let mut contours: Vec<Vec<[f64; 2]>> = Vec::new();
        self.reserve(&mut contours, rings.len())?;
        for ring in &rings {
            check_cancel(self.cancel)?;
            let simplified = simplify_ring(ring, 1.0, |a, b| {
                if !constrained {
                    return true;
                }
                self.charge(
                    (a[0] - b[0]).unsigned_abs() as usize + (a[1] - b[1]).unsigned_abs() as usize,
                );
                chord_inside(core, a, b)
            });
            self.charge(ring.len());
            if simplified.len() >= 3 {
                contours.push(
                    simplified
                        .iter()
                        .map(|&[col, row]| [frame.x(col as f64), frame.y(row as f64)])
                        .collect(),
                );
            }
        }
        drop(rings);
        if contours.is_empty() {
            return Ok(MultiPolygon(Vec::new()));
        }
        use i_overlay::core::overlay::ShapeType;
        use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
        use i_overlay::float::overlay::FloatOverlay;
        use i_overlay::i_float::{adapter::FloatPointAdapter, float::rect::FloatRect};
        let [lo, hi] = frame.bounds();
        let adapter = FloatPointAdapter::new(FloatRect {
            min_x: lo[0],
            min_y: lo[1],
            max_x: hi[0],
            max_y: hi[1],
        });
        let points: usize = contours.iter().map(Vec::len).sum();
        (self.admit)(points.saturating_mul(64))?;
        self.charge(points);
        let shapes = FloatOverlay::with_adapter(adapter, points)
            .unsafe_add_source(&contours, ShapeType::Subject)
            .overlay(OverlayRule::Subject, FillRule::EvenOdd);
        Ok(MultiPolygon(
            shapes
                .into_iter()
                .map(|shape| {
                    let mut rings = shape.into_iter().map(|ring| {
                        let mut line = LineString(ring.into_iter().map(geo::Coord::from).collect());
                        line.close();
                        line
                    });
                    Polygon::new(
                        rings.next().unwrap_or_else(LineString::empty),
                        rings.collect(),
                    )
                })
                .collect(),
        ))
    }

    /// Closed boundary rings in corner coordinates, interior on the left
    /// (exteriors counterclockwise, holes clockwise). At a corner shared by
    /// two diagonal cells the ring turns left, so such cells stay separate.
    fn trace(&self, bits: &Bits) -> Result<Vec<Vec<[i64; 2]>>, ManufacturingError> {
        // (start corner key, direction): 0 +x, 1 +y, 2 −x, 3 −y.
        let mut edges: Vec<(u64, u8)> = Vec::new();
        let key = |col: usize, row: usize| ((row as u64) << 32) | col as u64;
        let width = bits.width;
        let zero = vec![0u64; width];
        for r in 0..=bits.rows() {
            check_cancel(self.cancel)?;
            let row = bits.row0 + r;
            let above = bits.row(row).unwrap_or(&zero);
            let below = row
                .checked_sub(1)
                .and_then(|n| bits.row(n))
                .unwrap_or(&zero);
            for w in 0..width {
                let base = (bits.word0 + w) * 64;
                let (up, down) = (above[w] & !below[w], below[w] & !above[w]);
                self.reserve(&mut edges, (up.count_ones() + down.count_ones()) as usize)?;
                for_bits(up, |b| edges.push((key(base + b, row), 0)));
                for_bits(down, |b| edges.push((key(base + b + 1, row), 2)));
            }
            if r == bits.rows() {
                break;
            }
            let mut previous = 0u64;
            for w in 0..=width {
                let current = above.get(w).copied().unwrap_or(0);
                let left = (current << 1) | (previous >> 63);
                let (rise, fall) = (current & !left, !current & left);
                previous = current;
                let base = (bits.word0 + w) * 64;
                self.reserve(&mut edges, (rise.count_ones() + fall.count_ones()) as usize)?;
                for_bits(rise, |b| edges.push((key(base + b, row + 1), 3)));
                for_bits(fall, |b| edges.push((key(base + b, row), 1)));
            }
            self.charge(width * 2);
        }
        edges.sort_unstable();
        let mut used = Vec::new();
        self.reserve(&mut used, edges.len())?;
        used.resize(edges.len(), false);
        let step = |k: u64, dir: u8| -> u64 {
            let (col, row) = ((k & 0xffff_ffff) as i64, (k >> 32) as i64);
            let (col, row) = match dir {
                0 => (col + 1, row),
                1 => (col, row + 1),
                2 => (col - 1, row),
                _ => (col, row - 1),
            };
            ((row as u64) << 32) | col as u64
        };
        let mut rings = Vec::new();
        let mut walk: Vec<(u64, u8)> = Vec::new();
        for start in 0..edges.len() {
            if used[start] {
                continue;
            }
            check_cancel(self.cancel)?;
            walk.clear();
            let mut at = start;
            loop {
                used[at] = true;
                let (corner, dir) = edges[at];
                self.reserve(&mut walk, 1)?;
                walk.push((corner, dir));
                let end = step(corner, dir);
                let from = edges.partition_point(|e| e.0 < end);
                let candidates = &edges[from..edges[from..].partition_point(|e| e.0 == end) + from];
                let next = match candidates {
                    [only] => only.1,
                    [_, _] => (dir + 1) % 4,
                    _ => {
                        return Err(error(
                            ManufacturingErrorCode::NativeConditioningFailure,
                            "support grid boundary is not a closed ring",
                        ));
                    }
                };
                let Some(offset) = candidates.iter().position(|e| e.1 == next) else {
                    return Err(error(
                        ManufacturingErrorCode::NativeConditioningFailure,
                        "support grid boundary turn is not available",
                    ));
                };
                at = from + offset;
                if used[at] {
                    break;
                }
            }
            self.charge(walk.len());
            let n = walk.len();
            let mut ring = Vec::new();
            self.reserve(&mut ring, n)?;
            for i in 0..n {
                if walk[(i + n - 1) % n].1 != walk[i].1 {
                    let k = walk[i].0;
                    ring.push([(k & 0xffff_ffff) as i64, (k >> 32) as i64]);
                }
            }
            rings.push(ring);
        }
        Ok(rings)
    }
}

/// Polygon edges as coordinate pairs, closing each ring.
fn edges(polygon: &Polygon<f64>) -> impl Iterator<Item = [[f64; 2]; 2]> + '_ {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .flat_map(|ring| {
            let n = ring.0.len();
            (0..n).filter_map(move |i| {
                let (p, q) = (ring.0[i], ring.0[(i + 1) % n]);
                (p != q).then_some([[p.x, p.y], [q.x, q.y]])
            })
        })
}

fn for_bits(mut word: u64, mut visit: impl FnMut(usize)) {
    while word != 0 {
        let bit = word.trailing_zeros() as usize;
        visit(bit);
        word &= word - 1;
    }
}

/// Set columns `[from, to)` of one row slice.
fn set_range(row: &mut [u64], from: usize, to: usize) {
    if from >= to {
        return;
    }
    let (first, last) = (from / 64, (to - 1) / 64);
    for (w, word) in row.iter_mut().enumerate().take(last + 1).skip(first) {
        let mut mask = !0u64;
        if w == first {
            mask &= !0u64 << (from % 64);
        }
        if w == last {
            mask &= !0u64 >> (63 - (to - 1) % 64);
        }
        *word |= mask;
    }
}

/// Word `w` of a row whose bit `b` is the source bit `64w + b + shift`.
fn shifted(source: &[u64], w: usize, shift: i64) -> u64 {
    let bit = w as i64 * 64 + shift;
    let (q, off) = (bit.div_euclid(64), bit.rem_euclid(64) as u32);
    let get = |i: i64| {
        usize::try_from(i)
            .ok()
            .and_then(|i| source.get(i))
            .copied()
            .unwrap_or(0)
    };
    if off == 0 {
        get(q)
    } else {
        (get(q) >> off) | (get(q + 1) << (64 - off))
    }
}

/// Horizontal reach in cells for each row offset of the closed-distance
/// disk of `radius` cells: offset `(k, j)` is within reach when the gap
/// between the two closed cells is at most `radius`.
fn structuring(radius: f64) -> Vec<usize> {
    let mut reach = Vec::new();
    for k in 0usize.. {
        let gap = k.saturating_sub(1) as f64;
        if gap > radius {
            break;
        }
        reach.push(1 + (radius * radius - gap * gap).sqrt().floor() as usize);
    }
    reach
}

/// X extent of the closed capsule `ab ⊕ disk(radius)` within the horizontal
/// band `[y0, y1]`. A convex set's extreme in a slab lies on a slab line or
/// is the set's own extreme.
fn capsule_band(a: [f64; 2], b: [f64; 2], radius: f64, band: [f64; 2]) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut take = |range: Option<(f64, f64)>| {
        if let Some((l, h)) = range {
            lo = lo.min(l);
            hi = hi.max(h);
        }
    };
    take(capsule_line(a, b, radius, band[0]));
    take(capsule_line(a, b, radius, band[1]));
    let inside = |y: f64| y >= band[0] && y <= band[1];
    let (left, right) = if a[0] <= b[0] { (a, b) } else { (b, a) };
    if a[0] == b[0] {
        if a[1].max(b[1]) >= band[0] && a[1].min(b[1]) <= band[1] {
            take(Some((a[0] - radius, a[0] + radius)));
        }
    } else {
        if inside(left[1]) {
            take(Some((left[0] - radius, left[0] - radius)));
        }
        if inside(right[1]) {
            take(Some((right[0] + radius, right[0] + radius)));
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// X interval of the closed capsule on the line `y`.
fn capsule_line(a: [f64; 2], b: [f64; 2], radius: f64, y: f64) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in [a, b] {
        let dy = y - p[1];
        if dy.abs() <= radius {
            let half = (radius * radius - dy * dy).max(0.0).sqrt();
            lo = lo.min(p[0] - half);
            hi = hi.max(p[0] + half);
        }
    }
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx.hypot(dy);
    if length > 0.0 {
        let (ux, uy) = (dx / length, dy / length);
        // Along the segment s(x) = ux·(x − ax) + uy·(y − ay) ∈ [0, L];
        // across it q(x) = −uy·(x − ax) + ux·(y − ay) ∈ [−r, r].
        let along = solve(ux, uy * (y - a[1]) - ux * a[0], 0.0, length);
        let across = solve(-uy, ux * (y - a[1]) + uy * a[0], -radius, radius);
        if let (Some(s), Some(q)) = (along, across) {
            let (l, h) = (s.0.max(q.0), s.1.min(q.1));
            if l <= h {
                lo = lo.min(l);
                hi = hi.max(h);
            }
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// The x interval with `k·x + m ∈ [lo, hi]`.
fn solve(k: f64, m: f64, lo: f64, hi: f64) -> Option<(f64, f64)> {
    if k.abs() < 1.0e-12 {
        return (m >= lo && m <= hi).then_some((f64::NEG_INFINITY, f64::INFINITY));
    }
    let (p, q) = ((lo - m) / k, (hi - m) / k);
    Some((p.min(q), p.max(q)))
}

/// Douglas–Peucker on a closed ring of integer corners, splitting at the
/// vertex farthest from the first. Every dropped vertex lies within
/// `tolerance` of its replacing segment, and every replacing segment that
/// spans dropped vertices is one `accept` admits.
fn simplify_ring(
    ring: &[[i64; 2]],
    tolerance: f64,
    mut accept: impl FnMut([i64; 2], [i64; 2]) -> bool,
) -> Vec<[i64; 2]> {
    let n = ring.len();
    if n < 4 {
        return ring.to_vec();
    }
    let point = |i: usize| ring[i % n].map(|v| v as f64);
    let far = (1..n)
        .max_by(|&i, &j| {
            let d = |k: usize| {
                let p = point(k);
                let o = point(0);
                (p[0] - o[0]).hypot(p[1] - o[1])
            };
            d(i).total_cmp(&d(j)).then(j.cmp(&i))
        })
        .unwrap_or(1);
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    let mut stack = vec![(0usize, far), (far, n)];
    while let Some((i, j)) = stack.pop() {
        let (a, b) = (point(i), point(j));
        let mut best = (0.0f64, 0usize);
        for k in i + 1..j {
            let d = segment_distance(point(k), a, b);
            if d > best.0 {
                best = (d, k);
            }
        }
        if j > i + 1 && (best.0 > tolerance || !accept(ring[i], ring[j % n])) {
            let split = if best.1 > i { best.1 } else { (i + j) / 2 };
            keep[split] = true;
            stack.push((i, split));
            stack.push((split, j));
        }
    }
    (0..n).filter(|&i| keep[i]).map(|i| ring[i]).collect()
}

/// Whether segment `ab` (cell-corner coordinates) lies inside the closed
/// set cells: it is cut at every cell line, and each piece, lying in one
/// cell's closure, needs a set cell holding its midpoint.
fn chord_inside(bits: &Bits, a: [i64; 2], b: [i64; 2]) -> bool {
    let (p, q) = (a.map(|v| v as f64), b.map(|v| v as f64));
    let mut cuts = vec![0.0, 1.0];
    for axis in 0..2 {
        let (lo, hi) = (a[axis].min(b[axis]), a[axis].max(b[axis]));
        for line in lo + 1..hi {
            cuts.push((line as f64 - p[axis]) / (q[axis] - p[axis]));
        }
    }
    cuts.sort_unstable_by(f64::total_cmp);
    cuts.dedup();
    let candidates = |v: f64| {
        let f = v.floor();
        if f == v { [f - 1.0, f] } else { [f, f] }
    };
    cuts.windows(2).all(|w| {
        let t = (w[0] + w[1]) * 0.5;
        let m = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
        candidates(m[0]).iter().any(|&col| {
            candidates(m[1]).iter().any(|&row| {
                col >= 0.0
                    && row >= 0.0
                    && bits.word(row as usize, col as usize / 64) >> (col as usize % 64) & 1 == 1
            })
        })
    })
}

fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length).clamp(0.0, 1.0)
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

#[cfg(test)]
mod tests;
