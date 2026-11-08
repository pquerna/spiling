// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Bounded session-owned exact numeric predicates; not manufacturing authority.

mod kernels;
mod runtime;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

pub use runtime::{configure, status};
pub use spiling_contracts::compute::{
    ComputeBackend, ComputeDevice, ComputeSettings, ComputeStatus,
};

/// Admission limits, not a speedup promise. Smaller/larger work stays on the caller's baseline.
pub const MIN_BATCH_PAIRS: usize = 4_096;
pub const MAX_BATCH_ITEMS: usize = 65_536;
pub const MAX_MASK_WORDS: usize = 262_144;
const TILE_WORDS: usize = 4_096;

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
    fn admit(rows: usize, columns: usize) -> Option<Self> {
        if rows > MAX_BATCH_ITEMS || columns > MAX_BATCH_ITEMS {
            return None;
        }
        let pairs = rows.checked_mul(columns)?;
        let words_per_row = columns.div_ceil(32);
        let words = rows.checked_mul(words_per_row)?;
        if pairs < MIN_BATCH_PAIRS || words > MAX_MASK_WORDS {
            return None;
        }
        Some(Self {
            rows,
            columns,
            words_per_row,
            words,
        })
    }
}

/// Exact half-open crossing, including reversed endpoints: `(a <= q && q < b) || (b <= q && q < a)`.
/// `None` means only that the bounded acceleration budget declined the batch.
pub fn row_crossings(
    edge_ranges: &[[f64; 2]],
    coordinates: &[f64],
    cancel: &AtomicBool,
) -> Result<Option<MaskRows>, ComputeError> {
    intervals(edge_ranges, coordinates, cancel, 0)
}

/// Exact closed interval membership. Endpoints may be reversed; zero-width intervals include their point.
pub fn interval_membership(
    ranges: &[[f64; 2]],
    coordinates: &[f64],
    cancel: &AtomicBool,
) -> Result<Option<MaskRows>, ComputeError> {
    intervals(ranges, coordinates, cancel, 2)
}

fn intervals(
    ranges: &[[f64; 2]],
    coordinates: &[f64],
    cancel: &AtomicBool,
    kind: u32,
) -> Result<Option<MaskRows>, ComputeError> {
    check_cancel(cancel)?;
    if ranges.len() > MAX_BATCH_ITEMS || coordinates.len() > MAX_BATCH_ITEMS {
        return Ok(None);
    }
    validate_finite(ranges.iter().flatten().chain(coordinates), cancel)?;
    let Some(shape) = Shape::admit(coordinates.len(), ranges.len()) else {
        return Ok(None);
    };
    runtime::execute(shape, ranges, coordinates, cancel, kind)
}

/// Exact closed overlap of `[min_x, min_y, max_x, max_y]` boxes. Inverted boxes are invalid.
pub fn box_overlaps(
    boxes: &[[f64; 4]],
    queries: &[[f64; 4]],
    cancel: &AtomicBool,
) -> Result<Option<MaskRows>, ComputeError> {
    check_cancel(cancel)?;
    if boxes.len() > MAX_BATCH_ITEMS || queries.len() > MAX_BATCH_ITEMS {
        return Ok(None);
    }
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
    let Some(shape) = Shape::admit(queries.len(), boxes.len()) else {
        return Ok(None);
    };
    runtime::execute_boxes(shape, boxes, queries, cancel)
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
