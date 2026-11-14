// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Captured single-tool process declarations, not physical material qualification.
use super::*;

pub const MATERIAL_RESOLVER_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialDefinition {
    pub id: String,
    pub revision: String,
    pub manufacturer: String,
    pub product: String,
    pub polymer_family: String,
    pub filler: Option<MaterialFiller>,
    pub filament_diameter_mm: f64,
    pub requires_abrasion_resistant_nozzle: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialFiller {
    pub kind: MaterialFillerKind,
    pub mass_fraction: Option<f64>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialFillerKind {
    CarbonFiber,
    GlassFiber,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialToolSetup {
    pub hotend_id: String,
    pub nozzle_material: String,
    pub abrasion_resistant_nozzle: bool,
    pub cooling_configuration_id: String,
    /// The installed extruder drive; it selects the profile's retraction.
    pub extruder_drive: ExtruderDrive,
}
/// How the extruder pushes filament to the hotend: direct drive at the
/// toolhead, or through a Bowden tube (longer retraction).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtruderDrive {
    DirectDrive,
    Bowden,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialProfileSourceKind {
    SyntheticTest,
    ImportedRecommendation,
    OperatorDeclaration,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialProfileSource {
    pub kind: MaterialProfileSourceKind,
    pub source_id: String,
    pub source_revision: String,
    pub source_hash: SourceHash,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialProcessProfile {
    pub id: String,
    pub revision: String,
    pub material: MaterialDefinition,
    /// Compatibility key: nozzle diameter class plus the hotend/nozzle
    /// material/cooling class below and the material's polymer family. A
    /// profile is never bound to one printer identity or revision, so editing
    /// a printer's other facts keeps its profiles.
    pub nozzle_diameter_mm: f64,
    pub hotend_id: String,
    pub nozzle_material: String,
    pub cooling_configuration_id: String,
    pub deposition_limits: DepositionLimits,
    pub nozzle_temperature_c: f64,
    /// Zero is an off bed (the material prints unheated).
    pub bed_temperature_c: f64,
    pub first_layer_nozzle_temperature_c: f64,
    pub first_layer_bed_temperature_c: f64,
    /// Non-oozing nozzle temperature while the bed heats, homes and probes.
    pub standby_nozzle_temperature_c: f64,
    /// The material cannot print on an unheated bed.
    pub requires_heated_bed: bool,
    /// The material's cap on every deposition speed.
    pub print_speed_mm_s: f64,
    pub flow_multiplier: f64,
    pub max_volumetric_flow_mm3_s: f64,
    /// Direct-drive process; `bowden_retraction` replaces its retraction on
    /// a Bowden setup.
    pub process: ProcessPolicy,
    pub bowden_retraction: RetractionPolicy,
    pub source: MaterialProfileSource,
}
/// Explicit overrides are captured; absence retains the selected profile value.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialProfileOverrides {
    pub nozzle_temperature_c: Option<f64>,
    pub bed_temperature_c: Option<f64>,
    pub print_speed_mm_s: Option<f64>,
    pub flow_multiplier: Option<f64>,
    pub process: Option<ProcessPolicy>,
    pub first_layer_nozzle_temperature_c: Option<f64>,
    pub first_layer_bed_temperature_c: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialProfileSelection {
    pub profile: MaterialProcessProfile,
    pub setup: MaterialToolSetup,
    pub overrides: MaterialProfileOverrides,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedMaterialProfile {
    pub resolver_version: u32,
    pub selection: MaterialProfileSelection,
}
impl MaterialProfileSelection {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        self.profile.validate()?;
        self.setup.validate()?;
        self.overrides.validate()?;
        let p = &self.profile;
        let m = &p.material;
        if m.requires_abrasion_resistant_nozzle && !self.setup.abrasion_resistant_nozzle {
            return Err(invalid(
                "selected material requires a declared abrasion-resistant nozzle",
            ));
        }
        if p.cooling_configuration_id != self.setup.cooling_configuration_id
            || p.hotend_id != self.setup.hotend_id
            || p.nozzle_material != self.setup.nozzle_material
        {
            return Err(invalid(
                "material profile hotend/nozzle material/cooling does not match installed setup",
            ));
        }
        json_bytes(self, MAX_PROFILE_JSON_BYTES as usize)?;
        Ok(())
    }
    /// Profile versus explicit override is determined for each supported
    /// value. Without an explicit process, a Bowden setup takes the
    /// profile's Bowden retraction.
    pub fn process(&self) -> ProcessPolicy {
        if let Some(process) = &self.overrides.process {
            return process.clone();
        }
        let mut process = self.profile.process.clone();
        if self.setup.extruder_drive == ExtruderDrive::Bowden {
            process.retraction = self.profile.bowden_retraction.clone();
        }
        process
    }
    /// The explicit or profile process without the Bowden substitution: a
    /// fixed-dual tool's process (its hotends are direct-drive).
    pub fn declared_process(&self) -> &ProcessPolicy {
        self.overrides
            .process
            .as_ref()
            .unwrap_or(&self.profile.process)
    }
    pub fn first_layer_nozzle_temperature_c(&self) -> f64 {
        self.overrides
            .first_layer_nozzle_temperature_c
            .unwrap_or(self.profile.first_layer_nozzle_temperature_c)
    }
    pub fn first_layer_bed_temperature_c(&self) -> f64 {
        self.overrides
            .first_layer_bed_temperature_c
            .unwrap_or(self.profile.first_layer_bed_temperature_c)
    }
    pub fn print_speed_mm_s(&self) -> f64 {
        self.overrides
            .print_speed_mm_s
            .unwrap_or(self.profile.print_speed_mm_s)
    }
    pub fn flow_multiplier(&self) -> f64 {
        self.overrides
            .flow_multiplier
            .unwrap_or(self.profile.flow_multiplier)
    }
    pub fn nozzle_temperature_c(&self) -> f64 {
        self.overrides
            .nozzle_temperature_c
            .unwrap_or(self.profile.nozzle_temperature_c)
    }
    pub fn bed_temperature_c(&self) -> f64 {
        self.overrides
            .bed_temperature_c
            .unwrap_or(self.profile.bed_temperature_c)
    }
}
impl ResolvedMaterialProfile {
    pub fn validate_for(&self, intent: &ManufacturingIntent) -> Result<(), ManufacturingError> {
        self.selection.validate()?;
        let s = &self.selection;
        let p = &s.profile;
        let r = &intent.recipe;
        if self.resolver_version != MATERIAL_RESOLVER_VERSION {
            return Err(invalid("unsupported material resolver version"));
        }
        if p.nozzle_diameter_mm != intent.printer.nozzle_diameter_mm {
            return Err(invalid(
                "material profile nozzle diameter differs from the selected printer nozzle",
            ));
        }
        if r.material_id != p.material.id
            || r.material_revision != p.material.revision
            || r.filament_diameter_mm != p.material.filament_diameter_mm
            || r.print_speed_mm_s != s.print_speed_mm_s()
            || r.flow_multiplier != s.flow_multiplier()
            || r.process != s.process()
        {
            return Err(invalid(
                "recipe disagrees with captured material profile/explicit overrides",
            ));
        }
        let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow else {
            return Err(invalid(
                "material profile resolution currently requires a complete single-extruder Marlin job",
            ));
        };
        let standby_ok = job.lifecycle.start.iter().all(|step| match step {
            LifecycleStep::NozzleTarget { standby_c } | LifecycleStep::NozzleWait { standby_c } => {
                standby_c.is_none_or(|t| t == p.standby_nozzle_temperature_c)
            }
            _ => true,
        });
        if job.nozzle_temperature_c != s.nozzle_temperature_c()
            || job.bed_temperature_c != s.bed_temperature_c()
            || job.first_layer_nozzle_temperature_c != s.first_layer_nozzle_temperature_c()
            || job.first_layer_bed_temperature_c != s.first_layer_bed_temperature_c()
            || !standby_ok
        {
            return Err(invalid(
                "job thermal targets disagree with captured material profile/explicit overrides",
            ));
        }
        if p.requires_heated_bed && job.maximum_bed_temperature_c == 0.0 {
            return Err(invalid(
                "the material requires a heated bed and the printer declares none",
            ));
        }
        Ok(())
    }
}
pub(super) fn captured_material_profile<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<ResolvedMaterialProfile>, D::Error> {
    Option::<ResolvedMaterialProfile>::deserialize(d)
}

impl MaterialDefinition {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        for v in [
            &self.id,
            &self.revision,
            &self.manufacturer,
            &self.product,
            &self.polymer_family,
        ] {
            label(v)?;
        }
        positive(&[self.filament_diameter_mm])?;
        if let Some(filler) = &self.filler {
            filler.validate()?;
        }
        Ok(())
    }
}
impl MaterialFiller {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self
            .mass_fraction
            .is_some_and(|v| !v.is_finite() || v <= 0.0 || v >= 1.0)
        {
            return Err(invalid(
                "material filler mass fraction must be finite in (0,1)",
            ));
        }
        Ok(())
    }
}
impl MaterialToolSetup {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        label(&self.hotend_id)?;
        label(&self.nozzle_material)?;
        label(&self.cooling_configuration_id)
    }
}
impl MaterialProfileSource {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        label(&self.source_id)?;
        label(&self.source_revision)
    }
}
impl MaterialProcessProfile {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        for v in [
            &self.id,
            &self.revision,
            &self.cooling_configuration_id,
            &self.hotend_id,
            &self.nozzle_material,
        ] {
            label(v)?;
        }
        self.material.validate()?;
        self.source.validate()?;
        self.process.validate()?;
        self.deposition_limits.validate()?;
        self.bowden_retraction.validate()?;
        positive(&[
            self.nozzle_diameter_mm,
            self.nozzle_temperature_c,
            self.first_layer_nozzle_temperature_c,
            self.standby_nozzle_temperature_c,
            self.print_speed_mm_s,
            self.flow_multiplier,
            self.max_volumetric_flow_mm3_s,
        ])?;
        let nozzles = [
            self.nozzle_temperature_c,
            self.first_layer_nozzle_temperature_c,
            self.standby_nozzle_temperature_c,
        ];
        let beds = [self.bed_temperature_c, self.first_layer_bed_temperature_c];
        if nozzles.iter().any(|t| *t > 500.0 || t.fract() != 0.0)
            || beds
                .iter()
                .any(|t| !t.is_finite() || *t < 0.0 || *t > 200.0 || t.fract() != 0.0)
            || (self.requires_heated_bed && beds.contains(&0.0))
        {
            return Err(invalid(
                "material profile thermal targets require bounded integral Celsius",
            ));
        }
        Ok(())
    }
}
impl MaterialProfileOverrides {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        for v in [
            self.nozzle_temperature_c,
            self.first_layer_nozzle_temperature_c,
            self.print_speed_mm_s,
            self.flow_multiplier,
        ]
        .into_iter()
        .flatten()
        {
            positive(&[v])?;
        }
        // An explicit zero bed is off: an unheated machine or material choice.
        for v in [self.bed_temperature_c, self.first_layer_bed_temperature_c]
            .into_iter()
            .flatten()
        {
            if !v.is_finite() || v < 0.0 {
                return Err(invalid("bed temperature overrides are zero or positive"));
            }
        }
        for v in [
            self.nozzle_temperature_c,
            self.first_layer_nozzle_temperature_c,
            self.bed_temperature_c,
            self.first_layer_bed_temperature_c,
        ]
        .into_iter()
        .flatten()
        {
            if v.fract() != 0.0 {
                return Err(invalid(
                    "material thermal overrides require integral Celsius",
                ));
            }
        }
        if let Some(p) = &self.process {
            p.validate()?;
        }
        Ok(())
    }
}
impl ResolvedMaterialProfile {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.resolver_version != MATERIAL_RESOLVER_VERSION {
            return Err(invalid("unsupported material resolver version"));
        }
        self.selection.validate()
    }
}

