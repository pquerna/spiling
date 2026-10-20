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
    process: u32,
    expected_events: u32,
}
impl ReplaySink {
    pub(crate) fn new(
        plan: &NormalizedPrintPlan,
        intent: &ManufacturingIntent,
    ) -> Result<Self, ManufacturingError> {
        plan.validate(intent)?;
        let deposition: usize = plan
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .map(|p| p.points_mm.len() - 1)
            .sum();
        let paths: usize = plan.layers.iter().map(|l| l.paths.len()).sum();
        let (travels, process) = match intent.recipe.process.retraction {
            RetractionPolicy::Disabled => (paths, 0),
            RetractionPolicy::Enabled { z_hop_mm, .. } => (
                (paths * if z_hop_mm > 0.0 { 3 } else { 1 }).min(100_000),
                (paths * 2).min(100_000),
            ),
        };
        let lifecycle = if matches!(intent.printer.workflow, JobWorkflow::CompleteMarlin { .. }) {
            4
        } else {
            0
        };
        let events = deposition
            .checked_add(travels)
            .and_then(|n| n.checked_add(process))
            .and_then(|n| n.checked_add(lifecycle))
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
            process: 0,
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
        self.push_associated(position, e, feed, line, layer, kind, None)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_associated(
        &mut self,
        position: [f64; 3],
        e: f64,
        feed: f64,
        line: u32,
        layer: u16,
        kind: u8,
        association: Option<super::expected::Association>,
    ) -> Result<(), ManufacturingError> {
        let count = match kind {
            0 => self.travels,
            6 | 7 => self.lifecycle,
            11 | 12 => self.process,
            _ => self.depositions,
        };
        let max = if matches!(kind, 6 | 7) { 4 } else { 100_000 };
        if count >= max {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "replay category event budget exceeded",
            ));
        }
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
        self.bytes
            .push(u8::from(self.events != 0) | if association.is_some() { 2 } else { 0 });
        let (width, physical_z, owner) = match association {
            Some(association) => {
                let physical_z = association.physical_z;
                let owner = match association.owner {
                    None => u32::MAX,
                    Some(DepositionOwner::MandatorySkin { .. }) => u32::MAX - 1,
                    Some(DepositionOwner::Regional { region_index }) => u32::from(region_index),
                    Some(DepositionOwner::RegionalSkinBridge { .. }) => {
                        return Err(super::failed(
                            "mixed path ownership must be projected to each actual deposition segment",
                        ));
                    }
                };
                (association.width, physical_z, owner)
            }
            None => (0.0, [0.0; 2], u32::MAX),
        };
        self.bytes.extend_from_slice(&width.to_le_bytes());
        for z in physical_z {
            self.bytes.extend_from_slice(&z.to_le_bytes());
        }
        self.bytes.extend_from_slice(&owner.to_le_bytes());
        self.bytes.extend_from_slice(&0u32.to_le_bytes());
        for (axis, value) in position.iter().enumerate() {
            self.min[axis] = self.min[axis].min(*value);
            self.max[axis] = self.max[axis].max(*value);
        }
        self.events += 1;
        if matches!(kind, 6 | 7) {
            self.lifecycle += 1;
        } else if matches!(kind, 11 | 12) {
            self.process += 1;
        } else if kind == 0 {
            self.travels += 1;
        } else {
            self.depositions += 1;
        }
        Ok(())
    }
    pub(crate) fn finish(mut self) -> Result<(Vec<u8>, AabbMm), ManufacturingError> {
        if self.events == 0 || !self.process.is_multiple_of(2) {
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
            (88, self.process),
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
