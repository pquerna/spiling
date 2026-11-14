// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Checked hardware wire adapters. Private records never enter the capture type.
use super::required;
use crate::{geometry::SourceHash, hardware::*, rpc};
impl TryFrom<rpc::HardwareModelDefinition> for HardwareModelDefinition {
    type Error = String;
    fn try_from(v: rpc::HardwareModelDefinition) -> Result<Self, String> {
        let value = Self {
            id: v.id,
            vendor: v.vendor,
            model: v.model,
            source: v.source,
            mounting_interface: v.mounting_interface,
            declared_volume_mm: v.declared_volume_mm.map(TryInto::try_into).transpose()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<HardwareModelDefinition> for rpc::HardwareModelDefinition {
    type Error = String;
    fn try_from(v: HardwareModelDefinition) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            id: v.id,
            vendor: v.vendor,
            model: v.model,
            source: v.source,
            mounting_interface: v.mounting_interface,
            declared_volume_mm: v.declared_volume_mm.map(TryInto::try_into).transpose()?,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::HardwareSetupDefinition> for HardwareSetupDefinition {
    type Error = String;
    fn try_from(v: rpc::HardwareSetupDefinition) -> Result<Self, String> {
        let value = Self {
            id: v.id,
            label: v.label,
            model: required(v.model, "HardwareSetupDefinition.model")?.try_into()?,
            machine: v.machine.map(TryInto::try_into).transpose()?,
            printer: required(v.printer, "HardwareSetupDefinition.printer")?.try_into()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<HardwareSetupDefinition> for rpc::HardwareSetupDefinition {
    type Error = String;
    fn try_from(v: HardwareSetupDefinition) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            id: v.id,
            label: v.label,
            model: Some(v.model.try_into()?),
            machine: v.machine.map(TryInto::try_into).transpose()?,
            printer: Some(v.printer.try_into()?),
        };
        Ok(value)
    }
}
impl TryFrom<rpc::PlateFaceDefinition> for PlateFaceDefinition {
    type Error = String;
    fn try_from(v: rpc::PlateFaceDefinition) -> Result<Self, String> {
        let value = Self {
            key: v.key,
            label: v.label,
            surface: v.surface,
            usable_mm: required(v.usable_mm, "PlateFaceDefinition.usable_mm")?.try_into()?,
            keepouts_mm: v
                .keepouts_mm
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            max_bed_celsius: v.max_bed_celsius,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<PlateFaceDefinition> for rpc::PlateFaceDefinition {
    type Error = String;
    fn try_from(v: PlateFaceDefinition) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            key: v.key,
            label: v.label,
            surface: v.surface,
            usable_mm: Some(v.usable_mm.try_into()?),
            keepouts_mm: v
                .keepouts_mm
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            max_bed_celsius: v.max_bed_celsius,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::PlateDefinition> for PlateDefinition {
    type Error = String;
    fn try_from(v: rpc::PlateDefinition) -> Result<Self, String> {
        let value = Self {
            id: v.id,
            label: v.label,
            mounting_interface: v.mounting_interface,
            thickness_mm: v.thickness_mm,
            surface_z_mm: v.surface_z_mm,
            faces: v
                .faces
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            source: v.source,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<PlateDefinition> for rpc::PlateDefinition {
    type Error = String;
    fn try_from(v: PlateDefinition) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            id: v.id,
            label: v.label,
            mounting_interface: v.mounting_interface,
            thickness_mm: v.thickness_mm,
            surface_z_mm: v.surface_z_mm,
            faces: v
                .faces
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            source: v.source,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::ProjectHardwareCapture> for ProjectHardwareCapture {
    type Error = String;
    fn try_from(v: rpc::ProjectHardwareCapture) -> Result<Self, String> {
        let value = Self {
            setup: required(v.setup, "ProjectHardwareCapture.setup")?.try_into()?,
            plate: required(v.plate, "ProjectHardwareCapture.plate")?.try_into()?,
            face: v.face,
            accessories: v
                .accessories
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<ProjectHardwareCapture> for rpc::ProjectHardwareCapture {
    type Error = String;
    fn try_from(v: ProjectHardwareCapture) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            setup: Some(v.setup.try_into()?),
            plate: Some(v.plate.try_into()?),
            face: v.face,
            accessories: v
                .accessories
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::PrinterIdentity> for PrinterIdentity {
    type Error = String;
    fn try_from(v: rpc::PrinterIdentity) -> Result<Self, String> {
        let value = Self {
            manufacturer: v.manufacturer,
            model: v.model,
            catalog_origin: v
                .catalog_origin
                .map(|v| (*v).try_into().map(Box::new))
                .transpose()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<PrinterIdentity> for rpc::PrinterIdentity {
    type Error = String;
    fn try_from(v: PrinterIdentity) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            manufacturer: v.manufacturer,
            model: v.model,
            catalog_origin: v
                .catalog_origin
                .map(|v| (*v).try_into().map(Box::new))
                .transpose()?,
        })
    }
}
impl TryFrom<rpc::OwnedMachine> for OwnedMachine {
    type Error = String;
    fn try_from(v: rpc::OwnedMachine) -> Result<Self, String> {
        let configuration = match required(v.configuration, "OwnedMachine.configuration")? {
            rpc::owned_machine::Configuration::Setup(setup) => MachineConfiguration::Configured {
                setup: Box::new((*setup).try_into()?),
            },
            rpc::owned_machine::Configuration::Unconfigured(()) => {
                MachineConfiguration::Unconfigured
            }
        };
        let value = Self {
            id: v.id,
            nickname: v.nickname,
            serial: v.serial,
            location: v.location,
            identity: required(v.identity, "OwnedMachine.identity")?.try_into()?,
            configuration,
            generation: v.generation,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<OwnedMachine> for rpc::OwnedMachine {
    type Error = String;
    fn try_from(v: OwnedMachine) -> Result<Self, String> {
        v.validate()?;
        let configuration = Some(match v.configuration {
            MachineConfiguration::Configured { setup } => {
                rpc::owned_machine::Configuration::Setup(Box::new((*setup).try_into()?))
            }
            MachineConfiguration::Unconfigured => {
                rpc::owned_machine::Configuration::Unconfigured(())
            }
        });
        Ok(Self {
            id: v.id,
            nickname: v.nickname,
            serial: v.serial,
            location: v.location,
            identity: Some(v.identity.try_into()?),
            configuration,
            generation: v.generation,
        })
    }
}
impl TryFrom<rpc::OwnedPlate> for OwnedPlate {
    type Error = String;
    fn try_from(v: rpc::OwnedPlate) -> Result<Self, String> {
        let value = Self {
            id: v.id,
            nickname: v.nickname,
            definition: required(v.definition, "OwnedPlate.definition")?.try_into()?,
            installed_machine: v.installed_machine,
            notes: v.notes,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<OwnedPlate> for rpc::OwnedPlate {
    type Error = String;
    fn try_from(v: OwnedPlate) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            id: v.id,
            nickname: v.nickname,
            definition: Some(v.definition.try_into()?),
            installed_machine: v.installed_machine,
            notes: v.notes,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::OwnedAccessory> for OwnedAccessory {
    type Error = String;
    fn try_from(v: rpc::OwnedAccessory) -> Result<Self, String> {
        let value = Self {
            id: v.id,
            nickname: v.nickname,
            definition: required(v.definition, "OwnedAccessory.definition")?.try_into()?,
            installed_machine: v.installed_machine,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<OwnedAccessory> for rpc::OwnedAccessory {
    type Error = String;
    fn try_from(v: OwnedAccessory) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            id: v.id,
            nickname: v.nickname,
            definition: Some(v.definition.try_into()?),
            installed_machine: v.installed_machine,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::EndpointRoute> for EndpointRoute {
    type Error = String;
    fn try_from(v: rpc::EndpointRoute) -> Result<Self, String> {
        let value = Self {
            id: v.id,
            machine_id: v.machine_id,
            family: v.family.as_str().try_into()?,
            endpoint: v.endpoint,
            expected_identity: v.expected_identity,
            generation: v.generation,
            credential: v.credential.map(TryInto::try_into).transpose()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<EndpointRoute> for rpc::EndpointRoute {
    type Error = String;
    fn try_from(v: EndpointRoute) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            id: v.id,
            machine_id: v.machine_id,
            family: v.family.into(),
            endpoint: v.endpoint,
            expected_identity: v.expected_identity,
            generation: v.generation,
            credential: v.credential.map(TryInto::try_into).transpose()?,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::CredentialRef> for CredentialRef {
    type Error = String;
    fn try_from(v: rpc::CredentialRef) -> Result<Self, String> {
        let value = Self {
            slot_id: v.slot_id,
            role: v.role.as_str().try_into()?,
            backend: v.backend.as_str().try_into()?,
        };
        Ok(value)
    }
}
impl TryFrom<CredentialRef> for rpc::CredentialRef {
    type Error = String;
    fn try_from(v: CredentialRef) -> Result<Self, String> {
        let value = Self {
            slot_id: v.slot_id,
            role: v.role.into(),
            backend: v.backend.into(),
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LocalProjectHardwareBinding> for LocalProjectHardwareBinding {
    type Error = String;
    fn try_from(v: rpc::LocalProjectHardwareBinding) -> Result<Self, String> {
        let value = Self {
            project_id: v.project_id,
            layout_id: crate::project::PrintLayoutId::parse(v.layout_id)
                .map_err(|e| e.to_string())?,
            attachment: required(v.attachment, "LocalProjectHardwareBinding.attachment")?
                .try_into()?,
            capture_hash: SourceHash::parse(v.capture_hash).map_err(|e| e.to_string())?,
            machine_id: v.machine_id,
            plate_id: v.plate_id,
            machine_generation: v.machine_generation,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LocalProjectHardwareBinding> for rpc::LocalProjectHardwareBinding {
    type Error = String;
    fn try_from(v: LocalProjectHardwareBinding) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            project_id: v.project_id,
            layout_id: v.layout_id.as_str().into(),
            attachment: Some(v.attachment.try_into()?),
            capture_hash: v.capture_hash.as_str().into(),
            machine_id: v.machine_id,
            plate_id: v.plate_id,
            machine_generation: v.machine_generation,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::HardwareLibrary> for HardwareLibrary {
    type Error = String;
    fn try_from(v: rpc::HardwareLibrary) -> Result<Self, String> {
        let value = Self {
            machines: v
                .machines
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            plates: v
                .plates
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            accessories: v
                .accessories
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            routes: v
                .routes
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            bindings: v
                .bindings
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<HardwareLibrary> for rpc::HardwareLibrary {
    type Error = String;
    fn try_from(v: HardwareLibrary) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            machines: v
                .machines
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            plates: v
                .plates
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            accessories: v
                .accessories
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            routes: v
                .routes
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            bindings: v
                .bindings
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryHardware> for LibraryHardware {
    type Error = String;
    fn try_from(v: rpc::LibraryHardware) -> Result<Self, String> {
        let value = Self {
            identity: v.identity.map(TryInto::try_into).transpose()?,
            hardware: required(v.hardware, "LibraryHardware.hardware")?.try_into()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryHardware> for rpc::LibraryHardware {
    type Error = String;
    fn try_from(v: LibraryHardware) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            identity: v.identity.map(TryInto::try_into).transpose()?,
            hardware: Some(v.hardware.try_into()?),
        };
        Ok(value)
    }
}
impl TryFrom<&str> for CredentialBackend {
    type Error = String;
    fn try_from(v: &str) -> Result<Self, String> {
        match v {
            "keyring" => Ok(Self::Keyring),
            "plaintext_file" => Ok(Self::PlaintextFile),
            _ => Err("unknown hardware enum value".into()),
        }
    }
}
impl From<CredentialBackend> for String {
    fn from(v: CredentialBackend) -> Self {
        match v {
            CredentialBackend::Keyring => "keyring".into(),
            CredentialBackend::PlaintextFile => "plaintext_file".into(),
        }
    }
}
impl TryFrom<&str> for CredentialRole {
    type Error = String;
    fn try_from(v: &str) -> Result<Self, String> {
        match v {
            "lan-access-code" => Ok(Self::LanAccessCode),
            "api-key" => Ok(Self::ApiKey),
            "password" => Ok(Self::Password),
            "refresh-token" => Ok(Self::RefreshToken),
            _ => Err("unknown hardware enum value".into()),
        }
    }
}
impl From<CredentialRole> for String {
    fn from(v: CredentialRole) -> Self {
        match v {
            CredentialRole::LanAccessCode => "lan-access-code".into(),
            CredentialRole::ApiKey => "api-key".into(),
            CredentialRole::Password => "password".into(),
            CredentialRole::RefreshToken => "refresh-token".into(),
        }
    }
}
impl TryFrom<&str> for ConnectionFamily {
    type Error = String;
    fn try_from(v: &str) -> Result<Self, String> {
        match v {
            "file" => Ok(Self::File),
            "bambu_connect" => Ok(Self::BambuConnect),
            "bambu_developer_lan" => Ok(Self::BambuDeveloperLan),
            "moonraker" => Ok(Self::Moonraker),
            "octo_print" => Ok(Self::OctoPrint),
            "prusa_link" => Ok(Self::PrusaLink),
            "development_simulator" => Ok(Self::DevelopmentSimulator),
            _ => Err("unknown hardware enum value".into()),
        }
    }
}
impl From<ConnectionFamily> for String {
    fn from(v: ConnectionFamily) -> Self {
        match v {
            ConnectionFamily::File => "file".into(),
            ConnectionFamily::BambuConnect => "bambu_connect".into(),
            ConnectionFamily::BambuDeveloperLan => "bambu_developer_lan".into(),
            ConnectionFamily::Moonraker => "moonraker".into(),
            ConnectionFamily::OctoPrint => "octo_print".into(),
            ConnectionFamily::PrusaLink => "prusa_link".into(),
            ConnectionFamily::DevelopmentSimulator => "development_simulator".into(),
        }
    }
}
impl TryFrom<rpc::AccessoryDefinition> for AccessoryDefinition {
    type Error = String;
    fn try_from(v: rpc::AccessoryDefinition) -> Result<Self, String> {
        use rpc::accessory_definition::Effect;
        let value = Self {
            id: v.id,
            label: v.label,
            mounting_interface: v.mounting_interface,
            effect: match required(v.effect, "accessory effect")? {
                Effect::Nozzle(n) => AccessoryEffect::Nozzle {
                    diameter_mm: n.diameter_mm,
                    material: n.material,
                },
                Effect::Fixture(b) => AccessoryEffect::Fixture {
                    bounds_mm: b.try_into()?,
                },
            },
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<AccessoryDefinition> for rpc::AccessoryDefinition {
    type Error = String;
    fn try_from(v: AccessoryDefinition) -> Result<Self, String> {
        v.validate()?;
        use rpc::accessory_definition::Effect;
        Ok(Self {
            id: v.id,
            label: v.label,
            mounting_interface: v.mounting_interface,
            effect: Some(match v.effect {
                AccessoryEffect::Nozzle {
                    diameter_mm,
                    material,
                } => Effect::Nozzle(rpc::HardwareNozzle {
                    diameter_mm,
                    material,
                }),
                AccessoryEffect::Fixture { bounds_mm } => Effect::Fixture(bounds_mm.try_into()?),
            }),
        })
    }
}

impl TryFrom<rpc::HardwareCatalog> for HardwareCatalog {
    type Error = String;
    fn try_from(v: rpc::HardwareCatalog) -> Result<Self, String> {
        let c = Self {
            models: v
                .models
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            setups: v
                .setups
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            plates: v
                .plates
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        c.validate()?;
        Ok(c)
    }
}
impl TryFrom<HardwareCatalog> for rpc::HardwareCatalog {
    type Error = String;
    fn try_from(v: HardwareCatalog) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            models: v
                .models
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            setups: v
                .setups
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            plates: v
                .plates
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

fn enum_index<T: Copy + PartialEq>(all: &[T], v: T) -> i32 {
    all.iter().position(|x| *x == v).map_or(0, |i| i as i32 + 1)
}
fn enum_value<T: Copy>(all: &[T], v: i32, what: &str) -> Result<T, String> {
    usize::try_from(v - 1)
        .ok()
        .and_then(|i| all.get(i).copied())
        .ok_or_else(|| format!("unsupported {what}"))
}
const FIRMWARE: [FirmwareFamily; 4] = [
    FirmwareFamily::Marlin2,
    FirmwareFamily::PrusaBuddy,
    FirmwareFamily::Klipper,
    FirmwareFamily::BambuLab,
];
const KINEMATICS: [MachineKinematics; 2] =
    [MachineKinematics::BedSlinger, MachineKinematics::CoreXy];
const LEVELLING: [BedLevelling; 3] = [
    BedLevelling::Manual,
    BedLevelling::ProbeMesh,
    BedLevelling::StoredMesh,
];
impl TryFrom<rpc::MachineDescription> for MachineDescription {
    type Error = String;
    fn try_from(v: rpc::MachineDescription) -> Result<Self, String> {
        use rpc::fact_source::Kind;
        let value = Self {
            id: v.id,
            lineage: v.lineage,
            firmware: enum_value(&FIRMWARE, v.firmware, "firmware family")?,
            kinematics: enum_value(&KINEMATICS, v.kinematics, "machine kinematics")?,
            levelling: enum_value(&LEVELLING, v.levelling, "bed levelling")?,
            heated_bed: v.heated_bed,
            arc_moves: v.arc_moves,
            extruder_drive: super::manufacturing::drive_from_rpc(v.extruder_drive)?,
            printer_model: v.printer_model,
            start_block: v.start_block.map(block_from_rpc).transpose()?,
            end_block: v.end_block.map(block_from_rpc).transpose()?,
            limits: v
                .limits
                .into_iter()
                .map(|l| {
                    Ok(SourcedLimit {
                        key: enum_value(&MachineLimit::ALL, l.key, "machine limit")?,
                        value: l.value,
                        source: match required(l.source.and_then(|s| s.kind), "FactSource.kind")? {
                            Kind::Published(reference) => FactSource::Published { reference },
                            Kind::FamilyDefault(family) => FactSource::FamilyDefault { family },
                            Kind::OperatorDeclared(()) => FactSource::OperatorDeclared,
                        },
                    })
                })
                .collect::<Result<_, String>>()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<MachineDescription> for rpc::MachineDescription {
    type Error = String;
    fn try_from(v: MachineDescription) -> Result<Self, String> {
        use rpc::fact_source::Kind;
        v.validate()?;
        Ok(Self {
            firmware: enum_index(&FIRMWARE, v.firmware),
            kinematics: enum_index(&KINEMATICS, v.kinematics),
            levelling: enum_index(&LEVELLING, v.levelling),
            heated_bed: v.heated_bed,
            arc_moves: v.arc_moves,
            extruder_drive: super::manufacturing::drive_to_rpc(v.extruder_drive),
            printer_model: v.printer_model,
            start_block: v.start_block.map(block_to_rpc),
            end_block: v.end_block.map(block_to_rpc),
            limits: v
                .limits
                .into_iter()
                .map(|l| rpc::SourcedLimit {
                    key: enum_index(&MachineLimit::ALL, l.key),
                    value: l.value,
                    source: Some(rpc::FactSource {
                        kind: Some(match l.source {
                            FactSource::Published { reference } => Kind::Published(reference),
                            FactSource::FamilyDefault { family } => Kind::FamilyDefault(family),
                            FactSource::OperatorDeclared => Kind::OperatorDeclared(()),
                        }),
                    }),
                })
                .collect(),
            id: v.id,
            lineage: v.lineage,
        })
    }
}

use crate::manufacturing::{
    BlockEffects, ExtrusionEffect, FirmwareLifecycle, LevellingEffect, LifecycleBlock,
    LifecycleStep, ModeEffect, PositionEffect, ThermalEffect,
};
const POSITION: [PositionEffect; 2] = [PositionEffect::Preserved, PositionEffect::Unknown];
const THERMAL: [ThermalEffect; 4] = [
    ThermalEffect::Unchanged,
    ThermalEffect::Target,
    ThermalEffect::Wait,
    ThermalEffect::Off,
];
const LEVELLING_EFFECT: [LevellingEffect; 3] = [
    LevellingEffect::Unchanged,
    LevellingEffect::Active,
    LevellingEffect::Off,
];
const EXTRUSION: [ExtrusionEffect; 3] = [
    ExtrusionEffect::Preserved,
    ExtrusionEffect::Reset,
    ExtrusionEffect::Unknown,
];
const MODES: [ModeEffect; 2] = [ModeEffect::Preserved, ModeEffect::Unknown];

fn block_from_rpc(v: rpc::LifecycleBlock) -> Result<LifecycleBlock, String> {
    let e = required(v.effects, "block effects")?;
    Ok(LifecycleBlock {
        label: v.label,
        lines: v.lines,
        effects: BlockEffects {
            homes: e.homes,
            position: enum_value(&POSITION, e.position, "position effect")?,
            levelling: enum_value(&LEVELLING_EFFECT, e.levelling, "levelling effect")?,
            bed: enum_value(&THERMAL, e.bed, "bed effect")?,
            nozzle: enum_value(&THERMAL, e.nozzle, "nozzle effect")?,
            extrusion: enum_value(&EXTRUSION, e.extrusion, "extrusion effect")?,
            modes: enum_value(&MODES, e.modes, "mode effect")?,
            fan_off: e.fan_off,
            motors_off: e.motors_off,
        },
    })
}
fn block_to_rpc(v: LifecycleBlock) -> rpc::LifecycleBlock {
    let e = v.effects;
    rpc::LifecycleBlock {
        label: v.label,
        lines: v.lines,
        effects: Some(rpc::BlockEffects {
            homes: e.homes,
            position: enum_index(&POSITION, e.position),
            levelling: enum_index(&LEVELLING_EFFECT, e.levelling),
            bed: enum_index(&THERMAL, e.bed),
            nozzle: enum_index(&THERMAL, e.nozzle),
            extrusion: enum_index(&EXTRUSION, e.extrusion),
            modes: enum_index(&MODES, e.modes),
            fan_off: e.fan_off,
            motors_off: e.motors_off,
        }),
    }
}
fn step_from_rpc(v: rpc::LifecycleStep) -> Result<LifecycleStep, String> {
    use LifecycleStep as S;
    use rpc::lifecycle_step::Step as R;
    Ok(match required(v.step, "lifecycle step")? {
        R::CelsiusUnits(()) => S::CelsiusUnits,
        R::VolumetricOff(()) => S::VolumetricOff,
        R::FeedScaleReset(()) => S::FeedScaleReset,
        R::FlowScaleReset(()) => S::FlowScaleReset,
        R::FanOff(()) => S::FanOff,
        R::NozzleCheck(()) => S::NozzleCheck,
        R::ModelCheck(model) => S::ModelCheck { model },
        R::BedTarget(()) => S::BedTarget,
        R::BedWait(()) => S::BedWait,
        R::NozzleTarget(stage) => S::NozzleTarget {
            standby_c: stage.standby_c,
        },
        R::NozzleWait(stage) => S::NozzleWait {
            standby_c: stage.standby_c,
        },
        R::Home(()) => S::Home,
        R::ProbeMesh(()) => S::ProbeMesh,
        R::RestoreMesh(()) => S::RestoreMesh,
        R::LevellingOff(()) => S::LevellingOff,
        R::RestoreModes(()) => S::RestoreModes,
        R::ResetExtrusion(()) => S::ResetExtrusion,
        R::Prime(()) => S::Prime,
        R::Barrier(()) => S::Barrier,
        R::Park(()) => S::Park,
        R::NozzleOff(()) => S::NozzleOff,
        R::BedOff(()) => S::BedOff,
        R::MotorsOff(()) => S::MotorsOff,
        R::Block(block) => S::Block {
            block: block_from_rpc(block)?,
        },
    })
}
fn step_to_rpc(v: LifecycleStep) -> rpc::LifecycleStep {
    use LifecycleStep as S;
    use rpc::lifecycle_step::Step as R;
    let step = match v {
        S::CelsiusUnits => R::CelsiusUnits(()),
        S::VolumetricOff => R::VolumetricOff(()),
        S::FeedScaleReset => R::FeedScaleReset(()),
        S::FlowScaleReset => R::FlowScaleReset(()),
        S::FanOff => R::FanOff(()),
        S::NozzleCheck => R::NozzleCheck(()),
        S::ModelCheck { model } => R::ModelCheck(model),
        S::BedTarget => R::BedTarget(()),
        S::BedWait => R::BedWait(()),
        S::NozzleTarget { standby_c } => R::NozzleTarget(rpc::NozzleStage { standby_c }),
        S::NozzleWait { standby_c } => R::NozzleWait(rpc::NozzleStage { standby_c }),
        S::Home => R::Home(()),
        S::ProbeMesh => R::ProbeMesh(()),
        S::RestoreMesh => R::RestoreMesh(()),
        S::LevellingOff => R::LevellingOff(()),
        S::RestoreModes => R::RestoreModes(()),
        S::ResetExtrusion => R::ResetExtrusion(()),
        S::Prime => R::Prime(()),
        S::Barrier => R::Barrier(()),
        S::Park => R::Park(()),
        S::NozzleOff => R::NozzleOff(()),
        S::BedOff => R::BedOff(()),
        S::MotorsOff => R::MotorsOff(()),
        S::Block { block } => R::Block(block_to_rpc(block)),
    };
    rpc::LifecycleStep { step: Some(step) }
}
/// Checked lifecycle adapters; the job's validation checks the sequence.
pub(crate) fn lifecycle_from_rpc(v: rpc::FirmwareLifecycle) -> Result<FirmwareLifecycle, String> {
    Ok(FirmwareLifecycle {
        start: v
            .start
            .into_iter()
            .map(step_from_rpc)
            .collect::<Result<_, _>>()?,
        end: v
            .end
            .into_iter()
            .map(step_from_rpc)
            .collect::<Result<_, _>>()?,
    })
}
pub(crate) fn lifecycle_to_rpc(v: FirmwareLifecycle) -> rpc::FirmwareLifecycle {
    rpc::FirmwareLifecycle {
        start: v.start.into_iter().map(step_to_rpc).collect(),
        end: v.end.into_iter().map(step_to_rpc).collect(),
    }
}
