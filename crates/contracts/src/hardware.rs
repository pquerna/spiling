// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Portable hardware definitions and explicitly private physical inventory.
use crate::{
    geometry::{AabbMm, NativePath, SourceHash},
    library::validate_library_uuid,
    manufacturing::{ManufacturingIntent, PrinterSpecification},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use ts_rs::TS;

pub const MAX_HARDWARE_BYTES: usize = 48 * 1024;
pub const MAX_CAPTURE_BYTES: usize = 24 * 1024;
fn text(v: &str) -> Result<(), String> {
    if v.is_empty() || v.len() > 256 || v.trim() != v || v.chars().any(char::is_control) {
        return Err("hardware text must be nonempty, canonical and bounded".into());
    }
    Ok(())
}
fn option_text(v: &Option<String>) -> Result<(), String> {
    v.as_deref().map_or(Ok(()), text)
}
fn bounds(v: &AabbMm) -> Result<(), String> {
    v.validate().map_err(|e| e.to_string())?;
    if (0..3).any(|a| v.min[a] >= v.max[a]) {
        return Err("hardware bounds must have positive extent".into());
    }
    Ok(())
}
fn unique<'a>(values: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for v in values {
        text(v)?;
        if !seen.insert(v) {
            return Err("duplicate hardware identity".into());
        }
    }
    Ok(())
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HardwareModelDefinition {
    pub id: String,
    pub vendor: String,
    pub model: String,
    /// Public attribution only, never an import path or private device endpoint.
    pub source: String,
    pub mounting_interface: String,
    #[serde(deserialize_with = "required_option")]
    pub declared_volume_mm: Option<AabbMm>,
}
impl HardwareModelDefinition {
    pub fn validate(&self) -> Result<(), String> {
        for v in [
            &self.id,
            &self.vendor,
            &self.model,
            &self.source,
            &self.mounting_interface,
        ] {
            text(v)?
        }
        self.declared_volume_mm.as_ref().map_or(Ok(()), bounds)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HardwareSetupDefinition {
    pub id: String,
    pub label: String,
    pub model: HardwareModelDefinition,
    pub printer: PrinterSpecification,
}
impl HardwareSetupDefinition {
    pub fn validate(&self) -> Result<(), String> {
        text(&self.id)?;
        text(&self.label)?;
        self.model.validate()?;
        self.printer.validate().map_err(|e| e.to_string())
    }
    pub fn revision(&self) -> Result<SourceHash, String> {
        self.validate()?;
        Ok(SourceHash::from_bytes(
            &serde_json::to_vec(self).map_err(|e| e.to_string())?,
        ))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlateFaceDefinition {
    pub key: String,
    pub label: String,
    pub surface: String,
    pub usable_mm: AabbMm,
    pub keepouts_mm: Vec<AabbMm>,
    pub max_bed_celsius: f64,
}
impl PlateFaceDefinition {
    pub fn validate(&self) -> Result<(), String> {
        for v in [&self.key, &self.label, &self.surface] {
            text(v)?
        }
        bounds(&self.usable_mm)?;
        if !self.max_bed_celsius.is_finite()
            || !(0.0..=300.0).contains(&self.max_bed_celsius)
            || self.keepouts_mm.len() > 16
        {
            return Err("invalid plate temperature or keepout count".into());
        }
        for b in &self.keepouts_mm {
            bounds(b)?
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlateDefinition {
    pub id: String,
    pub label: String,
    pub mounting_interface: String,
    pub thickness_mm: f64,
    /// Installed top surface in machine coordinates; thickness is not subtracted again.
    pub surface_z_mm: f64,
    pub faces: Vec<PlateFaceDefinition>,
    pub source: String,
}
impl PlateDefinition {
    pub fn validate(&self) -> Result<(), String> {
        for v in [
            &self.id,
            &self.label,
            &self.mounting_interface,
            &self.source,
        ] {
            text(v)?
        }
        if !self.thickness_mm.is_finite()
            || self.thickness_mm <= 0.0
            || self.thickness_mm > 100.0
            || !self.surface_z_mm.is_finite()
            || self.faces.is_empty()
            || self.faces.len() > 4
        {
            return Err("invalid plate datum, thickness or faces".into());
        }
        unique(self.faces.iter().map(|f| f.key.as_str()))?;
        for f in &self.faces {
            f.validate()?
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccessoryEffect {
    Nozzle { diameter_mm: f64, material: String },
    Fixture { bounds_mm: AabbMm },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AccessoryDefinition {
    pub id: String,
    pub label: String,
    pub mounting_interface: String,
    pub effect: AccessoryEffect,
}
impl AccessoryDefinition {
    pub fn validate(&self) -> Result<(), String> {
        for v in [&self.id, &self.label, &self.mounting_interface] {
            text(v)?
        }
        match &self.effect {
            AccessoryEffect::Nozzle {
                diameter_mm,
                material,
            } => {
                text(material)?;
                if !diameter_mm.is_finite() || !(0.1..=3.0).contains(diameter_mm) {
                    return Err("invalid nozzle diameter".into());
                }
            }
            AccessoryEffect::Fixture { bounds_mm } => bounds(bounds_mm)?,
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectHardwareCapture {
    pub setup: HardwareSetupDefinition,
    pub plate: PlateDefinition,
    pub face: String,
    pub accessories: Vec<AccessoryDefinition>,
}
impl ProjectHardwareCapture {
    pub fn validate(&self) -> Result<(), String> {
        self.setup.validate()?;
        self.plate.validate()?;
        text(&self.face)?;
        if self.accessories.len() > 16 {
            return Err("too many captured accessories".into());
        }
        unique(self.accessories.iter().map(|a| a.id.as_str()))?;
        for a in &self.accessories {
            a.validate()?;
            if a.mounting_interface != self.setup.model.mounting_interface {
                return Err("accessory mount is incompatible with setup".into());
            }
            if let AccessoryEffect::Nozzle { diameter_mm, .. } = a.effect
                && diameter_mm != self.setup.printer.nozzle_diameter_mm
            {
                return Err(
                    "nozzle accessory disagrees with captured printer; author a new setup".into(),
                );
            }
        }
        if self.plate.mounting_interface != self.setup.model.mounting_interface {
            return Err("plate mount is incompatible with setup".into());
        }
        self.selected_face()?;
        if self.plate.surface_z_mm != self.setup.printer.build_envelope.min[2] {
            return Err("unsupported plate datum: current planar backend requires the installed surface at machine bed Z".into());
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_CAPTURE_BYTES {
            return Err("hardware capture exceeds 24 KiB".into());
        }
        Ok(())
    }
    pub fn selected_face(&self) -> Result<&PlateFaceDefinition, String> {
        self.plate
            .faces
            .iter()
            .find(|f| f.key == self.face)
            .ok_or_else(|| "selected plate face does not exist".into())
    }
    pub fn hash(&self) -> Result<SourceHash, String> {
        self.validate()?;
        Ok(SourceHash::from_bytes(
            &serde_json::to_vec(self).map_err(|e| e.to_string())?,
        ))
    }
    pub fn validate_intent(&self, intent: &ManufacturingIntent) -> Result<(), String> {
        self.validate()?;
        let mut installed = self.setup.printer.clone();
        // Installed geometry/capabilities stay fixed. A captured material profile
        // owns the operating thermal targets, not the setup's starting values.
        if let Some(profile) = &intent.material_profile {
            profile.validate_for(intent).map_err(|e| e.to_string())?;
            if let crate::manufacturing::JobWorkflow::CompleteMarlin { job } =
                &mut installed.workflow
            {
                job.nozzle_temperature_c = profile.selection.nozzle_temperature_c();
                job.bed_temperature_c = profile.selection.bed_temperature_c();
            }
        }
        if installed != intent.printer {
            return Err("manufacturing printer differs from portable hardware capture".into());
        }
        if match &intent.printer.workflow {
            crate::manufacturing::JobWorkflow::DiagnosticBodyOnly => 0.0,
            crate::manufacturing::JobWorkflow::CompleteMarlin { job } => job.bed_temperature_c,
            crate::manufacturing::JobWorkflow::FixedDualMarlin { job } => job.bed_temperature_c,
        } > self.selected_face()?.max_bed_celsius
        {
            return Err("process bed temperature exceeds selected plate face limit".into());
        }
        Ok(())
    }
    /// Conservative whole-material admission, separate from machine travel and purge areas.
    pub fn admits_bounds(&self, b: &AabbMm) -> Result<(), String> {
        bounds(b)?;
        let f = self.selected_face()?;
        let p = &self.setup.printer.build_envelope;
        if (0..3).any(|a| b.min[a] < p.min[a] - 1e-6 || b.max[a] > p.max[a] + 1e-6)
            || (0..2).any(|a| {
                b.min[a] < f.usable_mm.min[a] - 1e-6 || b.max[a] > f.usable_mm.max[a] + 1e-6
            })
        {
            return Err(
                "material lies outside selected plate face usable region or machine volume".into(),
            );
        }
        for obstacle in f
            .keepouts_mm
            .iter()
            .chain(self.accessories.iter().filter_map(|a| match &a.effect {
                AccessoryEffect::Fixture { bounds_mm } => Some(bounds_mm),
                _ => None,
            }))
        {
            if (0..3).all(|a| b.min[a] < obstacle.max[a] && b.max[a] > obstacle.min[a]) {
                return Err("material intersects selected plate keepout or fixture".into());
            }
        }
        Ok(())
    }
}
/// Public model identity is independent of a complete installed slicing setup.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrinterIdentity {
    pub manufacturer: String,
    pub model: String,
    #[serde(deserialize_with = "required_option")]
    pub catalog_origin: Option<Box<crate::catalog::CatalogRecord>>,
}
impl PrinterIdentity {
    pub fn from_setup(setup: &HardwareSetupDefinition) -> Self {
        Self {
            manufacturer: setup.model.vendor.clone(),
            model: setup.model.model.clone(),
            catalog_origin: None,
        }
    }
    pub fn from_reference(record: crate::catalog::CatalogRecord) -> Result<Self, String> {
        record.validate()?;
        let crate::catalog::CatalogItem::Printer {
            manufacturer,
            model,
            ..
        } = &record.item
        else {
            return Err("select a printer reference".into());
        };
        let identity = Self {
            manufacturer: manufacturer.clone(),
            model: model.clone(),
            catalog_origin: Some(Box::new(record)),
        };
        identity.validate()?;
        Ok(identity)
    }
    pub fn validate(&self) -> Result<(), String> {
        text(&self.manufacturer)?;
        text(&self.model)?;
        if let Some(origin) = &self.catalog_origin {
            origin.validate()?;
            if origin.item.kind() != crate::catalog::CatalogKind::Printer {
                return Err("machine origin must describe a printer".into());
            }
        }
        Ok(())
    }
    pub fn matches_setup(&self, setup: &HardwareSetupDefinition) -> bool {
        self.manufacturer == setup.model.vendor && self.model == setup.model.model
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum MachineConfiguration {
    Unconfigured,
    Configured { setup: Box<HardwareSetupDefinition> },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OwnedMachine {
    pub id: String,
    pub nickname: String,
    #[serde(deserialize_with = "required_option")]
    pub serial: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub location: Option<String>,
    pub identity: PrinterIdentity,
    pub configuration: MachineConfiguration,
    pub generation: u64,
}
impl OwnedMachine {
    pub fn configured_setup(&self) -> Result<&HardwareSetupDefinition, String> {
        match &self.configuration {
            MachineConfiguration::Configured { setup } => Ok(setup),
            MachineConfiguration::Unconfigured => Err(
                "Setup needed: declare a complete installed setup before applying hardware".into(),
            ),
        }
    }
    pub fn configured_setup_mut(&mut self) -> Result<&mut HardwareSetupDefinition, String> {
        match &mut self.configuration {
            MachineConfiguration::Configured { setup } => Ok(setup),
            MachineConfiguration::Unconfigured => Err(
                "Setup needed: declare a complete installed setup before applying hardware".into(),
            ),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id)?;
        text(&self.nickname)?;
        option_text(&self.serial)?;
        option_text(&self.location)?;
        self.identity.validate()?;
        if let MachineConfiguration::Configured { setup } = &self.configuration {
            setup.validate()?;
            if !self.identity.matches_setup(setup) {
                return Err("installed setup belongs to a different printer model".into());
            }
        }
        if self.generation == 0 {
            return Err("machine generation must be positive".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OwnedPlate {
    pub id: String,
    pub nickname: String,
    pub definition: PlateDefinition,
    #[serde(deserialize_with = "required_option")]
    pub installed_machine: Option<String>,
    pub notes: String,
}
impl OwnedPlate {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id)?;
        text(&self.nickname)?;
        self.definition.validate()?;
        if let Some(id) = &self.installed_machine {
            validate_library_uuid(id)?;
        }
        if self.notes.len() > 1024 || self.notes.chars().any(char::is_control) {
            return Err("invalid plate notes".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OwnedAccessory {
    pub id: String,
    pub nickname: String,
    pub definition: AccessoryDefinition,
    #[serde(deserialize_with = "required_option")]
    pub installed_machine: Option<String>,
}
impl OwnedAccessory {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id)?;
        text(&self.nickname)?;
        self.definition.validate()?;
        if let Some(id) = &self.installed_machine {
            validate_library_uuid(id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialBackend {
    Keyring,
    PlaintextFile,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialRole {
    LanAccessCode,
    ApiKey,
    Password,
    RefreshToken,
}
impl CredentialRole {
    pub fn token(self) -> &'static str {
        match self {
            Self::LanAccessCode => "lan-access-code",
            Self::ApiKey => "api-key",
            Self::Password => "password",
            Self::RefreshToken => "refresh-token",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialRef {
    pub slot_id: String,
    pub role: CredentialRole,
    pub backend: CredentialBackend,
}
impl CredentialRef {
    pub const SERVICE: &'static str = "org.spiling.hardware";
    pub fn account(&self) -> Result<String, String> {
        validate_library_uuid(&self.slot_id)?;
        Ok(format!("v1.{}.{}", self.slot_id, self.role.token()))
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionFamily {
    File,
    BambuConnect,
    BambuDeveloperLan,
    Moonraker,
    OctoPrint,
    PrusaLink,
    DevelopmentSimulator,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EndpointRoute {
    pub id: String,
    pub machine_id: String,
    pub family: ConnectionFamily,
    pub endpoint: String,
    pub expected_identity: String,
    pub generation: u64,
    #[serde(deserialize_with = "required_option")]
    pub credential: Option<CredentialRef>,
}
impl EndpointRoute {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id)?;
        validate_library_uuid(&self.machine_id)?;
        text(&self.endpoint)?;
        text(&self.expected_identity)?;
        if self.generation == 0
            || self.endpoint.contains('@')
            || self.endpoint.contains('?')
            || self.endpoint.contains('#')
        {
            return Err("route requires a generation and a secret-free endpoint".into());
        }
        if self.family == ConnectionFamily::File
            && (self.endpoint != "local-file"
                || self.credential.is_some()
                || self.expected_identity != self.machine_id)
        {
            return Err(
                "file route requires local-file, its owned machine identity and no credential"
                    .into(),
            );
        }
        if let Some(c) = &self.credential {
            c.account()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalProjectHardwareBinding {
    pub layout_id: crate::project::PrintLayoutId,
    pub project_id: String,
    pub attachment: NativePath,
    pub capture_hash: SourceHash,
    pub machine_id: String,
    pub plate_id: String,
    pub machine_generation: u64,
}
impl LocalProjectHardwareBinding {
    pub fn validate(&self) -> Result<(), String> {
        crate::project::ProjectId::parse(self.project_id.clone()).map_err(|e| e.to_string())?;
        self.attachment.validate().map_err(|e| e.to_string())?;
        validate_library_uuid(&self.machine_id)?;
        validate_library_uuid(&self.plate_id)?;
        if self.machine_generation == 0 {
            return Err("local hardware binding requires a machine generation".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HardwareLibrary {
    pub machines: Vec<OwnedMachine>,
    pub plates: Vec<OwnedPlate>,
    pub accessories: Vec<OwnedAccessory>,
    pub routes: Vec<EndpointRoute>,
    pub bindings: Vec<LocalProjectHardwareBinding>,
}
impl HardwareLibrary {
    pub fn validate(&self) -> Result<(), String> {
        if self.machines.len() > 32
            || self.plates.len() > 64
            || self.accessories.len() > 64
            || self.routes.len() > 64
            || self.bindings.len() > 128
        {
            return Err("hardware inventory count budget exceeded".into());
        }
        unique(self.machines.iter().map(|x| x.id.as_str()))?;
        unique(self.plates.iter().map(|x| x.id.as_str()))?;
        unique(self.accessories.iter().map(|x| x.id.as_str()))?;
        unique(self.routes.iter().map(|x| x.id.as_str()))?;
        for m in &self.machines {
            m.validate()?;
        }
        let machine = |id: &str| -> Result<&OwnedMachine, String> {
            self.machines
                .iter()
                .find(|m| m.id == id)
                .ok_or_else(|| "hardware record references a missing owned machine".into())
        };
        for p in &self.plates {
            p.validate()?;
            if let Some(id) = &p.installed_machine
                && p.definition.mounting_interface
                    != machine(id)?.configured_setup()?.model.mounting_interface
            {
                return Err("installed plate mount mismatch".into());
            }
        }
        for a in &self.accessories {
            a.validate()?;
            if let Some(id) = &a.installed_machine
                && a.definition.mounting_interface
                    != machine(id)?.configured_setup()?.model.mounting_interface
            {
                return Err("installed accessory mount mismatch".into());
            }
        }
        for r in &self.routes {
            r.validate()?;
            machine(&r.machine_id)?;
        }
        let mut bindings = BTreeSet::new();
        for b in &self.bindings {
            b.validate()?;
            machine(&b.machine_id)?;
            if !self.plates.iter().any(|p| p.id == b.plate_id)
                || !bindings.insert((
                    &b.project_id,
                    &b.layout_id,
                    serde_json::to_string(&b.attachment).map_err(|e| e.to_string())?,
                ))
            {
                return Err("invalid or duplicate local project hardware binding".into());
            }
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_HARDWARE_BYTES {
            return Err("hardware inventory exceeds 48 KiB control budget".into());
        }
        Ok(())
    }
    pub fn capture(
        &self,
        machine: &str,
        plate: &str,
        face: &str,
    ) -> Result<ProjectHardwareCapture, String> {
        self.validate()?;
        let m = self
            .machines
            .iter()
            .find(|m| m.id == machine)
            .ok_or("owned machine missing")?;
        m.configured_setup()?;
        let p = self
            .plates
            .iter()
            .find(|p| p.id == plate)
            .ok_or("owned plate missing")?;
        if p.installed_machine
            .as_deref()
            .is_some_and(|id| id != machine)
        {
            return Err("selected plate is recorded installed on another machine".into());
        }
        let c = ProjectHardwareCapture {
            setup: m.configured_setup()?.clone(),
            plate: p.definition.clone(),
            face: face.into(),
            accessories: self
                .accessories
                .iter()
                .filter(|a| a.installed_machine.as_deref() == Some(machine))
                .map(|a| a.definition.clone())
                .collect(),
        };
        c.validate()?;
        Ok(c)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryHardware {
    #[serde(deserialize_with = "required_option")]
    pub identity: Option<crate::library::LibraryIdentity>,
    pub hardware: HardwareLibrary,
}
impl LibraryHardware {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(i) = &self.identity {
            i.validate()?
        }
        if self.identity.is_none() && self.hardware != HardwareLibrary::default() {
            return Err("missing personal library cannot contain hardware records".into());
        }
        self.hardware.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HardwareCatalog {
    pub models: Vec<HardwareModelDefinition>,
    pub setups: Vec<HardwareSetupDefinition>,
    pub plates: Vec<PlateDefinition>,
}
impl HardwareCatalog {
    pub fn validate(&self) -> Result<(), String> {
        if self.models.len() > 64 || self.setups.len() > 16 || self.plates.len() > 32 {
            return Err("hardware catalog budget exceeded".into());
        }
        unique(self.models.iter().map(|m| m.id.as_str()))?;
        unique(self.setups.iter().map(|s| s.id.as_str()))?;
        unique(self.plates.iter().map(|p| p.id.as_str()))?;
        for m in &self.models {
            m.validate()?
        }
        for s in &self.setups {
            s.validate()?;
            if !self.models.contains(&s.model) {
                return Err("setup model does not match catalog definition".into());
            }
        }
        for p in &self.plates {
            p.validate()?
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_HARDWARE_BYTES {
            return Err("hardware catalog exceeds bounded control size".into());
        }
        Ok(())
    }
}
pub fn builtin_catalog() -> Result<HardwareCatalog, String> {
    let c: HardwareCatalog = serde_json::from_str(include_str!("../data/hardware-catalog.json"))
        .map_err(|e| e.to_string())?;
    c.validate()?;
    Ok(c)
}

#[cfg(test)]
mod tests;
