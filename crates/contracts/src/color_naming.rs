// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Derived English labels for encoded sRGB samples, never material identity or calibration.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const COLOR_NAMING_VERSION: u32 = 1;
pub const COLOR_NAMING_DATASET_SHA256: &str =
    "450cca88fa6fa9a1e79c969969e05e6900b41a94f0a3a5f134e3d0b79077f890";
pub const MAX_COLOR_NAMING_SAMPLES: usize = 64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorNamingMode {
    Exact,
    Nearest,
    #[default]
    Descriptive,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ColorModifier {
    Dark,
    Light,
    Muted,
    Pale,
    Vivid,
}
impl ColorModifier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Muted => "Muted",
            Self::Pale => "Pale",
            Self::Vivid => "Vivid",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColorName {
    pub label: String,
    /// Original xkcd survey spelling; a reference sample, not a material ID.
    pub reference_name: String,
    pub reference_srgb8: [u8; 3],
    pub modifier: Option<ColorModifier>,
    pub generated: bool,
    /// Euclidean distance in normalized OKLab, not a calibrated confidence.
    pub distance_oklab: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NamedColor {
    pub srgb8: [u8; 3],
    pub hex: String,
    /// Exact mode has no name when the input is absent from the pinned survey.
    pub name: Option<ColorName>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ColorNamingRequest {
    pub mode: ColorNamingMode,
    pub colors: Vec<[u8; 3]>,
}
impl ColorNamingRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.colors.is_empty() || self.colors.len() > MAX_COLOR_NAMING_SAMPLES {
            return Err("color naming requires 1..64 encoded sRGB samples".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColorNamingResponse {
    pub algorithm_version: u32,
    pub dataset_sha256: String,
    pub mode: ColorNamingMode,
    pub colors: Vec<NamedColor>,
}
impl ColorNamingResponse {
    pub fn validate(&self) -> Result<(), String> {
        ColorNamingRequest {
            mode: self.mode,
            colors: self.colors.iter().map(|v| v.srgb8).collect(),
        }
        .validate()?;
        if self.algorithm_version != COLOR_NAMING_VERSION
            || self.dataset_sha256 != COLOR_NAMING_DATASET_SHA256
        {
            return Err("unknown color naming algorithm or dataset".into());
        }
        for color in &self.colors {
            if color.hex != srgb8_hex(color.srgb8) {
                return Err("color naming hex disagrees with encoded sRGB".into());
            }
            let Some(name) = &color.name else {
                if self.mode != ColorNamingMode::Exact {
                    return Err("non-exact color naming requires a label".into());
                }
                continue;
            };
            for text in [&name.label, &name.reference_name] {
                if text.is_empty() || text.len() > 96 || text.chars().any(char::is_control) {
                    return Err("invalid color naming text".into());
                }
            }
            let expected_label = name.modifier.map_or_else(
                || {
                    let mut chars = name.reference_name.chars();
                    chars
                        .next()
                        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
                },
                |modifier| format!("{} {}", modifier.label(), name.reference_name),
            );
            if name.label != expected_label {
                return Err("color label disagrees with its reference/modifier".into());
            }
            if !name.distance_oklab.is_finite()
                || !(0.0..=2.0).contains(&name.distance_oklab)
                || name.generated != name.modifier.is_some()
                || (self.mode != ColorNamingMode::Descriptive && name.generated)
                || (self.mode == ColorNamingMode::Exact
                    && (name.reference_srgb8 != color.srgb8 || name.distance_oklab != 0.0))
                || (name.reference_srgb8 == color.srgb8
                    && (name.distance_oklab != 0.0 || name.generated))
            {
                return Err("invalid color naming match provenance".into());
            }
        }
        Ok(())
    }
}
pub fn srgb8_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}
pub fn parse_srgb8_hex(value: &str) -> Result<[u8; 3], String> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 || !value.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err("color requires six hexadecimal sRGB digits, optionally prefixed by #; alpha is not supported".into());
    }
    Ok(std::array::from_fn(|i| {
        u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).expect("checked ASCII hex")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hex_has_explicit_srgb_and_no_implicit_alpha_or_shorthand() {
        assert_eq!(parse_srgb8_hex("#cf524E").unwrap(), [207, 82, 78]);
        assert_eq!(parse_srgb8_hex("cf524e").unwrap(), [207, 82, 78]);
        for input in ["#fff", "#ffffffff", " #FFFFFF", "#gg0033", "💙0000", ""] {
            assert!(parse_srgb8_hex(input).is_err(), "{input}");
        }
        assert_eq!(srgb8_hex([0, 128, 255]), "#0080FF");
    }
    #[test]
    fn batches_are_bounded_before_transport() {
        for colors in [vec![], vec![[0; 3]; MAX_COLOR_NAMING_SAMPLES + 1]] {
            assert!(
                ColorNamingRequest {
                    mode: ColorNamingMode::Nearest,
                    colors
                }
                .validate()
                .is_err()
            );
        }
    }
}
