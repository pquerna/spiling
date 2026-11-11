// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Descriptive index over the exact SPLR6 command stream, never new motion.
use super::*;
use std::io::Read;
fn corrupt(message: &str) -> ManufacturingError {
    ManufacturingError::new(ManufacturingErrorCode::CorruptArtifact, message)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReplayPage {
    /// Global ordinal; u64 on the wire even though SPLR6 counts are u32.
    #[ts(type = "number")]
    pub first_event: u64,
    pub event_count: u32,
    pub hash: SourceHash,
    pub first_line: u32,
    pub last_line: u32,
    /// Body layers only; MAX denotes a page containing lifecycle commands only.
    pub first_layer: u32,
    pub last_layer: u32,
    pub bounds_mm: AabbMm,
    pub physical_z_mm: [f64; 2],
}

pub fn validate_replay_index(
    pages: &[ReplayPage],
    info: &ReplayDisplayInfo,
) -> Result<(), ManufacturingError> {
    info.validate()?;
    if pages.len() != info.event_count.div_ceil(REPLAY_PAGE_EVENTS) as usize {
        return Err(corrupt("replay index page count mismatch"));
    }
    let mut first = 0u64;
    let mut line = 0;
    for page in pages {
        let remaining = u64::from(info.event_count)
            .checked_sub(first)
            .ok_or_else(|| corrupt("replay index ordinal overflow"))?;
        if page.first_event != first
            || page.event_count != remaining.min(u64::from(REPLAY_PAGE_EVENTS)) as u32
            || page.first_line < line
            || page.last_line < page.first_line
            || !page.physical_z_mm.iter().all(|n| n.is_finite())
            || page.physical_z_mm[0] > page.physical_z_mm[1]
            || (page.first_layer == u32::MAX) != (page.last_layer == u32::MAX)
            || (page.first_layer != u32::MAX
                && (page.last_layer < page.first_layer || page.last_layer >= info.layer_count))
        {
            return Err(corrupt("invalid replay page index"));
        }
        page.bounds_mm.validate().map_err(|e| corrupt(&e.message))?;
        for axis in 0..3 {
            if page.bounds_mm.min[axis] < info.bounds_mm.min[axis]
                || page.bounds_mm.max[axis] > info.bounds_mm.max[axis]
            {
                return Err(corrupt("replay page escapes document bounds"));
            }
        }
        first += u64::from(page.event_count);
        line = page.last_line;
    }
    Ok(())
}

/// Build the index with one bounded page buffer. The caller must separately run
/// full semantic admission; an index/hash alone cannot authorize a toolpath.
pub fn index_replay(
    mut reader: impl Read,
    info: &ReplayDisplayInfo,
) -> Result<Vec<ReplayPage>, ManufacturingError> {
    info.validate()?;
    let mut header = [0; REPLAY_HEADER_BYTES];
    reader
        .read_exact(&mut header)
        .map_err(|_| corrupt("replay header unavailable"))?;
    let mut pages = Vec::new();
    let mut first = 0u64;
    while first < u64::from(info.event_count) {
        let count = (u64::from(info.event_count) - first).min(u64::from(REPLAY_PAGE_EVENTS)) as u32;
        let mut hash = Sha256::new();
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut z = [f64::INFINITY, f64::NEG_INFINITY];
        let mut first_line = 0;
        let mut last_line = 0;
        let mut first_layer = u32::MAX;
        let mut last_layer = u32::MAX;
        for i in 0..count {
            let mut r = [0; REPLAY_RECORD_BYTES];
            reader
                .read_exact(&mut r)
                .map_err(|_| corrupt("replay page unavailable"))?;
            hash.update(r);
            let number = |at| f64::from_le_bytes(r[at..at + 8].try_into().unwrap());
            for axis in 0..3 {
                let p = number(axis * 8);
                min[axis] = min[axis].min(p);
                max[axis] = max[axis].max(p);
            }
            let line = u32::from_le_bytes(r[40..44].try_into().unwrap());
            if i == 0 {
                first_line = line;
            }
            last_line = line;
            let layer = u16::from_le_bytes(r[44..46].try_into().unwrap());
            if layer != u16::MAX {
                first_layer = first_layer.min(u32::from(layer));
                last_layer = if last_layer == u32::MAX {
                    u32::from(layer)
                } else {
                    last_layer.max(u32::from(layer))
                };
            }
            let span = if r[47] & 2 != 0 {
                [number(56), number(64)]
            } else {
                [number(16); 2]
            };
            z[0] = z[0].min(span[0]);
            z[1] = z[1].max(span[1]);
        }
        pages.push(ReplayPage {
            first_event: first,
            event_count: count,
            hash: SourceHash::from_digest(&hash.finalize().into()),
            first_line,
            last_line,
            first_layer,
            last_layer,
            bounds_mm: AabbMm::new(min, max).map_err(|e| corrupt(&e.message))?,
            physical_z_mm: z,
        });
        first += u64::from(count);
    }
    validate_replay_index(&pages, info)?;
    Ok(pages)
}
