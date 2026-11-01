// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use crate::{color_naming::*, rpc};
fn mode(v: i32) -> Result<ColorNamingMode, String> {
    match rpc::ColorNamingMode::try_from(v) {
        Ok(rpc::ColorNamingMode::Exact) => Ok(ColorNamingMode::Exact),
        Ok(rpc::ColorNamingMode::Nearest) => Ok(ColorNamingMode::Nearest),
        Ok(rpc::ColorNamingMode::Descriptive) => Ok(ColorNamingMode::Descriptive),
        _ => Err("unknown or unspecified color naming mode".into()),
    }
}
fn wire_mode(v: ColorNamingMode) -> i32 {
    match v {
        ColorNamingMode::Exact => rpc::ColorNamingMode::Exact as i32,
        ColorNamingMode::Nearest => rpc::ColorNamingMode::Nearest as i32,
        ColorNamingMode::Descriptive => rpc::ColorNamingMode::Descriptive as i32,
    }
}
fn rgb(v: Option<rpc::ColorRgb8>) -> Result<[u8; 3], String> {
    let v = v.ok_or("missing sRGB sample")?;
    Ok([
        u8::try_from(v.red).map_err(|_| "red exceeds 255")?,
        u8::try_from(v.green).map_err(|_| "green exceeds 255")?,
        u8::try_from(v.blue).map_err(|_| "blue exceeds 255")?,
    ])
}
fn wire_rgb(v: [u8; 3]) -> rpc::ColorRgb8 {
    rpc::ColorRgb8 {
        red: v[0].into(),
        green: v[1].into(),
        blue: v[2].into(),
    }
}
impl TryFrom<rpc::NameColorsRequest> for ColorNamingRequest {
    type Error = String;
    fn try_from(v: rpc::NameColorsRequest) -> Result<Self, String> {
        if v.colors.len() > MAX_COLOR_NAMING_SAMPLES {
            return Err("too many color samples".into());
        }
        let value = Self {
            mode: mode(v.mode)?,
            colors: v
                .colors
                .into_iter()
                .map(|c| rgb(Some(c)))
                .collect::<Result<_, _>>()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<&ColorNamingRequest> for rpc::NameColorsRequest {
    type Error = String;
    fn try_from(v: &ColorNamingRequest) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            mode: wire_mode(v.mode),
            colors: v.colors.iter().map(|c| wire_rgb(*c)).collect(),
        })
    }
}
fn modifier(v: i32) -> Result<Option<ColorModifier>, String> {
    match rpc::ColorModifier::try_from(v) {
        Ok(rpc::ColorModifier::None) => Ok(None),
        Ok(rpc::ColorModifier::Dark) => Ok(Some(ColorModifier::Dark)),
        Ok(rpc::ColorModifier::Light) => Ok(Some(ColorModifier::Light)),
        Ok(rpc::ColorModifier::Muted) => Ok(Some(ColorModifier::Muted)),
        Ok(rpc::ColorModifier::Pale) => Ok(Some(ColorModifier::Pale)),
        Ok(rpc::ColorModifier::Vivid) => Ok(Some(ColorModifier::Vivid)),
        _ => Err("unknown color modifier".into()),
    }
}
fn wire_modifier(v: Option<ColorModifier>) -> i32 {
    match v {
        None => rpc::ColorModifier::None as i32,
        Some(ColorModifier::Dark) => rpc::ColorModifier::Dark as i32,
        Some(ColorModifier::Light) => rpc::ColorModifier::Light as i32,
        Some(ColorModifier::Muted) => rpc::ColorModifier::Muted as i32,
        Some(ColorModifier::Pale) => rpc::ColorModifier::Pale as i32,
        Some(ColorModifier::Vivid) => rpc::ColorModifier::Vivid as i32,
    }
}
impl TryFrom<rpc::NameColorsReply> for ColorNamingResponse {
    type Error = String;
    fn try_from(v: rpc::NameColorsReply) -> Result<Self, String> {
        if v.colors.len() > MAX_COLOR_NAMING_SAMPLES {
            return Err("too many named colors".into());
        }
        let colors = v
            .colors
            .into_iter()
            .map(|c| {
                Ok(NamedColor {
                    srgb8: rgb(c.srgb8)?,
                    hex: c.hex,
                    name: c
                        .name
                        .map(|n| -> Result<ColorName, String> {
                            Ok(ColorName {
                                label: n.label,
                                reference_name: n.reference_name,
                                reference_srgb8: rgb(n.reference_srgb8)?,
                                modifier: modifier(n.modifier)?,
                                generated: n.generated,
                                distance_oklab: n.distance_oklab,
                            })
                        })
                        .transpose()?,
                })
            })
            .collect::<Result<_, String>>()?;
        let result = Self {
            algorithm_version: v.algorithm_version,
            dataset_sha256: v.dataset_sha256,
            mode: mode(v.mode)?,
            colors,
        };
        result.validate()?;
        Ok(result)
    }
}
impl TryFrom<&ColorNamingResponse> for rpc::NameColorsReply {
    type Error = String;
    fn try_from(v: &ColorNamingResponse) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            algorithm_version: v.algorithm_version,
            dataset_sha256: v.dataset_sha256.clone(),
            mode: wire_mode(v.mode),
            colors: v
                .colors
                .iter()
                .map(|c| rpc::NamedColorSample {
                    srgb8: Some(wire_rgb(c.srgb8)),
                    hex: c.hex.clone(),
                    name: c.name.as_ref().map(|n| rpc::ColorNameMatch {
                        label: n.label.clone(),
                        reference_name: n.reference_name.clone(),
                        reference_srgb8: Some(wire_rgb(n.reference_srgb8)),
                        modifier: wire_modifier(n.modifier),
                        generated: n.generated,
                        distance_oklab: n.distance_oklab,
                    }),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_rejects_unknown_modes_oversized_channels_and_false_provenance() {
        let request = ColorNamingRequest {
            mode: ColorNamingMode::Descriptive,
            colors: vec![[1, 2, 3]],
        };
        let wire = rpc::NameColorsRequest::try_from(&request).unwrap();
        assert_eq!(ColorNamingRequest::try_from(wire.clone()).unwrap(), request);
        for unknown in [0, 999] {
            let mut bad = wire.clone();
            bad.mode = unknown;
            assert!(ColorNamingRequest::try_from(bad).is_err());
        }
        let mut bad = wire;
        bad.colors[0].red = 256;
        assert!(ColorNamingRequest::try_from(bad).is_err());
        let response = ColorNamingResponse {
            algorithm_version: COLOR_NAMING_VERSION,
            dataset_sha256: COLOR_NAMING_DATASET_SHA256.into(),
            mode: ColorNamingMode::Exact,
            colors: vec![NamedColor {
                srgb8: [255, 0, 0],
                hex: "#FF0000".into(),
                name: Some(ColorName {
                    label: "Red".into(),
                    reference_name: "red".into(),
                    reference_srgb8: [255, 0, 0],
                    modifier: None,
                    generated: false,
                    distance_oklab: 0.0,
                }),
            }],
        };
        let wire = rpc::NameColorsReply::try_from(&response).unwrap();
        assert_eq!(
            ColorNamingResponse::try_from(wire.clone()).unwrap(),
            response
        );
        let mut bad = wire.clone();
        bad.colors[0].name.as_mut().unwrap().distance_oklab = f64::NAN;
        assert!(ColorNamingResponse::try_from(bad).is_err());
        let mut bad = wire.clone();
        bad.colors[0].hex = "#000000".into();
        assert!(ColorNamingResponse::try_from(bad).is_err());
        let mut bad = wire.clone();
        bad.algorithm_version += 1;
        assert!(ColorNamingResponse::try_from(bad).is_err());
        let mut bad = wire;
        bad.colors[0].name.as_mut().unwrap().modifier = 999;
        assert!(ColorNamingResponse::try_from(bad).is_err());
    }
}
