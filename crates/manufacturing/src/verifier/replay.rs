// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use spiling_contracts::{geometry::AabbMm, manufacturing::*};

/// Optional bounded byte sink; no per-motion object vector or planner positions.
pub(crate) struct ReplaySink {
    bytes: Vec<u8>,
    min: [f64; 3],
    max: [f64; 3],
    events: u32,
    depositions: u32,
    travels: u32,
    layers: u32,
    lifecycle: u32,
    expected_events: u32,
}
impl ReplaySink {
    pub(crate) fn new(
        plan: &NormalizedPrintPlan,
        intent: &ManufacturingIntent,
    ) -> Result<Self, ManufacturingError> {
        plan.validate(intent)?;
        let events = plan
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .try_fold(0usize, |n, p| n.checked_add(p.points_mm.len()))
            .and_then(|n| {
                n.checked_add(
                    if matches!(intent.printer.workflow, JobWorkflow::CompleteMarlin { .. }) {
                        4
                    } else {
                        0
                    },
                )
            })
            .ok_or_else(|| super::failed("replay event count overflow"))?;
        let bytes = events
            .checked_mul(REPLAY_RECORD_BYTES)
            .and_then(|n| n.checked_add(REPLAY_HEADER_BYTES))
            .ok_or_else(|| super::failed("replay byte count overflow"))?;
        if events > MAX_REPLAY_EVENTS as usize || bytes > MAX_REPLAY_DISPLAY_BYTES as usize {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "replay display budget exceeded",
            ));
        }
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(bytes).map_err(|_| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "replay display allocation failed",
            )
        })?;
        if buffer.capacity() > MAX_REPLAY_DISPLAY_BYTES as usize {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "replay display actual capacity exceeds byte budget",
            ));
        }
        buffer.resize(REPLAY_HEADER_BYTES, 0);
        Ok(Self {
            bytes: buffer,
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
            events: 0,
            depositions: 0,
            travels: 0,
            layers: plan.layers.len() as u32,
            lifecycle: 0,
            expected_events: events as u32,
        })
    }
    pub(super) fn push(
        &mut self,
        position: [f64; 3],
        e: f64,
        feed: f64,
        line: u32,
        layer: u16,
        kind: u8,
    ) -> Result<(), ManufacturingError> {
        if self.events >= self.expected_events
            || self.bytes.len() + REPLAY_RECORD_BYTES > self.bytes.capacity()
        {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "replay sink exceeds admitted capacity",
            ));
        }
        for value in position {
            self.bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.bytes.extend_from_slice(&e.to_le_bytes());
        self.bytes.extend_from_slice(&feed.to_le_bytes());
        self.bytes.extend_from_slice(&line.to_le_bytes());
        self.bytes.extend_from_slice(&layer.to_le_bytes());
        self.bytes.push(kind);
        self.bytes.push(u8::from(self.events != 0));
        for (axis, value) in position.iter().enumerate() {
            self.min[axis] = self.min[axis].min(*value);
            self.max[axis] = self.max[axis].max(*value);
        }
        self.events += 1;
        if kind >= 6 {
            self.lifecycle += 1;
        } else if kind == 0 {
            self.travels += 1;
        } else {
            self.depositions += 1;
        }
        Ok(())
    }
    pub(crate) fn finish(mut self) -> Result<(Vec<u8>, AabbMm), ManufacturingError> {
        if self.events != self.expected_events {
            return Err(super::failed("incomplete replay display"));
        }
        self.bytes[..4].copy_from_slice(b"SPLR");
        self.bytes[4..6].copy_from_slice(&(REPLAY_SCHEMA_VERSION as u16).to_le_bytes());
        self.bytes[6..8].copy_from_slice(&(REPLAY_HEADER_BYTES as u16).to_le_bytes());
        for (offset, value) in [
            (8, self.events),
            (12, REPLAY_RECORD_BYTES as u32),
            (16, self.depositions),
            (20, self.travels),
            (24, self.layers),
            (28, self.lifecycle),
        ] {
            self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for axis in 0..3 {
            self.bytes[32 + 8 * axis..40 + 8 * axis].copy_from_slice(&self.min[axis].to_le_bytes());
            self.bytes[56 + 8 * axis..64 + 8 * axis].copy_from_slice(&self.max[axis].to_le_bytes());
        }
        let bounds = AabbMm::new(self.min, self.max).map_err(|e| super::failed(e.message))?;
        Ok((self.bytes, bounds))
    }
}
