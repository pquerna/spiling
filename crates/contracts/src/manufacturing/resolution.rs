// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::{MAX_PRINT_LAYERS, ManufacturingError, invalid, resource};
use crate::{
    geometry::{FaceId, OccurrenceId, SourceHash},
    surface::{SurfaceFinish, SurfaceLayerId, SurfaceOverlays},
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const MAX_SURFACE_RESOLUTION_REPRESENTATIVES: usize = 8;
pub const MAX_SURFACE_RESOLUTION_RECORDS: usize =
    MAX_PRINT_LAYERS as usize * crate::surface::MAX_SURFACE_LAYERS;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceFeatureClass {
    Resolved,
    Ambiguous,
    BelowNominalResolution,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFeatureResolution {
    pub source_u: f64,
    pub source_z_mm: f64,
    pub cell: u32,
    pub repeat: u32,
    pub classification: SurfaceFeatureClass,
    pub residual_lower_mm: f64,
    pub residual_upper_mm: f64,
}
impl SurfaceFeatureResolution {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if !self.source_u.is_finite()
            || !(0.0..1.0).contains(&self.source_u)
            || !self.source_z_mm.is_finite()
            || !self.residual_lower_mm.is_finite()
            || !self.residual_upper_mm.is_finite()
            || self.residual_lower_mm < 0.0
            || self.residual_lower_mm > self.residual_upper_mm
        {
            return Err(invalid(
                "invalid feature resolution coordinates or residual interval",
            ));
        }
        let valid = match self.classification {
            SurfaceFeatureClass::Resolved => self.residual_upper_mm == 0.0,
            SurfaceFeatureClass::Ambiguous => {
                self.residual_lower_mm == 0.0 && self.residual_upper_mm > 0.0
            }
            SurfaceFeatureClass::BelowNominalResolution => self.residual_lower_mm > 0.0,
            SurfaceFeatureClass::Unavailable => {
                self.residual_lower_mm == 0.0 && self.residual_upper_mm == 0.0
            }
        };
        if !valid {
            return Err(invalid(
                "feature resolution classification disagrees with residual interval",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceResolution {
    pub surface_layer_id: SurfaceLayerId,
    pub occurrence_id: OccurrenceId,
    pub face_id: FaceId,
    pub image_hash: SourceHash,
    pub layer_index: u32,
    pub source_z_mm: f64,
    pub evaluated_witnesses: u32,
    pub resolved: u32,
    pub ambiguous: u32,
    pub below_nominal_resolution: u32,
    pub unavailable: u32,
    pub coverage_complete: bool,
    pub representatives_truncated: bool,
    pub max_residual_lower_mm: f64,
    pub max_residual_upper_mm: f64,
    #[serde(deserialize_with = "representatives")]
    pub representatives: Vec<SurfaceFeatureResolution>,
}
fn representatives<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SurfaceFeatureResolution>, D::Error> {
    super::bounded_vec::<D, _, MAX_SURFACE_RESOLUTION_REPRESENTATIVES>(d)
}
pub(super) fn records<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SurfaceResolution>, D::Error> {
    struct Records;
    impl<'de> serde::de::Visitor<'de> for Records {
        type Value = Vec<SurfaceResolution>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded surface resolution records")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut records = Vec::new();
            let mut nested_bytes = 0usize;
            while let Some(record) = seq.next_element::<SurfaceResolution>()? {
                record.validate().map_err(serde::de::Error::custom)?;
                nested_bytes += record.allocated_bytes() - std::mem::size_of::<SurfaceResolution>();
                if records.len() == MAX_SURFACE_RESOLUTION_RECORDS {
                    return Err(serde::de::Error::custom(
                        "surface resolution record budget exceeded",
                    ));
                }
                if records.len() == records.capacity() {
                    records.reserve_exact((MAX_SURFACE_RESOLUTION_RECORDS - records.len()).min(64));
                }
                if records.capacity() * std::mem::size_of::<SurfaceResolution>() + nested_bytes
                    > super::MAX_MANUFACTURING_BUNDLE_BYTES as usize
                {
                    return Err(serde::de::Error::custom(
                        "surface resolution allocation budget exceeded",
                    ));
                }
                records.push(record);
            }
            Ok(records)
        }
    }
    d.deserialize_seq(Records)
}
impl SurfaceResolution {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let sum = u64::from(self.resolved)
            + u64::from(self.ambiguous)
            + u64::from(self.below_nominal_resolution)
            + u64::from(self.unavailable);
        if sum != u64::from(self.evaluated_witnesses)
            || self.evaluated_witnesses > 20_000_000
            || self.layer_index >= MAX_PRINT_LAYERS
            || !self.source_z_mm.is_finite()
            || !self.max_residual_lower_mm.is_finite()
            || !self.max_residual_upper_mm.is_finite()
            || self.max_residual_lower_mm < 0.0
            || self.max_residual_lower_mm > self.max_residual_upper_mm
            || (self.unavailable > 0 && self.coverage_complete)
        {
            return Err(invalid(
                "invalid surface resolution counts, coverage or residual maxima",
            ));
        }
        if self.representatives.len() > MAX_SURFACE_RESOLUTION_REPRESENTATIVES
            || self.representatives.capacity() > MAX_SURFACE_RESOLUTION_REPRESENTATIVES
        {
            return Err(resource(
                "surface resolution representative budget exceeded",
            ));
        }
        if self.representatives.len() > self.evaluated_witnesses as usize
            || self.representatives_truncated
                != (self.evaluated_witnesses as usize > self.representatives.len())
        {
            return Err(invalid(
                "surface resolution representative truncation mismatch",
            ));
        }
        let mut counts = [0u32; 4];
        for witness in &self.representatives {
            witness.validate()?;
            if witness.source_z_mm != self.source_z_mm
                || witness.residual_lower_mm > self.max_residual_lower_mm
                || witness.residual_upper_mm > self.max_residual_upper_mm
            {
                return Err(invalid(
                    "surface resolution representative disagrees with group",
                ));
            }
            counts[match witness.classification {
                SurfaceFeatureClass::Resolved => 0,
                SurfaceFeatureClass::Ambiguous => 1,
                SurfaceFeatureClass::BelowNominalResolution => 2,
                SurfaceFeatureClass::Unavailable => 3,
            }] += 1;
        }
        if counts
            .into_iter()
            .zip([
                self.resolved,
                self.ambiguous,
                self.below_nominal_resolution,
                self.unavailable,
            ])
            .any(|(a, b)| a > b)
        {
            return Err(invalid(
                "surface resolution representative class exceeds group count",
            ));
        }
        if self.evaluated_witnesses == self.unavailable
            && (self.max_residual_lower_mm != 0.0 || self.max_residual_upper_mm != 0.0)
            || self.below_nominal_resolution == 0 && self.max_residual_lower_mm != 0.0
            || self.below_nominal_resolution > 0 && self.max_residual_lower_mm <= 0.0
            || self.ambiguous == 0
                && self.below_nominal_resolution == 0
                && self.max_residual_upper_mm != 0.0
        {
            return Err(invalid(
                "surface resolution maxima disagree with classifications",
            ));
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.face_id.allocated_bytes()
            + self.image_hash.allocated_bytes()
            + self.representatives.capacity() * std::mem::size_of::<SurfaceFeatureResolution>()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceResolutionTotals {
    #[ts(type = "number")]
    pub evaluated_witnesses: u64,
    #[ts(type = "number")]
    pub resolved: u64,
    #[ts(type = "number")]
    pub ambiguous: u64,
    #[ts(type = "number")]
    pub below_nominal_resolution: u64,
    #[ts(type = "number")]
    pub unavailable: u64,
    pub coverage_complete: bool,
}
impl Default for SurfaceResolutionTotals {
    fn default() -> Self {
        Self::empty()
    }
}
impl SurfaceResolutionTotals {
    pub fn empty() -> Self {
        Self {
            evaluated_witnesses: 0,
            resolved: 0,
            ambiguous: 0,
            below_nominal_resolution: 0,
            unavailable: 0,
            coverage_complete: true,
        }
    }
    pub fn from_records(records: &[SurfaceResolution]) -> Self {
        let mut totals = Self::empty();
        for record in records {
            totals.evaluated_witnesses += u64::from(record.evaluated_witnesses);
            totals.resolved += u64::from(record.resolved);
            totals.ambiguous += u64::from(record.ambiguous);
            totals.below_nominal_resolution += u64::from(record.below_nominal_resolution);
            totals.unavailable += u64::from(record.unavailable);
            totals.coverage_complete &= record.coverage_complete;
        }
        totals
    }
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let sum = self
            .resolved
            .checked_add(self.ambiguous)
            .and_then(|n| n.checked_add(self.below_nominal_resolution))
            .and_then(|n| n.checked_add(self.unavailable));
        if sum != Some(self.evaluated_witnesses)
            || self.evaluated_witnesses > 20_000_000
            || self.unavailable > 0 && self.coverage_complete
        {
            return Err(invalid("invalid surface resolution totals"));
        }
        Ok(())
    }
}
/// Caller has already validated the plan's bounded records and unique schedule identities.
pub(super) fn validate_attribution(
    records: &[SurfaceResolution],
    overlays: &SurfaceOverlays,
    occurrences: &[crate::geometry::OccurrenceRecord],
) -> Result<(), ManufacturingError> {
    for record in records {
        let occurrence = occurrences
            .iter()
            .find(|o| o.occurrence_id == record.occurrence_id)
            .ok_or_else(|| invalid("unknown surface resolution occurrence"))?;
        let layer = overlays
            .effective_layer(
                &occurrence.definition_id,
                occurrence.occurrence_id,
                &record.face_id,
            )
            .ok_or_else(|| invalid("surface resolution has no effective finish"))?;
        let (hash, parameters) = match &layer.finish {
            SurfaceFinish::ImageRelief {
                image_hash,
                parameters,
            } => (image_hash, parameters),
            _ => return Err(invalid("surface resolution attributes nonimage finish")),
        };
        if layer.layer_id != record.surface_layer_id || hash != &record.image_hash {
            return Err(invalid("surface resolution finish identity mismatch"));
        }
        let image = overlays
            .images
            .iter()
            .find(|image| &image.image_hash == hash)
            .ok_or_else(|| invalid("surface resolution image missing"))?;
        if record
            .representatives
            .iter()
            .any(|w| w.cell >= image.width || w.repeat >= parameters.u_repeats)
        {
            return Err(invalid("surface resolution cell or repeat outside mapping"));
        }
    }
    Ok(())
}
