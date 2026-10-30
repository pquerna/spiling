// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Inert, bounded appearance-study records. Catalog RGB is not measured filament evidence.
use crate::geometry::SourceHash;
use crate::project::{ProjectError, ProjectErrorCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};
use ts_rs::TS;

pub const APPEARANCE_PRODUCER: &str = "spiling-appearance-c1a-css-d65-ciede2000-1";
pub const APPEARANCE_REPORT_MIME: &str = "application/x-spiling-appearance-report+json";
pub const MAX_PALETTE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PALETTE_ROWS: usize = 4096;
pub const MAX_APPEARANCE_ALTERNATIVES: usize = 256;
pub const MAX_APPEARANCE_REPORT_BYTES: usize = 1024 * 1024;
pub const PALETTE_CONVERSION: &str = "spiling-c1a-explicit-color-1";

fn invalid(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::InvalidProject, message)
}
fn resource(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::ResourceLimit, message)
}
fn text(value: &str, max: usize) -> Result<(), ProjectError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > max
        || value.chars().any(char::is_control)
    {
        return Err(invalid(
            "appearance text is empty, noncanonical, contains controls or exceeds its bound",
        ));
    }
    Ok(())
}
fn optional_text(value: &Option<String>) -> Result<(), ProjectError> {
    if let Some(value) = value {
        text(value, 128)?;
    }
    Ok(())
}
pub(crate) fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
fn bounded_text<'de, D: serde::Deserializer<'de>, const N: usize>(
    d: D,
) -> Result<String, D::Error> {
    struct Visitor<const N: usize>;
    impl<'de, const N: usize> serde::de::Visitor<'de> for Visitor<N> {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "a string of at most {N} bytes")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
            if value.len() > N {
                return Err(E::custom("appearance string exceeds limit"));
            }
            Ok(value.to_owned())
        }
    }
    d.deserialize_str(Visitor::<N>)
}
pub(crate) fn text64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_text::<D, 64>(d)
}
fn text128<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_text::<D, 128>(d)
}
fn text256<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_text::<D, 256>(d)
}
fn text1024<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_text::<D, 1024>(d)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "encoding", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppearanceColor {
    Srgb { channels: [f64; 3] },
    XyzY1 { channels: [f64; 3] },
    XyzY100 { channels: [f64; 3] },
    D65Lab { channels: [f64; 3] },
}
impl AppearanceColor {
    pub fn validate(&self) -> Result<(), ProjectError> {
        match self {
            Self::Srgb { channels } => {
                if channels
                    .iter()
                    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                {
                    return Err(invalid("sRGB requires finite encoded channels in [0,1]"));
                }
            }
            Self::XyzY1 { channels } | Self::XyzY100 { channels } => {
                if channels.iter().any(|v| !v.is_finite()) {
                    return Err(invalid(
                        "XYZ requires finite signed channels in its explicit Y scale",
                    ));
                }
            }
            Self::D65Lab { channels } => {
                if !channels.iter().all(|v| v.is_finite()) {
                    return Err(invalid(
                        "D65 Lab requires finite signed channels, without display gamut or lightness clipping",
                    ));
                }
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceIlluminant {
    D65,
    D50,
    Unknown,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceObserver {
    Cie1931TwoDegree,
    Cie1964TenDegree,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppearanceConditions {
    pub illuminant: AppearanceIlluminant,
    pub observer: AppearanceObserver,
    #[serde(deserialize_with = "optional_text128")]
    pub geometry: Option<String>,
    #[serde(deserialize_with = "optional_text128")]
    pub backing: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub thickness_mm: Option<f64>,
    #[serde(deserialize_with = "optional_text128")]
    pub cohort: Option<String>,
    #[serde(deserialize_with = "optional_text128")]
    pub instrument: Option<String>,
    #[serde(deserialize_with = "optional_text128")]
    pub calibration: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub aperture_mm: Option<f64>,
    #[serde(deserialize_with = "optional_text128")]
    pub specular: Option<String>,
    #[serde(deserialize_with = "optional_text128")]
    pub orientation: Option<String>,
    /// Declared process/construction context references only. They bind supplied
    /// readings, not execution authority or physical qualification.
    #[serde(deserialize_with = "optional_text128")]
    pub process: Option<String>,
    #[serde(deserialize_with = "optional_text128")]
    pub construction: Option<String>,
}
impl Default for AppearanceConditions {
    fn default() -> Self {
        Self {
            illuminant: AppearanceIlluminant::D65,
            observer: AppearanceObserver::Cie1931TwoDegree,
            geometry: None,
            backing: None,
            thickness_mm: None,
            cohort: None,
            instrument: None,
            calibration: None,
            aperture_mm: None,
            specular: None,
            orientation: None,
            process: None,
            construction: None,
        }
    }
}
impl AppearanceConditions {
    pub fn validate(&self) -> Result<(), ProjectError> {
        optional_text(&self.geometry)?;
        optional_text(&self.backing)?;
        optional_text(&self.cohort)?;
        optional_text(&self.instrument)?;
        optional_text(&self.calibration)?;
        optional_text(&self.specular)?;
        optional_text(&self.orientation)?;
        optional_text(&self.process)?;
        optional_text(&self.construction)?;
        if self
            .aperture_mm
            .is_some_and(|v| !v.is_finite() || v <= 0.0 || v > 1000.0)
        {
            return Err(invalid(
                "measurement aperture must be finite and in (0,1000] mm",
            ));
        }
        if self
            .thickness_mm
            .is_some_and(|v| !v.is_finite() || v <= 0.0 || v > 1000.0)
        {
            return Err(invalid(
                "appearance thickness must be finite and in (0,1000] mm",
            ));
        }
        Ok(())
    }
    /// Complete declared metadata enables matching declared readings only. Presence
    /// of these strings does not independently qualify an instrument or coupon.
    pub fn is_measured(&self) -> bool {
        self.geometry.is_some()
            && self.backing.is_some()
            && self.thickness_mm.is_some()
            && self.cohort.is_some()
            && self.instrument.is_some()
            && self.calibration.is_some()
            && self.aperture_mm.is_some()
            && self.specular.is_some()
            && self.orientation.is_some()
            && self.process.is_some()
            && self.construction.is_some()
    }
    /// Only identifies the implemented numerical white/observer convention.
    pub fn is_supported(&self) -> bool {
        self.illuminant == AppearanceIlluminant::D65
            && self.observer == AppearanceObserver::Cie1931TwoDegree
    }
    fn allocated_bytes(&self) -> usize {
        [
            &self.geometry,
            &self.backing,
            &self.cohort,
            &self.instrument,
            &self.calibration,
            &self.specular,
            &self.orientation,
            &self.process,
            &self.construction,
        ]
        .into_iter()
        .flatten()
        .map(String::capacity)
        .sum()
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceScope {
    CatalogEstimates,
    Measured,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppearanceTarget {
    pub color: AppearanceColor,
    pub delta_e00_tolerance: f64,
    #[serde(deserialize_with = "required_option")]
    pub lightness_tolerance: Option<f64>,
    pub conditions: AppearanceConditions,
    pub evidence_scope: EvidenceScope,
}
impl AppearanceTarget {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.color.validate()?;
        self.conditions.validate()?;
        if !self.delta_e00_tolerance.is_finite()
            || self.delta_e00_tolerance < 0.0
            || self
                .lightness_tolerance
                .is_some_and(|v| !v.is_finite() || v < 0.0)
        {
            return Err(invalid(
                "appearance tolerances must be finite and nonnegative",
            ));
        }
        if self.evidence_scope == EvidenceScope::Measured && !self.conditions.is_measured() {
            return Err(invalid(
                "measured targets require all declared measurement conditions and cohort",
            ));
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> usize {
        self.conditions.allocated_bytes()
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SearchLimits {
    pub max_visited: u32,
    pub max_alternatives: u32,
}
impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_visited: MAX_PALETTE_ROWS as u32,
            max_alternatives: MAX_APPEARANCE_ALTERNATIVES as u32,
        }
    }
}
impl SearchLimits {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.max_visited == 0
            || self.max_visited as usize > MAX_PALETTE_ROWS
            || self.max_alternatives == 0
            || self.max_alternatives as usize > MAX_APPEARANCE_ALTERNATIVES
        {
            return Err(resource(
                "appearance search limits exceed supported nonzero bounds",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[expect(
    clippy::large_enum_variant,
    reason = "Bounded palette evidence stays inline; boxing adds an allocation per measured row"
)]
pub enum AppearanceEvidence {
    CatalogEstimate {
        srgb: [f64; 3],
    },
    /// A declared reading, assessable only against matching declared conditions.
    /// This record never establishes independent physical or optical qualification.
    Measured {
        color: AppearanceColor,
        conditions: AppearanceConditions,
        /// Candidate-specific declared lot. Never a target/global filter.
        #[serde(deserialize_with = "optional_text128")]
        material_lot: Option<String>,
    },
    Unknown {
        #[serde(deserialize_with = "text256")]
        reason: String,
    },
}
impl AppearanceEvidence {
    pub fn validate(&self) -> Result<(), ProjectError> {
        match self {
            Self::CatalogEstimate { srgb } => AppearanceColor::Srgb { channels: *srgb }.validate(),
            Self::Measured {
                color,
                conditions,
                material_lot,
            } => {
                color.validate()?;
                conditions.validate()?;
                optional_text(material_lot)?;
                Ok(())
            }
            Self::Unknown { reason } => text(reason, 256),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PaletteRow {
    #[serde(deserialize_with = "text64")]
    pub key: String,
    #[serde(deserialize_with = "text64")]
    pub material_id: String,
    #[serde(deserialize_with = "text64")]
    pub evidence_id: String,
    #[serde(deserialize_with = "text256")]
    pub name: String,
    pub evidence: AppearanceEvidence,
}
impl PaletteRow {
    pub fn validate(&self) -> Result<(), ProjectError> {
        text(&self.key, 64)?;
        text(&self.material_id, 64)?;
        text(&self.evidence_id, 64)?;
        text(&self.name, 256)?;
        if !self
            .material_id
            .bytes()
            .chain(self.evidence_id.bytes())
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(invalid(
                "palette identity components require ASCII alphanumerics, dot, underscore or hyphen",
            ));
        }
        if self.material_id.contains('/')
            || self.evidence_id.contains('/')
            || self.key.len() != self.material_id.len() + 1 + self.evidence_id.len()
            || !self.key.starts_with(&self.material_id)
            || self.key.as_bytes().get(self.material_id.len()) != Some(&b'/')
            || !self.key.ends_with(&self.evidence_id)
        {
            return Err(invalid(
                "palette key must be material_id/evidence_id with slash-free components",
            ));
        }
        self.evidence.validate()
    }
    pub fn evidence_hash(&self) -> Result<SourceHash, ProjectError> {
        self.validate()?;
        hash_json(
            &(&self.material_id, &self.evidence_id, &self.evidence),
            8192,
        )
        .map(|(hash, _)| hash)
    }
}
fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<PaletteRow>, D::Error> {
    struct Rows;
    impl<'de> serde::de::Visitor<'de> for Rows {
        type Value = Vec<PaletteRow>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded palette row array")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            struct RowSeed(bool);
            impl<'de> serde::de::DeserializeSeed<'de> for RowSeed {
                type Value = PaletteRow;
                fn deserialize<D: serde::Deserializer<'de>>(
                    self,
                    d: D,
                ) -> Result<Self::Value, D::Error> {
                    if !self.0 {
                        return Err(serde::de::Error::custom("palette row count exceeds limit"));
                    }
                    PaletteRow::deserialize(d)
                }
            }
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element_seed(RowSeed(rows.len() < MAX_PALETTE_ROWS))? {
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PaletteDocument {
    #[serde(deserialize_with = "text256")]
    pub name: String,
    #[serde(deserialize_with = "text1024")]
    pub source_url: String,
    #[serde(deserialize_with = "text128")]
    pub source_revision: String,
    #[serde(deserialize_with = "text128")]
    pub license: String,
    #[serde(deserialize_with = "text128")]
    pub conversion: String,
    #[serde(deserialize_with = "bounded_rows")]
    pub rows: Vec<PaletteRow>,
}
impl PaletteDocument {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProjectError> {
        decode_palette(bytes)
    }
    /// Conservative pre-allocation charge: at most source bytes of owned string
    /// contents (double charged for transient serde scratch), geometrically grown
    /// row backing storage bounded by lexical object count, and 8KiB scratch.
    pub fn allocation_bound(bytes: &[u8]) -> Result<usize, ProjectError> {
        preflight_appearance_json(bytes, MAX_PALETTE_BYTES)?;
        let objects = bytes
            .iter()
            .filter(|&&b| b == b'{')
            .count()
            .min(MAX_PALETTE_ROWS);
        Ok(bytes.len() * 2
            + objects.max(4).next_power_of_two() * std::mem::size_of::<PaletteRow>()
            + 8192)
    }
    pub fn decode_bounded(bytes: &[u8], budget: usize) -> Result<Self, ProjectError> {
        if Self::allocation_bound(bytes)? > budget {
            return Err(resource(
                "parsed palette exceeds reserved allocation budget",
            ));
        }
        let document = Self::decode(bytes)?;
        if document.allocated_bytes() > budget {
            return Err(resource(
                "parsed palette exceeds reserved allocation budget",
            ));
        }
        Ok(document)
    }
    pub fn allocated_bytes(&self) -> usize {
        self.name.capacity()
            + self.source_url.capacity()
            + self.source_revision.capacity()
            + self.license.capacity()
            + self.conversion.capacity()
            + self.rows.capacity() * std::mem::size_of::<PaletteRow>()
            + self
                .rows
                .iter()
                .map(|row| {
                    row.key.capacity()
                        + row.material_id.capacity()
                        + row.evidence_id.capacity()
                        + row.name.capacity()
                        + match &row.evidence {
                            AppearanceEvidence::CatalogEstimate { .. } => 0,
                            AppearanceEvidence::Measured {
                                conditions,
                                material_lot,
                                ..
                            } => {
                                conditions.allocated_bytes()
                                    + material_lot.as_ref().map_or(0, String::capacity)
                            }
                            AppearanceEvidence::Unknown { reason } => reason.capacity(),
                        }
                })
                .sum::<usize>()
    }
    pub fn validate_canonical_bytes(&self, bytes: &[u8]) -> Result<(), ProjectError> {
        self.validate()?;
        // Compare the serializer's chunks directly: no second JSON byte buffer.
        struct Compare<'a> {
            remaining: &'a [u8],
        }
        impl std::io::Write for Compare<'_> {
            fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
                if !self.remaining.starts_with(chunk) {
                    return Err(std::io::Error::other("canonical palette mismatch"));
                }
                self.remaining = &self.remaining[chunk.len()..];
                Ok(chunk.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        #[derive(Serialize)]
        struct Canonical<'a> {
            name: &'a str,
            source_url: &'a str,
            source_revision: &'a str,
            license: &'a str,
            conversion: &'a str,
            rows: Vec<&'a PaletteRow>,
        }
        let mut rows: Vec<_> = self.rows.iter().collect();
        rows.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        let mut compare = Compare { remaining: bytes };
        serde_json::to_writer(
            &mut compare,
            &Canonical {
                name: &self.name,
                source_url: &self.source_url,
                source_revision: &self.source_revision,
                license: &self.license,
                conversion: &self.conversion,
                rows,
            },
        )
        .map_err(|_| invalid("palette canonical bytes do not match document"))?;
        if !compare.remaining.is_empty() {
            return Err(invalid("trailing canonical palette bytes"));
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), ProjectError> {
        text(&self.name, 256)?;
        text(&self.source_url, 1024)?;
        text(&self.source_revision, 128)?;
        text(&self.license, 128)?;
        text(&self.conversion, 128)?;
        if !self.source_url.starts_with("https://") || self.conversion != PALETTE_CONVERSION {
            return Err(invalid(
                "palette requires an HTTPS source and supported conversion identity",
            ));
        }
        if self.rows.is_empty() || self.rows.len() > MAX_PALETTE_ROWS {
            return Err(resource("palette requires 1..4096 rows"));
        }
        let mut keys = BTreeSet::new();
        for row in &self.rows {
            row.validate()?;
            if !keys.insert(&row.key) {
                return Err(invalid("duplicate palette material/evidence key"));
            }
        }
        Ok(())
    }
    /// Serialize in stable material/evidence key order, independent of import row order.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ProjectError> {
        self.canonical_bytes_bounded(MAX_PALETTE_BYTES)
    }
    pub fn canonical_bytes_bounded(&self, max_bytes: usize) -> Result<Vec<u8>, ProjectError> {
        self.validate()?;
        #[derive(Serialize)]
        struct Canonical<'a> {
            name: &'a str,
            source_url: &'a str,
            source_revision: &'a str,
            license: &'a str,
            conversion: &'a str,
            rows: Vec<&'a PaletteRow>,
        }
        let mut rows: Vec<_> = self.rows.iter().collect();
        rows.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        encode_bounded(
            &Canonical {
                name: &self.name,
                source_url: &self.source_url,
                source_revision: &self.source_revision,
                license: &self.license,
                conversion: &self.conversion,
                rows,
            },
            max_bytes.min(MAX_PALETTE_BYTES),
        )
    }
    /// Hash canonical JSON directly without allocating a canonical byte buffer.
    pub fn canonical_hash(&self) -> Result<SourceHash, ProjectError> {
        self.validate()?;
        #[derive(Serialize)]
        struct Canonical<'a> {
            name: &'a str,
            source_url: &'a str,
            source_revision: &'a str,
            license: &'a str,
            conversion: &'a str,
            rows: Vec<&'a PaletteRow>,
        }
        let mut rows: Vec<_> = self.rows.iter().collect();
        rows.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        hash_json(
            &Canonical {
                name: &self.name,
                source_url: &self.source_url,
                source_revision: &self.source_revision,
                license: &self.license,
                conversion: &self.conversion,
                rows,
            },
            MAX_PALETTE_BYTES,
        )
        .map(|(hash, _)| hash)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PaletteRecord {
    pub name: String,
    pub original_hash: SourceHash,
    pub original_bytes: u32,
    pub canonical_hash: SourceHash,
    pub canonical_bytes: u32,
    pub source_url: String,
    pub source_revision: String,
    pub license: String,
    pub conversion: String,
}
impl PaletteRecord {
    pub fn new(
        document: &PaletteDocument,
        original: &[u8],
        canonical: &[u8],
        source_name: String,
    ) -> Result<Self, ProjectError> {
        document.validate()?;
        text(&source_name, 256)?;
        document.validate_canonical_bytes(canonical)?;
        let record = Self {
            name: source_name,
            original_hash: SourceHash::from_bytes(original),
            original_bytes: u32::try_from(original.len())
                .map_err(|_| resource("palette bytes overflow"))?,
            canonical_hash: SourceHash::from_bytes(canonical),
            canonical_bytes: u32::try_from(canonical.len())
                .map_err(|_| resource("palette bytes overflow"))?,
            source_url: document.source_url.clone(),
            source_revision: document.source_revision.clone(),
            license: document.license.clone(),
            conversion: document.conversion.clone(),
        };
        record.validate_bytes(original, canonical)?;
        Ok(record)
    }
    pub fn validate(&self) -> Result<(), ProjectError> {
        text(&self.name, 256)?;
        text(&self.source_url, 1024)?;
        text(&self.source_revision, 128)?;
        text(&self.license, 128)?;
        if !self.source_url.starts_with("https://") || self.conversion != PALETTE_CONVERSION {
            return Err(invalid("unsupported palette provenance or conversion"));
        }
        if self.original_bytes == 0
            || self.canonical_bytes == 0
            || self.original_bytes as usize > MAX_PALETTE_BYTES
            || self.canonical_bytes as usize > MAX_PALETTE_BYTES
        {
            return Err(resource("palette byte count outside supported bounds"));
        }
        Ok(())
    }
    pub fn validate_hashes(&self, original: &[u8], canonical: &[u8]) -> Result<(), ProjectError> {
        self.validate()?;
        if original.len() != self.original_bytes as usize
            || canonical.len() != self.canonical_bytes as usize
            || !self.original_hash.matches_bytes(original)
            || !self.canonical_hash.matches_bytes(canonical)
        {
            return Err(ProjectError::new(
                ProjectErrorCode::CorruptAsset,
                "palette byte count or content hash mismatch",
            ));
        }
        Ok(())
    }
    pub fn validate_document(
        &self,
        document: &PaletteDocument,
        canonical: &[u8],
    ) -> Result<(), ProjectError> {
        document.validate_canonical_bytes(canonical)?;
        self.validate_provenance(document)
    }
    pub fn validate_provenance(&self, document: &PaletteDocument) -> Result<(), ProjectError> {
        self.validate()?;
        if document.source_url != self.source_url
            || document.source_revision != self.source_revision
            || document.license != self.license
            || document.conversion != self.conversion
        {
            return Err(invalid(
                "palette provenance differs from canonical document",
            ));
        }
        Ok(())
    }
    pub fn validate_bytes(&self, original: &[u8], canonical: &[u8]) -> Result<(), ProjectError> {
        self.validate_hashes(original, canonical)?;
        let original_document = decode_palette(original)?;
        self.validate_document(&original_document, canonical)?;
        Ok(())
    }
    pub fn allocated_bytes(&self) -> usize {
        self.name.capacity()
            + self.original_hash.allocated_bytes()
            + self.canonical_hash.allocated_bytes()
            + self.source_url.capacity()
            + self.source_revision.capacity()
            + self.license.capacity()
            + self.conversion.capacity()
    }
}

/// Allocation-free lexical admission before serde builds strings/rows. Bounds escaped
/// string scratch to 4096 source bytes and rejects excessive structure before growth.
/// Row/text visitors bound owned storage. PaletteDocument::allocation_bound gives
/// a conservative source-dependent charge BEFORE parsing; byte buffers are separate.
pub fn preflight_appearance_json(bytes: &[u8], max_bytes: usize) -> Result<(), ProjectError> {
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(resource("appearance JSON byte budget exceeded"));
    }
    let mut quoted = false;
    let mut escaped = false;
    let mut string_bytes = 0usize;
    let mut structures = 0usize;
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
                string_bytes += 1;
            } else if byte == b'\\' {
                escaped = true;
                string_bytes += 1;
            } else if byte == b'"' {
                quoted = false;
            } else {
                string_bytes += 1;
            }
            if string_bytes > 4096 {
                return Err(resource("appearance JSON string exceeds lexical budget"));
            }
        } else if byte == b'"' {
            quoted = true;
            string_bytes = 0;
        } else if byte == b'{' || byte == b'[' {
            structures += 1;
            if structures > MAX_PALETTE_ROWS * 8 + 64 {
                return Err(resource("appearance JSON structure budget exceeded"));
            }
        }
    }
    Ok(())
}
pub fn decode_palette(bytes: &[u8]) -> Result<PaletteDocument, ProjectError> {
    preflight_appearance_json(bytes, MAX_PALETTE_BYTES)?;
    let document: PaletteDocument =
        serde_json::from_slice(bytes).map_err(|_| invalid("invalid bounded palette JSON"))?;
    document.validate()?;
    Ok(document)
}
pub(crate) fn encode_bounded<T: Serialize>(value: &T, max: usize) -> Result<Vec<u8>, ProjectError> {
    struct Writer {
        bytes: Vec<u8>,
        max: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.max.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other(
                    "appearance encoding exceeds byte budget",
                ));
            }
            let required = self.bytes.len() + bytes.len();
            if required > self.bytes.capacity() {
                let capacity = required
                    .max(self.bytes.capacity().saturating_mul(2))
                    .min(self.max);
                self.bytes
                    .try_reserve_exact(capacity - self.bytes.len())
                    .map_err(|_| std::io::Error::other("appearance encoding allocation failed"))?;
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        bytes: Vec::new(),
        max,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| resource("appearance encoding exceeds byte budget"))?;
    Ok(writer.bytes)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceOutcome {
    AssessedMatch,
    AssessedMismatch,
    NoAssessedMatch,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppearanceStatus {
    Complete { outcome: AppearanceOutcome },
    WorkLimit,
    Cancelled,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
pub enum AssessedEvidence {
    #[serde(rename = "c")]
    CatalogEstimate,
    #[serde(rename = "m")]
    Measured,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "k", deny_unknown_fields)]
pub enum RowAssessment {
    #[serde(rename = "f")]
    FilteredEvidence,
    #[serde(rename = "i")]
    ConditionsMismatch,
    #[serde(rename = "u")]
    UnknownEvidence,
    #[serde(rename = "a")]
    Assessed {
        #[serde(rename = "e")]
        evidence: AssessedEvidence,
        #[serde(rename = "d")]
        delta_e00: f64,
        #[serde(rename = "l")]
        delta_lightness: f64,
        #[serde(rename = "m")]
        within_tolerance: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
/// Compact JSON member names reserve room for EVERY visited row under the 1MiB
/// report bound: key/evidence hash/assessment, then kind/evidence/DE00/dL/match.
pub struct AppearanceVisit {
    #[serde(rename = "k", deserialize_with = "text64")]
    pub key: String,
    #[serde(rename = "e")]
    pub evidence_hash: SourceHash,
    #[serde(rename = "a")]
    pub assessment: RowAssessment,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppearanceAlternative {
    #[serde(deserialize_with = "text64")]
    pub key: String,
    #[serde(deserialize_with = "text256")]
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppearanceReport {
    pub producer: String,
    pub input_hash: SourceHash,
    pub palette_hash: SourceHash,
    pub target: AppearanceTarget,
    pub limits: SearchLimits,
    pub total_rows: u32,
    #[serde(deserialize_with = "bounded_visits")]
    pub visits: Vec<AppearanceVisit>,
    #[serde(deserialize_with = "bounded_alternatives")]
    pub alternatives: Vec<AppearanceAlternative>,
    pub alternatives_truncated: bool,
    pub status: AppearanceStatus,
}
/// Input identity binds canonical palette bytes by their SHA256, full target including
/// evidence scope/conditions, explicit limits and exact producer. No time/environment.
pub fn appearance_input_hash(
    palette_hash: &SourceHash,
    target: &AppearanceTarget,
    limits: SearchLimits,
) -> Result<SourceHash, ProjectError> {
    target.validate()?;
    limits.validate()?;
    Ok(SourceHash::from_bytes(&encode_bounded(
        &(APPEARANCE_PRODUCER, palette_hash, target, limits),
        8192,
    )?))
}
impl AppearanceReport {
    pub fn is_complete(&self) -> bool {
        matches!(self.status, AppearanceStatus::Complete { .. })
    }
    pub fn has_selection(&self, key: &str) -> bool {
        self.alternatives.iter().any(|v| v.key == key)
    }
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.target.validate()?;
        self.limits.validate()?;
        if self.producer != APPEARANCE_PRODUCER
            || self.input_hash
                != appearance_input_hash(&self.palette_hash, &self.target, self.limits)?
        {
            return Err(invalid(
                "appearance report producer/input identity mismatch",
            ));
        }
        if self.total_rows == 0
            || self.total_rows as usize > MAX_PALETTE_ROWS
            || self.visits.len() > self.total_rows as usize
            || self.visits.len() > self.limits.max_visited as usize
            || self.alternatives.len() > self.limits.max_alternatives as usize
        {
            return Err(resource(
                "appearance report row/alternative bounds exceeded",
            ));
        }
        let mut previous: Option<&str> = None;
        let mut assessed = Vec::with_capacity(self.visits.len());
        let mut any_match = false;
        let mut unassessed = false;
        for visit in &self.visits {
            text(&visit.key, 64)?;
            if previous.is_some_and(|p| p >= visit.key.as_str()) {
                return Err(invalid("appearance visits are not unique sorted keys"));
            }
            previous = Some(&visit.key);
            unassessed |= matches!(
                visit.assessment,
                RowAssessment::UnknownEvidence | RowAssessment::ConditionsMismatch
            );
            if let RowAssessment::Assessed {
                evidence,
                delta_e00,
                delta_lightness,
                within_tolerance,
            } = visit.assessment
            {
                if !delta_e00.is_finite()
                    || delta_e00 < 0.0
                    || !delta_lightness.is_finite()
                    || delta_lightness < 0.0
                    || within_tolerance
                        != (delta_e00 <= self.target.delta_e00_tolerance
                            && self
                                .target
                                .lightness_tolerance
                                .is_none_or(|max| delta_lightness <= max))
                    || (evidence == AssessedEvidence::CatalogEstimate)
                        != (self.target.evidence_scope == EvidenceScope::CatalogEstimates)
                {
                    return Err(invalid(
                        "appearance assessment is nonfinite or inconsistent with target",
                    ));
                }
                any_match |= within_tolerance;
                assessed.push((delta_e00, visit.key.as_str()));
            }
        }
        assessed.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        let kept = assessed.len().min(self.limits.max_alternatives as usize);
        for alternative in &self.alternatives {
            text(&alternative.key, 64)?;
            text(&alternative.name, 256)?;
        }
        if self.alternatives.len() != kept
            || self
                .alternatives
                .iter()
                .zip(&assessed)
                .any(|(alternative, (_, expected))| alternative.key != *expected)
            || self.alternatives_truncated != (assessed.len() > kept)
        {
            return Err(invalid(
                "appearance alternatives do not match ranked assessed visits",
            ));
        }
        match self.status {
            AppearanceStatus::Complete { outcome } => {
                let expected = if any_match {
                    AppearanceOutcome::AssessedMatch
                } else if assessed.is_empty() || unassessed {
                    AppearanceOutcome::NoAssessedMatch
                } else {
                    AppearanceOutcome::AssessedMismatch
                };
                if self.visits.len() != self.total_rows as usize || outcome != expected {
                    return Err(invalid(
                        "complete appearance report has incomplete visits or wrong outcome",
                    ));
                }
            }
            AppearanceStatus::WorkLimit
                if self.visits.len() != self.limits.max_visited as usize
                    || self.visits.len() == self.total_rows as usize =>
            {
                return Err(invalid(
                    "work-limited report has inconsistent visited count",
                ));
            }
            AppearanceStatus::Cancelled if self.visits.len() == self.total_rows as usize => {
                return Err(invalid("cancelled report already exhausted all rows"));
            }
            _ => {}
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, ProjectError> {
        self.validate()?;
        encode_bounded(self, MAX_APPEARANCE_REPORT_BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProjectError> {
        preflight_appearance_json(bytes, MAX_APPEARANCE_REPORT_BYTES)?;
        let report: Self =
            serde_json::from_slice(bytes).map_err(|_| invalid("invalid appearance report JSON"))?;
        report.validate()?;
        Ok(report)
    }
    pub fn record(&self, bytes: &[u8]) -> Result<AppearanceReportRecord, ProjectError> {
        self.validate()?;
        preflight_appearance_json(bytes, MAX_APPEARANCE_REPORT_BYTES)?;
        validate_encoded_report(self, bytes)?;
        Ok(AppearanceReportRecord {
            hash: SourceHash::from_bytes(bytes),
            byte_count: bytes.len() as u64,
            input_hash: self.input_hash.clone(),
            producer: self.producer.clone(),
            complete: self.is_complete(),
        })
    }
    pub fn matches_inputs(
        &self,
        palette: &PaletteRecord,
        target: &AppearanceTarget,
        limits: SearchLimits,
    ) -> Result<(), ProjectError> {
        self.validate()?;
        palette.validate()?;
        target.validate()?;
        limits.validate()?;
        if self.palette_hash != palette.canonical_hash
            || self.target != *target
            || self.limits != limits
        {
            return Err(invalid(
                "appearance report does not match active study inputs",
            ));
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> usize {
        self.producer.capacity()
            + self.input_hash.allocated_bytes()
            + self.palette_hash.allocated_bytes()
            + self.target.allocated_bytes()
            + self.visits.capacity() * std::mem::size_of::<AppearanceVisit>()
            + self
                .visits
                .iter()
                .map(|v| v.key.capacity() + v.evidence_hash.allocated_bytes())
                .sum::<usize>()
            + self.alternatives.capacity() * std::mem::size_of::<AppearanceAlternative>()
            + self
                .alternatives
                .iter()
                .map(|v| v.key.capacity() + v.name.capacity())
                .sum::<usize>()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppearanceReportRecord {
    pub hash: SourceHash,
    pub byte_count: u64,
    pub input_hash: SourceHash,
    pub producer: String,
    pub complete: bool,
}
impl AppearanceReportRecord {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.byte_count == 0 || self.byte_count > MAX_APPEARANCE_REPORT_BYTES as u64 {
            return Err(resource("appearance report byte count exceeds bounds"));
        }
        if self.producer != APPEARANCE_PRODUCER {
            return Err(invalid("unsupported appearance report producer"));
        }
        Ok(())
    }
    pub fn validate_bytes(&self, bytes: &[u8]) -> Result<AppearanceReport, ProjectError> {
        self.validate()?;
        if bytes.len() as u64 != self.byte_count || !self.hash.matches_bytes(bytes) {
            return Err(ProjectError::new(
                ProjectErrorCode::CorruptAsset,
                "appearance report content hash/length mismatch",
            ));
        }
        let report = AppearanceReport::decode(bytes)?;
        if report.input_hash != self.input_hash
            || report.producer != self.producer
            || report.is_complete() != self.complete
        {
            return Err(invalid(
                "appearance report descriptor does not bind decoded report",
            ));
        }
        Ok(report)
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppearanceStudy {
    #[serde(deserialize_with = "required_option")]
    pub palette: Option<PaletteRecord>,
    #[serde(deserialize_with = "required_option")]
    pub target: Option<AppearanceTarget>,
    pub limits: SearchLimits,
    #[serde(deserialize_with = "required_option")]
    pub report: Option<Arc<AppearanceReport>>,
    #[serde(deserialize_with = "required_option")]
    pub report_record: Option<AppearanceReportRecord>,
    #[serde(deserialize_with = "required_option")]
    pub selection: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub optical: Option<crate::optical::OpticalStudy>,
}
impl AppearanceStudy {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.limits.validate()?;
        if let Some(palette) = &self.palette {
            palette.validate()?;
        }
        if let Some(target) = &self.target {
            target.validate()?;
        }
        if let Some(optical) = &self.optical {
            optical.validate(
                self.palette
                    .as_ref()
                    .ok_or_else(|| invalid("optical study has no palette"))?,
            )?;
        }
        if let Some(report) = &self.report {
            self.matches_report(report)?;
            if !report.is_complete() {
                return Err(invalid("incomplete appearance report cannot be accepted"));
            }
            let record = self
                .report_record
                .as_ref()
                .ok_or_else(|| invalid("accepted appearance report lacks cached record"))?;
            if report.canonical_record()? != *record {
                return Err(invalid("cached appearance report record mismatch"));
            }
        }
        if self.report.is_none() && self.report_record.is_some() {
            return Err(invalid("appearance report record lacks report"));
        }
        if let Some(key) = &self.selection {
            text(key, 64)?;
            if !self
                .report
                .as_ref()
                .is_some_and(|report| report.has_selection(key))
            {
                return Err(invalid(
                    "appearance selection is not an assessed retained alternative",
                ));
            }
        }
        Ok(())
    }
    pub fn matches_report(&self, report: &AppearanceReport) -> Result<(), ProjectError> {
        report.matches_inputs(
            self.palette
                .as_ref()
                .ok_or_else(|| invalid("appearance study has no palette"))?,
            self.target
                .as_ref()
                .ok_or_else(|| invalid("appearance study has no target"))?,
            self.limits,
        )
    }
    /// Project snapshots already validate studies before exposing infallible info.
    pub fn info(&self) -> AppearanceStudyInfo {
        AppearanceStudyInfo {
            palette: self.palette.clone(),
            target: self.target.clone(),
            limits: self.limits,
            report: self.report_record.clone(),
            selection: self.selection.clone(),
            optical: self
                .optical
                .as_ref()
                .map(crate::optical::OpticalStudy::info),
        }
    }
    /// Does not include shared Arc report storage; callers deduplicate report pointers.
    pub fn allocated_bytes(&self) -> usize {
        self.palette
            .as_ref()
            .map_or(0, PaletteRecord::allocated_bytes)
            + self
                .target
                .as_ref()
                .map_or(0, AppearanceTarget::allocated_bytes)
            + self.selection.as_ref().map_or(0, String::capacity)
            + self.report_record.as_ref().map_or(0, |r| {
                r.hash.allocated_bytes() + r.input_hash.allocated_bytes() + r.producer.capacity()
            })
            + self
                .optical
                .as_ref()
                .map_or(0, crate::optical::OpticalStudy::allocated_bytes)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppearanceStudyInfo {
    #[serde(deserialize_with = "required_option")]
    pub palette: Option<PaletteRecord>,
    #[serde(deserialize_with = "required_option")]
    pub target: Option<AppearanceTarget>,
    pub limits: SearchLimits,
    #[serde(deserialize_with = "required_option")]
    pub report: Option<AppearanceReportRecord>,
    #[serde(deserialize_with = "required_option")]
    pub selection: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub optical: Option<crate::optical::OpticalStudyInfo>,
}
impl AppearanceStudyInfo {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.limits.validate()?;
        if let Some(palette) = &self.palette {
            palette.validate()?;
        }
        if let Some(target) = &self.target {
            target.validate()?;
        }
        if let Some(optical) = &self.optical {
            optical.validate(
                self.palette
                    .as_ref()
                    .ok_or_else(|| invalid("optical study info has no palette"))?,
            )?;
        }
        if let Some(report) = &self.report {
            report.validate()?;
            let palette = self
                .palette
                .as_ref()
                .ok_or_else(|| invalid("report info lacks palette"))?;
            let target = self
                .target
                .as_ref()
                .ok_or_else(|| invalid("report info lacks target"))?;
            if !report.complete
                || report.input_hash
                    != appearance_input_hash(&palette.canonical_hash, target, self.limits)?
            {
                return Err(invalid("report info does not bind complete study inputs"));
            }
        }
        if let Some(selection) = &self.selection {
            text(selection, 64)?;
            if self.report.is_none() {
                return Err(invalid("selection info lacks accepted report"));
            }
        }
        Ok(())
    }
}
fn optional_text128<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    struct Optional;
    impl<'de> serde::de::Visitor<'de> for Optional {
        type Value = Option<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("null or a bounded appearance string")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            text128(d).map(Some)
        }
    }
    d.deserialize_option(Optional)
}
pub(crate) fn bounded_sequence<
    'de,
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
    const N: usize,
>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    struct Sequence<T, const N: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Sequence<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded appearance sequence")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            struct Seed<T>(bool, std::marker::PhantomData<T>);
            impl<'de, T: Deserialize<'de>> serde::de::DeserializeSeed<'de> for Seed<T> {
                type Value = T;
                fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
                    if !self.0 {
                        return Err(serde::de::Error::custom(
                            "appearance sequence exceeds limit",
                        ));
                    }
                    T::deserialize(d)
                }
            }
            let mut values = Vec::new();
            while let Some(value) =
                seq.next_element_seed(Seed(values.len() < N, std::marker::PhantomData))?
            {
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Sequence::<T, N>(std::marker::PhantomData))
}
fn bounded_visits<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AppearanceVisit>, D::Error> {
    bounded_sequence::<D, AppearanceVisit, MAX_PALETTE_ROWS>(d)
}
fn bounded_alternatives<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AppearanceAlternative>, D::Error> {
    bounded_sequence::<D, AppearanceAlternative, MAX_APPEARANCE_ALTERNATIVES>(d)
}
fn validate_encoded_report(report: &AppearanceReport, bytes: &[u8]) -> Result<(), ProjectError> {
    struct Compare<'a>(&'a [u8]);
    impl std::io::Write for Compare<'_> {
        fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
            if !self.0.starts_with(chunk) {
                return Err(std::io::Error::other("report bytes mismatch"));
            }
            self.0 = &self.0[chunk.len()..];
            Ok(chunk.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut compare = Compare(bytes);
    serde_json::to_writer(&mut compare, report)
        .map_err(|_| invalid("appearance bytes are not the canonical encoding of this report"))?;
    if !compare.0.is_empty() {
        return Err(invalid("trailing appearance report bytes"));
    }
    Ok(())
}
pub(crate) fn hash_json<T: Serialize>(
    value: &T,
    max: usize,
) -> Result<(SourceHash, usize), ProjectError> {
    struct HashWriter {
        hash: Sha256,
        count: usize,
        max: usize,
    }
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.max.saturating_sub(self.count) {
                return Err(std::io::Error::other(
                    "appearance JSON hash byte budget exceeded",
                ));
            }
            self.hash.update(bytes);
            self.count += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter {
        hash: Sha256::new(),
        count: 0,
        max,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| resource("appearance JSON hash byte budget exceeded"))?;
    Ok((
        SourceHash::from_digest(&writer.hash.finalize().into()),
        writer.count,
    ))
}
impl AppearanceReport {
    /// Checked descriptor without allocating a second report buffer.
    pub fn canonical_record(&self) -> Result<AppearanceReportRecord, ProjectError> {
        self.validate()?;
        let (hash, count) = hash_json(self, MAX_APPEARANCE_REPORT_BYTES)?;
        Ok(AppearanceReportRecord {
            hash,
            byte_count: count as u64,
            input_hash: self.input_hash.clone(),
            producer: self.producer.clone(),
            complete: self.is_complete(),
        })
    }
}

/// Validated immutable lookup inputs. Preparing once admits original/canonical
/// correspondence, sorts stable keys and caches evidence identities; comparisons
/// never reparse, revalidate, resort, canonicalize or rehash this palette.
#[derive(Debug)]
pub struct PreparedPalette {
    document: PaletteDocument,
    canonical_hash: SourceHash,
    evidence_hashes: Vec<SourceHash>,
}
impl PreparedPalette {
    pub fn new(mut document: PaletteDocument, canonical: &[u8]) -> Result<Self, ProjectError> {
        document.validate_canonical_bytes(canonical)?;
        document.rows.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        let mut evidence_hashes = Vec::with_capacity(document.rows.len());
        for row in &document.rows {
            evidence_hashes.push(row.evidence_hash()?);
        }
        Ok(Self {
            document,
            canonical_hash: SourceHash::from_bytes(canonical),
            evidence_hashes,
        })
    }
    pub fn cache_allocation_bound(row_count: usize) -> usize {
        row_count
            .saturating_mul(std::mem::size_of::<SourceHash>() + 64)
            .saturating_add(64)
    }
    pub fn allocation_bound(original: &[u8]) -> Result<usize, ProjectError> {
        let rows_upper = original
            .iter()
            .filter(|&&b| b == b'{')
            .count()
            .min(MAX_PALETTE_ROWS);
        Ok(PaletteDocument::allocation_bound(original)? + Self::cache_allocation_bound(rows_upper))
    }
    pub fn decode_bounded(
        original: &[u8],
        canonical: &[u8],
        budget: usize,
    ) -> Result<Self, ProjectError> {
        if Self::allocation_bound(original)? > budget {
            return Err(resource(
                "prepared palette exceeds reserved allocation budget",
            ));
        }
        let document = PaletteDocument::decode(original)?;
        Self::new_bounded(document, canonical, budget)
    }
    pub fn new_bounded(
        document: PaletteDocument,
        canonical: &[u8],
        budget: usize,
    ) -> Result<Self, ProjectError> {
        if document
            .allocated_bytes()
            .checked_add(Self::cache_allocation_bound(document.rows.len()))
            .is_none_or(|bytes| bytes > budget)
        {
            return Err(resource(
                "prepared palette cache exceeds reserved allocation budget",
            ));
        }
        Self::new(document, canonical)
    }
    pub fn document(&self) -> &PaletteDocument {
        &self.document
    }
    pub fn canonical_hash(&self) -> &SourceHash {
        &self.canonical_hash
    }
    pub fn evidence_hashes(&self) -> &[SourceHash] {
        &self.evidence_hashes
    }
    pub fn allocated_bytes(&self) -> usize {
        self.document.allocated_bytes()
            + self.canonical_hash.allocated_bytes()
            + self.evidence_hashes.capacity() * std::mem::size_of::<SourceHash>()
            + self
                .evidence_hashes
                .iter()
                .map(SourceHash::allocated_bytes)
                .sum::<usize>()
    }
}
