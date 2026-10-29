// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::required;
use crate::{ArtifactView, appearance::*, geometry::SourceHash, rpc};

impl TryFrom<rpc::AppearanceColor> for AppearanceColor {
    type Error = String;
    fn try_from(value: rpc::AppearanceColor) -> Result<Self, String> {
        use rpc::appearance_color::Encoding;
        let channels = |value: rpc::AppearanceChannels| [value.first, value.second, value.third];
        let color = match required(value.encoding, "appearance color encoding")? {
            Encoding::Srgb(value) => Self::Srgb {
                channels: channels(value),
            },
            Encoding::XyzY1(value) => Self::XyzY1 {
                channels: channels(value),
            },
            Encoding::XyzY100(value) => Self::XyzY100 {
                channels: channels(value),
            },
            Encoding::D65Lab(value) => Self::D65Lab {
                channels: channels(value),
            },
        };
        color.validate().map_err(|error| error.to_string())?;
        Ok(color)
    }
}
impl TryFrom<AppearanceColor> for rpc::AppearanceColor {
    type Error = String;
    fn try_from(value: AppearanceColor) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        use rpc::appearance_color::Encoding;
        let channels = |[first, second, third]: [f64; 3]| rpc::AppearanceChannels {
            first,
            second,
            third,
        };
        Ok(Self {
            encoding: Some(match value {
                AppearanceColor::Srgb { channels: values } => Encoding::Srgb(channels(values)),
                AppearanceColor::XyzY1 { channels: values } => Encoding::XyzY1(channels(values)),
                AppearanceColor::XyzY100 { channels: values } => {
                    Encoding::XyzY100(channels(values))
                }
                AppearanceColor::D65Lab { channels: values } => Encoding::D65Lab(channels(values)),
            }),
        })
    }
}
impl TryFrom<rpc::AppearanceConditions> for AppearanceConditions {
    type Error = String;
    fn try_from(value: rpc::AppearanceConditions) -> Result<Self, String> {
        let illuminant = match rpc::AppearanceIlluminant::try_from(value.illuminant)
            .map_err(|_| "unknown appearance illuminant")?
        {
            rpc::AppearanceIlluminant::D65 => AppearanceIlluminant::D65,
            rpc::AppearanceIlluminant::D50 => AppearanceIlluminant::D50,
            rpc::AppearanceIlluminant::Unknown => AppearanceIlluminant::Unknown,
            rpc::AppearanceIlluminant::Unspecified => {
                return Err("missing appearance illuminant".into());
            }
        };
        let observer = match rpc::AppearanceObserver::try_from(value.observer)
            .map_err(|_| "unknown appearance observer")?
        {
            rpc::AppearanceObserver::Cie1931TwoDegree => AppearanceObserver::Cie1931TwoDegree,
            rpc::AppearanceObserver::Cie1964TenDegree => AppearanceObserver::Cie1964TenDegree,
            rpc::AppearanceObserver::Unknown => AppearanceObserver::Unknown,
            rpc::AppearanceObserver::Unspecified => {
                return Err("missing appearance observer".into());
            }
        };
        let result = Self {
            illuminant,
            observer,
            geometry: value.geometry,
            backing: value.backing,
            thickness_mm: value.thickness_mm,
            cohort: value.cohort,
            instrument: value.instrument,
            calibration: value.calibration,
            aperture_mm: value.aperture_mm,
            specular: value.specular,
            orientation: value.orientation,
            process: value.process,
            construction: value.construction,
        };
        result.validate().map_err(|error| error.to_string())?;
        Ok(result)
    }
}
impl TryFrom<AppearanceConditions> for rpc::AppearanceConditions {
    type Error = String;
    fn try_from(value: AppearanceConditions) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            illuminant: match value.illuminant {
                AppearanceIlluminant::D65 => rpc::AppearanceIlluminant::D65,
                AppearanceIlluminant::D50 => rpc::AppearanceIlluminant::D50,
                AppearanceIlluminant::Unknown => rpc::AppearanceIlluminant::Unknown,
            } as i32,
            observer: match value.observer {
                AppearanceObserver::Cie1931TwoDegree => rpc::AppearanceObserver::Cie1931TwoDegree,
                AppearanceObserver::Cie1964TenDegree => rpc::AppearanceObserver::Cie1964TenDegree,
                AppearanceObserver::Unknown => rpc::AppearanceObserver::Unknown,
            } as i32,
            geometry: value.geometry,
            backing: value.backing,
            thickness_mm: value.thickness_mm,
            cohort: value.cohort,
            instrument: value.instrument,
            calibration: value.calibration,
            aperture_mm: value.aperture_mm,
            specular: value.specular,
            orientation: value.orientation,
            process: value.process,
            construction: value.construction,
        })
    }
}
impl TryFrom<rpc::AppearanceTarget> for AppearanceTarget {
    type Error = String;
    fn try_from(value: rpc::AppearanceTarget) -> Result<Self, String> {
        let evidence_scope = match rpc::AppearanceEvidenceScope::try_from(value.evidence_scope)
            .map_err(|_| "unknown evidence scope")?
        {
            rpc::AppearanceEvidenceScope::CatalogEstimates => EvidenceScope::CatalogEstimates,
            rpc::AppearanceEvidenceScope::Measured => EvidenceScope::Measured,
            rpc::AppearanceEvidenceScope::Unspecified => {
                return Err("missing evidence scope".into());
            }
        };
        let target = Self {
            color: required(value.color, "appearance target color")?.try_into()?,
            delta_e00_tolerance: value.delta_e00_tolerance,
            lightness_tolerance: value.lightness_tolerance,
            conditions: required(value.conditions, "appearance target conditions")?.try_into()?,
            evidence_scope,
        };
        target.validate().map_err(|error| error.to_string())?;
        Ok(target)
    }
}
impl TryFrom<AppearanceTarget> for rpc::AppearanceTarget {
    type Error = String;
    fn try_from(value: AppearanceTarget) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            color: Some(value.color.try_into()?),
            delta_e00_tolerance: value.delta_e00_tolerance,
            lightness_tolerance: value.lightness_tolerance,
            conditions: Some(value.conditions.try_into()?),
            evidence_scope: match value.evidence_scope {
                EvidenceScope::CatalogEstimates => rpc::AppearanceEvidenceScope::CatalogEstimates,
                EvidenceScope::Measured => rpc::AppearanceEvidenceScope::Measured,
            } as i32,
        })
    }
}
impl TryFrom<rpc::AppearanceSearchLimits> for SearchLimits {
    type Error = String;
    fn try_from(value: rpc::AppearanceSearchLimits) -> Result<Self, String> {
        let limits = Self {
            max_visited: value.max_visited,
            max_alternatives: value.max_alternatives,
        };
        limits.validate().map_err(|error| error.to_string())?;
        Ok(limits)
    }
}
impl TryFrom<SearchLimits> for rpc::AppearanceSearchLimits {
    type Error = String;
    fn try_from(value: SearchLimits) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            max_visited: value.max_visited,
            max_alternatives: value.max_alternatives,
        })
    }
}
impl TryFrom<rpc::AppearancePaletteRecord> for PaletteRecord {
    type Error = String;
    fn try_from(value: rpc::AppearancePaletteRecord) -> Result<Self, String> {
        let record = Self {
            name: value.name,
            original_hash: SourceHash::parse(value.original_hash)
                .map_err(|error| error.to_string())?,
            original_bytes: value.original_bytes,
            canonical_hash: SourceHash::parse(value.canonical_hash)
                .map_err(|error| error.to_string())?,
            canonical_bytes: value.canonical_bytes,
            source_url: value.source_url,
            source_revision: value.source_revision,
            license: value.license,
            conversion: value.conversion,
        };
        record.validate().map_err(|error| error.to_string())?;
        Ok(record)
    }
}
impl TryFrom<PaletteRecord> for rpc::AppearancePaletteRecord {
    type Error = String;
    fn try_from(value: PaletteRecord) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            name: value.name,
            original_hash: value.original_hash.into(),
            original_bytes: value.original_bytes,
            canonical_hash: value.canonical_hash.into(),
            canonical_bytes: value.canonical_bytes,
            source_url: value.source_url,
            source_revision: value.source_revision,
            license: value.license,
            conversion: value.conversion,
        })
    }
}
impl TryFrom<rpc::AppearanceReportRecord> for AppearanceReportRecord {
    type Error = String;
    fn try_from(value: rpc::AppearanceReportRecord) -> Result<Self, String> {
        let record = Self {
            hash: SourceHash::parse(value.hash).map_err(|error| error.to_string())?,
            byte_count: value.byte_count,
            input_hash: SourceHash::parse(value.input_hash).map_err(|error| error.to_string())?,
            producer: value.producer,
            complete: value.complete,
        };
        record.validate().map_err(|error| error.to_string())?;
        Ok(record)
    }
}
impl TryFrom<AppearanceReportRecord> for rpc::AppearanceReportRecord {
    type Error = String;
    fn try_from(value: AppearanceReportRecord) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            hash: value.hash.into(),
            byte_count: value.byte_count,
            input_hash: value.input_hash.into(),
            producer: value.producer,
            complete: value.complete,
        })
    }
}
impl TryFrom<rpc::AppearanceStudyInfo> for AppearanceStudyInfo {
    type Error = String;
    fn try_from(value: rpc::AppearanceStudyInfo) -> Result<Self, String> {
        let info = Self {
            palette: value.palette.map(TryInto::try_into).transpose()?,
            target: value.target.map(TryInto::try_into).transpose()?,
            limits: required(value.limits, "appearance limits")?.try_into()?,
            report: value.report.map(TryInto::try_into).transpose()?,
            selection: value.selection,
            optical: value.optical.map(TryInto::try_into).transpose()?,
        };
        info.validate().map_err(|error| error.to_string())?;
        Ok(info)
    }
}
impl TryFrom<AppearanceStudyInfo> for rpc::AppearanceStudyInfo {
    type Error = String;
    fn try_from(value: AppearanceStudyInfo) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            palette: value.palette.map(TryInto::try_into).transpose()?,
            target: value.target.map(TryInto::try_into).transpose()?,
            limits: Some(value.limits.try_into()?),
            report: value.report.map(TryInto::try_into).transpose()?,
            selection: value.selection,
            optical: value.optical.map(TryInto::try_into).transpose()?,
        })
    }
}

pub(super) fn validate_report_resource(
    record: &AppearanceReportRecord,
    resource: &ArtifactView,
) -> Result<(), String> {
    record.validate().map_err(|error| error.to_string())?;
    let size = super::validate_artifact_view(resource, MAX_APPEARANCE_REPORT_BYTES as u64)?;
    if size != record.byte_count
        || resource.sha256 != record.hash.as_str()
        || resource.name.strip_prefix("artifacts/") != Some(record.hash.as_str())
        || resource.media_type != APPEARANCE_REPORT_MIME
    {
        return Err("appearance report descriptor disagrees with checked record".into());
    }
    Ok(())
}
