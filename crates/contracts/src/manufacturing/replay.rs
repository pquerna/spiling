// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! SPLR schema 2 admission. This is display integrity, not program verification.
use super::*;
use crate::ArtifactView;

pub const MAX_REPLAY_DISPLAY_BYTES: u32 = 16 * 1024 * 1024;
pub const MAX_REPLAY_EVENTS: u32 = 200_004;
pub const REPLAY_HEADER_BYTES: usize = 96;
pub const REPLAY_RECORD_BYTES: usize = 48;
pub const REPLAY_SCHEMA_VERSION: u32 = 2;
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
    pub kernel: KernelIdentity,
    pub planner_version: String,
    pub emitter_version: String,
    pub verifier_version: String,
}
impl ManufacturingInspectionProvenance {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
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
        Ok(())
    }
}
impl From<&ManufacturingProvenance> for ManufacturingInspectionProvenance {
    fn from(p: &ManufacturingProvenance) -> Self {
        Self {
            project_id: p.project_id.clone(),
            input_revision: p.input_revision,
            input_hash: p.input_hash.clone(),
            kernel: p.kernel.clone(),
            planner_version: p.planner_version.clone(),
            emitter_version: p.emitter_version.clone(),
            verifier_version: p.verifier_version.clone(),
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
            || (self.lifecycle_count != 0 && self.lifecycle_count != 4)
            || self
                .deposition_count
                .checked_add(self.travel_count)
                .and_then(|n| n.checked_add(self.lifecycle_count))
                != Some(self.event_count)
            || self.layer_count == 0
            || self.layer_count > MAX_PRINT_LAYERS
            || self.layer_count > self.deposition_count
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
            || self.info.deposition_count != record.summary.deposition_segments
            || self.info.travel_count != record.summary.paths
        {
            return Err(corrupt("replay descriptor disagrees with bundle record"));
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
        || u32_at(bytes, 12) != 48
        || u32_at(bytes, 16) != info.deposition_count
        || u32_at(bytes, 20) != info.travel_count
        || u32_at(bytes, 24) != info.layer_count
        || u32_at(bytes, 28) != info.lifecycle_count
        || bytes[80..96].iter().any(|b| *b != 0)
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
        if position.iter().any(|v| !v.is_finite())
            || !e.is_finite()
            || e < 0.0
            || !feed.is_finite()
            || feed <= 0.0
            || line <= previous_line
            || line > MAX_MANUFACTURING_BUNDLE_BYTES
            || (kind < 6 && next_layer >= info.layer_count)
            || kind > 7
            || flags != u8::from(index != 0)
            || (index == 0 && (kind != 0 && kind != 6))
            || (index == 0 && kind == 0 && next_layer != 0)
        {
            return Err(corrupt("invalid replay record fields"));
        }
        if kind >= 6 {
            if info.lifecycle_count != 4 || next_layer != u16::MAX as u32 {
                return Err(corrupt("invalid lifecycle replay attribution"));
            }
            let required_kind = if lifecycle_phase == 1 { 7 } else { 6 };
            if kind != required_kind
                || lifecycle_phase >= 4
                || (lifecycle_phase < 2 && depositions != 0)
                || (lifecycle_phase >= 2
                    && (depositions != info.deposition_count || travels != info.travel_count))
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
            if depositions == 0 && travels == 0 && (kind != 0 || next_layer != 0) {
                return Err(corrupt("body must start with layer-zero travel"));
            }
            if next_layer != layer {
                if next_layer != layer + 1 || !layer_deposition || kind != 0 {
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
            } else {
                if index == 0 || e <= previous_e || position == previous {
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
        for (offset, value) in [(8, 4u32), (12, 48), (16, 2), (20, 2), (24, 1)] {
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
            bytes.extend_from_slice(&[kind, u8::from(index != 0)]);
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
    fn rebind(bytes: &[u8], descriptor: &mut ReplayDisplayDescriptor) {
        let hash = SourceHash::from_bytes(bytes);
        descriptor.resource.name = format!("artifacts/{}", hash.as_str());
        descriptor.resource.sha256 = hash.as_str().into();
    }
    #[test]
    fn packed_admission_checks_semantics_even_with_rehashed_corruption() {
        let (bytes, descriptor) = encoded();
        validate_replay_display(&bytes, &descriptor).unwrap();
        for (offset, value) in [
            (28, 1),
            (80, 1),
            (96 + 47, 1),
            (96 + 48 + 47, 0),
            (96 + 46, 1),
            (96 + 2 * 48 + 46, 3),
            (96 + 2 * 48 + 44, 1),
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
            (96 + 2 * 48 + 24, 1.5),
            (96 + 48 + 24, 0.0),
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
        let mut corrupted = bytes.clone();
        corrupted[96 + 48 + 40..96 + 48 + 44].copy_from_slice(&5u32.to_le_bytes());
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
