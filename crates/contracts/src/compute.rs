// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ComputeBackend {
    #[default]
    Auto,
    Cpu,
    Vulkan,
}

impl ComputeBackend {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Cpu, Self::Vulkan];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::Vulkan => "vulkan",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|backend| backend.as_str() == value)
            .ok_or_else(|| format!("unsupported compute backend: {value}"))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ComputeSettings {
    pub default_backend: ComputeBackend,
    pub fea_backend: Option<ComputeBackend>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ComputeDevice {
    pub backend: ComputeBackend,
    pub name: String,
    pub hardware_accelerated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ComputeStatus {
    pub settings: ComputeSettings,
    pub default_device: ComputeDevice,
    pub fea_device: ComputeDevice,
}

impl ComputeStatus {
    pub fn validate(&self) -> Result<(), String> {
        for device in [&self.default_device, &self.fea_device] {
            if device.backend == ComputeBackend::Auto
                || device.name.is_empty()
                || device.name.len() > 256
                || device.name.contains('\0')
                || device.backend == ComputeBackend::Cpu && device.hardware_accelerated
            {
                return Err("invalid compute device".into());
            }
        }
        if self.settings.default_backend != ComputeBackend::Auto
            && self.settings.default_backend != self.default_device.backend
        {
            return Err("compute default does not match the requested backend".into());
        }
        match self.settings.fea_backend {
            None if self.default_device != self.fea_device => {
                Err("inherited FEA compute device differs from the default".into())
            }
            Some(backend)
                if backend != ComputeBackend::Auto && backend != self.fea_device.backend =>
            {
                Err("FEA compute device does not match the requested backend".into())
            }
            _ => Ok(()),
        }
    }
}

impl TryFrom<i32> for ComputeBackend {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match crate::rpc::ComputeBackend::try_from(value) {
            Ok(crate::rpc::ComputeBackend::Auto) => Ok(Self::Auto),
            Ok(crate::rpc::ComputeBackend::Cpu) => Ok(Self::Cpu),
            Ok(crate::rpc::ComputeBackend::Vulkan) => Ok(Self::Vulkan),
            _ => Err("missing or unknown compute backend".into()),
        }
    }
}

impl From<ComputeBackend> for crate::rpc::ComputeBackend {
    fn from(value: ComputeBackend) -> Self {
        match value {
            ComputeBackend::Auto => Self::Auto,
            ComputeBackend::Cpu => Self::Cpu,
            ComputeBackend::Vulkan => Self::Vulkan,
        }
    }
}

impl TryFrom<crate::rpc::ComputeStatus> for ComputeStatus {
    type Error = String;

    fn try_from(value: crate::rpc::ComputeStatus) -> Result<Self, Self::Error> {
        let settings = value.settings.ok_or("compute settings missing")?;
        let device = |value: Option<crate::rpc::ComputeDevice>| -> Result<ComputeDevice, String> {
            let value = value.ok_or("compute device missing")?;
            Ok(ComputeDevice {
                backend: value.backend.try_into()?,
                name: value.name,
                hardware_accelerated: value.hardware_accelerated,
            })
        };
        let status = Self {
            settings: ComputeSettings {
                default_backend: settings.default_backend.try_into()?,
                fea_backend: settings.fea_backend.map(TryInto::try_into).transpose()?,
            },
            default_device: device(value.default_device)?,
            fea_device: device(value.fea_device)?,
        };
        status.validate()?;
        Ok(status)
    }
}

impl TryFrom<&ComputeStatus> for crate::rpc::ComputeStatus {
    type Error = String;

    fn try_from(value: &ComputeStatus) -> Result<Self, Self::Error> {
        value.validate()?;
        let device = |value: &ComputeDevice| crate::rpc::ComputeDevice {
            backend: crate::rpc::ComputeBackend::from(value.backend) as i32,
            name: value.name.clone(),
            hardware_accelerated: value.hardware_accelerated,
        };
        Ok(Self {
            settings: Some(crate::rpc::ComputeSettings {
                default_backend: crate::rpc::ComputeBackend::from(value.settings.default_backend)
                    as i32,
                fea_backend: value
                    .settings
                    .fea_backend
                    .map(|backend| crate::rpc::ComputeBackend::from(backend) as i32),
            }),
            default_device: Some(device(&value.default_device)),
            fea_device: Some(device(&value.fea_device)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_are_two_level_and_strict() {
        assert_eq!(
            ComputeSettings::default().default_backend,
            ComputeBackend::Auto
        );
        assert_eq!(ComputeSettings::default().fea_backend, None);
        assert!(ComputeBackend::parse("inherit").is_err());
        assert!(
            serde_json::from_str::<ComputeSettings>(
                r#"{"default_backend":"auto","fea_backend":null,"planner_backend":"cpu"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn inheritance_and_explicit_backend_are_checked() {
        let device = ComputeDevice {
            backend: ComputeBackend::Cpu,
            name: "CPU".into(),
            hardware_accelerated: false,
        };
        let mut status = ComputeStatus {
            settings: ComputeSettings::default(),
            default_device: device.clone(),
            fea_device: device,
        };
        status.validate().unwrap();
        status.fea_device.name = "different".into();
        assert!(status.validate().is_err());
        status.settings.fea_backend = Some(ComputeBackend::Cpu);
        status.validate().unwrap();
        status.settings.default_backend = ComputeBackend::Vulkan;
        assert!(status.validate().is_err());
    }

    #[test]
    fn wire_roundtrip_requires_complete_known_compute_identity() {
        let device = ComputeDevice {
            backend: ComputeBackend::Cpu,
            name: "Test CPU".into(),
            hardware_accelerated: false,
        };
        let status = ComputeStatus {
            settings: ComputeSettings::default(),
            default_device: device.clone(),
            fea_device: device,
        };
        let wire = crate::rpc::ComputeStatus::try_from(&status).unwrap();
        assert_eq!(ComputeStatus::try_from(wire.clone()).unwrap(), status);
        let mut missing = wire.clone();
        missing.settings = None;
        assert!(ComputeStatus::try_from(missing).is_err());
        let mut unknown = wire.clone();
        unknown.default_device.as_mut().unwrap().backend = 99;
        assert!(ComputeStatus::try_from(unknown).is_err());
        let mut unspecified = wire;
        unspecified.settings.as_mut().unwrap().fea_backend = Some(0);
        assert!(ComputeStatus::try_from(unspecified).is_err());
    }
}
