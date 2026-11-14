// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use spiling_contracts::{geometry::AabbMm, manufacturing::*};
use std::io::{Read, Write};

/// Layer of the fixed start/end lifecycle moves.
pub(crate) const LIFECYCLE_LAYER: u32 = u32::MAX;

/// Optional bounded byte sink; no per-motion object vector or planner positions.
pub(crate) struct ReplaySink {
    bytes: spiling_chunks::ChunkWriter,
    min: [f64; 3],
    max: [f64; 3],
    events: u32,
    depositions: u32,
    travels: u32,
    layers: u32,
    lifecycle: u32,
    process: u32,
    epochs: u32,
    active_tool: u32,
    expected_events: u32,
    /// The actual stream outgrew the display budget. Recording stopped and the
    /// buffer was released; independent verification still covers the program.
    omitted: bool,
}
impl ReplaySink {
    pub(crate) fn new(
        plan: &NormalizedPrintPlan,
        intent: &ManufacturingIntent,
    ) -> Result<Self, ManufacturingError> {
        Self::with_capacity(plan, intent, MAX_REPLAY_EVENTS as usize)
    }
    /// A sink admitting at most `capacity` display events.
    pub(crate) fn with_capacity(
        plan: &NormalizedPrintPlan,
        intent: &ManufacturingIntent,
        capacity: usize,
    ) -> Result<Self, ManufacturingError> {
        plan.validate(intent)?;
        let events = capacity.min(MAX_REPLAY_EVENTS as usize);
        let buffer = spiling_chunks::ChunkWriter::spooled(u64::from(MAX_REPLAY_DISPLAY_BYTES))
            .map_err(|e| super::failed(format!("replay spool: {e}")))?;
        Ok(Self {
            bytes: buffer,
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
            events: 0,
            depositions: 0,
            travels: 0,
            layers: plan.layers.len() as u32,
            lifecycle: 0,
            process: 0,
            epochs: 0,
            active_tool: 0,
            expected_events: events as u32,
            omitted: false,
        })
    }
    pub(super) fn push_epoch(
        &mut self,
        position: [f64; 3],
        tool: u8,
        line: u32,
        layer: u32,
    ) -> Result<(), ManufacturingError> {
        self.active_tool = u32::from(tool);
        self.push(position, 0.0, 0.0, line, layer, 14)
    }
    pub(super) fn push(
        &mut self,
        position: [f64; 3],
        e: f64,
        feed: f64,
        line: u32,
        layer: u32,
        kind: u8,
    ) -> Result<(), ManufacturingError> {
        self.push_associated(position, e, feed, line, layer, kind, None)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_associated(
        &mut self,
        position: [f64; 3],
        e: f64,
        feed: f64,
        line: u32,
        layer: u32,
        kind: u8,
        association: Option<super::expected::Association>,
    ) -> Result<(), ManufacturingError> {
        if self.omitted {
            return Ok(());
        }
        // Category counts are bounded by the display byte budget below; only
        // the fixed lifecycle has a semantic count.
        if matches!(kind, 6 | 7) && self.lifecycle >= 4 {
            return Err(super::failed("replay lifecycle exceeds its four moves"));
        }
        // SPLR records carry a u16 layer with `u16::MAX` reserved for
        // lifecycle moves. A taller program omits its display, never its
        // verification.
        let record_layer = if layer == LIFECYCLE_LAYER {
            u16::MAX
        } else {
            match u16::try_from(layer) {
                Ok(layer) if layer != u16::MAX => layer,
                _ => {
                    self.omitted = true;
                    return Ok(());
                }
            }
        };
        if self.events >= self.expected_events {
            // Only an explicitly constrained sink can omit output. Production
            // derives its representation from the admitted command counts.
            self.omitted = true;
            return Ok(());
        }
        let (width, physical_z, owner) = match association {
            Some(association) => {
                let owner = match association.owner {
                    None => u32::MAX,
                    Some(DepositionOwner::MandatorySkin { .. }) => u32::MAX - 1,
                    Some(DepositionOwner::Wall) => u32::MAX - 2,
                    Some(DepositionOwner::Regional { region_index }) => u32::from(region_index),
                    Some(DepositionOwner::RegionalSkinBridge { .. }) => {
                        return Err(super::failed(
                            "mixed ownership must be projected per actual segment",
                        ));
                    }
                };
                (association.width, association.physical_z, owner)
            }
            None => (0.0, [0.0; 2], u32::MAX),
        };
        let mut record = [0; REPLAY_RECORD_BYTES];
        for (axis, value) in position.iter().enumerate() {
            record[axis * 8..axis * 8 + 8].copy_from_slice(&value.to_le_bytes());
        }
        record[24..32].copy_from_slice(&e.to_le_bytes());
        record[32..40].copy_from_slice(&feed.to_le_bytes());
        record[40..44].copy_from_slice(&line.to_le_bytes());
        record[44..46].copy_from_slice(&record_layer.to_le_bytes());
        record[46] = kind;
        record[47] =
            u8::from(self.events != 0 && kind != 14) | if association.is_some() { 2 } else { 0 };
        record[48..56].copy_from_slice(&width.to_le_bytes());
        record[56..64].copy_from_slice(&physical_z[0].to_le_bytes());
        record[64..72].copy_from_slice(&physical_z[1].to_le_bytes());
        record[72..76].copy_from_slice(&owner.to_le_bytes());
        record[76..80].copy_from_slice(&self.active_tool.to_le_bytes());
        self.bytes
            .write_all(&record)
            .map_err(|e| super::failed(format!("replay spool write: {e}")))?;
        for (axis, value) in position.iter().enumerate() {
            self.min[axis] = self.min[axis].min(*value);
            self.max[axis] = self.max[axis].max(*value);
        }
        self.events += 1;
        if matches!(kind, 6 | 7) {
            self.lifecycle += 1;
        } else if matches!(kind, 11 | 12) {
            self.process += 1;
        } else if kind == 14 {
            self.epochs += 1;
        } else if kind == 0 {
            self.travels += 1;
        } else {
            self.depositions += 1;
        }
        Ok(())
    }
    /// True when the actual stream exceeded the display budget.
    pub(crate) fn omitted(&self) -> bool {
        self.omitted
    }
    pub(crate) fn finish(
        self,
    ) -> Result<(spiling_chunks::ChunkSource, AabbMm), ManufacturingError> {
        if self.omitted {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "actual motion exceeds the replay display format (bytes or u16 layer index)",
            ));
        }
        if self.events == 0 || !self.process.is_multiple_of(2) {
            return Err(super::failed("incomplete replay display"));
        }
        let mut header = [0; REPLAY_HEADER_BYTES];
        header[..4].copy_from_slice(b"SPLR");
        header[4..6].copy_from_slice(&(REPLAY_SCHEMA_VERSION as u16).to_le_bytes());
        header[6..8].copy_from_slice(&(REPLAY_HEADER_BYTES as u16).to_le_bytes());
        for (offset, value) in [
            (8, self.events),
            (12, REPLAY_RECORD_BYTES as u32),
            (16, self.depositions),
            (20, self.travels),
            (24, self.layers),
            (28, self.lifecycle),
            (88, self.process),
            (92, self.epochs),
        ] {
            header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for axis in 0..3 {
            header[32 + 8 * axis..40 + 8 * axis].copy_from_slice(&self.min[axis].to_le_bytes());
            header[56 + 8 * axis..64 + 8 * axis].copy_from_slice(&self.max[axis].to_le_bytes());
        }
        let bounds = AabbMm::new(self.min, self.max).map_err(|e| super::failed(e.message))?;
        let body = self
            .bytes
            .finish()
            .map_err(|e| super::failed(format!("replay spool seal: {e}")))?;
        let mut stream = std::io::Cursor::new(header).chain(body.reader());
        let mut writer = spiling_chunks::ChunkWriter::spooled(u64::from(MAX_REPLAY_DISPLAY_BYTES))
            .map_err(|e| super::failed(format!("replay spool: {e}")))?;
        std::io::copy(&mut stream, &mut writer)
            .map_err(|e| super::failed(format!("replay spool finish: {e}")))?;
        let bytes = writer
            .finish()
            .map_err(|e| super::failed(format!("replay spool seal: {e}")))?;
        Ok((bytes, bounds))
    }
}
