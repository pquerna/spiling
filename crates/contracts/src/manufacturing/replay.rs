// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! SPLR schema 6 admission. Associations are nominal plan data, not measurements.
use super::*;
use crate::ArtifactView;

pub const MAX_REPLAY_DISPLAY_BYTES: u32 = 32 * 1024 * 1024;
pub const MAX_REPLAY_EVENTS: u32 = 300_004;
pub const REPLAY_HEADER_BYTES: usize = 96;
pub const REPLAY_RECORD_BYTES: usize = 80;
pub const REPLAY_SCHEMA_VERSION: u32 = 6;
pub const REPLAY_MEDIA_TYPE: &str = "application/x-spiling-replay-motion";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutput {
    ReportOnly,
    ReportAndMotion,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingInspectionProvenance {
    pub project_id: ProjectId,
    pub input_revision: ProjectRevision,
    pub input_hash: SourceHash,
    pub surface_overlays: SurfaceOverlays,
    pub process_document: ProcessDocument,
    pub kernel: KernelIdentity,
    pub planner_version: String,
    pub emitter_version: String,
    pub verifier_version: String,
    pub workflow: JobWorkflow,
}
impl ManufacturingInspectionProvenance {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        match &self.workflow {
            JobWorkflow::DiagnosticBodyOnly => {}
            JobWorkflow::CompleteMarlin { job } => job.validate_declaration()?,
            JobWorkflow::FixedDualMarlin { job } => job.validate_declaration()?,
        }
        self.surface_overlays
            .validate()
            .map_err(|e| invalid(&e.message))?;
        self.process_document
            .validate()
            .map_err(|e| invalid(&e.message))?;
        for value in [
            &self.kernel.name,
            &self.kernel.version,
            &self.kernel.revision,
            &self.planner_version,
            &self.emitter_version,
            &self.verifier_version,
        ] {
            label(value)?;
        }
        Ok(())
    }
    pub fn validate_for_record(
        &self,
        record: &ManufacturingArtifactRecord,
    ) -> Result<(), ManufacturingError> {
        self.validate()?;
        record.validate()?;
        if self.input_hash != record.input_hash {
            return Err(corrupt("inspection provenance input mismatch"));
        }
        match (&self.workflow, &record.summary.routing) {
            (JobWorkflow::FixedDualMarlin { job }, Some(routing))
                if job
                    .tools
                    .iter()
                    .zip(&routing.tools)
                    .all(|(declared, decoded)| declared.material.id == decoded.material_id) => {}
            (JobWorkflow::DiagnosticBodyOnly | JobWorkflow::CompleteMarlin { .. }, None) => {}
            _ => {
                return Err(corrupt(
                    "inspection workflow disagrees with captured routing",
                ));
            }
        }
        Ok(())
    }
}
impl From<&ManufacturingProvenance> for ManufacturingInspectionProvenance {
    fn from(p: &ManufacturingProvenance) -> Self {
        Self {
            project_id: p.project_id.clone(),
            input_revision: p.input_revision,
            input_hash: p.input_hash.clone(),
            surface_overlays: p.surface_overlays.clone(),
            process_document: p.process_document.clone(),
            kernel: p.kernel.clone(),
            planner_version: p.planner_version.clone(),
            emitter_version: p.emitter_version.clone(),
            verifier_version: p.verifier_version.clone(),
            workflow: p.intent.printer.workflow.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReplayDisplayInfo {
    pub schema_version: u32,
    pub bundle_hash: SourceHash,
    pub input_hash: SourceHash,
    pub program_hash: SourceHash,
    pub verifier_version: String,
    pub frame: PrinterCoordinateFrame,
    pub event_count: u32,
    pub deposition_count: u32,
    pub travel_count: u32,
    pub layer_count: u32,
    pub lifecycle_count: u32,
    pub process_count: u32,
    pub tool_epoch_count: u32,
    pub waste_count: u32,
    pub bounds_mm: AabbMm,
}
impl ReplayDisplayInfo {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        label(&self.verifier_version)?;
        self.bounds_mm.validate().map_err(|e| corrupt(&e.message))?;
        if self.schema_version != REPLAY_SCHEMA_VERSION
            || self.event_count == 0
            || self.event_count > MAX_REPLAY_EVENTS
            || self.deposition_count == 0
            || self.deposition_count > MAX_DEPOSITION_SEGMENTS
            || self.travel_count == 0
            || self.travel_count > MAX_DEPOSITION_SEGMENTS
            || if self.tool_epoch_count == 0 {
                self.waste_count != 0 || !matches!(self.lifecycle_count, 0 | 4)
            } else {
                self.lifecycle_count != 2
                    || self.layer_count.checked_mul(2) != Some(self.tool_epoch_count)
                    || self.tool_epoch_count > MAX_TOOL_SELECTIONS
                    || self.waste_count != self.tool_epoch_count
                    || self.waste_count >= self.deposition_count
                    || self
                        .layer_count
                        .checked_mul(4)
                        .and_then(|n| n.checked_sub(2))
                        != Some(self.process_count)
            }
            || self.process_count > MAX_DEPOSITION_SEGMENTS
            || self
                .deposition_count
                .checked_add(self.travel_count)
                .and_then(|n| n.checked_add(self.lifecycle_count))
                .and_then(|n| n.checked_add(self.process_count))
                .and_then(|n| n.checked_add(self.tool_epoch_count))
                != Some(self.event_count)
            || self.layer_count == 0
            || self.layer_count > MAX_PRINT_LAYERS
        {
            return Err(corrupt("invalid replay schema or counts"));
        }
        Ok(())
    }
    pub fn byte_count(&self) -> Result<u32, ManufacturingError> {
        self.validate()?;
        let bytes = (self.event_count as usize)
            .checked_mul(REPLAY_RECORD_BYTES)
            .and_then(|n| n.checked_add(REPLAY_HEADER_BYTES))
            .ok_or_else(|| resource("replay size overflow"))?;
        if bytes > MAX_REPLAY_DISPLAY_BYTES as usize {
            return Err(resource("replay byte limit exceeded"));
        }
        Ok(bytes as u32)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReplayDisplayDescriptor {
    pub info: ReplayDisplayInfo,
    pub resource: ArtifactView,
}
impl ReplayDisplayDescriptor {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let size =
            crate::native::validate_artifact_view(&self.resource, MAX_REPLAY_DISPLAY_BYTES as u64)
                .map_err(|e| corrupt(&e))?;
        if self.resource.media_type != REPLAY_MEDIA_TYPE
            || size != u64::from(self.info.byte_count()?)
        {
            return Err(corrupt("replay resource MIME or size mismatch"));
        }
        Ok(())
    }
    pub fn validate_for_record(
        &self,
        record: &ManufacturingArtifactRecord,
    ) -> Result<(), ManufacturingError> {
        self.validate()?;
        record.validate()?;
        if self.info.bundle_hash != record.hash
            || self.info.input_hash != record.input_hash
            || self.info.layer_count != record.summary.layers
            || self.info.deposition_count
                != record.summary.deposition_segments
                    + record.summary.support_deposition_segments
                    + self.info.waste_count
        {
            return Err(corrupt("replay descriptor disagrees with bundle record"));
        }
        match &record.summary.routing {
            None if self.info.tool_epoch_count == 0 && self.info.waste_count == 0 => {}
            Some(routing)
                if self.info.tool_epoch_count == routing.selection_count
                    && self.info.waste_count
                        == routing
                            .tools
                            .iter()
                            .map(|tool| tool.waste_deposition_segments)
                            .sum::<u32>() => {}
            _ => return Err(corrupt("replay routing disagrees with bundle record")),
        }
        Ok(())
    }
}
fn corrupt(message: &str) -> ManufacturingError {
    ManufacturingError::new(ManufacturingErrorCode::CorruptArtifact, message)
}
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn f64_at(bytes: &[u8], offset: usize) -> f64 {
    f64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
fn e8_ticks(value: f64) -> Result<i64, ManufacturingError> {
    let scaled = (value * 100_000_000.0).round();
    if !scaled.is_finite() || scaled.abs() > (1_u64 << 52) as f64 || scaled / 100_000_000.0 != value
    {
        return Err(corrupt(
            "routed extrusion is outside the exact E8 tick domain",
        ));
    }
    Ok(scaled as i64)
}

/// Validate one bounded immutable artifact without allocating event objects.
pub fn validate_replay_display(
    bytes: &[u8],
    descriptor: &ReplayDisplayDescriptor,
) -> Result<(), ManufacturingError> {
    descriptor.validate()?;
    let info = &descriptor.info;
    if bytes.len() != info.byte_count()? as usize
        || SourceHash::from_bytes(bytes).as_str() != descriptor.resource.sha256
    {
        return Err(corrupt("replay bytes size or hash mismatch"));
    }
    if &bytes[..4] != b"SPLR"
        || u16_at(bytes, 4) != REPLAY_SCHEMA_VERSION as u16
        || u16_at(bytes, 6) != 96
        || u32_at(bytes, 8) != info.event_count
        || u32_at(bytes, 12) != REPLAY_RECORD_BYTES as u32
        || u32_at(bytes, 16) != info.deposition_count
        || u32_at(bytes, 20) != info.travel_count
        || u32_at(bytes, 24) != info.layer_count
        || u32_at(bytes, 28) != info.lifecycle_count
        || bytes[80..88].iter().any(|b| *b != 0)
        || u32_at(bytes, 88) != info.process_count
        || u32_at(bytes, 92) != info.tool_epoch_count
    {
        return Err(corrupt("replay header mismatch"));
    }
    for axis in 0..3 {
        if f64_at(bytes, 32 + 8 * axis) != info.bounds_mm.min[axis]
            || f64_at(bytes, 56 + 8 * axis) != info.bounds_mm.max[axis]
        {
            return Err(corrupt("replay header bounds mismatch"));
        }
    }
    let mut previous = [0.0; 3];
    let mut previous_e = 0.0;
    let mut previous_line = 0;
    let mut layer = 0;
    let mut layer_deposition = false;
    let mut depositions = 0;
    let mut travels = 0;
    let mut lifecycle = 0;
    let mut lifecycle_phase = 0;
    let mut process = 0;
    let mut retracted_e = None;
    let dual = info.tool_epoch_count != 0;
    let mut active_tool = 0usize;
    let mut debt = [0i64; 2];
    let mut epochs = 0u32;
    let mut waste = 0u32;
    let mut epoch_waste = false;
    let mut epoch_model = false;
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for (index, record) in bytes[REPLAY_HEADER_BYTES..]
        .as_chunks::<REPLAY_RECORD_BYTES>()
        .0
        .iter()
        .enumerate()
    {
        let position = [f64_at(record, 0), f64_at(record, 8), f64_at(record, 16)];
        let e = f64_at(record, 24);
        let feed = f64_at(record, 32);
        let line = u32_at(record, 40);
        let next_layer = u16_at(record, 44) as u32;
        let kind = record[46];
        let flags = record[47];
        let tool = u32_at(record, 76);
        if position.iter().any(|v| !v.is_finite())
            || !e.is_finite()
            || (e < 0.0
                && kind != 11
                && if dual {
                    debt[active_tool] == 0
                } else {
                    retracted_e.is_none()
                })
            || !feed.is_finite()
            || if kind == 14 { feed != 0.0 } else { feed <= 0.0 }
            || line <= previous_line
            || line > MAX_MANUFACTURING_BUNDLE_BYTES
            || (!(6..8).contains(&kind) && next_layer >= info.layer_count)
            || kind > 15
            || flags
                != (u8::from(index != 0 && kind != 14)
                    | if matches!(kind, 1..=5 | 8..=10 | 13 | 15) {
                        2
                    } else {
                        0
                    })
            || (dual && (tool > 1 || (kind != 14 && tool as usize != active_tool)))
            || (!dual && (tool != 0 || kind >= 14))
            || (index == 0
                && if dual {
                    kind != 14
                } else {
                    kind != 0 && kind != 6
                })
            || (index == 0 && kind == 0 && next_layer != 0)
        {
            return Err(corrupt("invalid replay record fields"));
        }
        let width = f64_at(record, 48);
        let lower = f64_at(record, 56);
        let upper = f64_at(record, 64);
        let owner = u32_at(record, 72);
        if kind == 15 && owner != u32::MAX {
            return Err(corrupt("waste cannot claim model ownership"));
        }
        if flags & 2 != 0 {
            if !width.is_finite()
                || width <= 0.0
                || !lower.is_finite()
                || !upper.is_finite()
                || lower >= upper
                || (upper - position[2]).abs() > 0.000_000_500_001
                || (owner < u32::MAX - 2 && owner as usize >= MAX_PROCESS_REGIONS)
            {
                return Err(corrupt("invalid nominal deposition association"));
            }
        } else if record[48..72].iter().any(|b| *b != 0) || owner != u32::MAX {
            return Err(corrupt("nondeposition has an association"));
        }
        if dual {
            if kind == 14 {
                if lifecycle != 0
                    || epochs >= info.tool_epoch_count
                    || tool != epochs % 2
                    || next_layer != epochs / 2
                    || e != 0.0
                    || (epochs != 0 && (!epoch_waste || !epoch_model || debt[active_tool] <= 0))
                {
                    return Err(corrupt("invalid fixed-dual tool epoch"));
                }
                active_tool = tool as usize;
                layer = next_layer;
                epochs += 1;
                epoch_waste = false;
                epoch_model = false;
                layer_deposition = false;
            } else if kind == 6 {
                if next_layer != u16::MAX as u32
                    || lifecycle >= 2
                    || depositions != info.deposition_count
                    || travels != info.travel_count
                    || epochs != info.tool_epoch_count
                    || !epoch_waste
                    || !epoch_model
                    || debt.iter().any(|value| *value <= 0)
                    || e != previous_e
                    || (lifecycle == 0
                        && (position[0] != previous[0]
                            || position[1] != previous[1]
                            || position[2] <= previous[2]))
                    || (lifecycle == 1 && position[2] != previous[2])
                {
                    return Err(corrupt("invalid fixed-dual final lift/park"));
                }
                lifecycle += 1;
            } else {
                if lifecycle != 0
                    || epochs == 0
                    || kind == 7
                    || next_layer < layer
                    || next_layer > layer + 1
                    || (next_layer != layer
                        && !(tool == 1 && epoch_model && matches!(kind, 0 | 11)))
                {
                    return Err(corrupt("motion outside fixed-dual tool epoch"));
                }
                match kind {
                    0 => {
                        if e != previous_e {
                            return Err(corrupt("travel changes extrusion"));
                        }
                        travels += 1;
                    }
                    11 => {
                        if position != previous
                            || e >= previous_e
                            || debt[active_tool] != 0
                            || !epoch_model
                        {
                            return Err(corrupt("invalid fixed-dual retraction"));
                        }
                        debt[active_tool] = e8_ticks(previous_e)? - e8_ticks(e)?;
                        process += 1;
                    }
                    12 => {
                        if position != previous
                            || e <= previous_e
                            || debt[active_tool] <= 0
                            || e8_ticks(e)? - e8_ticks(previous_e)? != debt[active_tool]
                            || epoch_waste
                            || epoch_model
                        {
                            return Err(corrupt("invalid fixed-dual recovery"));
                        }
                        debt[active_tool] = 0;
                        process += 1;
                    }
                    _ => {
                        if next_layer != layer
                            || e <= previous_e
                            || position == previous
                            || debt[active_tool] != 0
                            || (kind == 15 && (epoch_waste || epoch_model))
                            || (kind != 15 && !epoch_waste)
                            || matches!(kind, 9 | 10 | 13)
                        {
                            return Err(corrupt("invalid fixed-dual model or waste deposition"));
                        }
                        if kind == 15 {
                            waste += 1;
                            epoch_waste = true;
                        } else {
                            epoch_model = true;
                        }
                        depositions += 1;
                        layer_deposition = true;
                    }
                }
            }
        } else if kind == 6 || kind == 7 {
            if info.lifecycle_count != 4 || next_layer != u16::MAX as u32 {
                return Err(corrupt("invalid lifecycle replay attribution"));
            }
            let required_kind = if lifecycle_phase == 1 { 7 } else { 6 };
            if kind != required_kind
                || lifecycle_phase >= 4
                || (lifecycle_phase < 2 && depositions != 0)
                || (lifecycle_phase >= 2
                    && (depositions != info.deposition_count || travels != info.travel_count))
                || retracted_e.is_some()
            {
                return Err(corrupt("invalid lifecycle replay sequence"));
            }
            if (lifecycle_phase == 2
                && (position[0] != previous[0]
                    || position[1] != previous[1]
                    || position[2] <= previous[2]))
                || (lifecycle_phase == 3 && position[2] != previous[2])
            {
                return Err(corrupt("invalid final lift/park replay"));
            }
            if kind == 7 {
                if index == 0 || e <= previous_e || position == previous {
                    return Err(corrupt("invalid priming replay"));
                }
            } else if e != previous_e {
                return Err(corrupt("lifecycle travel changes extrusion"));
            }
            lifecycle += 1;
            lifecycle_phase += 1;
        } else {
            if info.lifecycle_count == 4 && lifecycle_phase != 2 {
                return Err(corrupt("plan motion outside lifecycle body"));
            }
            let startup_retraction = kind == 11
                && process == 0
                && info.lifecycle_count == 4
                && lifecycle_phase == 2
                && index > 0;
            if depositions == 0
                && travels == 0
                && ((!startup_retraction && kind != 0) || next_layer != 0)
            {
                return Err(corrupt(
                    "body must start with layer-zero travel or complete-job retraction",
                ));
            }
            if next_layer != layer {
                if next_layer <= layer
                    || (travels != 0 && !layer_deposition)
                    || (kind != 0 && kind != 11)
                {
                    return Err(corrupt("invalid replay layer sequence"));
                }
                layer = next_layer;
                layer_deposition = false;
            }
            if kind == 0 {
                if e != previous_e {
                    return Err(corrupt("travel changes extrusion"));
                }
                travels += 1;
            } else if kind == 11 || kind == 12 {
                if index == 0 || position != previous {
                    return Err(corrupt("process extrusion must be known and stationary"));
                }
                if kind == 11 {
                    if retracted_e.is_some() || e >= previous_e {
                        return Err(corrupt("invalid replay retraction"));
                    }
                    retracted_e = Some(previous_e);
                } else {
                    if e <= previous_e || retracted_e != Some(e) {
                        return Err(corrupt("invalid replay recovery"));
                    }
                    retracted_e = None;
                }
                process += 1;
            } else {
                if index == 0 || e <= previous_e || position == previous || retracted_e.is_some() {
                    return Err(corrupt("invalid replay deposition"));
                }
                depositions += 1;
                layer_deposition = true;
            }
        }
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
        previous = position;
        previous_e = e;
        previous_line = line;
    }
    if !layer_deposition
        || layer + 1 != info.layer_count
        || depositions != info.deposition_count
        || travels != info.travel_count
        || lifecycle != info.lifecycle_count
        || process != info.process_count
        || retracted_e.is_some()
        || (dual
            && (epochs != info.tool_epoch_count
                || waste != info.waste_count
                || process != info.layer_count * 4 - 2
                || !epoch_waste
                || !epoch_model
                || debt.iter().any(|value| *value <= 0)))
        || min != info.bounds_mm.min
        || max != info.bounds_mm.max
    {
        return Err(corrupt("replay aggregate counts or bounds mismatch"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded() -> (Vec<u8>, ReplayDisplayDescriptor) {
        let mut bytes = vec![0; REPLAY_HEADER_BYTES];
        bytes[..4].copy_from_slice(b"SPLR");
        bytes[4..6].copy_from_slice(&(REPLAY_SCHEMA_VERSION as u16).to_le_bytes());
        bytes[6..8].copy_from_slice(&96u16.to_le_bytes());
        for (offset, value) in [(8, 4u32), (12, 80), (16, 2), (20, 2), (24, 1)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for axis in 0..3 {
            bytes[56 + axis * 8..64 + axis * 8].copy_from_slice(&1.0f64.to_le_bytes());
        }
        // Unknown first endpoint, deposition, known coincident travel, deposition.
        for (index, (position, e, kind)) in [
            ([0.0f64; 3], 0.0f64, 0u8),
            ([1.0; 3], 1.0, 1),
            ([1.0; 3], 1.0, 0),
            ([0.0; 3], 2.0, 2),
        ]
        .into_iter()
        .enumerate()
        {
            for value in position {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&e.to_le_bytes());
            bytes.extend_from_slice(&600.0f64.to_le_bytes());
            bytes.extend_from_slice(&(index as u32 + 5).to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            append_association(&mut bytes, index, kind, position[2]);
        }
        let hash = SourceHash::from_bytes(&bytes);
        let descriptor = ReplayDisplayDescriptor {
            info: ReplayDisplayInfo {
                schema_version: REPLAY_SCHEMA_VERSION,
                bundle_hash: SourceHash::from_bytes(b"bundle"),
                input_hash: SourceHash::from_bytes(b"input"),
                program_hash: SourceHash::from_bytes(b"program"),
                verifier_version: "1".into(),
                frame: PrinterCoordinateFrame::RightHandedMillimetres,
                event_count: 4,
                deposition_count: 2,
                travel_count: 2,
                layer_count: 1,
                lifecycle_count: 0,
                process_count: 0,
                tool_epoch_count: 0,
                waste_count: 0,
                bounds_mm: AabbMm::new([0.0; 3], [1.0; 3]).unwrap(),
            },
            resource: ArtifactView {
                name: format!("artifacts/{}", hash.as_str()),
                sha256: hash.as_str().into(),
                size_bytes: bytes.len().to_string(),
                media_type: REPLAY_MEDIA_TYPE.into(),
            },
        };
        (bytes, descriptor)
    }
    fn append_association(bytes: &mut Vec<u8>, index: usize, kind: u8, z: f64) {
        let associated = matches!(kind, 1..=5 | 8..=10 | 13);
        bytes.extend_from_slice(&[kind, u8::from(index != 0) | if associated { 2 } else { 0 }]);
        for value in if associated {
            [0.45, z - 0.2, z]
        } else {
            [0.0; 3]
        } {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
    }
    #[test]
    fn fixed_dual_epochs_break_frames_and_keep_waste_explicit() {
        let (body, mut descriptor) = encoded();
        let mut bytes = body[..REPLAY_HEADER_BYTES].to_vec();
        let events = [
            ([0.0, 0.0, 0.0], 0.0, 14, 0),
            ([0.0, 0.0, 1.0], 0.0, 0, 0),
            ([1.0, 0.0, 1.0], 1.0, 15, 0),
            ([1.0, 1.0, 1.0], 1.0, 0, 0),
            ([0.0, 1.0, 1.0], 2.0, 1, 0),
            ([0.0, 1.0, 1.0], 1.5, 11, 0),
            ([0.0, 1.0, 2.0], 1.5, 0, 0),
            ([2.0, 1.0, 2.0], 0.0, 14, 1),
            ([2.0, 0.0, 1.0], 0.0, 0, 1),
            ([3.0, 0.0, 1.0], 1.0, 15, 1),
            ([3.0, 1.0, 1.0], 1.0, 0, 1),
            ([2.0, 1.0, 1.0], 2.0, 2, 1),
            ([2.0, 1.0, 1.0], 1.5, 11, 1),
            ([2.0, 1.0, 3.0], 1.5, 6, 1),
            ([3.0, 1.0, 3.0], 1.5, 6, 1),
        ];
        for (index, (position, e, kind, tool)) in events.into_iter().enumerate() {
            for coordinate in position {
                bytes.extend_from_slice(&f64::to_le_bytes(coordinate));
            }
            bytes.extend_from_slice(&f64::to_le_bytes(e));
            let feed = if kind == 14 { 0.0f64 } else { 600.0f64 };
            bytes.extend_from_slice(&feed.to_le_bytes());
            bytes.extend_from_slice(&(index as u32 + 1).to_le_bytes());
            let layer = if kind == 6 { u16::MAX } else { 0 };
            bytes.extend_from_slice(&layer.to_le_bytes());
            let associated = matches!(kind, 1 | 2 | 15);
            bytes.extend_from_slice(&[
                kind,
                u8::from(index != 0 && kind != 14) | if associated { 2 } else { 0 },
            ]);
            for value in if associated {
                [0.45, position[2] - 0.2, position[2]]
            } else {
                [0.0; 3]
            } {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&u32::MAX.to_le_bytes());
            bytes.extend_from_slice(&u32::to_le_bytes(tool));
        }
        for (offset, value) in [(8, 15u32), (16, 4), (20, 5), (28, 2), (88, 2), (92, 2)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let bounds = AabbMm::new([0.0; 3], [3.0, 1.0, 3.0]).unwrap();
        for axis in 0..3 {
            bytes[56 + axis * 8..64 + axis * 8].copy_from_slice(&bounds.max[axis].to_le_bytes());
        }
        descriptor.info.event_count = 15;
        descriptor.info.deposition_count = 4;
        descriptor.info.travel_count = 5;
        descriptor.info.lifecycle_count = 2;
        descriptor.info.process_count = 2;
        descriptor.info.tool_epoch_count = 2;
        descriptor.info.waste_count = 2;
        descriptor.info.bounds_mm = bounds;
        descriptor.resource.size_bytes = bytes.len().to_string();
        rebind(&bytes, &mut descriptor);
        validate_replay_display(&bytes, &descriptor).unwrap();
        for (record, offset, value) in [
            (0, 47, 1u32),
            (1, 76, 1),
            (2, 72, 0),
            (7, 76, 0),
            (7, 47, 1),
            (9, 46, 1),
            (12, 46, 0),
        ] {
            let mut bad = bytes.clone();
            let start = REPLAY_HEADER_BYTES + record * REPLAY_RECORD_BYTES + offset;
            if offset >= 72 {
                bad[start..start + 4].copy_from_slice(&value.to_le_bytes());
            } else {
                bad[start] = value as u8;
            }
            let mut rebound = descriptor.clone();
            rebind(&bad, &mut rebound);
            assert!(
                validate_replay_display(&bad, &rebound).is_err(),
                "record {record}, offset {offset}"
            );
        }
        let mut old = bytes.clone();
        old[4..6].copy_from_slice(&5u16.to_le_bytes());
        rebind(&old, &mut descriptor);
        assert!(validate_replay_display(&old, &descriptor).is_err());
    }
    #[test]
    fn nominal_associations_require_deposition_and_bounded_finite_dimensions() {
        let (bytes, descriptor) = encoded();
        for (record, offset, value) in [
            (0, 48, 1.0f64),
            (1, 48, 0.0),
            (1, 56, f64::NEG_INFINITY),
            (1, 64, 42.0),
        ] {
            let mut bad = bytes.clone();
            let start = REPLAY_HEADER_BYTES + record * REPLAY_RECORD_BYTES + offset;
            bad[start..start + 8].copy_from_slice(&value.to_le_bytes());
            let mut rebound = descriptor.clone();
            rebind(&bad, &mut rebound);
            assert!(validate_replay_display(&bad, &rebound).is_err());
        }
        for (record, owner) in [(0, 0), (1, MAX_PROCESS_REGIONS as u32)] {
            let mut bad = bytes.clone();
            let start = REPLAY_HEADER_BYTES + record * REPLAY_RECORD_BYTES + 72;
            bad[start..start + 4].copy_from_slice(&owner.to_le_bytes());
            let mut rebound = descriptor.clone();
            rebind(&bad, &mut rebound);
            assert!(validate_replay_display(&bad, &rebound).is_err());
        }
    }
    fn rebind(bytes: &[u8], descriptor: &mut ReplayDisplayDescriptor) {
        let hash = SourceHash::from_bytes(bytes);
        descriptor.resource.name = format!("artifacts/{}", hash.as_str());
        descriptor.resource.sha256 = hash.as_str().into();
    }
    #[test]
    fn complete_body_startup_retraction_uses_known_prime_endpoint() {
        let (body, mut descriptor) = encoded();
        let mut bytes = body[..96].to_vec();
        let records = [
            ([0.0f64; 3], 0.0f64, 6u8, u16::MAX),
            ([0.0, 1.0, 0.0], 1.0, 7, u16::MAX),
            ([0.0, 1.0, 0.0], 0.2, 11, 0),
            ([0.0; 3], 0.2, 0, 0),
            ([0.0; 3], 1.0, 12, 0),
            ([1.0; 3], 2.0, 1, 0),
            ([1.0; 3], 2.0, 0, 0),
            ([0.0; 3], 3.0, 2, 0),
            ([0.0, 0.0, 2.0], 3.0, 6, u16::MAX),
            ([1.0, 0.0, 2.0], 3.0, 6, u16::MAX),
        ];
        for (index, (position, e, kind, layer)) in records.into_iter().enumerate() {
            for coordinate in position {
                bytes.extend_from_slice(&coordinate.to_le_bytes());
            }
            bytes.extend_from_slice(&e.to_le_bytes());
            bytes.extend_from_slice(&600.0f64.to_le_bytes());
            bytes.extend_from_slice(&(index as u32 + 1).to_le_bytes());
            bytes.extend_from_slice(&layer.to_le_bytes());
            append_association(&mut bytes, index, kind, position[2]);
        }
        for (offset, value) in [(8, 10u32), (28, 4), (88, 2)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[72..80].copy_from_slice(&2.0f64.to_le_bytes());
        descriptor.info.event_count = 10;
        descriptor.info.lifecycle_count = 4;
        descriptor.info.process_count = 2;
        descriptor.info.bounds_mm.max[2] = 2.0;
        descriptor.resource.size_bytes = bytes.len().to_string();
        rebind(&bytes, &mut descriptor);
        validate_replay_display(&bytes, &descriptor).unwrap();
        for (offset, value) in [(256 + 47, 0u8), (256 + 44, 1), (256 + 46, 12)] {
            let mut bad = bytes.clone();
            bad[offset] = value;
            let mut rebound = descriptor.clone();
            rebind(&bad, &mut rebound);
            assert!(validate_replay_display(&bad, &rebound).is_err());
        }
        for (offset, value) in [(256, 0.1f64), (416 + 24, 1.1)] {
            let mut bad = bytes.clone();
            bad[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            let mut rebound = descriptor.clone();
            rebind(&bad, &mut rebound);
            assert!(validate_replay_display(&bad, &rebound).is_err());
        }
    }
    #[test]
    fn stationary_process_roles_and_exact_recovery_are_checked() {
        let (mut bytes, mut descriptor) = encoded();
        let mut events = Vec::with_capacity(bytes.len() + 160);
        events.extend_from_slice(&bytes[..176]);
        for (e, kind, line) in [(-0.8f64, 11u8, 6u32), (0.0, 12, 7)] {
            let mut record = bytes[96..176].to_vec();
            record[24..32].copy_from_slice(&e.to_le_bytes());
            record[40..44].copy_from_slice(&line.to_le_bytes());
            record[46] = kind;
            record[47] = 1;
            events.extend_from_slice(&record);
        }
        for record in bytes[176..].as_chunks_mut::<80>().0 {
            let line = u32_at(record, 40) + 2;
            record[40..44].copy_from_slice(&line.to_le_bytes());
        }
        events.extend_from_slice(&bytes[176..]);
        events[8..12].copy_from_slice(&6u32.to_le_bytes());
        events[88..92].copy_from_slice(&2u32.to_le_bytes());
        descriptor.info.event_count = 6;
        descriptor.info.process_count = 2;
        descriptor.resource.size_bytes = events.len().to_string();
        rebind(&events, &mut descriptor);
        validate_replay_display(&events, &descriptor).unwrap();
        for (offset, value) in [(176, 0.1f64), (176 + 24, 0.0), (256 + 24, 0.1)] {
            let mut corrupted = events.clone();
            corrupted[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            let mut rebound = descriptor.clone();
            rebind(&corrupted, &mut rebound);
            assert!(validate_replay_display(&corrupted, &rebound).is_err());
        }
        for kind in [8, 9, 10, 13] {
            let mut role = events.clone();
            role[336 + 46] = kind;
            let mut rebound = descriptor.clone();
            rebind(&role, &mut rebound);
            validate_replay_display(&role, &rebound).unwrap();
        }
    }
    #[test]
    fn packed_admission_checks_semantics_even_with_rehashed_corruption() {
        let (bytes, descriptor) = encoded();
        validate_replay_display(&bytes, &descriptor).unwrap();
        for (offset, value) in [
            (28, 1),
            (80, 1),
            (96 + 47, 1),
            (96 + 80 + 47, 0),
            (96 + 46, 1),
            (96 + 2 * 80 + 46, 3),
            (96 + 2 * 80 + 44, 1),
            (96 + 80 + 46, 14),
        ] {
            let mut corrupted = bytes.clone();
            corrupted[offset] = value;
            let mut rebound = descriptor.clone();
            rebind(&corrupted, &mut rebound);
            assert!(
                validate_replay_display(&corrupted, &rebound).is_err(),
                "offset {offset}"
            );
        }
        for (offset, value) in [
            (96, f64::NAN),
            (96 + 32, 0.0),
            (96 + 2 * 80 + 24, 1.5),
            (96 + 80 + 24, 0.0),
        ] {
            let mut corrupted = bytes.clone();
            corrupted[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            let mut rebound = descriptor.clone();
            rebind(&corrupted, &mut rebound);
            assert!(
                validate_replay_display(&corrupted, &rebound).is_err(),
                "offset {offset}"
            );
        }
        let mut old = bytes.clone();
        old[4..6].copy_from_slice(&3u16.to_le_bytes());
        let mut old_descriptor = descriptor.clone();
        rebind(&old, &mut old_descriptor);
        assert!(validate_replay_display(&old, &old_descriptor).is_err());
        old_descriptor.info.schema_version = 3;
        assert!(old_descriptor.validate().is_err());
        let mut corrupted = bytes.clone();
        corrupted[96 + 80 + 40..96 + 80 + 44].copy_from_slice(&5u32.to_le_bytes());
        let mut rebound = descriptor.clone();
        rebind(&corrupted, &mut rebound);
        assert!(validate_replay_display(&corrupted, &rebound).is_err());
        let mut wrong_bounds = descriptor.clone();
        wrong_bounds.info.bounds_mm.max[0] = 2.0;
        assert!(validate_replay_display(&bytes, &wrong_bounds).is_err());
        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(validate_replay_display(&truncated, &descriptor).is_err());
    }
}
