// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Pure scalar C1a lookup. No optical mixing, spool inventory or physical accuracy claim.
//! Conversion uses CSS Color 4's xy D65 (0.3127, 0.3290), the matching rational
//! sRGB matrix and explicit XYZ Y=1/Y=100 scales. D65 Lab is NOT CSS D50 Lab.
//! CIEDE2000 uses the standard hue branches and kL=kC=kH=1, without gamut clamps.
use spiling_contracts::appearance::*;
use spiling_contracts::project::{ProjectError, ProjectErrorCode};
use std::f64::consts::PI;

fn numerical(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::InvalidProject, message)
}
fn finite3(value: [f64; 3]) -> Result<[f64; 3], ProjectError> {
    if value.iter().all(|v| v.is_finite()) {
        Ok(value)
    } else {
        Err(numerical(
            "appearance calculation produced nonfinite channels",
        ))
    }
}
pub fn parse_palette(bytes: &[u8]) -> Result<PaletteDocument, ProjectError> {
    PaletteDocument::decode(bytes)
}
pub fn canonical_palette(document: &PaletteDocument) -> Result<Vec<u8>, ProjectError> {
    document.canonical_bytes()
}

pub const D65_WHITE_Y1: [f64; 3] = [0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290];
fn srgb_decode(v: f64) -> f64 {
    if v.abs() <= 0.04045 {
        v / 12.92
    } else {
        v.signum() * ((v.abs() + 0.055) / 1.055).powf(2.4)
    }
}
fn srgb_encode(v: f64) -> f64 {
    if v.abs() <= 0.0031308 {
        12.92 * v
    } else {
        v.signum() * (1.055 * v.abs().powf(1.0 / 2.4) - 0.055)
    }
}
/// Extended transfer function, never clipping: admission of persisted sRGB is a
/// separate AppearanceColor validation step; this scalar primitive accepts finite values.
pub fn srgb_to_xyz_y1(srgb: [f64; 3]) -> Result<[f64; 3], ProjectError> {
    finite3(srgb)?;
    let [r, g, b] = srgb.map(srgb_decode);
    finite3([
        (506752.0 / 1228815.0) * r + (87881.0 / 245763.0) * g + (12673.0 / 70218.0) * b,
        (87098.0 / 409605.0) * r + (175762.0 / 245763.0) * g + (12673.0 / 175545.0) * b,
        (7918.0 / 409605.0) * r + (87881.0 / 737289.0) * g + (1001167.0 / 1053270.0) * b,
    ])
}
/// Inverse of the same rational CSS matrix; out-of-gamut values are preserved.
pub fn xyz_y1_to_srgb(xyz: [f64; 3]) -> Result<[f64; 3], ProjectError> {
    let [x, y, z] = finite3(xyz)?;
    finite3(
        [
            (12831.0 / 3959.0) * x - (329.0 / 214.0) * y - (1974.0 / 3959.0) * z,
            -(851781.0 / 878810.0) * x + (1648619.0 / 878810.0) * y + (36519.0 / 878810.0) * z,
            (705.0 / 12673.0) * x - (2585.0 / 12673.0) * y + (705.0 / 667.0) * z,
        ]
        .map(srgb_encode),
    )
}
pub fn xyz_y1_to_d65_lab(xyz: [f64; 3]) -> Result<[f64; 3], ProjectError> {
    finite3(xyz)?;
    let f = |v: f64| {
        if v > 216.0 / 24389.0 {
            v.cbrt()
        } else {
            ((24389.0 / 27.0) * v + 16.0) / 116.0
        }
    };
    let [x, y, z] = std::array::from_fn(|i| f(xyz[i] / D65_WHITE_Y1[i]));
    finite3([116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)])
}
pub fn d65_lab_to_xyz_y1(lab: [f64; 3]) -> Result<[f64; 3], ProjectError> {
    let [l, a, b] = finite3(lab)?;
    let y = (l + 16.0) / 116.0;
    let f = [y + a / 500.0, y, y - b / 200.0];
    finite3(std::array::from_fn(|i| {
        let v = f[i];
        let cube = v * v * v;
        D65_WHITE_Y1[i]
            * if cube > 216.0 / 24389.0 {
                cube
            } else {
                (116.0 * v - 16.0) / (24389.0 / 27.0)
            }
    }))
}
pub fn color_to_d65_lab(color: AppearanceColor) -> Result<[f64; 3], ProjectError> {
    color.validate()?;
    match color {
        AppearanceColor::Srgb { channels } => xyz_y1_to_d65_lab(srgb_to_xyz_y1(channels)?),
        AppearanceColor::XyzY1 { channels } => xyz_y1_to_d65_lab(channels),
        AppearanceColor::XyzY100 { channels } => xyz_y1_to_d65_lab(channels.map(|v| v / 100.0)),
        AppearanceColor::D65Lab { channels } => Ok(channels),
    }
}
fn degrees(radians: f64) -> f64 {
    radians * 180.0 / PI
}
fn radians(degrees: f64) -> f64 {
    degrees * PI / 180.0
}
fn hue(a: f64, b: f64) -> f64 {
    if a == 0.0 && b == 0.0 {
        0.0
    } else {
        degrees(b.atan2(a)).rem_euclid(360.0)
    }
}
fn chroma_ratio(c: f64) -> f64 {
    // c^7/(c^7+25^7), avoiding the overflow of the direct seventh powers.
    if c > 25.0 {
        1.0 / (1.0 + (25.0 / c).powi(7))
    } else {
        let p = (c / 25.0).powi(7);
        p / (1.0 + p)
    }
}
/// Standard CIEDE2000, k=(1,1,1). Inputs are finite D65 Lab coordinates;
/// numerical overflow is an explicit error, never a clipped or fabricated distance.
pub fn ciede2000(lab1: [f64; 3], lab2: [f64; 3]) -> Result<f64, ProjectError> {
    let [l1, a1, b1] = finite3(lab1)?;
    let [l2, a2, b2] = finite3(lab2)?;
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let mean_c = c1 / 2.0 + c2 / 2.0;
    let g = 0.5 * (1.0 - chroma_ratio(mean_c).sqrt());
    let ap1 = (1.0 + g) * a1;
    let ap2 = (1.0 + g) * a2;
    let cp1 = ap1.hypot(b1);
    let cp2 = ap2.hypot(b2);
    let h1 = hue(ap1, b1);
    let h2 = hue(ap2, b2);
    let dl = l2 - l1;
    let dc = cp2 - cp1;
    let dh = if cp1 == 0.0 || cp2 == 0.0 {
        0.0
    } else if (h2 - h1).abs() <= 180.0 {
        h2 - h1
    } else if h2 <= h1 {
        h2 - h1 + 360.0
    } else {
        h2 - h1 - 360.0
    };
    let dh_term = 2.0 * cp1.sqrt() * cp2.sqrt() * radians(dh / 2.0).sin();
    let mean_l = l1 / 2.0 + l2 / 2.0;
    let mean_cp = cp1 / 2.0 + cp2 / 2.0;
    let mean_h = if cp1 == 0.0 || cp2 == 0.0 {
        h1 + h2
    } else if (h1 - h2).abs() <= 180.0 {
        (h1 + h2) / 2.0
    } else if h1 + h2 < 360.0 {
        (h1 + h2 + 360.0) / 2.0
    } else {
        (h1 + h2 - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * radians(mean_h - 30.0).cos()
        + 0.24 * radians(2.0 * mean_h).cos()
        + 0.32 * radians(3.0 * mean_h + 6.0).cos()
        - 0.20 * radians(4.0 * mean_h - 63.0).cos();
    let theta = 30.0 * (-((mean_h - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * chroma_ratio(mean_cp).sqrt();
    let lm50 = mean_l - 50.0;
    let sl = 1.0 + 0.015 * lm50 * lm50 / (20.0 + lm50 * lm50).sqrt();
    let sc = 1.0 + 0.045 * mean_cp;
    let sh = 1.0 + 0.015 * mean_cp * t;
    let rt = -radians(2.0 * theta).sin() * rc;
    let [vl, vc, vh] = [dl / sl, dc / sc, dh_term / sh];
    let squared = vl * vl + vc * vc + vh * vh + rt * vc * vh;
    let distance = squared.sqrt();
    if [c1, c2, ap1, ap2, cp1, cp2, sl, sc, sh, squared, distance]
        .iter()
        .all(|v| v.is_finite())
        && squared >= 0.0
    {
        Ok(distance)
    } else {
        Err(numerical(
            "nonfinite or invalid CIEDE2000 numerical condition",
        ))
    }
}

fn assess(
    row: &PaletteRow,
    target: &AppearanceTarget,
    target_lab: [f64; 3],
) -> Result<RowAssessment, ProjectError> {
    let (color, evidence) = match &row.evidence {
        AppearanceEvidence::Unknown { .. } => return Ok(RowAssessment::UnknownEvidence),
        AppearanceEvidence::CatalogEstimate { srgb } => {
            if target.evidence_scope != EvidenceScope::CatalogEstimates {
                return Ok(RowAssessment::FilteredEvidence);
            }
            // A catalog GUI label supplies no geometry, backing, thickness or cohort.
            if !target.conditions.is_supported()
                || target.conditions != AppearanceConditions::default()
            {
                return Ok(RowAssessment::ConditionsMismatch);
            }
            (
                AppearanceColor::Srgb { channels: *srgb },
                AssessedEvidence::CatalogEstimate,
            )
        }
        AppearanceEvidence::Measured {
            color,
            conditions,
            material_lot,
        } => {
            if target.evidence_scope != EvidenceScope::Measured {
                return Ok(RowAssessment::FilteredEvidence);
            }
            if material_lot.is_none() || !conditions.is_measured() {
                return Ok(RowAssessment::UnknownEvidence);
            }
            if !conditions.is_supported()
                || !target.conditions.is_supported()
                || conditions != &target.conditions
            {
                return Ok(RowAssessment::ConditionsMismatch);
            }
            (*color, AssessedEvidence::Measured)
        }
    };
    let lab = color_to_d65_lab(color)?;
    let delta_e00 = ciede2000(lab, target_lab)?;
    let delta_lightness = (lab[0] - target_lab[0]).abs();
    if !delta_lightness.is_finite() {
        return Err(numerical("nonfinite appearance lightness difference"));
    }
    Ok(RowAssessment::Assessed {
        evidence,
        delta_e00,
        delta_lightness,
        within_tolerance: delta_e00 <= target.delta_e00_tolerance
            && target
                .lightness_tolerance
                .is_none_or(|max| delta_lightness <= max),
    })
}

/// Enumerate stable keys, charging every visited row, including filtered/unknown rows.
/// Alternative limits truncate only ranked output; they never terminate enumeration.
/// Cancellation/work stops remain incomplete even if a matching row was already found.
/// Scratch is report visits plus min(row count, alternative limit) borrowed ranked
/// rows. Palette bytes, sorting and evidence hashes are prepared once by the owner.
/// Output encoding separately enforces the 1MiB report bound before buffer growth.
pub fn select_single(
    palette: &PreparedPalette,
    target: &AppearanceTarget,
    limits: SearchLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<AppearanceReport, ProjectError> {
    target.validate()?;
    limits.validate()?;
    let palette_hash = palette.canonical_hash().clone();
    let input_hash = appearance_input_hash(&palette_hash, target, limits)?;
    let target_lab = color_to_d65_lab(target.color)?;
    let rows = &palette.document().rows;
    let mut report = AppearanceReport {
        producer: APPEARANCE_PRODUCER.to_owned(),
        input_hash,
        palette_hash,
        target: target.clone(),
        limits,
        total_rows: rows.len() as u32,
        visits: Vec::with_capacity(rows.len().min(limits.max_visited as usize)),
        alternatives: Vec::new(),
        alternatives_truncated: false,
        status: AppearanceStatus::Cancelled,
    };
    let mut ranked: Vec<(f64, &PaletteRow)> =
        Vec::with_capacity(rows.len().min(limits.max_alternatives as usize));
    let mut assessed_count = 0usize;
    let mut any_match = false;
    let mut unassessed = false;
    for (row, evidence_hash) in rows.iter().zip(palette.evidence_hashes()) {
        if cancelled() {
            report.status = AppearanceStatus::Cancelled;
            break;
        }
        if report.visits.len() == limits.max_visited as usize {
            report.status = AppearanceStatus::WorkLimit;
            break;
        }
        let assessment = assess(row, target, target_lab)?;
        unassessed |= matches!(
            assessment,
            RowAssessment::UnknownEvidence | RowAssessment::ConditionsMismatch
        );
        if let RowAssessment::Assessed {
            delta_e00,
            within_tolerance,
            ..
        } = assessment
        {
            assessed_count += 1;
            any_match |= within_tolerance;
            let item = (delta_e00, row);
            let position = ranked
                .binary_search_by(|v| {
                    v.0.total_cmp(&item.0)
                        .then_with(|| v.1.key.cmp(&item.1.key))
                })
                .unwrap_or_else(|p| p);
            if position < limits.max_alternatives as usize {
                if ranked.len() == limits.max_alternatives as usize {
                    ranked.pop();
                }
                ranked.insert(position, item);
            }
        }
        report.visits.push(AppearanceVisit {
            key: row.key.clone(),
            evidence_hash: evidence_hash.clone(),
            assessment,
        });
    }
    if report.visits.len() == report.total_rows as usize {
        report.status = AppearanceStatus::Complete {
            outcome: if any_match {
                AppearanceOutcome::AssessedMatch
            } else if assessed_count == 0 || unassessed {
                AppearanceOutcome::NoAssessedMatch
            } else {
                AppearanceOutcome::AssessedMismatch
            },
        };
    }
    report.alternatives_truncated = assessed_count > ranked.len();
    report.alternatives = ranked
        .into_iter()
        .map(|(_, row)| AppearanceAlternative {
            key: row.key.clone(),
            name: row.name.clone(),
        })
        .collect();
    report.validate()?;
    Ok(report)
}
