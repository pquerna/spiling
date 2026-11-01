// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Versioned presentation-only naming. Survey data stays CC0; no physical color claim.
//! OKLab equations: Björn Ottosson, https://bottosson.github.io/posts/oklab/.
//! Use the existing encoded-sRGB transfer function; do not change CIEDE2000 ranking.
use spiling_contracts::color_naming::*;
use std::sync::OnceLock;

const DATA: &str = include_str!("../data/xkcd-rgb.txt");
// Deliberately small recognizable vocabulary. Exact/nearest retain all 949 survey names.
// Selection and modifier thresholds are versioned editorial heuristics, not calibration.
const BASES: &[&str] = &[
    "black",
    "white",
    "grey",
    "red",
    "orange",
    "yellow",
    "green",
    "blue",
    "purple",
    "pink",
    "brown",
    "coral",
    "salmon",
    "peach",
    "teal",
    "turquoise",
    "cyan",
    "aqua",
    "mint",
    "lime",
    "indigo",
    "violet",
    "magenta",
    "fuchsia",
    "lavender",
    "lilac",
    "mauve",
    "rose",
    "plum",
    "navy",
    "maroon",
    "burgundy",
    "olive",
    "mustard",
    "gold",
    "ochre",
    "tan",
    "beige",
    "cream",
    "rust",
    "copper",
    "chocolate",
    "sienna",
    "terracotta",
    "apricot",
    "amber",
    "cerulean",
    "emerald",
    "jade",
    "khaki",
    "charcoal",
    "scarlet",
    "crimson",
    "periwinkle",
    "raspberry",
    "aubergine",
    "cobalt",
    "denim",
    "sage",
    "seafoam",
];
struct Reference {
    name: &'static str,
    rgb: [u8; 3],
    lab: [f64; 3],
    base: bool,
    descriptive_exact: bool,
}
fn references() -> &'static [Reference] {
    static REFERENCES: OnceLock<Vec<Reference>> = OnceLock::new();
    REFERENCES.get_or_init(|| {
        let mut rows: Vec<_> = DATA
            .lines()
            .skip(1)
            .map(|line| {
                let mut fields = line.split('\t');
                let name = fields.next().expect("pinned survey name");
                let rgb = parse_srgb8_hex(fields.next().expect("pinned survey RGB"))
                    .expect("pinned survey hex");
                let base = BASES.contains(&name);
                let descriptive_exact = base
                    || [
                        "dark ", "light ", "muted ", "pale ", "vivid ", "bright ", "deep ",
                        "dusty ",
                    ]
                    .iter()
                    .any(|prefix| {
                        name.strip_prefix(prefix)
                            .is_some_and(|n| BASES.contains(&n))
                    });
                Reference {
                    name,
                    rgb,
                    lab: srgb8_to_oklab(rgb),
                    base,
                    descriptive_exact,
                }
            })
            .collect();
        rows.sort_by_key(|row| (row.name, row.rgb));
        rows
    })
}
/// Normalized OKLab from an explicitly encoded, opaque sRGB sample.
pub fn srgb8_to_oklab(rgb: [u8; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(|v| super::srgb_decode(f64::from(v) / 255.0));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}
fn distance_squared(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}
fn chroma(lab: [f64; 3]) -> f64 {
    lab[1].hypot(lab[2])
}
fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}
fn result(rgb: [u8; 3], reference: &Reference, modifier: Option<ColorModifier>) -> NamedColor {
    let label = modifier.map_or_else(
        || sentence_case(reference.name),
        |m| format!("{} {}", m.label(), reference.name),
    );
    NamedColor {
        srgb8: rgb,
        hex: srgb8_hex(rgb),
        name: Some(ColorName {
            label,
            reference_name: reference.name.into(),
            reference_srgb8: reference.rgb,
            modifier,
            generated: modifier.is_some(),
            distance_oklab: if rgb == reference.rgb {
                0.0
            } else {
                distance_squared(srgb8_to_oklab(rgb), reference.lab).sqrt()
            },
        }),
    }
}
/// Default descriptions use common bases and at most one modifier. Exact survey
/// spellings for common bases/modified bases win over generated alternatives.
pub fn describe(rgb: [u8; 3], mode: ColorNamingMode) -> NamedColor {
    let rows = references();
    let exact = rows.iter().find(|r| r.rgb == rgb);
    if let Some(reference) =
        exact.filter(|r| mode != ColorNamingMode::Descriptive || r.descriptive_exact)
    {
        return result(rgb, reference, None);
    }
    if mode == ColorNamingMode::Exact {
        return NamedColor {
            srgb8: rgb,
            hex: srgb8_hex(rgb),
            name: None,
        };
    }
    let lab = srgb8_to_oklab(rgb);
    if mode == ColorNamingMode::Nearest {
        let reference = rows
            .iter()
            .min_by(|a, b| distance_squared(lab, a.lab).total_cmp(&distance_squared(lab, b.lab)))
            .expect("pinned nonempty survey");
        return result(rgb, reference, None);
    }
    let c = chroma(lab);
    if c < 0.025 {
        let name = if lab[0] < 0.22 {
            "black"
        } else if lab[0] > 0.91 {
            "white"
        } else {
            "grey"
        };
        let reference = rows
            .iter()
            .find(|r| r.name == name)
            .expect("pinned neutral reference");
        let modifier = if name != "grey" {
            None
        } else if lab[0] < reference.lab[0] - 0.11 {
            Some(ColorModifier::Dark)
        } else if lab[0] > reference.lab[0] + 0.11 {
            Some(ColorModifier::Light)
        } else {
            None
        };
        return result(rgb, reference, modifier);
    }
    // Hue uses normalized a/b vectors, so the red hue wrap is continuous. Near
    // neutrals never enter this branch; there is no unstable achromatic hue.
    let score = |r: &Reference| {
        let rc = chroma(r.lab);
        let hue = (lab[1] / c - r.lab[1] / rc).powi(2) + (lab[2] / c - r.lab[2] / rc).powi(2);
        0.15 * (lab[0] - r.lab[0]).powi(2) + 0.35 * (c - rc).powi(2) + 0.025 * hue
    };
    let reference = rows
        .iter()
        .filter(|r| r.base && chroma(r.lab) >= 0.025)
        .min_by(|a, b| score(a).total_cmp(&score(b)))
        .expect("pinned chromatic bases");
    let dl = lab[0] - reference.lab[0];
    let rc = chroma(reference.lab);
    let muted = c < rc * 0.70 && rc - c > 0.035;
    let modifier = if dl > 0.10 && muted {
        Some(ColorModifier::Pale)
    } else if dl < -0.10 {
        Some(ColorModifier::Dark)
    } else if muted {
        Some(ColorModifier::Muted)
    } else if dl > 0.10 {
        Some(ColorModifier::Light)
    } else if c > rc * 1.25 && c - rc > 0.035 {
        Some(ColorModifier::Vivid)
    } else {
        None
    };
    result(rgb, reference, modifier)
}
pub fn name_colors(request: &ColorNamingRequest) -> Result<ColorNamingResponse, String> {
    request.validate()?;
    Ok(ColorNamingResponse {
        algorithm_version: COLOR_NAMING_VERSION,
        dataset_sha256: COLOR_NAMING_DATASET_SHA256.into(),
        mode: request.mode,
        colors: request
            .colors
            .iter()
            .map(|rgb| describe(*rgb, request.mode))
            .collect(),
    })
}
