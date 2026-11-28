// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Bounded session-owned data-parallel kernels whose results are identical on every backend.

#[doc(hidden)]
pub mod capsule_cases;
mod kernels;
mod runtime;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

pub use runtime::{configure, stage_slots, status};
pub use spiling_contracts::compute::{
    ComputeBackend, ComputeDevice, ComputeSettings, ComputeStatus,
};

/// Output words one mask segment uploads and reads back at most, unless a
/// single row is wider. Larger masks run as several segments.
const MASK_SEGMENT_WORDS: usize = 1 << 20;
/// Encoded input words one mask segment uploads at most, unless a single
/// group is larger.
const MASK_SEGMENT_INPUT: usize = 1 << 22;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComputeError {
    Cancelled,
    InvalidInput(&'static str),
    AlreadyConfigured,
    Initialization(String),
    Execution(String),
    Allocation,
    Poisoned,
}

impl fmt::Display for ComputeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("compute batch cancelled"),
            Self::InvalidInput(message) => write!(formatter, "invalid compute input: {message}"),
            Self::AlreadyConfigured => {
                formatter.write_str("compute settings are immutable after initialization")
            }
            Self::Initialization(message) => {
                write!(formatter, "compute initialization failed: {message}")
            }
            Self::Execution(message) => write!(formatter, "compute execution failed: {message}"),
            Self::Allocation => formatter.write_str("compute buffer allocation failed"),
            Self::Poisoned => formatter.write_str("compute session lock is poisoned"),
        }
    }
}

impl std::error::Error for ComputeError {}

/// Row-major query × input mask. Padding bits are zero and cannot be addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskRows {
    rows: usize,
    columns: usize,
    words_per_row: usize,
    words: Vec<u32>,
}

impl MaskRows {
    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn selected_columns(&self, row: usize) -> Option<impl Iterator<Item = usize> + '_> {
        if row >= self.rows {
            return None;
        }
        let start = row * self.words_per_row;
        Some(
            self.words[start..start + self.words_per_row]
                .iter()
                .copied()
                .enumerate()
                .flat_map(|(word_index, mut bits)| {
                    std::iter::from_fn(move || {
                        if bits == 0 {
                            return None;
                        }
                        let bit =
                            usize::try_from(bits.trailing_zeros()).expect("bit index fits usize");
                        bits &= bits - 1;
                        Some(word_index * 32 + bit)
                    })
                })
                .take_while(|column| *column < self.columns),
        )
    }

    pub fn get(&self, row: usize, column: usize) -> Option<bool> {
        if row >= self.rows || column >= self.columns {
            return None;
        }
        let word = self.words.get(row * self.words_per_row + column / 32)?;
        Some(word & (1 << (column % 32)) != 0)
    }
}

#[derive(Clone, Copy)]
struct Shape {
    rows: usize,
    columns: usize,
    words_per_row: usize,
    words: usize,
}

impl Shape {
    fn new(rows: usize, columns: usize) -> Result<Self, ComputeError> {
        let words_per_row = columns.div_ceil(32);
        let words = rows
            .checked_mul(words_per_row)
            .ok_or(ComputeError::InvalidInput("mask shape overflows"))?;
        if u32::try_from(columns).is_err() {
            return Err(ComputeError::InvalidInput("mask columns exceed u32"));
        }
        Ok(Self {
            rows,
            columns,
            words_per_row,
            words,
        })
    }

    fn empty(self) -> Result<MaskRows, ComputeError> {
        Ok(MaskRows {
            rows: self.rows,
            columns: self.columns,
            words_per_row: self.words_per_row,
            words: zeroed_words(self.words)?,
        })
    }
}

/// Exact half-open crossing, including reversed endpoints: `(a <= q && q < b) || (b <= q && q < a)`.
/// Rows are coordinates and columns are ranges; any size runs.
pub fn row_crossings(
    edge_ranges: &[[f64; 2]],
    coordinates: &[f64],
    cancel: &AtomicBool,
) -> Result<MaskRows, ComputeError> {
    intervals(edge_ranges, coordinates, cancel, 0)
}

