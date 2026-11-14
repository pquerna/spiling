// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! The one-step defaults request: printer (the project's captured hardware) +
//! material + quality + supports. The engine creates a complete, ordinary
//! intent from it; every choice and every fit stays visible in that intent
//! (hardware capture, material selection and its overrides, recipe identity
//! naming the preset, support policy).
use super::{ManufacturingError, ManufacturingErrorCode, MaterialProfileSelection};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Print quality: layer heights and bead widths from the nozzle, the way
/// PrusaSlicer's DRAFT / SPEED (standard) / QUALITY presets scale them
/// (0.28 / 0.20 / 0.15 mm at a 0.4 mm nozzle, first layer 0.2 mm, 0.45 mm
/// beads).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    Draft,
    Standard,
    Fine,
}
impl QualityPreset {
    pub const ALL: [Self; 3] = [Self::Draft, Self::Standard, Self::Fine];
    pub fn layer_height_mm(self, nozzle_mm: f64) -> f64 {
        let ratio = match self {
            Self::Draft => 0.7,
            Self::Standard => 0.5,
            Self::Fine => 0.375,
        };
        (ratio * nozzle_mm * 100.0).round() / 100.0
    }
    pub fn first_layer_height_mm(self, nozzle_mm: f64) -> f64 {
        (0.5 * nozzle_mm * 100.0).round() / 100.0
    }
    pub fn bead_width_mm(self, nozzle_mm: f64) -> f64 {
        (1.125 * nozzle_mm * 1_000_000.0).round() / 1_000_000.0
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Standard => "standard",
            Self::Fine => "fine",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Standard => "Standard",
            Self::Fine => "Fine",
        }
    }
    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.key() == key)
    }
    /// The recipe identity a preset creates; it names the preset in the intent.
    pub fn recipe_id(self) -> String {
        format!("spiling.default.{}", self.key())
    }
    /// The preset a recipe identity was created from, if any.
    pub fn from_recipe_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.recipe_id() == id)
    }
}

/// Generated supports: `Automatic` uses the printer's declared support
/// capability (refused when it declares none); `Off` creates none.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupportChoice {
    Automatic,
    Off,
}

/// Where the material profile comes from.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DefaultMaterialChoice {
    /// The shipped generic profile of a polymer family (`PLA`, `PETG`, `ABS`,
    /// `ASA`, `TPU`; labels such as `PLA+` name their family) for the
    /// printer's nozzle.
    Generic { polymer_family: String },
    /// A filament captured in the project (from the catalog or the personal
    /// library). Its profile for the printer's nozzle is used; `profile_id`
    /// chooses when several match.
    ProjectFilament {
        reference: crate::library::FilamentRef,
        profile_id: Option<String>,
    },
    /// An explicit profile selection (a library profile with its tool setup).
    Profile {
        selection: Box<MaterialProfileSelection>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DefaultIntentRequest {
    pub material: DefaultMaterialChoice,
    pub quality: QualityPreset,
    pub supports: SupportChoice,
}
impl DefaultIntentRequest {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let invalid =
            |m: String| ManufacturingError::new(ManufacturingErrorCode::InvalidSpecification, m);
        match &self.material {
            DefaultMaterialChoice::Generic { polymer_family } => {
                if polymer_family.is_empty()
                    || polymer_family.len() > 64
                    || polymer_family.chars().any(char::is_control)
                {
                    return Err(invalid("invalid polymer family".into()));
                }
                if super::generic_polymer_family(polymer_family).is_none() {
                    return Err(invalid(format!(
                        "no generic profile covers material {polymer_family:?}; known families: {}",
                        GENERIC_FAMILIES.join(", ")
                    )));
                }
            }
            DefaultMaterialChoice::ProjectFilament {
                reference,
                profile_id,
            } => {
                reference.validate().map_err(invalid)?;
                if profile_id
                    .as_ref()
                    .is_some_and(|p| p.is_empty() || p.len() > 256)
                {
                    return Err(invalid("invalid profile id".into()));
                }
            }
            DefaultMaterialChoice::Profile { selection } => selection.validate()?,
        }
        Ok(())
    }
}

/// The polymer families the shipped generic profiles cover.
pub const GENERIC_FAMILIES: [&str; 5] = ["PLA", "PETG", "ABS", "ASA", "TPU"];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_name_their_recipe_and_scale_from_the_nozzle() {
        for q in QualityPreset::ALL {
            assert_eq!(QualityPreset::parse(q.key()), Some(q));
            assert_eq!(QualityPreset::from_recipe_id(&q.recipe_id()), Some(q));
        }
        assert_eq!(QualityPreset::Standard.layer_height_mm(0.4), 0.2);
        assert_eq!(QualityPreset::Draft.layer_height_mm(0.4), 0.28);
        assert_eq!(QualityPreset::Fine.layer_height_mm(0.4), 0.15);
        assert_eq!(QualityPreset::parse("ultra"), None);
    }
    #[test]
    fn unknown_families_refuse_with_the_known_list() {
        let request = DefaultIntentRequest {
            material: DefaultMaterialChoice::Generic {
                polymer_family: "nylon".into(),
            },
            quality: QualityPreset::Standard,
            supports: SupportChoice::Off,
        };
        let error = request.validate().unwrap_err();
        assert!(error.message.contains("PLA, PETG"), "{}", error.message);
        let request = DefaultIntentRequest {
            material: DefaultMaterialChoice::Generic {
                polymer_family: "pla+".into(),
            },
            ..request
        };
        request.validate().unwrap();
    }
}