pub(super) fn captured_hardware<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<std::sync::Arc<crate::hardware::ProjectHardwareCapture>>, D::Error> {
    serde::Deserialize::deserialize(d)
}

/// Generic process profiles for the common polymer families, one per nozzle
/// class (authored in `data/material-profiles.ts`, cited sources inside).
pub fn generic_material_profiles() -> Result<Vec<MaterialProcessProfile>, String> {
    let profiles: Vec<MaterialProcessProfile> =
        serde_json::from_str(include_str!("../../data/material-profiles.json"))
            .map_err(|e| e.to_string())?;
    for profile in &profiles {
        profile.validate().map_err(|e| e.to_string())?;
    }
    Ok(profiles)
}

/// The polymer family a free-text material label names, when a generic
/// profile covers it (`PLA+`, `Silk PLA` are PLA; `PCTG` prints as PETG).
pub fn generic_polymer_family(label: &str) -> Option<&'static str> {
    let label = label.to_ascii_uppercase();
    [
        ("PETG", "PETG"),
        ("PCTG", "PETG"),
        ("PET", "PETG"),
        ("ASA", "ASA"),
        ("ABS", "ABS"),
        ("TPU", "TPU"),
        ("TPE", "TPU"),
        ("FLEX", "TPU"),
        ("PLA", "PLA"),
    ]
    .into_iter()
    .find(|(token, _)| label.contains(token))
    .map(|(_, family)| family)
}

/// The generic profiles of `material`'s polymer family, carrying its identity.
pub fn generic_profiles_for(
    material: &MaterialDefinition,
) -> Result<Vec<MaterialProcessProfile>, String> {
    let Some(family) = generic_polymer_family(&material.polymer_family) else {
        return Ok(Vec::new());
    };
    generic_material_profiles()?
        .into_iter()
        .filter(|p| p.material.polymer_family == family)
        .map(|mut p| {
            p.id = format!("{}.{}", p.id, material.id);
            p.material = material.clone();
            p.material.polymer_family = family.into();
            p.validate().map_err(|e| e.to_string())?;
            Ok(p)
        })
        .collect()
}