/// `row_crossings` for many independent `(edge_ranges, coordinates)` groups
/// in one request: one stage's worth of work shares one launch.
pub fn row_crossings_groups(
    groups: &[(&[[f64; 2]], &[f64])],
    cancel: &AtomicBool,
) -> Result<Vec<MaskRows>, ComputeError> {
    check_cancel(cancel)?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(groups.len())
        .map_err(|_| ComputeError::Allocation)?;
    for (ranges, coordinates) in groups {
        validate_finite(ranges.iter().flatten().chain(coordinates.iter()), cancel)?;
        let shape = Shape::new(coordinates.len(), ranges.len())?;
        encoded.push(runtime::interval_group(
            shape,
            ranges,
            coordinates,
            cancel,
            0,
        )?);
    }
    let masks = runtime::execute_masks(encoded, cancel)?;
    if masks.len() != groups.len() {
        return Err(ComputeError::Execution(
            "mask round returned the wrong group count".into(),
        ));
    }
    Ok(masks)
}

/// Exact closed interval membership. Endpoints may be reversed; zero-width intervals include their point.
pub fn interval_membership(
    ranges: &[[f64; 2]],
    coordinates: &[f64],
    cancel: &AtomicBool,
) -> Result<MaskRows, ComputeError> {
    intervals(ranges, coordinates, cancel, 2)
}

fn intervals(
    ranges: &[[f64; 2]],
    coordinates: &[f64],
    cancel: &AtomicBool,
    kind: u32,
) -> Result<MaskRows, ComputeError> {
    check_cancel(cancel)?;
    validate_finite(ranges.iter().flatten().chain(coordinates), cancel)?;
    let shape = Shape::new(coordinates.len(), ranges.len())?;
    if shape.words == 0 {
        return shape.empty();
    }
    let group = runtime::interval_group(shape, ranges, coordinates, cancel, kind)?;
    single(runtime::execute_masks(vec![group], cancel)?)
}

fn single(mut masks: Vec<MaskRows>) -> Result<MaskRows, ComputeError> {
    match (masks.pop(), masks.is_empty()) {
        (Some(mask), true) => Ok(mask),
        _ => Err(ComputeError::Execution(
            "mask round returned the wrong group count".into(),
        )),
    }
}

/// Exact closed overlap of `[min_x, min_y, max_x, max_y]` boxes. Inverted boxes are invalid.
/// Rows are queries and columns are boxes; any size runs.
pub fn box_overlaps(
    boxes: &[[f64; 4]],
    queries: &[[f64; 4]],
    cancel: &AtomicBool,
) -> Result<MaskRows, ComputeError> {
    check_cancel(cancel)?;
    validate_finite(
        boxes.iter().flatten().chain(queries.iter().flatten()),
        cancel,
    )?;
    if boxes
        .iter()
        .chain(queries)
        .any(|bounds| bounds[0] > bounds[2] || bounds[1] > bounds[3])
    {
        return Err(ComputeError::InvalidInput("box minima exceed maxima"));
    }
    let shape = Shape::new(queries.len(), boxes.len())?;
    if shape.words == 0 {
        return shape.empty();
    }
    let group = runtime::box_group(shape, boxes, queries, cancel)?;
    single(runtime::execute_masks(vec![group], cancel)?)
}

/// Largest capsule coordinate or radius, in ticks: keeps every product the
/// exact capsule test forms inside `i64`.
pub const MAX_CAPSULE_TICKS: i64 = 1 << 30;
/// Longest capsule segment, in ticks, the exact test admits.
pub const MAX_CAPSULE_SEGMENT_TICKS: i64 = 1 << 12;
/// Largest capsule radius, in ticks, the exact test admits.
pub const MAX_CAPSULE_RADIUS_TICKS: i64 = 1 << 17;

