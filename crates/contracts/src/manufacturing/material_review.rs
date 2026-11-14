// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Bounded, revision-owned declaration review; no geometry or physical admission.
use super::*;

pub const MAX_MATERIAL_CATALOG_PROFILES: usize = 16;
pub const MATERIAL_REVIEW_PAGE_SIZE: usize = 4;
pub const MAX_MATERIAL_REVIEW_BYTES: usize = 48 * 1024;
pub const MAX_MATERIAL_SEARCH_BYTES: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialCatalog {
    pub setup: MaterialToolSetup,
    pub profiles: Vec<MaterialProcessProfile>,
}
impl MaterialCatalog {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        self.setup.validate()?;
        if self.profiles.is_empty() || self.profiles.len() > MAX_MATERIAL_CATALOG_PROFILES {
            return Err(invalid("material catalog requires 1..=16 profiles"));
        }
        let mut identities = BTreeSet::new();
        for p in &self.profiles {
            p.validate()?;
            if !identities.insert((&p.id, &p.revision)) {
                return Err(invalid("duplicate material profile ID/revision in catalog"));
            }
        }
        json_bytes(self, MAX_PROFILE_JSON_BYTES as usize)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialReviewRequest {
    /// None inspects the currently captured selection, including its overrides.
    pub catalog: Option<MaterialCatalog>,
    pub query: String,
    pub offset: u32,
}
impl MaterialReviewRequest {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.query.len() > MAX_MATERIAL_SEARCH_BYTES || self.query.chars().any(char::is_control)
        {
            return Err(invalid(
                "material query exceeds 128 bytes or contains control characters",
            ));
        }
        if self.offset as usize > MAX_MATERIAL_CATALOG_PROFILES {
            return Err(invalid("material review offset exceeds catalog limit"));
        }
        if let Some(catalog) = &self.catalog {
            catalog.validate()?;
        }
        json_bytes(self, MAX_PROFILE_JSON_BYTES as usize)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialCompatibilityCode {
    NozzleMismatch,
    SetupMismatch,
    AbrasionRequirement,
    ProcessConstraints,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialCompatibilityIssue {
    pub code: MaterialCompatibilityCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialSettingField {
    NozzleTemperatureC,
    BedTemperatureC,
    PrintSpeedMmS,
    FilamentDiameterMm,
    FlowMultiplier,
    MaxVolumetricFlowMm3S,
    FirstLayerSpeedFactor,
    FirstLayerFlowFactor,
    FanFirstLayers,
    FanAfterFirstLayers,
    BridgeSpeedFactor,
    BridgeFlowFactor,
    BridgeFanPwm,
    MaxBridgeSpanMm,
    RetractionDistanceMm,
    RetractionSpeedMmS,
    RetractionMinimumTravelMm,
    RetractionZHopMm,
    MinBeadWidthMm,
    MaxBeadWidthMm,
    MinBeadHeightMm,
    MaxBeadHeightMm,
}
impl MaterialSettingField {
    pub const ALL: [Self; 22] = [
        Self::NozzleTemperatureC,
        Self::BedTemperatureC,
        Self::PrintSpeedMmS,
        Self::FilamentDiameterMm,
        Self::FlowMultiplier,
        Self::MaxVolumetricFlowMm3S,
        Self::FirstLayerSpeedFactor,
        Self::FirstLayerFlowFactor,
        Self::FanFirstLayers,
        Self::FanAfterFirstLayers,
        Self::BridgeSpeedFactor,
        Self::BridgeFlowFactor,
        Self::BridgeFanPwm,
        Self::MaxBridgeSpanMm,
        Self::RetractionDistanceMm,
        Self::RetractionSpeedMmS,
        Self::RetractionMinimumTravelMm,
        Self::RetractionZHopMm,
        Self::MinBeadWidthMm,
        Self::MaxBeadWidthMm,
        Self::MinBeadHeightMm,
        Self::MaxBeadHeightMm,
    ];
    fn retraction(self) -> bool {
        matches!(
            self,
            Self::RetractionDistanceMm
                | Self::RetractionSpeedMmS
                | Self::RetractionMinimumTravelMm
                | Self::RetractionZHopMm
        )
    }
    fn validate_value(self, value: Option<f64>) -> Result<(), ManufacturingError> {
        let Some(value) = value else {
            return if self.retraction() {
                Ok(())
            } else {
                Err(invalid("missing material setting value"))
            };
        };
        if !value.is_finite() || value < 0.0 {
            return Err(invalid("nonfinite or negative material review value"));
        }
        if matches!(
            self,
            Self::FanFirstLayers | Self::FanAfterFirstLayers | Self::BridgeFanPwm
        ) && (value > 255.0 || value.fract() != 0.0)
        {
            return Err(invalid("invalid material review fan PWM"));
        }
        let fan = matches!(
            self,
            Self::FanFirstLayers | Self::FanAfterFirstLayers | Self::BridgeFanPwm
        );
        if value == 0.0 && !fan && self != Self::RetractionZHopMm {
            return Err(invalid("nonpositive material process value"));
        }
        if matches!(self, Self::FirstLayerSpeedFactor | Self::BridgeSpeedFactor) && value > 1.0 {
            return Err(invalid("material review speed factor exceeds one"));
        }
        if matches!(self, Self::NozzleTemperatureC | Self::BedTemperatureC) && value.fract() != 0.0
        {
            return Err(invalid(
                "material review temperature is not integral Celsius",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialValueOrigin {
    CurrentIntent,
    SelectedProfile,
    ExplicitOverride,
    MachineLimit,
    MaterialLimit,
    SharedLimit,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialSettingReview {
    pub field: MaterialSettingField,
    pub current: Option<f64>,
    pub proposed: Option<f64>,
    pub current_origin: MaterialValueOrigin,
    pub proposed_origin: MaterialValueOrigin,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialCandidateReview {
    /// Incompatible selections are inspectable but cannot be applied unchanged.
    pub selection: MaterialProfileSelection,
    pub issues: Vec<MaterialCompatibilityIssue>,
    /// None for incompatible declarations; otherwise includes metadata/capture changes.
    pub input_changed: Option<bool>,
    pub settings: Vec<MaterialSettingReview>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialReviewReport {
    pub input_revision: ProjectRevision,
    pub query: String,
    pub offset: u32,
    pub catalog_profiles: u32,
    pub matching_profiles: u32,
    pub candidates: Vec<MaterialCandidateReview>,
}
impl MaterialReviewReport {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        MaterialReviewRequest {
            catalog: None,
            query: self.query.clone(),
            offset: self.offset,
        }
        .validate()?;
        let count = self.catalog_profiles as usize;
        if count == 0
            || count > MAX_MATERIAL_CATALOG_PROFILES
            || self.matching_profiles > self.catalog_profiles
            || self.candidates.len()
                != (self.matching_profiles.saturating_sub(self.offset) as usize)
                    .min(MATERIAL_REVIEW_PAGE_SIZE)
        {
            return Err(invalid("inconsistent material review page"));
        }
        let mut ids = BTreeSet::new();
        for c in &self.candidates {
            c.selection.profile.validate()?;
            c.selection.setup.validate()?;
            c.selection.overrides.validate()?;
            if !ids.insert((&c.selection.profile.id, &c.selection.profile.revision))
                || c.issues.len() > 8
            {
                return Err(invalid("duplicate candidate or excessive material issues"));
            }
            for issue in &c.issues {
                if issue.message.is_empty() || issue.message.len() > MAX_MANUFACTURING_ERROR_BYTES {
                    return Err(invalid("invalid material compatibility message"));
                }
            }
            if c.issues.is_empty() {
                c.selection.validate()?;
                if c.input_changed.is_none() || c.settings.len() != MaterialSettingField::ALL.len()
                {
                    return Err(invalid(
                        "compatible material review lacks effective settings",
                    ));
                }
                for (row, field) in c.settings.iter().zip(MaterialSettingField::ALL) {
                    if row.field != field {
                        return Err(invalid("material setting order/identity mismatch"));
                    }
                    field.validate_value(row.current)?;
                    field.validate_value(row.proposed)?;
                }
                for proposed in [false, true] {
                    let active = c
                        .settings
                        .iter()
                        .filter(|row| row.field.retraction())
                        .filter(|row| {
                            if proposed {
                                row.proposed.is_some()
                            } else {
                                row.current.is_some()
                            }
                        })
                        .count();
                    if active != 0 && active != 4 {
                        return Err(invalid("inconsistent material retraction settings"));
                    }
                }
            } else if c.input_changed.is_some() || !c.settings.is_empty() {
                return Err(invalid(
                    "incompatible material must not claim resolved settings",
                ));
            }
        }
        json_bytes(self, MAX_MATERIAL_REVIEW_BYTES)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> MaterialCatalog {
        let s: MaterialProfileSelection = serde_json::from_slice(include_bytes!(
            "../../../../fixtures/manufacturing/material-profile-a.json"
        ))
        .unwrap();
        MaterialCatalog {
            setup: s.setup,
            profiles: vec![s.profile],
        }
    }
    #[test]
    fn catalogs_reject_duplicate_identity_and_excessive_entries() {
        let mut c = catalog();
        c.profiles.push(c.profiles[0].clone());
        assert!(c.validate().unwrap_err().message.contains("duplicate"));
        c.profiles = vec![c.profiles[0].clone(); MAX_MATERIAL_CATALOG_PROFILES + 1];
        assert!(c.validate().is_err());
        c.profiles.clear();
        assert!(c.validate().is_err());
    }
    #[test]
    fn checked_wire_requests_reject_missing_unknown_and_unbounded_data() {
        let request = MaterialReviewRequest {
            catalog: Some(catalog()),
            query: "PLA".into(),
            offset: 0,
        };
        let wire = crate::rpc::MaterialReviewRequest::try_from(request.clone()).unwrap();
        assert_eq!(
            MaterialReviewRequest::try_from(wire.clone()).unwrap(),
            request
        );
        let mut bad = wire.clone();
        bad.query = "x".repeat(MAX_MATERIAL_SEARCH_BYTES + 1);
        assert!(MaterialReviewRequest::try_from(bad).is_err());
        let mut bad = wire.clone();
        bad.offset = 17;
        assert!(MaterialReviewRequest::try_from(bad).is_err());
        let mut bad = wire;
        bad.catalog.as_mut().unwrap().setup = None;
        assert!(MaterialReviewRequest::try_from(bad).is_err());
        assert!(MaterialSettingField::try_from(0).is_err());
        assert!(MaterialValueOrigin::try_from(999).is_err());
        assert!(MaterialCompatibilityCode::try_from(0).is_err());
        let mut bad = crate::rpc::MaterialSettingReview {
            field: 1,
            current: Some(1.0),
            proposed: Some(1.0),
            current_origin: 1,
            proposed_origin: 2,
        };
        bad.field = 999;
        assert!(MaterialSettingReview::try_from(bad).is_err());
    }
}