/// One group's cells as a dense row-major window of 64-cell words: rows
/// `row0..row0 + rows`, word columns `word0..word0 + width`; bit `b` of word
/// `w` is column `64 w + b`. An empty group has an empty window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CellMask {
    pub row0: u32,
    pub rows: u32,
    pub word0: u32,
    pub width: u32,
    pub words: Vec<u64>,
}

/// Every cell whose closed square meets a closed capsule `[ax, ay, bx, by,
/// radius]` (integer ticks, `cell` ticks per grid cell, a power of two; columns
/// `0..columns`), per group. Capsule `i` reaches `rows[i][1]` rows from
/// `rows[i][0]`; group `g` holds capsules `group_ends[g - 1]..group_ends[g]`.
/// Exact integer decisions: identical on every backend. Any batch size runs;
/// large batches are split into bounded segments of whole groups.
pub fn capsule_masks(
    capsules: &[[i64; 5]],
    rows: &[[u32; 2]],
    group_ends: &[usize],
    cell: i64,
    columns: i64,
    cancel: &AtomicBool,
) -> Result<Vec<CellMask>, ComputeError> {
    check_cancel(cancel)?;
    // A power-of-two cell lets the kernel divide by shifting.
    if !(1..=1 << 16).contains(&cell)
        || cell.count_ones() != 1
        || !(1..=i64::from(i32::MAX)).contains(&columns)
    {
        return Err(ComputeError::InvalidInput("capsule grid shape"));
    }
    if rows.len() != capsules.len()
        || group_ends.windows(2).any(|pair| pair[0] > pair[1])
        || group_ends.last().is_some_and(|&end| end != capsules.len())
        || (group_ends.is_empty() && !capsules.is_empty())
    {
        return Err(ComputeError::InvalidInput("capsule groups"));
    }
    // Each group's window holds every column and row its capsules can
    // reach, by the kernel's own bounds.
    let mut windows = Vec::new();
    windows
        .try_reserve_exact(group_ends.len())
        .map_err(|_| ComputeError::Allocation)?;
    let mut first = 0usize;
    for &end in group_ends {
        let mut bounds: Option<[i64; 4]> = None;
        for (index, (&[ax, ay, bx, by, radius], &[row, count])) in capsules[first..end]
            .iter()
            .zip(&rows[first..end])
            .enumerate()
        {
            if index % 1_024 == 0 {
                check_cancel(cancel)?;
            }
            if [ax, ay, bx, by].iter().any(|v| v.abs() > MAX_CAPSULE_TICKS)
                || !(0..=MAX_CAPSULE_RADIUS_TICKS).contains(&radius)
                || (bx - ax).abs().max((by - ay).abs()) > MAX_CAPSULE_SEGMENT_TICKS
            {
                return Err(ComputeError::InvalidInput(
                    "capsule outside the exact range",
                ));
            }
            if row.checked_add(count).is_none() {
                return Err(ComputeError::InvalidInput("capsule row range"));
            }
            if count == 0 {
                continue;
            }
            let first_column = ((ax.min(bx) - radius - 1).max(0) / cell).min(columns - 1);
            let last_column = ((ax.max(bx) + radius).max(0) / cell).min(columns - 1);
            let (row0, row1) = (i64::from(row), i64::from(row) + i64::from(count) - 1);
            bounds = Some(match bounds {
                None => [row0, row1, first_column, last_column],
                Some([r0, r1, c0, c1]) => [
                    r0.min(row0),
                    r1.max(row1),
                    c0.min(first_column),
                    c1.max(last_column),
                ],
            });
        }
        windows.push(bounds.map_or([0; 4], |[r0, r1, c0, c1]| {
            [
                r0 as u32,
                (r1 - r0 + 1) as u32,
                (c0 / 64) as u32,
                (c1 / 64 - c0 / 64 + 1) as u32,
            ]
        }));
        first = end;
    }
    runtime::execute_capsule_masks(capsules, rows, group_ends, &windows, cell, columns, cancel)
}

/// The exact capsule-row answer computed on the host, for reference checks.
pub fn capsule_rows_reference(capsule: [i64; 5], row: u32, cell: i64, columns: i64) -> [i32; 2] {
    let [ax, ay, bx, by, radius] = capsule;
    let r2 = radius * radius;
    let gap = |value: i64, low: i64, high: i64| (low - value).max(value - high).max(0);
    let within = |px: i64, py: i64| {
        let (dx, dy, wx, wy) = (bx - ax, by - ay, px - ax, py - ay);
        let along = wx * dx + wy * dy;
        let length2 = dx * dx + dy * dy;
        if along <= 0 {
            wx * wx + wy * wy <= r2
        } else if along >= length2 {
            (px - bx).pow(2) + (py - by).pow(2) <= r2
        } else {
            let cross = dx * wy - dy * wx;
            cross * cross <= r2 * length2
        }
    };
    let y0 = i64::from(row) * cell;
    let y1 = y0 + cell;
    let meets = |column: i64| {
        let (x0, x1) = (column * cell, column * cell + cell);
        if gap(ax, x0, x1).pow(2) + gap(ay, y0, y1).pow(2) <= r2
            || gap(bx, x0, x1).pow(2) + gap(by, y0, y1).pow(2) <= r2
        {
            return true;
        }
        if ax.max(bx) >= x0 && ax.min(bx) <= x1 && ay.max(by) >= y0 && ay.min(by) <= y1 {
            let (dx, dy) = (bx - ax, by - ay);
            let sides = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
                .map(|(x, y)| dx * (y - ay) - dy * (x - ax));
            if !(sides.iter().all(|s| *s > 0) || sides.iter().all(|s| *s < 0)) {
                return true;
            }
        }
        within(x0, y0) || within(x1, y0) || within(x0, y1) || within(x1, y1)
    };
    // A column outside the capsule's bounding box, widened by two cells,
    // cannot meet it; the scan covers every other column.
    let first = ((ax.min(bx) - radius).div_euclid(cell) - 2).max(0);
    let last = ((ax.max(bx) + radius).div_euclid(cell) + 2).min(columns - 1);
    let hits: Vec<i64> = (first..=last).filter(|&column| meets(column)).collect();
    match (hits.first(), hits.last()) {
        (Some(&first), Some(&last)) => [first as i32, last as i32],
        _ => [-1, -2],
    }
}

fn validate_finite<'value>(
    values: impl Iterator<Item = &'value f64>,
    cancel: &AtomicBool,
) -> Result<(), ComputeError> {
    for (index, value) in values.enumerate() {
        if index % 1_024 == 0 {
            check_cancel(cancel)?;
        }
        if !value.is_finite() {
            return Err(ComputeError::InvalidInput("all coordinates must be finite"));
        }
    }
    check_cancel(cancel)
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ComputeError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ComputeError::Cancelled)
    } else {
        Ok(())
    }
}

fn zeroed_words(length: usize) -> Result<Vec<u32>, ComputeError> {
    let mut words = Vec::new();
    words
        .try_reserve_exact(length)
        .map_err(|_| ComputeError::Allocation)?;
    words.resize(length, 0);
    Ok(words)
}

/// Finite IEEE-754 order as two unsigned keys; both zero signs have the same key.
fn ordered_key(value: f64) -> [u32; 2] {
    let bits = if value == 0.0 { 0 } else { value.to_bits() };
    let ordered = if bits & (1_u64 << 63) != 0 {
        !bits
    } else {
        bits ^ (1_u64 << 63)
    };
    [
        u32::try_from(ordered >> 32).expect("high half fits u32"),
        u32::try_from(ordered & u64::from(u32::MAX)).expect("low half fits u32"),
    ]
}

#[cfg(test)]
mod tests;
