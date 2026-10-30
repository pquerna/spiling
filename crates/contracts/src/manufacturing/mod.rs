// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded software-only manufacturing data. Profile declarations certify no machine.
use crate::process::ProcessDocument;
use crate::surface::{
    SurfaceImageContent, SurfaceOverlays, bounded_image_contents, validate_image_contents,
};
use crate::{geometry::*, project::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};
use ts_rs::TS;

mod replay;
pub use replay::*;
mod job;
pub use job::*;
mod process;
pub mod program_coordinates;
pub use process::*;
mod resolution;
pub use resolution::*;
mod diaphragm;
pub use diaphragm::*;
mod skin_fit_decode;

mod construction;
pub use construction::*;
mod routing;
pub use routing::*;

pub const MANUFACTURING_SCHEMA_VERSION: u32 = 11;
pub const MAX_MANUFACTURING_BUNDLE_BYTES: u32 = 16 * 1024 * 1024;
pub const MAX_MANUFACTURING_RETAINED_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PRINT_LAYERS: u32 = 4096;
pub const MAX_DEPOSITION_SEGMENTS: u32 = 100_000;
pub const MAX_MACHINE_COMPONENTS: u32 = 64;
pub const MAX_PROFILE_JSON_BYTES: u32 = 32_768;
pub const MAX_MANUFACTURING_ERROR_BYTES: usize = 2048;
pub const MAX_MANUFACTURING_LABEL_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManufacturingErrorCode {
    InvalidSpecification,
    UnsupportedCapability,
    UnsupportedGeometry,
    NativeKernelFailure,
    NativeConditioningFailure,
    NativeAnalysisIndeterminate,
    NativeSingularGeometry,
    NativeShapeErrorExceeded,
    ProgramCoordinateConditioningFailure,
    NoIntent,
    EmptyProject,
    StaleRevision,
    VerificationFailed,
    ResourceLimit,
    Cancelled,
    Io,
    CorruptArtifact,
    ReadOnly,
    Busy,
}
#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {message}")]
#[serde(deny_unknown_fields)]
pub struct ManufacturingError {
    pub code: ManufacturingErrorCode,
    pub message: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawError {
    code: ManufacturingErrorCode,
    message: String,
}
impl<'de> Deserialize<'de> for ManufacturingError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawError::deserialize(deserializer)?;
        if raw.message.len() > MAX_MANUFACTURING_ERROR_BYTES {
            return Err(serde::de::Error::custom(
                "manufacturing error exceeds byte limit",
            ));
        }
        Ok(Self {
            code: raw.code,
            message: raw.message,
        })
    }
}
impl ManufacturingError {
    pub fn new(code: ManufacturingErrorCode, message: impl AsRef<str>) -> Self {
        let message = message.as_ref();
        let mut end = message.len().min(MAX_MANUFACTURING_ERROR_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            code,
            message: message[..end].to_owned(),
        }
    }
}
fn invalid(message: &str) -> ManufacturingError {
    ManufacturingError::new(ManufacturingErrorCode::InvalidSpecification, message)
}
fn resource(message: &str) -> ManufacturingError {
    ManufacturingError::new(ManufacturingErrorCode::ResourceLimit, message)
}
fn label(value: &str) -> Result<(), ManufacturingError> {
    if value.trim().is_empty()
        || value.len() > MAX_MANUFACTURING_LABEL_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(invalid(
            "identity/name must be nonempty, bounded and free of control characters",
        ));
    }
    Ok(())
}
fn positive(values: &[f64]) -> Result<(), ManufacturingError> {
    if values.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(invalid(
            "manufacturing parameters must be finite and positive",
        ));
    }
    Ok(())
}
/// Count serialized bytes without allocating a second copy of native data.
fn json_bytes<T: Serialize>(value: &T, limit: usize) -> Result<usize, ManufacturingError> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let count = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("JSON byte limit exceeded"))?;
            self.bytes = count;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|e| resource(&e.to_string()))?;
    Ok(counter.bytes)
}

fn bounded_vec<'de, D, T, const LIMIT: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const N: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Bounded<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "an array with at most {N} elements")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut result = Vec::new();
            while result.len() < N {
                match seq.next_element()? {
                    Some(value) => result.push(value),
                    None => return Ok(result),
                }
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom(
                    "manufacturing array resource limit exceeded",
                ));
            }
            Ok(result)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, LIMIT>(std::marker::PhantomData))
}
fn components<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<MachineComponent>, D::Error> {
    bounded_vec::<D, _, { MAX_MACHINE_COMPONENTS as usize }>(d)
}
// One budget is owned by each admitted plan (or standalone layer/path), and
// explicitly borrowed by every nested seed before a point grows its vector.
// Per-edge widths have a separate cumulative allowance: they describe actual
// edges but must not halve the established point/segment budget.
// Skin fitting has its own unchanged witness-count and retained-byte allowance;
// every layer/domain seed borrows both before growing nested table capacity.
struct SegmentBudget {
    remaining_segments: u32,
    remaining_widths: u32,
    remaining_width_bytes: usize,
    remaining_skin_fits: u32,
    remaining_skin_fit_bytes: usize,
}
impl SegmentBudget {
    fn new() -> Self {
        Self {
            remaining_segments: MAX_DEPOSITION_SEGMENTS,
            remaining_widths: MAX_DEPOSITION_SEGMENTS,
            remaining_width_bytes: MAX_MANUFACTURING_BUNDLE_BYTES as usize,
            remaining_skin_fits: MAX_DEPOSITION_SEGMENTS,
            remaining_skin_fit_bytes: MAX_MANUFACTURING_BUNDLE_BYTES as usize,
        }
    }
    fn charge_width_metadata(&mut self) -> Result<(), &'static str> {
        self.charge_width_bytes(std::mem::size_of::<Vec<f64>>())
    }
    fn admit_width(&mut self) -> Result<(), &'static str> {
        if self.remaining_widths == 0 {
            return Err("deposition width limit exceeded");
        }
        self.remaining_widths -= 1;
        Ok(())
    }
    fn charge_width_capacity(&mut self, entries: usize) -> Result<(), &'static str> {
        let bytes = entries
            .checked_mul(std::mem::size_of::<f64>())
            .ok_or("deposition width byte limit exceeded")?;
        self.charge_width_bytes(bytes)
    }
    fn charge_width_bytes(&mut self, bytes: usize) -> Result<(), &'static str> {
        self.remaining_width_bytes = self
            .remaining_width_bytes
            .checked_sub(bytes)
            .ok_or("deposition width byte limit exceeded")?;
        Ok(())
    }
    fn charge_skin_fit_bytes(&mut self, bytes: usize) -> Result<(), &'static str> {
        self.remaining_skin_fit_bytes = self
            .remaining_skin_fit_bytes
            .checked_sub(bytes)
            .ok_or("skin fitting metadata byte limit exceeded")?;
        Ok(())
    }
}

struct PointsSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for PointsSeed<'_> {
    type Value = Vec<[f64; 3]>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> serde::de::Visitor<'de> for PointsSeed<'_> {
    type Value = Vec<[f64; 3]>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("at least two points within the cumulative segment budget")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut points = Vec::new();
        while let Some(point) = seq.next_element::<[f64; 3]>()? {
            if self.0.remaining_segments == 0 {
                return Err(serde::de::Error::custom(
                    "deposition segment limit exceeded",
                ));
            }
            if !points.is_empty() {
                self.0.remaining_segments -= 1;
            }
            points.push(point);
        }
        if points.len() < 2 {
            return Err(serde::de::Error::custom(
                "deposition path requires at least two points",
            ));
        }
        Ok(points)
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum WidthKind {
    Constant,
    PerEdge,
}
#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum WidthField {
    Kind,
    WidthMm,
    WidthsMm,
}
struct WidthProfileSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for WidthProfileSeed<'_> {
    type Value = WidthProfile;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}
impl<'de> serde::de::Visitor<'de> for WidthProfileSeed<'_> {
    type Value = WidthProfile;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a constant or per-edge deposition width profile")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut kind = None;
        let mut width_mm = None;
        let mut widths_mm = None;
        while let Some(field) = map.next_key::<WidthField>()? {
            match field {
                WidthField::Kind => {
                    if kind.is_some() {
                        return Err(serde::de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                WidthField::WidthMm => {
                    if width_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("width_mm"));
                    }
                    width_mm = Some(map.next_value()?);
                }
                WidthField::WidthsMm => {
                    if widths_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("widths_mm"));
                    }
                    widths_mm = Some(map.next_value_seed(WidthsSeed(&mut *self.0))?);
                }
            }
        }
        let profile = match kind.ok_or_else(|| serde::de::Error::missing_field("kind"))? {
            WidthKind::Constant => {
                if widths_mm.is_some() {
                    return Err(serde::de::Error::unknown_field(
                        "widths_mm",
                        &["kind", "width_mm"],
                    ));
                }
                WidthProfile::Constant {
                    width_mm: width_mm
                        .ok_or_else(|| serde::de::Error::missing_field("width_mm"))?,
                }
            }
            WidthKind::PerEdge => {
                if width_mm.is_some() {
                    return Err(serde::de::Error::unknown_field(
                        "width_mm",
                        &["kind", "widths_mm"],
                    ));
                }
                WidthProfile::PerEdge {
                    widths_mm: widths_mm
                        .ok_or_else(|| serde::de::Error::missing_field("widths_mm"))?,
                }
            }
        };
        profile
            .validate_values()
            .map_err(serde::de::Error::custom)?;
        Ok(profile)
    }
}
struct WidthsSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for WidthsSeed<'_> {
    type Value = Vec<f64>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> serde::de::Visitor<'de> for WidthsSeed<'_> {
    type Value = Vec<f64>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("one width per actual edge within the cumulative width budget")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        self.0
            .charge_width_metadata()
            .map_err(serde::de::Error::custom)?;
        let mut widths = Vec::new();
        while let Some(width) = seq.next_element::<f64>()? {
            if !width.is_finite() || width <= 0.0 {
                return Err(serde::de::Error::custom(
                    "deposition widths must be finite and positive",
                ));
            }
            self.0.admit_width().map_err(serde::de::Error::custom)?;
            if widths.len() == widths.capacity() {
                // Admit the current slot plus every remaining width entry, but
                // grow geometrically. The requested capacity is charged before
                // allocation, including any slack it creates.
                let allowed = match widths
                    .len()
                    .checked_add(1)
                    .and_then(|entries| entries.checked_add(self.0.remaining_widths as usize))
                {
                    Some(allowed) => allowed,
                    None => {
                        return Err(serde::de::Error::custom("deposition width limit exceeded"));
                    }
                };
                let headroom = allowed.saturating_sub(widths.capacity()).max(1);
                let additional = widths.capacity().max(4).min(headroom);
                self.0
                    .charge_width_capacity(additional)
                    .map_err(serde::de::Error::custom)?;
                widths.reserve_exact(additional);
            }
            widths.push(width);
        }
        Ok(widths)
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum PathField {
    Kind,
    Owner,
    Width,
    PhysicalZMm,
    MaterialDomain,
    PointsMm,
}
struct PathSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for PathSeed<'_> {
    type Value = DepositionPath;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_struct(
            "DepositionPath",
            &[
                "kind",
                "owner",
                "width",
                "physical_z_mm",
                "material_domain",
                "points_mm",
            ],
            self,
        )
    }
}
impl<'de> serde::de::Visitor<'de> for PathSeed<'_> {
    type Value = DepositionPath;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a deposition path")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut material_domain = None;
        let mut kind = None;
        let mut owner = None;
        let mut width = None;
        let mut physical_z_mm = None;
        let mut points_mm = None;
        while let Some(field) = map.next_key::<PathField>()? {
            match field {
                PathField::MaterialDomain => {
                    if material_domain.is_some() {
                        return Err(serde::de::Error::duplicate_field("material_domain"));
                    }
                    material_domain = Some(map.next_value()?);
                }
                PathField::Kind => {
                    if kind.is_some() {
                        return Err(serde::de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                PathField::Owner => {
                    if owner.is_some() {
                        return Err(serde::de::Error::duplicate_field("owner"));
                    }
                    owner = Some(map.next_value()?);
                }
                PathField::Width => {
                    if width.is_some() {
                        return Err(serde::de::Error::duplicate_field("width"));
                    }
                    width = Some(map.next_value_seed(WidthProfileSeed(&mut *self.0))?);
                }
                PathField::PhysicalZMm => {
                    if physical_z_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("physical_z_mm"));
                    }
                    physical_z_mm = Some(map.next_value()?);
                }
                PathField::PointsMm => {
                    if points_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("points_mm"));
                    }
                    points_mm = Some(map.next_value_seed(PointsSeed(&mut *self.0))?);
                }
            }
        }
        let path = DepositionPath {
            material_domain: material_domain
                .ok_or_else(|| serde::de::Error::missing_field("material_domain"))?,
            kind: kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?,
            owner: owner.ok_or_else(|| serde::de::Error::missing_field("owner"))?,
            width: width.ok_or_else(|| serde::de::Error::missing_field("width"))?,
            physical_z_mm: physical_z_mm
                .ok_or_else(|| serde::de::Error::missing_field("physical_z_mm"))?,
            points_mm: points_mm.ok_or_else(|| serde::de::Error::missing_field("points_mm"))?,
        };
        path.validate_owner().map_err(serde::de::Error::custom)?;
        Ok(path)
    }
}

struct PathsSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for PathsSeed<'_> {
    type Value = Vec<DepositionPath>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> serde::de::Visitor<'de> for PathsSeed<'_> {
    type Value = Vec<DepositionPath>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a bounded array of deposition paths")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut paths = Vec::new();
        while paths.len() < MAX_DEPOSITION_SEGMENTS as usize {
            match seq.next_element_seed(PathSeed(&mut *self.0))? {
                Some(path) => paths.push(path),
                None => return Ok(paths),
            }
        }
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::custom("deposition path limit exceeded"));
        }
        Ok(paths)
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum LayerField {
    Index,
    ZMm,
    LowerZMm,
    HeightMm,
    SectionZMm,
    Paths,
    RegionCoverage,
    SkinCommandFits,
}
struct LayerSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for LayerSeed<'_> {
    type Value = PrintLayer;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_struct(
            "PrintLayer",
            &[
                "index",
                "z_mm",
                "lower_z_mm",
                "height_mm",
                "section_z_mm",
                "paths",
                "region_coverage",
                "skin_command_fits",
            ],
            self,
        )
    }
}
impl<'de> serde::de::Visitor<'de> for LayerSeed<'_> {
    type Value = PrintLayer;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a print layer")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut index = None;
        let mut z_mm = None;
        let mut lower_z_mm = None;
        let mut height_mm = None;
        let mut section_z_mm = None;
        let mut paths = None;
        let mut region_coverage = None;
        let mut skin_command_fits = None;
        while let Some(field) = map.next_key::<LayerField>()? {
            match field {
                LayerField::Index => {
                    if index.is_some() {
                        return Err(serde::de::Error::duplicate_field("index"));
                    }
                    index = Some(map.next_value()?);
                }
                LayerField::ZMm => {
                    if z_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("z_mm"));
                    }
                    z_mm = Some(map.next_value()?);
                }
                LayerField::LowerZMm => {
                    if lower_z_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("lower_z_mm"));
                    }
                    lower_z_mm = Some(map.next_value()?);
                }
                LayerField::HeightMm => {
                    if height_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("height_mm"));
                    }
                    height_mm = Some(map.next_value()?);
                }
                LayerField::SectionZMm => {
                    if section_z_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("section_z_mm"));
                    }
                    section_z_mm = Some(map.next_value()?);
                }
                LayerField::Paths => {
                    if paths.is_some() {
                        return Err(serde::de::Error::duplicate_field("paths"));
                    }
                    paths = Some(map.next_value_seed(PathsSeed(&mut *self.0))?);
                }
                LayerField::RegionCoverage => {
                    if region_coverage.is_some() {
                        return Err(serde::de::Error::duplicate_field("region_coverage"));
                    }
                    region_coverage = Some(map.next_value()?);
                }
                LayerField::SkinCommandFits => {
                    if skin_command_fits.is_some() {
                        return Err(serde::de::Error::duplicate_field("skin_command_fits"));
                    }
                    skin_command_fits = Some(
                        map.next_value_seed(diaphragm::SkinCommandFitDomainsSeed(&mut *self.0))?,
                    );
                }
            }
        }
        Ok(PrintLayer {
            index: index.ok_or_else(|| serde::de::Error::missing_field("index"))?,
            z_mm: z_mm.ok_or_else(|| serde::de::Error::missing_field("z_mm"))?,
            lower_z_mm: lower_z_mm.ok_or_else(|| serde::de::Error::missing_field("lower_z_mm"))?,
            height_mm: height_mm.ok_or_else(|| serde::de::Error::missing_field("height_mm"))?,
            section_z_mm: section_z_mm
                .ok_or_else(|| serde::de::Error::missing_field("section_z_mm"))?,
            paths: paths.ok_or_else(|| serde::de::Error::missing_field("paths"))?,
            region_coverage: region_coverage
                .ok_or_else(|| serde::de::Error::missing_field("region_coverage"))?,
            skin_command_fits: skin_command_fits
                .ok_or_else(|| serde::de::Error::missing_field("skin_command_fits"))?,
        })
    }
}

struct LayersSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for LayersSeed<'_> {
    type Value = Vec<PrintLayer>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> serde::de::Visitor<'de> for LayersSeed<'_> {
    type Value = Vec<PrintLayer>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a bounded array of print layers")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut layers = Vec::new();
        while layers.len() < MAX_PRINT_LAYERS as usize {
            match seq.next_element_seed(LayerSeed(&mut *self.0))? {
                Some(layer) => layers.push(layer),
                None => return Ok(layers),
            }
        }
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::custom("print layer limit exceeded"));
        }
        Ok(layers)
    }
}

fn paths<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<DepositionPath>, D::Error> {
    serde::de::DeserializeSeed::deserialize(PathsSeed(&mut SegmentBudget::new()), d)
}
fn layers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<PrintLayer>, D::Error> {
    serde::de::DeserializeSeed::deserialize(LayersSeed(&mut SegmentBudget::new()), d)
}
fn definitions<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<StoredDefinition>, D::Error> {
    bounded_vec::<D, _, { MAX_DEFINITIONS as usize }>(d)
}
fn occurrences<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<OccurrenceRecord>, D::Error> {
    bounded_vec::<D, _, { MAX_OCCURRENCES as usize }>(d)
}
fn descriptions<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    bounded_vec::<D, _, 64>(d)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrinterCoordinateFrame {
    RightHandedMillimetres,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MotionSpace {
    CartesianXyz,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputDialect {
    CartesianAbsoluteGcodeV1,
    Marlin2SingleExtruderGcodeV1,
    Marlin2FixedDualGcodeV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrinterCapabilities {
    pub motion_space: MotionSpace,
    pub extruders: u32,
    pub output_dialect: OutputDialect,
    pub generated_supports: bool,
}
impl PrinterCapabilities {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let expected = if self.output_dialect == OutputDialect::Marlin2FixedDualGcodeV1 {
            2
        } else {
            1
        };
        if self.extruders != expected {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::UnsupportedCapability,
                "extruder count must match the selected explicit output dialect",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MachineComponentRole {
    Bed,
    Toolhead,
    Fixture,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MachineComponentShape {
    Box { bounds_mm: AabbMm },
    Cylinder { radius_mm: f64, height_mm: f64 },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MachineComponent {
    pub id: String,
    pub role: MachineComponentRole,
    pub pose: RigidPoseMm,
    pub shape: MachineComponentShape,
}
impl MachineComponent {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        label(&self.id)?;
        self.pose.validate().map_err(|e| invalid(&e.message))?;
        match &self.shape {
            MachineComponentShape::Box { bounds_mm } => {
                bounds_mm.validate().map_err(|e| invalid(&e.message))?;
                if (0..3).any(|i| {
                    bounds_mm.min[i] >= bounds_mm.max[i]
                        || !(bounds_mm.max[i] - bounds_mm.min[i]).is_finite()
                }) {
                    return Err(invalid(
                        "component box must have finite positive dimensions",
                    ));
                }
            }
            MachineComponentShape::Cylinder {
                radius_mm,
                height_mm,
            } => positive(&[*radius_mm, *height_mm])?,
        }
        Ok(())
    }
}
/// Declared software deposition envelope; not physical process qualification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DepositionLimits {
    pub min_bead_width_mm: f64,
    pub max_bead_width_mm: f64,
    pub min_bead_height_mm: f64,
    pub max_bead_height_mm: f64,
}
impl DepositionLimits {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        positive(&[
            self.min_bead_width_mm,
            self.max_bead_width_mm,
            self.min_bead_height_mm,
            self.max_bead_height_mm,
        ])?;
        if self.min_bead_width_mm > self.max_bead_width_mm
            || self.min_bead_height_mm > self.max_bead_height_mm
        {
            return Err(invalid("invalid deposition limits"));
        }
        Ok(())
    }
    pub fn admits(&self, width: f64, height: f64) -> bool {
        width.is_finite()
            && height.is_finite()
            && height <= width
            && width >= self.min_bead_width_mm
            && width <= self.max_bead_width_mm
            && height >= self.min_bead_height_mm
            && height <= self.max_bead_height_mm
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrinterSpecification {
    pub schema_version: u32,
    pub id: String,
    pub revision: String,
    pub name: String,
    pub coordinate_frame: PrinterCoordinateFrame,
    pub build_envelope: AabbMm,
    #[serde(deserialize_with = "components")]
    pub components: Vec<MachineComponent>,
    pub capabilities: PrinterCapabilities,
    pub workflow: JobWorkflow,
    pub nozzle_diameter_mm: f64,
    pub deposition_limits: DepositionLimits,
    pub max_feed_mm_s: f64,
    pub max_volumetric_flow_mm3_s: f64,
}
impl PrinterSpecification {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.schema_version != MANUFACTURING_SCHEMA_VERSION {
            return Err(invalid("unsupported printer schema"));
        }
        label(&self.id)?;
        label(&self.revision)?;
        label(&self.name)?;
        self.build_envelope
            .validate()
            .map_err(|e| invalid(&e.message))?;
        if (0..3).any(|i| {
            self.build_envelope.min[i] >= self.build_envelope.max[i]
                || !(self.build_envelope.max[i] - self.build_envelope.min[i]).is_finite()
        }) {
            return Err(invalid(
                "build envelope must have finite positive dimensions",
            ));
        }
        positive(&[
            self.nozzle_diameter_mm,
            self.max_feed_mm_s,
            self.max_volumetric_flow_mm3_s,
        ])?;
        self.deposition_limits.validate()?;
        self.capabilities.validate()?;
        match (&self.workflow, self.capabilities.output_dialect) {
            (JobWorkflow::DiagnosticBodyOnly, OutputDialect::CartesianAbsoluteGcodeV1) => {}
            (JobWorkflow::CompleteMarlin { job }, OutputDialect::Marlin2SingleExtruderGcodeV1) => {
                job.validate(self)?
            }
            (JobWorkflow::FixedDualMarlin { job }, OutputDialect::Marlin2FixedDualGcodeV1) => {
                job.validate(self)?
            }
            _ => return Err(invalid("workflow and output dialect are incompatible")),
        }
        if self.components.len() > MAX_MACHINE_COMPONENTS as usize {
            return Err(resource("machine component limit exceeded"));
        }
        let mut ids = BTreeSet::new();
        for component in &self.components {
            component.validate()?;
            if !ids.insert(&component.id) {
                return Err(invalid("duplicate machine component identity"));
            }
        }
        json_bytes(self, MAX_PROFILE_JSON_BYTES as usize)?;
        Ok(())
    }
}
/// Closed-path start selection in the printer's build/machine XY frame.
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeamPolicy {
    Nearest,
    PreferredBuildDirection { xy: [f64; 2] },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RawSeamPolicy {
    Nearest {},
    PreferredBuildDirection { xy: [f64; 2] },
}
impl<'de> Deserialize<'de> for SeamPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = match RawSeamPolicy::deserialize(deserializer)? {
            RawSeamPolicy::Nearest {} => Self::Nearest,
            RawSeamPolicy::PreferredBuildDirection { xy } => Self::PreferredBuildDirection { xy },
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl SeamPolicy {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if let Self::PreferredBuildDirection { xy } = self {
            let norm = xy[0].hypot(xy[1]);
            if !xy.iter().all(|value| value.is_finite())
                || !norm.is_finite()
                || (norm - 1.0).abs() > crate::geometry::POSE_NORM_TOLERANCE
            {
                return Err(invalid(
                    "preferred seam direction requires a finite unit build/machine XY vector",
                ));
            }
        }
        Ok(())
    }
}
/// Rectilinear deposition axes in the printer's build/machine XY frame.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RectilinearAxisPolicy {
    X,
    Y,
    #[serde(rename = "alternating_xy")]
    AlternatingXY,
    #[serde(rename = "alternating_yx")]
    AlternatingYX,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SparseInfillPolicy {
    Fine,
    CombineEligible { max_height_mm: f64 },
}
impl SparseInfillPolicy {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if let Self::CombineEligible { max_height_mm } = self {
            positive(&[*max_height_mm])?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanarPrintRecipe {
    pub id: String,
    pub revision: String,
    pub material_id: String,
    pub material_revision: String,
    pub filament_diameter_mm: f64,
    pub layer_height_mm: f64,
    pub native_shape_error_mm: f64,
    pub bead_width_mm: f64,
    pub sparse_infill: SparseInfillPolicy,
    pub perimeter_count: u32,
    pub infill_fraction: f64,
    pub top_skin_thickness_mm: f64,
    pub bottom_skin_thickness_mm: f64,
    pub print_speed_mm_s: f64,
    pub travel_speed_mm_s: f64,
    pub flow_multiplier: f64,
    pub process: ProcessPolicy,
    pub supports: SupportPolicy,
    pub seam_policy: SeamPolicy,
    pub rectilinear_axis: RectilinearAxisPolicy,
    pub top_skin_axis: RectilinearAxisPolicy,
}
impl PlanarPrintRecipe {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        label(&self.id)?;
        label(&self.revision)?;
        label(&self.material_id)?;
        label(&self.material_revision)?;
        self.process.validate()?;
        self.supports.validate()?;
        self.seam_policy.validate()?;
        self.sparse_infill.validate()?;
        positive(&[
            self.filament_diameter_mm,
            self.layer_height_mm,
            self.native_shape_error_mm,
            self.bead_width_mm,
            self.print_speed_mm_s,
            self.travel_speed_mm_s,
            self.flow_multiplier,
            self.top_skin_thickness_mm,
            self.bottom_skin_thickness_mm,
        ])?;
        if !(1..=8).contains(&self.perimeter_count) || self.layer_height_mm > self.bead_width_mm {
            return Err(invalid(
                "recipe requires 1..8 perimeters and layer height <= bead width",
            ));
        }
        if !self.infill_fraction.is_finite()
            || self.infill_fraction <= 0.0
            || self.infill_fraction > 1.0
        {
            return Err(invalid("infill fraction must be finite and in (0,1]"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingIntent {
    pub printer: PrinterSpecification,
    pub recipe: PlanarPrintRecipe,
}
impl ManufacturingIntent {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        self.printer.validate()?;
        self.recipe.validate()?;
        let p = &self.printer;
        let r = &self.recipe;
        r.process.validate_printer(p)?;
        if !p
            .deposition_limits
            .admits(r.bead_width_mm, r.layer_height_mm)
        {
            return Err(invalid(
                "nominal recipe deposition exceeds printer envelope",
            ));
        }
        if !matches!(r.supports, SupportPolicy::Disabled) && !p.capabilities.generated_supports {
            return Err(invalid(
                "breakaway supports require declared generated_supports capability",
            ));
        }
        let area = ((r.bead_width_mm - r.layer_height_mm) * r.layer_height_mm
            + std::f64::consts::PI * (r.layer_height_mm / 2.0).powi(2))
            * r.flow_multiplier;
        let filament_area = std::f64::consts::PI * (r.filament_diameter_mm / 2.0).powi(2);
        let flow = r.print_speed_mm_s * area;
        let extrusion_per_mm = area / filament_area;
        if !area.is_finite()
            || area <= 0.0
            || !filament_area.is_finite()
            || filament_area <= 0.0
            || !flow.is_finite()
            || flow <= 0.0
            || flow > p.max_volumetric_flow_mm3_s
            || !extrusion_per_mm.is_finite()
            || extrusion_per_mm <= 0.0
            || r.print_speed_mm_s > p.max_feed_mm_s
            || r.travel_speed_mm_s > p.max_feed_mm_s
            || !(r.travel_speed_mm_s * 60.0).is_finite()
            || r.travel_speed_mm_s * 60.0 < 0.000001
            || (r.travel_speed_mm_s * 60_000_000.0).round() / 1_000_000.0 / 60.0 > p.max_feed_mm_s
        {
            return Err(invalid(
                "recipe exceeds printer feed/rounded-bead flow limits or numeric range",
            ));
        }
        for (speed_factor, flow_factor) in [
            (1.0, 1.0),
            (
                r.process.first_layer_speed_factor,
                r.process.first_layer_flow_factor,
            ),
            (r.process.bridge_speed_factor, r.process.bridge_flow_factor),
            (
                r.process.first_layer_speed_factor * r.process.bridge_speed_factor,
                r.process.first_layer_flow_factor * r.process.bridge_flow_factor,
            ),
        ] {
            let process_flow = flow * speed_factor * flow_factor;
            let feed = r.print_speed_mm_s * speed_factor * 60.0;
            let process_extrusion = extrusion_per_mm * flow_factor;
            if !feed.is_finite()
                || feed < 0.000001
                || (feed * 1_000_000.0).round() / 1_000_000.0 / 60.0 > p.max_feed_mm_s
                || !process_extrusion.is_finite()
                || process_extrusion <= 0.0
                || !process_flow.is_finite()
                || process_flow <= 0.0
                || process_flow > p.max_volumetric_flow_mm3_s
            {
                return Err(invalid(
                    "process override exceeds printer feed/flow or serialized precision",
                ));
            }
        }
        if let JobWorkflow::CompleteMarlin { job } = &p.workflow {
            let length = (job.prime_end_mm[0] - job.prime_start_mm[0])
                .hypot(job.prime_end_mm[1] - job.prime_start_mm[1])
                .hypot(job.prime_end_mm[2] - job.prime_start_mm[2]);
            let prime_flow =
                job.prime_filament_length_mm * filament_area * job.prime_speed_mm_s / length;
            let prime_feed = (job.prime_speed_mm_s * 60_000_000.0).round() / 1_000_000.0;
            if !prime_feed.is_finite() || prime_feed <= 0.0 || prime_feed / 60.0 > p.max_feed_mm_s {
                return Err(invalid(
                    "priming feed exceeds printer limit at serialized precision",
                ));
            }
            if !prime_flow.is_finite() || prime_flow > p.max_volumetric_flow_mm3_s {
                return Err(invalid("priming exceeds declared volumetric flow limit"));
            }
        }
        if let JobWorkflow::FixedDualMarlin { job } = &p.workflow {
            if !matches!(r.supports, SupportPolicy::Disabled)
                || !matches!(r.sparse_infill, SparseInfillPolicy::Fine)
                || !matches!(r.process.retraction, RetractionPolicy::Disabled)
            {
                return Err(invalid(
                    "fixed-dual routing requires fine model deposition without generated supports",
                ));
            }
            for tool in &job.tools {
                let tool_filament_area =
                    std::f64::consts::PI * (tool.filament_diameter_mm * 0.5).powi(2);
                let waste_length = (tool.waste_line_xy_mm[1][0] - tool.waste_line_xy_mm[0][0])
                    .hypot(tool.waste_line_xy_mm[1][1] - tool.waste_line_xy_mm[0][1]);
                if !tool_filament_area.is_finite()
                    || tool_filament_area <= 0.0
                    || !waste_length.is_finite()
                    || waste_length <= 0.0
                    || !tool
                        .deposition_limits
                        .admits(r.bead_width_mm, r.layer_height_mm)
                    || area * waste_length < tool.minimum_reprime_volume_mm3
                {
                    return Err(invalid(
                        "fixed-dual waste or loaded-tool recipe envelope is invalid",
                    ));
                }
            }
        }
        json_bytes(self, MAX_PROFILE_JSON_BYTES as usize)?;
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DepositionKind {
    Perimeter,
    SolidFill,
    BottomSkin,
    TopSkin,
    SparseFill,
    Bridge,
    Support,
    SupportInterface,
    SeparatorWall,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DepositionOwner {
    MandatorySkin {
        skin: ProcessSkinKind,
    },
    Regional {
        region_index: u16,
    },
    /// Immutable original construction wall. Actual multi-boundary/site/depth
    /// correspondence remains internal to WallPlan, not persisted here.
    Wall,
    /// One straight continuous Bridge. Its middle path point is the sole
    /// Regional/MandatorySkin transition; no other ownership crossing is admitted.
    RegionalSkinBridge {
        region_index: u16,
        skin: ProcessSkinKind,
        skin_first: bool,
    },
}
impl DepositionOwner {
    pub const fn skin_kind(self) -> Option<ProcessSkinKind> {
        match self {
            Self::MandatorySkin { skin } => Some(skin),
            Self::Regional { .. } | Self::Wall | Self::RegionalSkinBridge { .. } => None,
        }
    }
}

/// Conservative central-rectangle width of the nominal stadium bead. This
/// full-height inner core is a software model, not measured adhesion strength.
pub fn nominal_stadium_core_width_mm(width: f64, height: f64) -> Option<f64> {
    if !width.is_finite() || !height.is_finite() || height <= 0.0 || width <= height {
        return None;
    }
    let core = (width - height).next_down();
    (core.is_finite() && core > 0.0).then_some(core)
}
/// Nominal bead width for every actual edge of a path. Per-edge values are
/// ordered with the path's actual edges, including the explicit closing edge.
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WidthProfile {
    Constant { width_mm: f64 },
    PerEdge { widths_mm: Vec<f64> },
}
impl<'de> Deserialize<'de> for WidthProfile {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::DeserializeSeed;
        WidthProfileSeed(&mut SegmentBudget::new()).deserialize(d)
    }
}
impl WidthProfile {
    /// Uniform nominal width only; a varying profile has no single width and
    /// never reports its first varying edge.
    pub fn constant_width_mm(&self) -> Option<f64> {
        match self {
            Self::Constant { width_mm } => Some(*width_mm),
            Self::PerEdge { .. } => None,
        }
    }
    fn validate_values(&self) -> Result<(), ManufacturingError> {
        match self {
            Self::Constant { width_mm } => positive(std::slice::from_ref(width_mm)),
            Self::PerEdge { widths_mm } => {
                if widths_mm.len() > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("deposition width allocation limit exceeded"));
                }
                if widths_mm
                    .iter()
                    .any(|width| !width.is_finite() || *width <= 0.0)
                {
                    return Err(invalid("deposition widths must be finite and positive"));
                }
                Ok(())
            }
        }
    }
    fn validate_for(&self, points_len: usize, height_mm: f64) -> Result<(), ManufacturingError> {
        positive(&[height_mm])?;
        if points_len < 2 {
            return Err(invalid("deposition path requires at least two points"));
        }
        match self {
            Self::Constant { width_mm } => {
                if !width_mm.is_finite() || *width_mm <= 0.0 || height_mm > *width_mm {
                    return Err(invalid("path requires finite positive bead dimensions"));
                }
            }
            Self::PerEdge { widths_mm } => {
                if widths_mm.len() > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("deposition width allocation limit exceeded"));
                }
                if widths_mm.len() != points_len - 1 {
                    return Err(invalid("path requires exactly one width per actual edge"));
                }
                if widths_mm
                    .iter()
                    .any(|width| !width.is_finite() || *width <= 0.0 || height_mm > *width)
                {
                    return Err(invalid("path requires finite positive bead dimensions"));
                }
            }
        }
        Ok(())
    }
}
/// One actual path edge. `points_mm` always contains exactly its two endpoints.
#[derive(Debug, Clone, Copy)]
pub struct DepositionEdge<'a> {
    pub points_mm: &'a [[f64; 3]],
    pub width_mm: f64,
}
#[derive(Debug, Clone, Copy)]
enum EdgeWidths<'a> {
    Constant(f64),
    PerEdge(&'a [f64]),
}
/// Validated, allocation-free view over a path's actual edges. Mutation of the
/// source path invalidates this view.
#[derive(Debug, Clone, Copy)]
pub struct DepositionEdges<'a> {
    points_mm: &'a [[f64; 3]],
    widths: EdgeWidths<'a>,
    front: usize,
    back: usize,
    len: usize,
}
impl<'a> DepositionEdges<'a> {
    /// Access an edge by its absolute index in the original path, regardless
    /// of the current iterator cursor. Returns `None` past the actual edges.
    pub fn edge(&self, index: usize) -> Option<DepositionEdge<'a>> {
        if index >= self.len {
            return None;
        }
        let width_mm = match self.widths {
            EdgeWidths::Constant(width_mm) => width_mm,
            EdgeWidths::PerEdge(widths_mm) => *widths_mm.get(index)?,
        };
        Some(DepositionEdge {
            points_mm: self.points_mm.get(index..index + 2)?,
            width_mm,
        })
    }
}
impl<'a> Iterator for DepositionEdges<'a> {
    type Item = DepositionEdge<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        let index = self.front;
        self.front += 1;
        self.edge(index)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.back - self.front;
        (remaining, Some(remaining))
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        let Some(index) = self.front.checked_add(n) else {
            self.front = self.back;
            return None;
        };
        if index >= self.back {
            self.front = self.back;
            return None;
        }
        self.front = index + 1;
        self.edge(index)
    }
}
impl<'a> DoubleEndedIterator for DepositionEdges<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        self.back -= 1;
        self.edge(self.back)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let Some(offset) = n.checked_add(1) else {
            self.back = self.front;
            return None;
        };
        let Some(index) = self.back.checked_sub(offset) else {
            self.back = self.front;
            return None;
        };
        if index < self.front {
            self.back = self.front;
            return None;
        }
        self.back = index;
        self.edge(index)
    }
}
impl ExactSizeIterator for DepositionEdges<'_> {}
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DepositionPath {
    pub material_domain: Option<MaterialDomainRef>,
    pub kind: DepositionKind,
    /// Immutable original construction purpose, never inferred from Bridge shape.
    pub owner: Option<DepositionOwner>,
    pub width: WidthProfile,
    /// Explicit occupied interval; upper endpoint belongs to the owning layer.
    pub physical_z_mm: [f64; 2],
    pub points_mm: Vec<[f64; 3]>,
}
impl<'de> Deserialize<'de> for DepositionPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::DeserializeSeed;
        PathSeed(&mut SegmentBudget::new()).deserialize(d)
    }
}
impl DepositionPath {
    pub fn height_mm(&self) -> f64 {
        self.physical_z_mm[1] - self.physical_z_mm[0]
    }
    /// Validate cardinality and dimensions once, then borrow the actual edges.
    /// Constant-width paths iterate without cloning or allocating.
    pub fn checked_edges(&self) -> Result<DepositionEdges<'_>, ManufacturingError> {
        if !self.physical_z_mm.iter().all(|z| z.is_finite()) {
            return Err(invalid("path requires finite positive bead dimensions"));
        }
        self.width
            .validate_for(self.points_mm.len(), self.height_mm())?;
        let widths = match &self.width {
            WidthProfile::Constant { width_mm } => EdgeWidths::Constant(*width_mm),
            WidthProfile::PerEdge { widths_mm } => EdgeWidths::PerEdge(widths_mm),
        };
        let len = self.points_mm.len() - 1;
        Ok(DepositionEdges {
            points_mm: &self.points_mm,
            widths,
            front: 0,
            back: len,
            len,
        })
    }
    /// Reverse point order and the per-edge widths that follow those points.
    /// A mixed Regional/Mandatory skin bridge toggles its actual first portion.
    pub fn reverse(&mut self) {
        self.points_mm.reverse();
        if let WidthProfile::PerEdge { widths_mm } = &mut self.width {
            widths_mm.reverse();
        }
        if let Some(DepositionOwner::RegionalSkinBridge { skin_first, .. }) = &mut self.owner {
            *skin_first = !*skin_first;
        }
    }
    /// Rotate a closed path so `seam_index` names its first actual edge. The
    /// explicit closing duplicate and each edge's width follow the rotation.
    pub fn rotate_closed(&mut self, seam_index: usize) -> Result<(), ManufacturingError> {
        let vertices = self.points_mm.len().saturating_sub(1);
        if vertices < 2 || self.points_mm.first() != self.points_mm.last() {
            return Err(invalid(
                "closed rotation requires an explicit closing duplicate",
            ));
        }
        if seam_index >= vertices {
            return Err(invalid("closed rotation seam is outside its actual edges"));
        }
        self.checked_edges()?;
        if seam_index == 0 {
            return Ok(());
        }
        self.points_mm[..vertices].rotate_left(seam_index);
        let closing = self.points_mm[0];
        self.points_mm[vertices] = closing;
        if let WidthProfile::PerEdge { widths_mm } = &mut self.width {
            widths_mm.rotate_left(seam_index);
        }
        Ok(())
    }
    fn validate_owner(&self) -> Result<(), ManufacturingError> {
        if !self.physical_z_mm.iter().all(|z| z.is_finite()) {
            return Err(invalid("path requires finite positive bead dimensions"));
        }
        self.width
            .validate_for(self.points_mm.len(), self.height_mm())?;
        if self.kind == DepositionKind::Bridge
            && !matches!(self.owner, Some(DepositionOwner::RegionalSkinBridge { .. }))
            && self.owner != Some(DepositionOwner::Wall)
            && self.points_mm.first() != self.points_mm.last()
            && self.points_mm.len() != 2
        {
            return Err(invalid(
                "ordinary open Bridge rows require two points; a transition requires explicit mixed ownership",
            ));
        }
        let valid = match self.kind {
            DepositionKind::BottomSkin => {
                self.owner
                    == Some(DepositionOwner::MandatorySkin {
                        skin: ProcessSkinKind::BottomSkin,
                    })
            }
            DepositionKind::TopSkin => {
                self.owner
                    == Some(DepositionOwner::MandatorySkin {
                        skin: ProcessSkinKind::TopSkin,
                    })
            }
            // Every Perimeter is a construction wall. A wall may be relabelled
            // to Bridge during construction while its immutable ownership stays.
            DepositionKind::Bridge => true,
            DepositionKind::Perimeter => self.owner == Some(DepositionOwner::Wall),
            DepositionKind::SolidFill | DepositionKind::SparseFill => {
                self.owner.is_none() || matches!(self.owner, Some(DepositionOwner::Regional { .. }))
            }
            _ => self.owner.is_none(),
        };
        if !valid {
            return Err(invalid(
                "deposition role disagrees with immutable construction ownership",
            ));
        }
        if let Some(DepositionOwner::RegionalSkinBridge { .. }) = self.owner {
            if self.kind != DepositionKind::Bridge || self.points_mm.len() != 3 {
                return Err(invalid(
                    "mixed skin bridge requires one three-point continuous Bridge",
                ));
            }
            let points = [
                program_coordinates::project_xyz6_point_mm(self.points_mm[0])?,
                program_coordinates::project_xyz6_point_mm(self.points_mm[1])?,
                program_coordinates::project_xyz6_point_mm(self.points_mm[2])?,
            ];
            let along = if points.iter().all(|p| p[0] == points[0][0]) {
                1
            } else if points.iter().all(|p| p[1] == points[0][1]) {
                0
            } else {
                return Err(invalid(
                    "mixed skin bridge must be one straight emitted row",
                ));
            };
            if !(points[0][along].min(points[2][along]) < points[1][along]
                && points[1][along] < points[0][along].max(points[2][along]))
            {
                return Err(invalid(
                    "mixed skin bridge transition must be strictly interior at XYZ6",
                ));
            }
        }
        Ok(())
    }
}
pub const MAX_PROCESS_REGIONS: usize = 256;
pub const MAX_PROCESS_REGION_CONTRIBUTIONS: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessRegionContribution {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub provenance: Option<crate::process::ProcessProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessRegionCoverage {
    #[serde(deserialize_with = "required_option")]
    pub material_domain: Option<MaterialDomainRef>,
    pub area_mm2: f64,
    pub fraction: f64,
    #[serde(deserialize_with = "process_contributions")]
    pub contributions: Vec<ProcessRegionContribution>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRegionStatus {
    Applied,
    NoEligibleInterior,
    NoDensityChange,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessRegionResolution {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub provenance: crate::process::ProcessProvenance,
    pub status: ProcessRegionStatus,
}
fn process_resolution<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ProcessRegionResolution>, D::Error> {
    bounded_vec::<D, _, MAX_PROCESS_REGIONS>(d)
}
fn validate_process_resolution(
    records: &[ProcessRegionResolution],
) -> Result<(), ManufacturingError> {
    if records.len() > MAX_PROCESS_REGIONS {
        return Err(resource("process resolution limit exceeded"));
    }
    let mut identities = BTreeSet::new();
    for record in records {
        if !identities.insert((
            record.occurrence_id,
            record.provenance.layer_id,
            record.provenance.operation_id,
        )) {
            return Err(invalid("duplicate process region resolution"));
        }
    }
    Ok(())
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessSkinKind {
    BottomSkin,
    TopSkin,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessEndpointBond {
    Wall {
        wall_segment_mm: [[f64; 2]; 2],
    },
    Skin {
        skin_kind: ProcessSkinKind,
        process_kind: DepositionKind,
        skin_segment_mm: [[f64; 2]; 2],
        wall_segment_mm: [[f64; 2]; 2],
    },
}

/// Reserved separator material and its actual build-frame XY deposition.
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessInterfaceCoverage {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub provenance: crate::process::ProcessProvenance,
    pub region_indices: [u16; 2],
    pub axis: u8,
    pub coordinate_mm: f64,
    pub thickness_mm: f64,
    pub bonding_overlap_mm: f64,
    pub area_mm2: f64,
    pub centerlines_mm: Vec<[[f64; 2]; 2]>,
    pub endpoint_bonds: Vec<[ProcessEndpointBond; 2]>,
}
#[derive(Deserialize)]
#[serde(remote = "ProcessInterfaceCoverage", deny_unknown_fields)]
struct RawProcessInterfaceCoverage {
    occurrence_id: OccurrenceId,
    definition_id: DefinitionId,
    provenance: crate::process::ProcessProvenance,
    region_indices: [u16; 2],
    axis: u8,
    coordinate_mm: f64,
    thickness_mm: f64,
    bonding_overlap_mm: f64,
    area_mm2: f64,
    #[serde(deserialize_with = "interface_centerlines")]
    centerlines_mm: Vec<[[f64; 2]; 2]>,
    #[serde(deserialize_with = "interface_bonds")]
    endpoint_bonds: Vec<[ProcessEndpointBond; 2]>,
}
fn interface_bonds<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<[ProcessEndpointBond; 2]>, D::Error> {
    bounded_vec::<D, _, { MAX_DEPOSITION_SEGMENTS as usize }>(d)
}
fn interface_centerlines<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<[[f64; 2]; 2]>, D::Error> {
    struct Centerlines;
    impl<'de> serde::de::Visitor<'de> for Centerlines {
        type Value = Vec<[[f64; 2]; 2]>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 100000 separator centerlines")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut lines = Vec::new();
            let limit = MAX_DEPOSITION_SEGMENTS as usize;
            while lines.len() < limit {
                let Some(line) = seq.next_element()? else {
                    return Ok(lines);
                };
                if lines.len() == lines.capacity() {
                    lines.reserve_exact(lines.capacity().max(4).min(limit - lines.len()));
                }
                lines.push(line);
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom(
                    "separator centerline limit exceeded",
                ));
            }
            Ok(lines)
        }
    }
    d.deserialize_seq(Centerlines)
}
impl<'de> Deserialize<'de> for ProcessInterfaceCoverage {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = RawProcessInterfaceCoverage::deserialize(d)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl ProcessInterfaceCoverage {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        positive(&[self.thickness_mm, self.bonding_overlap_mm, self.area_mm2])?;
        if self.axis > 1
            || !self.coordinate_mm.is_finite()
            || self.bonding_overlap_mm != self.thickness_mm / 2.0
            || self.centerlines_mm.is_empty()
        {
            return Err(invalid("invalid separator interface dimensions or axis"));
        }
        if self.centerlines_mm.capacity() > MAX_DEPOSITION_SEGMENTS as usize {
            return Err(resource("separator centerline allocation limit exceeded"));
        }
        if self.endpoint_bonds.len() != self.centerlines_mm.len()
            || self.endpoint_bonds.capacity() > MAX_DEPOSITION_SEGMENTS as usize
        {
            return Err(invalid(
                "separator endpoints require one bounded actual bond witness each",
            ));
        }
        for bond in self.endpoint_bonds.iter().flatten() {
            let (wall, skin) = match bond {
                ProcessEndpointBond::Wall { wall_segment_mm } => (wall_segment_mm, None),
                ProcessEndpointBond::Skin {
                    skin_kind,
                    process_kind,
                    skin_segment_mm,
                    wall_segment_mm,
                } => {
                    let original = match skin_kind {
                        ProcessSkinKind::BottomSkin => DepositionKind::BottomSkin,
                        ProcessSkinKind::TopSkin => DepositionKind::TopSkin,
                    };
                    if *process_kind != original && *process_kind != DepositionKind::Bridge {
                        return Err(invalid(
                            "skin bond requires its original skin role or actual Bridge",
                        ));
                    }
                    (wall_segment_mm, Some(skin_segment_mm))
                }
            };
            for line in std::iter::once(wall).chain(skin) {
                if line[0] == line[1] || line.iter().flatten().any(|v| !v.is_finite()) {
                    return Err(invalid(
                        "separator bond witness has invalid actual stroke coordinates",
                    ));
                }
            }
        }
        let axis = self.axis as usize;
        for line in &self.centerlines_mm {
            if line.iter().flatten().any(|v| !v.is_finite())
                || line[0] == line[1]
                || line[0][axis] != line[1][axis]
                || line[0][axis] != self.coordinate_mm
                || !(line[0][1 - axis] - line[1][1 - axis]).is_finite()
            {
                return Err(invalid("invalid separator interface centerline"));
            }
        }
        Ok(())
    }
}
fn process_interfaces<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ProcessInterfaceCoverage>, D::Error> {
    struct Interfaces;
    impl<'de> serde::de::Visitor<'de> for Interfaces {
        type Value = Vec<ProcessInterfaceCoverage>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 256 interfaces and 100000 separator centerlines")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut interfaces = Vec::new();
            let mut count = 0usize;
            while interfaces.len() < MAX_PROCESS_REGIONS {
                let Some(interface) = seq.next_element::<ProcessInterfaceCoverage>()? else {
                    return Ok(interfaces);
                };
                count += interface.centerlines_mm.len();
                if count > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(serde::de::Error::custom(
                        "separator centerline limit exceeded",
                    ));
                }
                interfaces.reserve_exact(1);
                interfaces.push(interface);
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom(
                    "separator interface limit exceeded",
                ));
            }
            Ok(interfaces)
        }
    }
    d.deserialize_seq(Interfaces)
}

struct ProcessContributionsSeed<'a>(&'a mut usize);
impl<'de> serde::de::DeserializeSeed<'de> for ProcessContributionsSeed<'_> {
    type Value = Vec<ProcessRegionContribution>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> serde::de::Visitor<'de> for ProcessContributionsSeed<'_> {
    type Value = Vec<ProcessRegionContribution>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("process contributions within the cumulative coverage budget")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut contributions = Vec::new();
        while *self.0 > 0 {
            match seq.next_element()? {
                Some(contribution) => {
                    *self.0 -= 1;
                    contributions.reserve_exact(1);
                    contributions.push(contribution);
                }
                None => return Ok(contributions),
            }
        }
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::custom(
                "process contribution limit exceeded",
            ));
        }
        Ok(contributions)
    }
}
fn process_contributions<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ProcessRegionContribution>, D::Error> {
    let mut remaining = MAX_PROCESS_REGION_CONTRIBUTIONS;
    serde::de::DeserializeSeed::deserialize(ProcessContributionsSeed(&mut remaining), d)
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum ProcessRegionField {
    MaterialDomain,
    AreaMm2,
    Fraction,
    Contributions,
}
struct ProcessRegionSeed<'a>(&'a mut usize);
impl<'de> serde::de::DeserializeSeed<'de> for ProcessRegionSeed<'_> {
    type Value = ProcessRegionCoverage;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_struct(
            "ProcessRegionCoverage",
            &["material_domain", "area_mm2", "fraction", "contributions"],
            self,
        )
    }
}
impl<'de> serde::de::Visitor<'de> for ProcessRegionSeed<'_> {
    type Value = ProcessRegionCoverage;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a process region with bounded contributions")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut material_domain = None;
        let mut area_mm2 = None;
        let mut fraction = None;
        let mut contributions = None;
        while let Some(field) = map.next_key::<ProcessRegionField>()? {
            match field {
                ProcessRegionField::MaterialDomain => {
                    if material_domain.is_some() {
                        return Err(serde::de::Error::duplicate_field("material_domain"));
                    }
                    material_domain = Some(map.next_value()?);
                }
                ProcessRegionField::AreaMm2 => {
                    if area_mm2.is_some() {
                        return Err(serde::de::Error::duplicate_field("area_mm2"));
                    }
                    area_mm2 = Some(map.next_value()?);
                }
                ProcessRegionField::Fraction => {
                    if fraction.is_some() {
                        return Err(serde::de::Error::duplicate_field("fraction"));
                    }
                    fraction = Some(map.next_value()?);
                }
                ProcessRegionField::Contributions => {
                    if contributions.is_some() {
                        return Err(serde::de::Error::duplicate_field("contributions"));
                    }
                    contributions =
                        Some(map.next_value_seed(ProcessContributionsSeed(&mut *self.0))?);
                }
            }
        }
        Ok(ProcessRegionCoverage {
            material_domain: material_domain
                .ok_or_else(|| serde::de::Error::missing_field("material_domain"))?,
            area_mm2: area_mm2.ok_or_else(|| serde::de::Error::missing_field("area_mm2"))?,
            fraction: fraction.ok_or_else(|| serde::de::Error::missing_field("fraction"))?,
            contributions: contributions
                .ok_or_else(|| serde::de::Error::missing_field("contributions"))?,
        })
    }
}
fn process_regions<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ProcessRegionCoverage>, D::Error> {
    struct Regions;
    impl<'de> serde::de::Visitor<'de> for Regions {
        type Value = Vec<ProcessRegionCoverage>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 256 process regions and 256 total contributions")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut regions = Vec::new();
            let mut remaining = MAX_PROCESS_REGION_CONTRIBUTIONS;
            while regions.len() < MAX_PROCESS_REGIONS {
                match seq.next_element_seed(ProcessRegionSeed(&mut remaining))? {
                    Some(region) => regions.push(region),
                    None => return Ok(regions),
                }
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("process region limit exceeded"));
            }
            Ok(regions)
        }
    }
    d.deserialize_seq(Regions)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanarRegionCoverage {
    pub section_area_mm2: f64,
    pub source_section_area_mm2: f64,
    pub surface_added_area_mm2: f64,
    pub wall_area_mm2: f64,
    pub bottom_skin_area_mm2: f64,
    pub top_skin_area_mm2: f64,
    pub solid_interior_area_mm2: f64,
    pub sparse_interior_area_mm2: f64,
    pub intentional_void_area_mm2: f64,
    pub protected_hole_area_mm2: f64,
    pub nominal_deposition_volume_mm3: f64,
    pub support_area_mm2: f64,
    pub support_nominal_deposition_volume_mm3: f64,
    #[serde(deserialize_with = "process_regions")]
    pub process_regions: Vec<ProcessRegionCoverage>,
    #[serde(deserialize_with = "process_interfaces")]
    pub process_interfaces: Vec<ProcessInterfaceCoverage>,
    pub separator_area_mm2: f64,
    #[serde(deserialize_with = "diaphragm::process_diaphragms")]
    pub process_diaphragms: Vec<ProcessDiaphragmCoverage>,
    pub diaphragm_area_mm2: f64,
    #[serde(deserialize_with = "diaphragm::skin_bridge_domains")]
    pub skin_bridge_domains: Vec<SkinBridgeDomain>,
}
impl PlanarRegionCoverage {
    pub fn validate(&self, recipe: &PlanarPrintRecipe) -> Result<(), ManufacturingError> {
        if self.skin_bridge_domains.len() > MAX_PROCESS_REGIONS {
            return Err(resource("mixed skin bridge domain limit exceeded"));
        }
        for (index, domain) in self.skin_bridge_domains.iter().enumerate() {
            domain.validate()?;
            if usize::from(domain.region_index) >= self.process_regions.len()
                || self.skin_bridge_domains[..index].iter().any(|other| {
                    other.region_index == domain.region_index && other.skin == domain.skin
                })
            {
                return Err(invalid(
                    "mixed skin bridge domain has missing or repeated regional ownership",
                ));
            }
        }
        if self.process_regions.len() > MAX_PROCESS_REGIONS
            || self.process_regions.capacity() > MAX_PROCESS_REGIONS
        {
            return Err(resource("process region allocation limit exceeded"));
        }
        if self.process_interfaces.capacity() > MAX_PROCESS_REGIONS {
            return Err(resource("separator interface allocation limit exceeded"));
        }
        let mut separator = 0.0;
        let mut centerline_capacity = 0usize;
        for interface in &self.process_interfaces {
            interface.validate()?;
            if interface.thickness_mm != recipe.bead_width_mm {
                return Err(invalid(
                    "separator thickness must equal the declared bead width",
                ));
            }
            if interface.region_indices[0] == interface.region_indices[1] {
                return Err(invalid("separator repeats its regional owner"));
            }
            for index in interface.region_indices {
                let region = self
                    .process_regions
                    .get(usize::from(index))
                    .ok_or_else(|| invalid("separator regional owner is outside layer coverage"))?;
                if !region.contributions.iter().any(|c| {
                    c.occurrence_id == interface.occurrence_id
                        && c.definition_id == interface.definition_id
                }) {
                    return Err(invalid(
                        "separator regional owner belongs to another occurrence",
                    ));
                }
            }
            separator += interface.area_mm2;
            centerline_capacity = centerline_capacity
                .checked_add(interface.centerlines_mm.capacity())
                .ok_or_else(|| resource("separator centerline allocation overflow"))?;
        }
        if centerline_capacity > MAX_DEPOSITION_SEGMENTS as usize {
            return Err(resource("separator centerline allocation limit exceeded"));
        }
        if self.process_diaphragms.capacity() > MAX_PROCESS_REGIONS {
            return Err(resource("diaphragm coverage allocation limit exceeded"));
        }
        let mut diaphragm = 0.0;
        let mut plate_owners = BTreeSet::new();
        for plate in &self.process_diaphragms {
            plate.validate()?;
            let region = self
                .process_regions
                .get(usize::from(plate.region_index))
                .ok_or_else(|| invalid("diaphragm owner is outside layer coverage"))?;
            if !plate_owners.insert(plate.region_index)
                || region.fraction != 1.0
                || region.area_mm2 != plate.area_mm2
                || region.contributions.len() != 1
                || region.contributions[0].occurrence_id != plate.occurrence_id
                || region.contributions[0].definition_id != plate.definition_id
                || region.contributions[0].provenance != Some(plate.provenance)
            {
                return Err(invalid(
                    "permanent diaphragm lacks its unique exact construction region",
                ));
            }
            diaphragm += plate.area_mm2;
        }
        let mut contribution_capacity = 0usize;
        let mut solid = 0.0;
        let mut sparse = 0.0;
        let mut void = 0.0;
        for (index, region) in self.process_regions.iter().enumerate() {
            contribution_capacity = contribution_capacity
                .checked_add(region.contributions.capacity())
                .ok_or_else(|| resource("process contribution allocation overflow"))?;
            if contribution_capacity > MAX_PROCESS_REGION_CONTRIBUTIONS {
                return Err(resource("process contribution allocation limit exceeded"));
            }
            if !region.area_mm2.is_finite()
                || region.area_mm2 <= 0.0
                || !region.fraction.is_finite()
                || region.fraction <= 0.0
                || region.fraction > 1.0
                || (region.contributions.is_empty()
                    && region.fraction != recipe.infill_fraction
                    && !(region.fraction == 1.0
                        && region
                            .material_domain
                            .is_some_and(|r| r.role != MaterialConstructionRole::Core)))
            {
                return Err(invalid("invalid process region domain or fraction"));
            }
            let mut contributors = BTreeSet::new();
            for contribution in &region.contributions {
                if !contributors.insert(contribution.occurrence_id) {
                    return Err(invalid("duplicate process region occurrence contribution"));
                }
            }
            if plate_owners.contains(&(index as u16)) {
                // Permanent construction is not an authored sparse-fraction override.
            } else if region.fraction == 1.0 {
                solid += region.area_mm2;
            } else {
                sparse += region.area_mm2;
            }
            void += region.area_mm2 * (1.0 - region.fraction);
        }
        let values = [
            self.section_area_mm2,
            self.source_section_area_mm2,
            self.surface_added_area_mm2,
            self.wall_area_mm2,
            self.separator_area_mm2,
            self.diaphragm_area_mm2,
            self.bottom_skin_area_mm2,
            self.top_skin_area_mm2,
            self.solid_interior_area_mm2,
            self.sparse_interior_area_mm2,
            self.intentional_void_area_mm2,
            self.protected_hole_area_mm2,
            self.nominal_deposition_volume_mm3,
            self.support_area_mm2,
            self.support_nominal_deposition_volume_mm3,
        ];
        let partition = self.wall_area_mm2
            + self.bottom_skin_area_mm2
            + self.top_skin_area_mm2
            + self.solid_interior_area_mm2
            + self.sparse_interior_area_mm2
            + self.separator_area_mm2
            + self.diaphragm_area_mm2;
        let tolerance = 1e-7 * self.section_area_mm2.max(1.0);
        if values.iter().any(|v| !v.is_finite() || *v < 0.0)
            || (self.section_area_mm2 - self.source_section_area_mm2 - self.surface_added_area_mm2)
                .abs()
                > tolerance
            || (self.section_area_mm2 == 0.0
                && (self.nominal_deposition_volume_mm3 != 0.0
                    || self.source_section_area_mm2 != 0.0
                    || self.surface_added_area_mm2 != 0.0
                    || !self.process_regions.is_empty()
                    || !self.process_interfaces.is_empty()
                    || partition != 0.0
                    || self.protected_hole_area_mm2 != 0.0
                    || self.intentional_void_area_mm2 != 0.0))
            || (self.support_area_mm2 == 0.0) != (self.support_nominal_deposition_volume_mm3 == 0.0)
            || (matches!(recipe.supports, SupportPolicy::Disabled)
                && (self.support_area_mm2 != 0.0
                    || self.support_nominal_deposition_volume_mm3 != 0.0))
            || !solid.is_finite()
            || !sparse.is_finite()
            || !void.is_finite()
            || !partition.is_finite()
            || !separator.is_finite()
            || (separator == 0.0) != (self.separator_area_mm2 == 0.0)
            || (self.separator_area_mm2 - separator).abs() > tolerance
            || !diaphragm.is_finite()
            || (self.diaphragm_area_mm2 - diaphragm).abs() > tolerance
            || (solid == 0.0) != (self.solid_interior_area_mm2 == 0.0)
            || (sparse == 0.0) != (self.sparse_interior_area_mm2 == 0.0)
            || (void == 0.0) != (self.intentional_void_area_mm2 == 0.0)
            || (partition - self.section_area_mm2).abs() > tolerance
            || (self.solid_interior_area_mm2 - solid).abs() > tolerance
            || (self.sparse_interior_area_mm2 - sparse).abs() > tolerance
            || (self.intentional_void_area_mm2 - void).abs() > tolerance
        {
            return Err(invalid("invalid planar region ownership accounting"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrintLayer {
    pub index: u32,
    pub z_mm: f64,
    pub lower_z_mm: f64,
    pub height_mm: f64,
    pub section_z_mm: f64,
    #[serde(deserialize_with = "paths")]
    pub paths: Vec<DepositionPath>,
    pub region_coverage: PlanarRegionCoverage,
    /// Source-owned minimum-length realization, distinct from skin classification
    /// and independently qualified Bridge/permanent-interface construction.
    #[serde(deserialize_with = "diaphragm::skin_command_fits")]
    pub skin_command_fits: Vec<SkinCommandFitDomain>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkinCoverageScope {
    ScheduledMaterialDistance,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupportClearanceScope {
    ContinuousTargetOuterOccupancy,
}
/// Partition of accepted scheduled layers by actual source-union evidence.
/// Owned critical contributions count unique original-occurrence/slab captures,
/// including endpoint strata and transition collars, never faces or attempts.
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeFeatureCoverageSummary {
    pub disjoint_source_layer_count: u32,
    pub invariant_source_union_layer_count: u32,
    pub native_interface_complex_layer_count: u32,
    pub owned_critical_slab_count: u32,
}
#[derive(Deserialize)]
#[serde(remote = "NativeFeatureCoverageSummary", deny_unknown_fields)]
struct RawNativeFeatureCoverageSummary {
    disjoint_source_layer_count: u32,
    invariant_source_union_layer_count: u32,
    native_interface_complex_layer_count: u32,
    owned_critical_slab_count: u32,
}
impl<'de> Deserialize<'de> for NativeFeatureCoverageSummary {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawNativeFeatureCoverageSummary::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl NativeFeatureCoverageSummary {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let layers = self
            .disjoint_source_layer_count
            .checked_add(self.invariant_source_union_layer_count)
            .and_then(|count| count.checked_add(self.native_interface_complex_layer_count))
            .ok_or_else(|| invalid("source feature layer count overflow"))?;
        if layers == 0
            || layers > MAX_PRINT_LAYERS
            || self.owned_critical_slab_count > MAX_PRINT_LAYERS * MAX_OCCURRENCES
        {
            return Err(invalid("invalid native source feature coverage counts"));
        }
        Ok(())
    }
}
/// Actual occurrence query/slab proof and scheduled material coverage, not a
/// generic all-Z varying-overlap or emitted physical union-boundary certificate.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NativeGeometryCoverageSummary {
    pub requested_shape_error_mm: f64,
    pub achieved_shape_error_mm: f64,
    pub height_analysis_count: u32,
    /// Accepted original-occurrence/slab evidence captures, including endpoint ownership.
    /// One certified invariant slab may cover several scheduled layers. Every
    /// accepted slab captures every original occurrence, including inactive ones.
    pub slab_count: u32,
    pub certified_layer_count: u32,
    pub event_count: u32,
    pub geometry_work: u64,
    pub evidence: PreflightEvidence,
    pub source_feature_coverage: NativeFeatureCoverageSummary,
    pub skin_coverage: SkinCoverageScope,
    pub support_clearance: SupportClearanceScope,
}
impl NativeGeometryCoverageSummary {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        positive(&[self.requested_shape_error_mm])?;
        if !self.achieved_shape_error_mm.is_finite()
            || self.achieved_shape_error_mm < 0.0
            || self.achieved_shape_error_mm > self.requested_shape_error_mm
            || self.height_analysis_count == 0
            || self.height_analysis_count > MAX_OCCURRENCES
            || self.certified_layer_count == 0
            || self.certified_layer_count > MAX_PRINT_LAYERS
            || self.slab_count == 0
            || self.slab_count > MAX_PRINT_LAYERS * MAX_OCCURRENCES
            || self.event_count > 4096
            || self.geometry_work == 0
            || self.evidence != PreflightEvidence::Certified
            || self.slab_count < self.height_analysis_count
            || u64::from(self.slab_count)
                > u64::from(self.height_analysis_count) * u64::from(self.certified_layer_count)
        {
            return Err(invalid("invalid native geometry coverage summary"));
        }
        self.source_feature_coverage.validate()?;
        let feature_layers = self
            .source_feature_coverage
            .disjoint_source_layer_count
            .checked_add(
                self.source_feature_coverage
                    .invariant_source_union_layer_count,
            )
            .and_then(|count| {
                count.checked_add(
                    self.source_feature_coverage
                        .native_interface_complex_layer_count,
                )
            })
            .ok_or_else(|| invalid("source feature layer count overflow"))?;
        if feature_layers != self.certified_layer_count
            || self.source_feature_coverage.owned_critical_slab_count > self.slab_count
        {
            return Err(invalid(
                "native source feature coverage does not match accepted layers/slab captures",
            ));
        }
        Ok(())
    }
}
pub const MAX_SPARSE_BANDS: usize = MAX_PRINT_LAYERS as usize * MAX_PROCESS_REGIONS;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SparseBandVeto {
    ProcessHeight,
    MandatoryNeighborhood,
    ChangingDomain,
    MissingLowerAnchor,
    UnsupportedLanding,
    FlowLimit,
    InterfaceBond,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SparseBandDisposition {
    Combined,
    RetainedFine { reason: SparseBandVeto },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SparseBandDecision {
    pub first_layer: u32,
    pub last_layer: u32,
    #[serde(deserialize_with = "required_band_owner")]
    pub owner: Option<DepositionOwner>,
    pub disposition: SparseBandDisposition,
}
fn required_band_owner<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<DepositionOwner>, D::Error> {
    Option::<DepositionOwner>::deserialize(d)
}
fn sparse_bands<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SparseBandDecision>, D::Error> {
    bounded_vec::<
        D,
        _,
        { MAX_MANUFACTURING_BUNDLE_BYTES as usize / std::mem::size_of::<SparseBandDecision>() },
    >(d)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedPrintPlan {
    #[serde(deserialize_with = "construction::frames")]
    pub material_frames: Vec<SurfaceColorFrame>,
    #[serde(deserialize_with = "construction::material_domains")]
    pub material_domains: Vec<ResolvedMaterialDomain>,
    pub schema_version: u32,
    #[serde(deserialize_with = "layers")]
    pub layers: Vec<PrintLayer>,
    pub sampling_tolerance_mm: f64,
    pub surface_tolerance_mm: f64,
    pub offset_tolerance_mm: f64,
    pub layer_quantization_mm: f64,
    #[serde(deserialize_with = "resolution::records")]
    pub surface_resolution: Vec<SurfaceResolution>,
    pub native_geometry: NativeGeometryCoverageSummary,
    #[serde(deserialize_with = "process_resolution")]
    pub process_resolution: Vec<ProcessRegionResolution>,
    #[serde(deserialize_with = "sparse_bands")]
    pub sparse_bands: Vec<SparseBandDecision>,
}
/// Conservative object XY ownership envelope, padded for command serialization.
pub type PrimeExclusionBoundsMm = [[f64; 2]; 2]; // [minimum XY, maximum XY]

fn prime_intersects_bounds(start: [f64; 3], end: [f64; 3], bounds: PrimeExclusionBoundsMm) -> bool {
    let mut enter = 0.0_f64;
    let mut leave = 1.0_f64;
    for axis in 0..2 {
        let delta = end[axis] - start[axis];
        if delta == 0.0 {
            if start[axis] < bounds[0][axis] || start[axis] > bounds[1][axis] {
                return false;
            }
        } else {
            let a = (bounds[0][axis] - start[axis]) / delta;
            let b = (bounds[1][axis] - start[axis]) / delta;
            enter = enter.max(a.min(b));
            leave = leave.min(a.max(b));
            if enter > leave {
                return false;
            }
        }
    }
    true // Tangency also violates the ownership exclusion.
}

impl NormalizedPrintPlan {
    /// Actual retained heap capacity of the distinct minimum-skin proof table.
    pub fn skin_command_fit_bytes(&self) -> Result<usize, ManufacturingError> {
        let mut bytes = 0usize;
        for layer in &self.layers {
            let records = layer
                .skin_command_fits
                .capacity()
                .checked_mul(std::mem::size_of::<SkinCommandFitDomain>())
                .ok_or_else(|| resource("skin fitting metadata byte overflow"))?;
            bytes = bytes
                .checked_add(records)
                .ok_or_else(|| resource("skin fitting metadata byte overflow"))?;
            for domain in &layer.skin_command_fits {
                bytes = bytes
                    .checked_add(domain.allocated_bytes())
                    .ok_or_else(|| resource("skin fitting metadata byte overflow"))?;
            }
        }
        Ok(bytes)
    }

    pub fn validate(&self, intent: &ManufacturingIntent) -> Result<(), ManufacturingError> {
        self.validate_and_prime_exclusion(intent).map(|_| ())
    }
    /// Validate once and expose the same bounded ownership obligation to replay.
    pub fn validate_and_prime_exclusion(
        &self,
        intent: &ManufacturingIntent,
    ) -> Result<Option<PrimeExclusionBoundsMm>, ManufacturingError> {
        intent.validate()?;
        if self.schema_version != MANUFACTURING_SCHEMA_VERSION {
            return Err(invalid("unsupported plan schema"));
        }
        self.native_geometry.validate()?;
        construction::validate_material_domains(self, intent)?;
        diaphragm::validate_layers(&self.layers)?;
        if self.skin_command_fit_bytes()? > MAX_MANUFACTURING_BUNDLE_BYTES as usize {
            return Err(resource(
                "skin fitting metadata exceeds the bounded bundle allowance",
            ));
        }
        let mut fit_count = 0usize;
        for layer in &self.layers {
            if layer.skin_command_fits.len() > MAX_PROCESS_REGIONS {
                return Err(resource("skin fitting domain limit exceeded"));
            }
            for (index, domain) in layer.skin_command_fits.iter().enumerate() {
                domain.validate()?;
                if layer
                    .region_coverage
                    .process_regions
                    .get(usize::from(domain.region_index))
                    != Some(&domain.region)
                    || layer.skin_command_fits[..index].iter().any(|other| {
                        other.region_index == domain.region_index && other.skin == domain.skin
                    })
                {
                    return Err(invalid(
                        "skin fitting lost its unique captured effective region",
                    ));
                }
                fit_count = fit_count
                    .checked_add(domain.fits.len())
                    .ok_or_else(|| resource("skin fitting witness count overflow"))?;
                if fit_count > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("cumulative skin fitting witness limit exceeded"));
                }
                if domain.fits.iter().any(|fit| {
                    fit.required_mm
                        .iter()
                        .chain(&fit.fitted_mm)
                        .any(|point| point[2] != layer.z_mm)
                }) {
                    return Err(invalid("skin fitting witness has a different layer height"));
                }
            }
        }
        validate_process_resolution(&self.process_resolution)?;
        if self.process_resolution.capacity() > MAX_PROCESS_REGIONS {
            return Err(resource("process resolution allocation limit exceeded"));
        }
        for record in &self.process_resolution {
            let used = self.layers.iter().any(|layer| {
                layer.region_coverage.process_regions.iter().any(|region| {
                    region.contributions.iter().any(|contribution| {
                        contribution.occurrence_id == record.occurrence_id
                            && contribution.definition_id == record.definition_id
                            && contribution.provenance == Some(record.provenance)
                    })
                }) || layer
                    .region_coverage
                    .process_interfaces
                    .iter()
                    .any(|interface| {
                        interface.occurrence_id == record.occurrence_id
                            && interface.definition_id == record.definition_id
                            && interface.provenance == record.provenance
                    })
            });
            if (record.status == ProcessRegionStatus::Applied) != used {
                return Err(invalid(
                    "process region resolution disagrees with classified output",
                ));
            }
        }
        for interface in self
            .layers
            .iter()
            .flat_map(|layer| &layer.region_coverage.process_interfaces)
        {
            if !self.process_resolution.iter().any(|record| {
                record.occurrence_id == interface.occurrence_id
                    && record.definition_id == interface.definition_id
                    && record.provenance == interface.provenance
                    && record.status == ProcessRegionStatus::Applied
            }) {
                return Err(invalid(
                    "separator interface omits applied process resolution",
                ));
            }
        }
        if self.native_geometry.requested_shape_error_mm != intent.recipe.native_shape_error_mm {
            return Err(invalid(
                "native geometry coverage does not bind recipe shape error",
            ));
        }
        if self.surface_resolution.len() > MAX_SURFACE_RESOLUTION_RECORDS
            || self.surface_resolution.capacity() > MAX_SURFACE_RESOLUTION_RECORDS
            || self.surface_resolution.capacity() * std::mem::size_of::<SurfaceResolution>()
                + self
                    .surface_resolution
                    .iter()
                    .map(|r| r.allocated_bytes() - std::mem::size_of::<SurfaceResolution>())
                    .sum::<usize>()
                > MAX_MANUFACTURING_BUNDLE_BYTES as usize
        {
            return Err(resource("surface resolution allocation budget exceeded"));
        }
        let mut resolution_ids = BTreeSet::new();
        for record in &self.surface_resolution {
            record.validate()?;
            if self
                .layers
                .get(record.layer_index as usize)
                .is_none_or(|layer| layer.index != record.layer_index)
                || !resolution_ids.insert((
                    record.layer_index,
                    record.occurrence_id,
                    record.surface_layer_id,
                    &record.face_id,
                    &record.image_hash,
                ))
            {
                return Err(invalid(
                    "surface resolution has duplicate or unknown plan attribution",
                ));
            }
        }
        SurfaceResolutionTotals::from_records(&self.surface_resolution).validate()?;
        positive(&[
            self.sampling_tolerance_mm,
            self.surface_tolerance_mm,
            self.offset_tolerance_mm,
            self.layer_quantization_mm,
        ])?;
        if self.surface_tolerance_mm > 0.001 {
            return Err(invalid("surface approximation bound exceeds 0.001 mm"));
        }
        if self.layers.is_empty() {
            return Err(invalid("print plan is empty"));
        }
        if self.layers.len() > MAX_PRINT_LAYERS as usize {
            return Err(resource("print layer limit exceeded"));
        }
        if self.native_geometry.certified_layer_count as usize != self.layers.len() {
            return Err(invalid(
                "native geometry coverage does not bind scheduled layers",
            ));
        }
        if self.sparse_bands.len() > MAX_SPARSE_BANDS
            || self
                .sparse_bands
                .capacity()
                .checked_mul(std::mem::size_of::<SparseBandDecision>())
                .is_none_or(|bytes| bytes > MAX_MANUFACTURING_BUNDLE_BYTES as usize)
        {
            return Err(resource("sparse band allocation budget exceeded"));
        }
        let mut band_keys = BTreeSet::new();
        for band in &self.sparse_bands {
            let owner_key = match band.owner {
                Some(DepositionOwner::Regional { region_index }) => u32::from(region_index),
                None => u32::MAX,
                _ => return Err(invalid("sparse band cannot own mandatory skin")),
            };
            if band.first_layer > band.last_layer
                || band.last_layer as usize >= self.layers.len()
                || !band_keys.insert((band.first_layer, band.last_layer, owner_key))
            {
                return Err(invalid("invalid or duplicate sparse band association"));
            }
            let terminal = &self.layers[band.last_layer as usize];
            let region = match band.owner {
                Some(DepositionOwner::Regional { region_index }) => Some(
                    terminal
                        .region_coverage
                        .process_regions
                        .get(usize::from(region_index))
                        .ok_or_else(|| invalid("sparse band owner is outside terminal layer"))?,
                ),
                _ => None,
            };
            if band.disposition == SparseBandDisposition::Combined {
                let SparseInfillPolicy::CombineEligible { max_height_mm } =
                    intent.recipe.sparse_infill
                else {
                    return Err(invalid("fine policy cannot contain combined sparse bands"));
                };
                let neighborhood = band.first_layer.saturating_sub(1) as usize
                    ..=(band.last_layer as usize + 1).min(self.layers.len() - 1);
                if self.layers[neighborhood]
                    .iter()
                    .any(|layer| !layer.region_coverage.process_diaphragms.is_empty())
                {
                    return Err(invalid(
                        "combined sparse band crosses a permanent diaphragm or its fine neighborhood",
                    ));
                }
                let first = &self.layers[band.first_layer as usize];
                let physical = [first.lower_z_mm, terminal.z_mm];
                if band.first_layer == band.last_layer
                    || physical[1] - physical[0] > max_height_mm
                    || !terminal.paths.iter().any(|path| {
                        matches!(
                            path.kind,
                            DepositionKind::SparseFill | DepositionKind::Bridge
                        ) && path.owner == band.owner
                            && path.physical_z_mm == physical
                    })
                {
                    return Err(invalid(
                        "combined sparse band lacks its explicit terminal deposition",
                    ));
                }
                for layer in &self.layers[band.first_layer as usize..=band.last_layer as usize] {
                    if let Some(region) = region {
                        if !layer
                            .region_coverage
                            .process_regions
                            .iter()
                            .any(|candidate| {
                                candidate.fraction == region.fraction
                                    && candidate.contributions == region.contributions
                            })
                        {
                            return Err(invalid(
                                "combined sparse band changes effective source/profile",
                            ));
                        }
                    } else if !layer.region_coverage.process_regions.is_empty() {
                        return Err(invalid(
                            "unowned combined sparse band crosses regional ownership",
                        ));
                    }
                    for path in &layer.paths {
                        let same_owner = match (region, path.owner) {
                            (None, None) => true,
                            (Some(region), Some(DepositionOwner::Regional { region_index })) => {
                                layer
                                    .region_coverage
                                    .process_regions
                                    .get(usize::from(region_index))
                                    .is_some_and(|candidate| {
                                        candidate.fraction == region.fraction
                                            && candidate.contributions == region.contributions
                                    })
                            }
                            _ => false,
                        };
                        if same_owner
                            && matches!(
                                path.kind,
                                DepositionKind::SparseFill | DepositionKind::Bridge
                            )
                            && (layer.index != band.last_layer || path.physical_z_mm != physical)
                        {
                            return Err(invalid("combined sparse band duplicates fine deposition"));
                        }
                    }
                }
            }
        }
        if let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow {
            if !self.sparse_bands.is_empty() {
                return Err(invalid("fixed-dual routing admits no coarse sparse bands"));
            }
            let mut routed_segments = self
                .layers
                .len()
                .checked_mul(2)
                .ok_or_else(|| resource("fixed-dual waste count overflow"))?;
            for layer in &self.layers {
                let mut seen = [false; 2];
                let mut previous_tool = 0;
                for path in &layer.paths {
                    if path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
                        || matches!(
                            path.kind,
                            DepositionKind::Support
                                | DepositionKind::SupportInterface
                                | DepositionKind::SeparatorWall
                        )
                    {
                        return Err(invalid(
                            "fixed-dual model paths require ordinary fine intervals without supports or separators",
                        ));
                    }
                    let tool = job.tool_for_path(path)?;
                    routed_segments = routed_segments
                        .checked_add(path.points_mm.len() - 1)
                        .ok_or_else(|| resource("fixed-dual model and waste count overflow"))?;
                    if routed_segments > MAX_DEPOSITION_SEGMENTS as usize {
                        return Err(resource(
                            "fixed-dual model and waste share the existing segment budget",
                        ));
                    }
                    if tool < previous_tool {
                        return Err(invalid(
                            "fixed-dual layer model paths must be grouped tool zero then tool one",
                        ));
                    }
                    previous_tool = tool;
                    seen[usize::from(tool)] = true;
                }
                if seen != [true; 2] {
                    return Err(invalid(
                        "both fixed-dual loaded tools must deposit model on every layer",
                    ));
                }
                if job.park_position_mm[2] <= layer.z_mm {
                    return Err(invalid(
                        "fixed-dual final park must clear every deposition layer",
                    ));
                }
            }
        }
        // Both current output identities use the reviewed Cartesian XYZ6
        // contract. A future dialect must choose its representation explicitly.
        match intent.printer.capabilities.output_dialect {
            OutputDialect::CartesianAbsoluteGcodeV1
            | OutputDialect::Marlin2SingleExtruderGcodeV1
            | OutputDialect::Marlin2FixedDualGcodeV1 => {}
        }
        let envelope = &intent.printer.build_envelope;
        let mut lower_z = envelope.min[2];
        let mut segments = 0usize;
        let mut object_segments = 0usize;
        let complete = matches!(intent.printer.workflow, JobWorkflow::CompleteMarlin { .. });
        let mut object_xy = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
        let mut gap_start = None;
        for (index, layer) in self.layers.iter().enumerate() {
            layer.region_coverage.validate(&intent.recipe)?;
            // The certificate and actual separator strokes are a bijection;
            // neither a missing wall nor duplicated deposition can hide in area totals.
            fn centerline_key(mut line: [[f64; 2]; 2]) -> [u64; 4] {
                line = line.map(|point| point.map(|v| if v == 0.0 { 0.0 } else { v }));
                if line[0][0]
                    .total_cmp(&line[1][0])
                    .then(line[0][1].total_cmp(&line[1][1]))
                    .is_gt()
                {
                    line.swap(0, 1);
                }
                [line[0][0], line[0][1], line[1][0], line[1][1]]
                    .map(|v| if v == 0.0 { 0 } else { v.to_bits() })
            }
            let mut separators = BTreeSet::new();
            for interface in &layer.region_coverage.process_interfaces {
                for line in &interface.centerlines_mm {
                    if !separators.insert(centerline_key(*line)) {
                        return Err(invalid("duplicate separator interface centerline"));
                    }
                }
            }
            for path in layer
                .paths
                .iter()
                .filter(|path| path.kind == DepositionKind::SeparatorWall)
            {
                for segment in path.points_mm.windows(2) {
                    let line = [
                        [segment[0][0], segment[0][1]],
                        [segment[1][0], segment[1][1]],
                    ];
                    if !separators.remove(&centerline_key(line)) {
                        return Err(invalid(
                            "separator path lacks a unique interface centerline",
                        ));
                    }
                }
            }
            if !separators.is_empty() {
                return Err(invalid(
                    "separator interface centerline has no actual deposition",
                ));
            }
            let height = layer.height_mm;
            if layer.index != index as u32
                || !layer.z_mm.is_finite()
                || !layer.section_z_mm.is_finite()
                || !height.is_finite()
                || height <= 0.0
                || height > intent.recipe.layer_height_mm + self.layer_quantization_mm
                || height > intent.recipe.bead_width_mm
                || !layer.lower_z_mm.is_finite()
                || layer.lower_z_mm != lower_z
                || (layer.z_mm - layer.lower_z_mm - height).abs() > 1e-9 * height.max(1.0)
                || layer.z_mm <= lower_z
                || layer.z_mm > envelope.max[2]
                || layer.section_z_mm <= lower_z
                || layer.section_z_mm >= layer.z_mm
            {
                return Err(invalid(
                    "invalid variable-thickness layer index, deposition height or interior section height",
                ));
            }
            lower_z = layer.z_mm;
            let fixed_axis_for = |policy| match policy {
                RectilinearAxisPolicy::X => 1,
                RectilinearAxisPolicy::Y => 0,
                RectilinearAxisPolicy::AlternatingXY => {
                    if layer.index.is_multiple_of(2) {
                        1
                    } else {
                        0
                    }
                }
                RectilinearAxisPolicy::AlternatingYX => {
                    if layer.index.is_multiple_of(2) {
                        0
                    } else {
                        1
                    }
                }
            };
            if layer.paths.is_empty() {
                let SupportPolicy::Breakaway { .. } = intent.recipe.supports else {
                    return Err(invalid(
                        "empty schedule layers require explicit breakaway support policy",
                    ));
                };
                if index == 0
                    || index + 1 == self.layers.len()
                    || !self.layers[0].paths.iter().any(|p| {
                        matches!(
                            p.kind,
                            DepositionKind::Support | DepositionKind::SupportInterface
                        )
                    })
                    || (gap_start.is_none()
                        && !self.layers[index - 1]
                            .paths
                            .iter()
                            .any(|p| p.kind == DepositionKind::SupportInterface))
                {
                    return Err(invalid(
                        "contact gap requires bed-anchored support and preceding support interface",
                    ));
                }
                if gap_start.is_none() {
                    let preceding = &self.layers[index - 1];
                    gap_start = Some(
                        preceding
                            .paths
                            .iter()
                            .filter(|path| {
                                matches!(
                                    path.kind,
                                    DepositionKind::Support | DepositionKind::SupportInterface
                                )
                            })
                            .try_fold(f64::NEG_INFINITY, |top, path| {
                                program_coordinates::project_xyz6_bead_height_mm(
                                    path.physical_z_mm[1],
                                    path.height_mm(),
                                )
                                .map(|interval| top.max(interval[1]))
                            })?,
                    );
                }
            } else if let Some(support_top) = gap_start {
                if !layer.paths.iter().any(|p| {
                    !matches!(
                        p.kind,
                        DepositionKind::Support | DepositionKind::SupportInterface
                    )
                }) {
                    return Err(invalid("contact gap must end at actual object deposition"));
                }
                let SupportPolicy::Breakaway { z_gap_mm, .. } = intent.recipe.supports else {
                    return Err(invalid("contact gap requires breakaway support policy"));
                };
                let object_bottom = layer
                    .paths
                    .iter()
                    .filter(|path| {
                        !matches!(
                            path.kind,
                            DepositionKind::Support | DepositionKind::SupportInterface
                        )
                    })
                    .try_fold(f64::INFINITY, |bottom, path| {
                        program_coordinates::project_xyz6_bead_height_mm(
                            path.physical_z_mm[1],
                            path.height_mm(),
                        )
                        .map(|interval| bottom.min(interval[0]))
                    })?;
                if program_coordinates::enclosed_vertical_gap_mm(object_bottom, support_top)?
                    < z_gap_mm
                {
                    return Err(invalid(
                        "actual XYZ6 contact gap is below the declared physical minimum",
                    ));
                }
                gap_start = None;
            }
            let has_object = layer.paths.iter().any(|p| {
                !matches!(
                    p.kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                )
            });
            let has_support = layer.paths.iter().any(|p| {
                matches!(
                    p.kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                )
            });
            if has_object != (layer.region_coverage.section_area_mm2 > 0.0)
                || has_support != (layer.region_coverage.support_area_mm2 > 0.0)
            {
                return Err(invalid(
                    "object/support paths disagree with region ownership",
                ));
            }
            for path in &layer.paths {
                path.validate_owner()?;
                let physical = path.physical_z_mm;
                let path_height = path.height_mm();
                if !physical.iter().all(|z| z.is_finite()) || physical[1] != layer.z_mm {
                    return Err(invalid(&format!(
                        "path physical interval exceeds owning layer: physical_z={physical:?}, height={path_height}, layer_z={}",
                        layer.z_mm
                    )));
                }
                let recipe = &intent.recipe;
                let first_speed = if layer.index == 0 {
                    recipe.process.first_layer_speed_factor
                } else {
                    1.0
                };
                let first_flow = if layer.index == 0 {
                    recipe.process.first_layer_flow_factor
                } else {
                    1.0
                };
                let (bridge_speed, bridge_flow) = if path.kind == DepositionKind::Bridge {
                    (
                        recipe.process.bridge_speed_factor,
                        recipe.process.bridge_flow_factor,
                    )
                } else {
                    (1.0, 1.0)
                };
                let speed = recipe.print_speed_mm_s * first_speed * bridge_speed;
                let feed = (speed * 60_000_000.0).round() / 1_000_000.0;
                for edge in path.checked_edges()? {
                    let width = edge.width_mm;
                    if !intent.printer.deposition_limits.admits(width, path_height) {
                        return Err(invalid(&format!(
                            "path dimensions exceed local deposition limits or owning layer: width={width}, physical_z={physical:?}, height={path_height}, layer_z={}, limits={:?}",
                            layer.z_mm, intent.printer.deposition_limits
                        )));
                    }
                    let area = ((width - path_height) * path_height
                        + std::f64::consts::PI * (path_height * 0.5).powi(2))
                        * recipe.flow_multiplier
                        * first_flow
                        * bridge_flow;
                    if !area.is_finite()
                        || area <= 0.0
                        || !feed.is_finite()
                        || feed < 0.000001
                        || area * feed / 60.0 > intent.printer.max_volumetric_flow_mm3_s
                    {
                        return Err(invalid(
                            "path dimensions exceed emitted-feed volumetric flow limit",
                        ));
                    }
                }
                let ordinary_lower = layer.lower_z_mm;
                if physical[0] != ordinary_lower
                    && (!(path.kind == DepositionKind::SparseFill
                        || path.kind == DepositionKind::Bridge
                            && matches!(path.owner, Some(DepositionOwner::Regional { .. })))
                        || !self.sparse_bands.iter().any(|band| {
                            band.last_layer == layer.index
                                && band.owner == path.owner
                                && band.disposition == SparseBandDisposition::Combined
                                && self
                                    .layers
                                    .get(band.first_layer as usize)
                                    .is_some_and(|first| physical[0] == first.lower_z_mm)
                        }))
                {
                    return Err(invalid(
                        "coarse interval requires schedule-contiguous combined regional sparse ownership",
                    ));
                }
                if let Some(DepositionOwner::Regional { region_index }) = path.owner {
                    if usize::from(region_index) >= layer.region_coverage.process_regions.len() {
                        return Err(invalid(
                            "deposition regional owner index is outside its layer coverage",
                        ));
                    }
                    if path.points_mm.len() != 2 {
                        return Err(invalid("regional deposition is not one open row"));
                    }
                }
                if let Some(DepositionOwner::RegionalSkinBridge {
                    region_index, skin, ..
                }) = path.owner
                    && !layer
                        .region_coverage
                        .skin_bridge_domains
                        .iter()
                        .any(|domain| domain.region_index == region_index && domain.skin == skin)
                {
                    return Err(invalid(
                        "mixed skin bridge is missing its source-classified domains",
                    ));
                }
                if matches!(
                    path.kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                ) && matches!(intent.recipe.supports, SupportPolicy::Disabled)
                {
                    return Err(invalid(
                        "support deposition requires enabled support policy",
                    ));
                }
                if path.points_mm.len() < 2 {
                    return Err(invalid("deposition path requires at least two points"));
                }
                segments = segments
                    .checked_add(path.points_mm.len() - 1)
                    .ok_or_else(|| resource("segment count overflow"))?;
                if !matches!(
                    path.kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                ) {
                    object_segments += path.points_mm.len() - 1;
                }
                if segments > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("deposition segment limit exceeded"));
                }
                // Automatic walls include open thin-feature and junction arms.
                // Only immutable Wall ownership admits that construction purpose;
                // unowned/other-role perimeters must not borrow this exception.
                if path.kind == DepositionKind::Perimeter
                    && path.owner != Some(DepositionOwner::Wall)
                    && path.points_mm.first() != path.points_mm.last()
                {
                    return Err(invalid(
                        "perimeter closure must be explicit unless owned by Wall",
                    ));
                }
                let fixed_axis = matches!(
                    path.kind,
                    DepositionKind::BottomSkin
                        | DepositionKind::TopSkin
                        | DepositionKind::SolidFill
                        | DepositionKind::SparseFill
                )
                .then(|| {
                    fixed_axis_for(if path.kind == DepositionKind::TopSkin {
                        intent.recipe.top_skin_axis
                    } else {
                        intent.recipe.rectilinear_axis
                    })
                });
                let mut previous_fixed_coordinate = None;
                for point in &path.points_mm {
                    if (0..3).any(|i| {
                        !point[i].is_finite()
                            || point[i] < envelope.min[i]
                            || point[i] > envelope.max[i]
                    }) || point[2] != layer.z_mm
                    {
                        return Err(invalid(
                            "path point is nonfinite, out of envelope or not planar",
                        ));
                    }
                    if let Some(axis) = fixed_axis {
                        // Bind ordinary deposition to actual emitted XYZ6
                        // coordinates, not a geometric angular tolerance.
                        let coordinate =
                            program_coordinates::project_xyz6_coordinate_mm(point[axis])?;
                        if previous_fixed_coordinate.is_some_and(|previous| previous != coordinate)
                        {
                            return Err(invalid(
                                "ordinary deposition segment violates the authored rectilinear axis",
                            ));
                        }
                        previous_fixed_coordinate = Some(coordinate);
                    }
                    if complete {
                        for axis in 0..2 {
                            object_xy[0][axis] = object_xy[0][axis].min(point[axis]);
                            object_xy[1][axis] = object_xy[1][axis].max(point[axis]);
                        }
                    }
                }
                if path.points_mm.windows(2).any(|w| {
                    w[0] == w[1] || !(w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]).is_finite()
                }) {
                    return Err(invalid(
                        "deposition segments must have finite positive length",
                    ));
                }
            }
        }
        if let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow {
            if self
                .layers
                .iter()
                .any(|layer| job.park_position_mm[2] <= layer.z_mm)
            {
                return Err(invalid("final park must clear every deposition layer"));
            }
            let [object_min, object_max] = &mut object_xy;
            for (axis, (minimum, maximum)) in
                object_min.iter_mut().zip(object_max.iter_mut()).enumerate()
            {
                let scale = minimum
                    .abs()
                    .max(maximum.abs())
                    .max(job.prime_start_mm[axis].abs())
                    .max(job.prime_end_mm[axis].abs())
                    .max(1.0);
                // Two independent XYZ6 half-steps (object and prime), plus
                // the replay's scale-aware arithmetic allowance for each.
                let padding =
                    intent.recipe.bead_width_mm * 0.5 + 0.000001 + 64.0 * f64::EPSILON * scale;
                *minimum -= padding;
                *maximum += padding;
                if !minimum.is_finite() || !maximum.is_finite() {
                    return Err(invalid("prime exclusion envelope exceeds numeric range"));
                }
            }
            let rounded_start = program_coordinates::project_xyz6_point_mm(job.prime_start_mm)?;
            let rounded_end = program_coordinates::project_xyz6_point_mm(job.prime_end_mm)?;
            if rounded_start
                .iter()
                .chain(&rounded_end)
                .any(|value| !value.is_finite())
                || prime_intersects_bounds(rounded_start, rounded_end, object_xy)
            {
                return Err(invalid(
                    "rounded priming violates conservative XY ownership exclusion",
                ));
            }
            if prime_intersects_bounds(job.prime_start_mm, job.prime_end_mm, object_xy) {
                return Err(invalid(
                    "priming intersects conservative object XY ownership envelope",
                ));
            }
        }
        if segments == 0 || object_segments == 0 {
            return Err(invalid("print plan has no object deposition"));
        }
        Ok(if complete { Some(object_xy) } else { None })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingProvenance {
    pub project_id: ProjectId,
    pub input_revision: ProjectRevision,
    pub input_hash: SourceHash,
    #[serde(deserialize_with = "definitions")]
    pub definitions: Vec<StoredDefinition>,
    #[serde(deserialize_with = "occurrences")]
    pub occurrences: Vec<OccurrenceRecord>,
    pub intent: ManufacturingIntent,
    pub surface_overlays: SurfaceOverlays,
    pub process_document: ProcessDocument,
    #[serde(deserialize_with = "bounded_image_contents")]
    pub surface_images: Vec<SurfaceImageContent>,
    pub kernel: KernelIdentity,
    pub planner_version: String,
    pub emitter_version: String,
    pub verifier_version: String,
}
impl ManufacturingProvenance {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.surface_images.capacity() > crate::surface::MAX_SURFACE_IMAGES {
            return Err(resource(
                "surface image content table allocation budget exceeded",
            ));
        }
        if self
            .surface_images
            .iter()
            .map(|image| image.image_hash.allocated_bytes())
            .sum::<usize>()
            > crate::surface::MAX_SURFACE_DOCUMENT_BYTES
        {
            return Err(resource(
                "surface image content identity allocation budget exceeded",
            ));
        }
        validate_image_contents(&self.surface_overlays.images, &self.surface_images)
            .map_err(|e| invalid(&e.message))?;
        label(&self.kernel.name)?;
        label(&self.kernel.version)?;
        label(&self.kernel.revision)?;
        label(&self.planner_version)?;
        label(&self.emitter_version)?;
        label(&self.verifier_version)?;
        if self.definitions.is_empty() || self.occurrences.is_empty() {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::EmptyProject,
                "manufacturing provenance requires geometry",
            ));
        }
        let hash = input_fingerprint(
            &self.project_id,
            &self.definitions,
            &self.occurrences,
            &self.intent,
            &self.surface_overlays,
            &self.process_document,
        )?;
        if hash != self.input_hash {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::CorruptArtifact,
                "provenance input hash mismatch",
            ));
        }
        Ok(())
    }
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

fn validate_routing_totals(
    routing: Option<&FixedDualVerification>,
    model_segments: u32,
    model_volume: f64,
    model_filament: f64,
    support_segments: u32,
) -> Result<u32, ManufacturingError> {
    let Some(routing) = routing else {
        return Ok(0);
    };
    routing.validate()?;
    if support_segments != 0
        || !routing.selection_count.is_multiple_of(2)
        || routing.tools.iter().any(|tool| {
            tool.waste_deposition_segments != routing.selection_count / 2
                || tool.model_deposition_segments < routing.selection_count / 2
        })
        || routing
            .tools
            .iter()
            .map(|tool| tool.model_deposition_segments)
            .sum::<u32>()
            != model_segments
        || routing
            .tools
            .iter()
            .map(|tool| tool.model_volume_mm3)
            .sum::<f64>()
            != model_volume
        || routing
            .tools
            .iter()
            .map(|tool| tool.model_filament_length_mm)
            .sum::<f64>()
            != model_filament
    {
        return Err(invalid(
            "routed model accounting disagrees with report totals",
        ));
    }
    Ok(routing
        .tools
        .iter()
        .map(|tool| tool.waste_deposition_segments)
        .sum())
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VerificationReport {
    pub verified: bool,
    #[serde(deserialize_with = "descriptions")]
    pub coverage: Vec<String>,
    #[serde(deserialize_with = "descriptions")]
    pub limitations: Vec<String>,
    pub deposition_segments: u32,
    pub travel_segments: u32,
    pub deposited_volume_mm3: f64,
    pub filament_length_mm: f64,
    pub max_position_error_mm: f64,
    pub max_extrusion_error_mm: f64,
    pub lifecycle_motion_count: u32,
    pub priming_filament_length_mm: f64,
    pub support_deposition_segments: u32,
    pub support_deposited_volume_mm3: f64,
    pub support_filament_length_mm: f64,
    pub retraction_count: u32,
    pub retracted_filament_mm: f64,
    #[serde(deserialize_with = "required_option")]
    pub routing: Option<FixedDualVerification>,
}
impl VerificationReport {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let waste_segments = validate_routing_totals(
            self.routing.as_ref(),
            self.deposition_segments,
            self.deposited_volume_mm3,
            self.filament_length_mm,
            self.support_deposition_segments,
        )?;
        if self
            .deposition_segments
            .checked_add(self.support_deposition_segments)
            .and_then(|n| n.checked_add(waste_segments))
            .is_none_or(|n| n > MAX_DEPOSITION_SEGMENTS)
            || self.retraction_count > 50_000
            || self.travel_segments > MAX_DEPOSITION_SEGMENTS
        {
            return Err(resource("verification segment limit exceeded"));
        }
        if let Some(routing) = &self.routing {
            if self.retraction_count != routing.selection_count
                || routing
                    .tools
                    .iter()
                    .any(|tool| tool.outstanding_retraction_mm <= 0.0)
            {
                return Err(invalid(
                    "fixed-dual final pressure debt or retraction count mismatch",
                ));
            }
        }
        if (self.routing.is_some()
            && (self.lifecycle_motion_count != 2
                || self.priming_filament_length_mm
                    != self
                        .routing
                        .as_ref()
                        .unwrap()
                        .tools
                        .iter()
                        .map(|tool| tool.waste_filament_length_mm)
                        .sum::<f64>()))
            || (self.routing.is_none()
                && self.lifecycle_motion_count != 0
                && self.lifecycle_motion_count != 4)
            || (self.lifecycle_motion_count == 0 && self.priming_filament_length_mm != 0.0)
            || (self.lifecycle_motion_count == 4 && self.priming_filament_length_mm <= 0.0)
        {
            return Err(invalid("invalid lifecycle motion count"));
        }
        if [
            self.deposited_volume_mm3,
            self.filament_length_mm,
            self.max_position_error_mm,
            self.max_extrusion_error_mm,
            self.priming_filament_length_mm,
            self.support_deposited_volume_mm3,
            self.support_filament_length_mm,
            self.retracted_filament_mm,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(invalid(
                "verification measurements must be finite and nonnegative",
            ));
        }
        if (self.retraction_count == 0) != (self.retracted_filament_mm == 0.0)
            || (self.support_deposition_segments == 0) != (self.support_filament_length_mm == 0.0)
            || (self.support_deposition_segments == 0) != (self.support_deposited_volume_mm3 == 0.0)
        {
            return Err(invalid(
                "verification process/support counts disagree with measurements",
            ));
        }
        if self.coverage.len() > 64 || self.limitations.len() > 64 {
            return Err(resource("verification description limit exceeded"));
        }
        for value in self.coverage.iter().chain(&self.limitations) {
            label(value)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingSummary {
    pub layers: u32,
    pub paths: u32,
    pub deposition_segments: u32,
    pub deposited_volume_mm3: f64,
    pub filament_length_mm: f64,
    pub support_deposition_segments: u32,
    pub support_deposited_volume_mm3: f64,
    pub support_filament_length_mm: f64,
    pub software_only: bool,
    pub surface_resolution: SurfaceResolutionTotals,
    pub native_geometry: NativeGeometryCoverageSummary,
    #[serde(deserialize_with = "process_resolution")]
    pub process_resolution: Vec<ProcessRegionResolution>,
    #[serde(deserialize_with = "required_option")]
    pub routing: Option<FixedDualVerification>,
}
impl ManufacturingSummary {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let waste_segments = validate_routing_totals(
            self.routing.as_ref(),
            self.deposition_segments,
            self.deposited_volume_mm3,
            self.filament_length_mm,
            self.support_deposition_segments,
        )?;
        if let Some(routing) = &self.routing {
            if self.layers.checked_mul(2) != Some(routing.selection_count)
                || routing.tools.iter().any(|tool| {
                    tool.waste_deposition_segments != self.layers
                        || tool.outstanding_retraction_mm <= 0.0
                })
            {
                return Err(invalid(
                    "fixed-dual summary layers disagree with route accounting",
                ));
            }
        }
        self.surface_resolution.validate()?;
        self.native_geometry.validate()?;
        validate_process_resolution(&self.process_resolution)?;
        if self.process_resolution.capacity() > MAX_PROCESS_REGIONS {
            return Err(resource("process resolution allocation limit exceeded"));
        }
        if self.native_geometry.certified_layer_count != self.layers {
            return Err(invalid(
                "native geometry coverage does not bind summary layers",
            ));
        }
        if self.layers == 0
            || self.layers > MAX_PRINT_LAYERS
            || self.paths == 0
            || self
                .deposition_segments
                .checked_add(self.support_deposition_segments)
                .is_none_or(|n| self.paths > n)
            || self
                .deposition_segments
                .checked_add(self.support_deposition_segments)
                .and_then(|n| n.checked_add(waste_segments))
                .is_none_or(|n| n > MAX_DEPOSITION_SEGMENTS)
        {
            return Err(invalid("invalid manufacturing summary counts"));
        }
        positive(&[self.deposited_volume_mm3, self.filament_length_mm])?;
        if [
            self.support_deposited_volume_mm3,
            self.support_filament_length_mm,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(invalid(
                "support summary measurements must be finite and nonnegative",
            ));
        }
        if !self.software_only {
            return Err(invalid("manufacturing artifacts must be software-only"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingBundle {
    pub schema_version: u32,
    pub provenance: ManufacturingProvenance,
    pub plan: NormalizedPrintPlan,
    pub program: String,
    pub verification: VerificationReport,
}
impl ManufacturingBundle {
    /// Schema/integrity validation only: consumers must independently replay the program.
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.schema_version != MANUFACTURING_SCHEMA_VERSION {
            return Err(invalid("unsupported manufacturing bundle schema"));
        }
        self.provenance.validate()?;
        self.plan.validate(&self.provenance.intent)?;
        if self.plan.native_geometry.height_analysis_count as usize
            != self.provenance.occurrences.len()
        {
            return Err(invalid(
                "native geometry coverage does not bind captured occurrences",
            ));
        }
        let recipe = &self.provenance.intent.recipe;
        let process = &self.provenance.process_document;
        let scope_matches = |scope: &crate::surface::SurfaceScope,
                             definition_id: &DefinitionId,
                             occurrence_id: OccurrenceId| {
            match scope {
                crate::surface::SurfaceScope::Definition { definition_id: id } => {
                    id == definition_id
                }
                crate::surface::SurfaceScope::Occurrence {
                    occurrence_id: id,
                    expected_definition_id,
                } => *id == occurrence_id && expected_definition_id == definition_id,
            }
        };
        for record in &self.plan.process_resolution {
            let source_known = self.provenance.occurrences.iter().any(|occurrence| {
                occurrence.occurrence_id == record.occurrence_id
                    && occurrence.definition_id == record.definition_id
            });
            let operation_known = process.layers.iter().any(|layer| {
                layer.enabled
                    && layer.layer_id == record.provenance.layer_id
                    && scope_matches(&layer.scope, &record.definition_id, record.occurrence_id)
                    && layer.operations.iter().any(|operation| {
                        operation.operation_id == record.provenance.operation_id
                            && matches!(
                                operation.selector,
                                crate::process::ProcessSelector::SourceBoxMm { .. }
                            )
                            && matches!(
                                operation.action,
                                crate::process::ProcessAction::Replace { .. }
                            )
                    })
            });
            if !source_known || !operation_known {
                return Err(invalid(
                    "process resolution references unknown source or box operation",
                ));
            }
        }
        let has_overrides = self.provenance.occurrences.iter().any(|occurrence| {
            process
                .resolve(
                    &occurrence.definition_id,
                    occurrence.occurrence_id,
                    recipe.infill_fraction,
                )
                .provenance
                .is_some()
                || process.layers.iter().any(|layer| {
                    layer.enabled
                        && scope_matches(
                            &layer.scope,
                            &occurrence.definition_id,
                            occurrence.occurrence_id,
                        )
                        && layer.operations.iter().any(|operation| {
                            matches!(
                                operation.selector,
                                crate::process::ProcessSelector::SourceBoxMm { .. }
                            )
                        })
                })
        });
        for (layer, index, region) in self.plan.layers.iter().flat_map(|layer| {
            layer
                .region_coverage
                .process_regions
                .iter()
                .enumerate()
                .map(move |(index, region)| (layer, index, region))
        }) {
            if region.contributions.is_empty() && has_overrides {
                return Err(invalid(
                    "process region omits captured override attribution",
                ));
            }
            for contribution in &region.contributions {
                if !self.provenance.occurrences.iter().any(|occurrence| {
                    occurrence.occurrence_id == contribution.occurrence_id
                        && occurrence.definition_id == contribution.definition_id
                }) {
                    return Err(invalid(
                        "process region references an unknown or inconsistent occurrence",
                    ));
                }
                if let Some(plate) = layer
                    .region_coverage
                    .process_diaphragms
                    .iter()
                    .find(|plate| usize::from(plate.region_index) == index)
                {
                    let occurrence = self
                        .provenance
                        .occurrences
                        .iter()
                        .find(|occurrence| {
                            occurrence.occurrence_id == plate.occurrence_id
                                && occurrence.definition_id == plate.definition_id
                        })
                        .ok_or_else(|| {
                            invalid("permanent diaphragm references an unknown occurrence")
                        })?;
                    diaphragm::validate_binding(
                        plate,
                        process,
                        &occurrence.pose,
                        recipe.infill_fraction,
                    )?;
                    continue;
                }
                let resolved = process.resolve(
                    &contribution.definition_id,
                    contribution.occurrence_id,
                    recipe.infill_fraction,
                );
                let spatial = process.layers.iter().any(|layer| {
                    layer.enabled
                        && scope_matches(
                            &layer.scope,
                            &contribution.definition_id,
                            contribution.occurrence_id,
                        )
                        && layer.operations.iter().any(|operation| {
                            matches!(
                                operation.selector,
                                crate::process::ProcessSelector::SourceBoxMm { .. }
                            )
                        })
                });
                // Without spatial geometry, validate captured operation/source/value
                // binding, not an invented whole-volume winner for a box partition.
                let valid = if spatial {
                    match contribution.provenance {
                        None => region.fraction == recipe.infill_fraction,
                        Some(provenance) => process.layers.iter().any(|layer| {
                            layer.enabled
                                && layer.layer_id == provenance.layer_id
                                && scope_matches(
                                    &layer.scope,
                                    &contribution.definition_id,
                                    contribution.occurrence_id,
                                )
                                && layer.operations.iter().any(|operation| {
                                    operation.operation_id == provenance.operation_id
                                        && operation.action
                                            == crate::process::ProcessAction::Replace {
                                                value:
                                                    crate::process::ProcessValue::SparseFraction {
                                                        fraction: region.fraction,
                                                    },
                                            }
                                        && (!matches!(
                                            operation.selector,
                                            crate::process::ProcessSelector::SourceBoxMm { .. }
                                        ) || self.plan.process_resolution.iter().any(
                                            |record| {
                                                record.occurrence_id == contribution.occurrence_id
                                                    && record.definition_id
                                                        == contribution.definition_id
                                                    && record.provenance == provenance
                                                    && record.status == ProcessRegionStatus::Applied
                                            },
                                        ))
                                })
                        }),
                    }
                } else {
                    resolved.provenance == contribution.provenance
                        && resolved.fraction == region.fraction
                };
                if !valid {
                    return Err(invalid(
                        "process region disagrees with captured process attribution",
                    ));
                }
            }
        }
        for interface in self
            .plan
            .layers
            .iter()
            .flat_map(|layer| &layer.region_coverage.process_interfaces)
        {
            let occurrence_known = self.provenance.occurrences.iter().any(|occurrence| {
                occurrence.occurrence_id == interface.occurrence_id
                    && occurrence.definition_id == interface.definition_id
            });
            let authorized = process.layers.iter().any(|layer| {
                layer.enabled
                    && layer.layer_id == interface.provenance.layer_id
                    && scope_matches(
                        &layer.scope,
                        &interface.definition_id,
                        interface.occurrence_id,
                    )
                    && layer.operations.iter().any(|operation| {
                        operation.operation_id == interface.provenance.operation_id
                            && matches!(
                                operation.selector,
                                crate::process::ProcessSelector::SourceBoxMm { .. }
                            )
                            && matches!(
                                operation.action,
                                crate::process::ProcessAction::Replace { .. }
                            )
                            && operation.interface
                                == Some(crate::process::ProcessInterfacePermission::SeparatorWall)
                    })
            });
            if !occurrence_known || !authorized || interface.thickness_mm != recipe.bead_width_mm {
                return Err(invalid(
                    "separator interface lacks captured source permission or bead width",
                ));
            }
        }
        resolution::validate_attribution(
            &self.plan.surface_resolution,
            &self.provenance.surface_overlays,
            &self.provenance.occurrences,
        )?;
        self.verification.validate()?;
        match (
            &self.provenance.intent.printer.workflow,
            &self.verification.routing,
        ) {
            (JobWorkflow::DiagnosticBodyOnly, None)
                if self.verification.lifecycle_motion_count == 0 => {}
            (JobWorkflow::CompleteMarlin { .. }, None)
                if self.verification.lifecycle_motion_count == 4 => {}
            (JobWorkflow::FixedDualMarlin { job }, Some(routing)) => {
                if self.provenance.occurrences.len() != job.assignments.len()
                    || self
                        .provenance
                        .surface_overlays
                        .layers
                        .iter()
                        .any(|layer| layer.enabled)
                    || self
                        .provenance
                        .process_document
                        .layers
                        .iter()
                        .any(|layer| layer.enabled && !layer.operations.is_empty())
                    || routing.selection_count != self.plan.layers.len() as u32 * 2
                {
                    return Err(invalid(
                        "fixed-dual captured source, finish, process or selection attribution mismatch",
                    ));
                }
                for assignment in &job.assignments {
                    if !self
                        .provenance
                        .occurrences
                        .iter()
                        .any(|occurrence| occurrence.occurrence_id == assignment.occurrence_id)
                    {
                        return Err(invalid(
                            "fixed-dual assignment does not bind a captured source occurrence",
                        ));
                    }
                }
                for construction in &job.surface_constructions {
                    let occurrence = self
                        .provenance
                        .occurrences
                        .iter()
                        .find(|o| {
                            o.occurrence_id == construction.occurrence_id
                                && o.definition_id == construction.definition_id
                        })
                        .ok_or_else(|| {
                            invalid("surface construction has no captured source occurrence")
                        })?;
                    if !self.provenance.definitions.iter().any(|d| {
                        d.definition_id == occurrence.definition_id
                            && d.provenance.source_hash == construction.source_hash
                    }) {
                        return Err(invalid(
                            "surface construction immutable source binding mismatch",
                        ));
                    }
                }
                let mut routed_segments = [0u32; 2];
                for path in self.plan.layers.iter().flat_map(|layer| &layer.paths) {
                    let tool = usize::from(job.tool_for_path(path)?);
                    routed_segments[tool] = routed_segments[tool]
                        .checked_add((path.points_mm.len() - 1) as u32)
                        .ok_or_else(|| resource("routed model count overflow"))?;
                }
                for (index, tool) in routing.tools.iter().enumerate() {
                    if tool.material_id != job.tools[index].material.id
                        || tool.model_deposition_segments != routed_segments[index]
                        || tool.waste_deposition_segments != self.plan.layers.len() as u32
                        || (tool.outstanding_retraction_mm - job.tools[index].retract_filament_mm)
                            .abs()
                            > 0.000_000_010_001
                    {
                        return Err(invalid(
                            "fixed-dual decoded tool accounting disagrees with captured route",
                        ));
                    }
                }
            }
            _ => {
                return Err(invalid(
                    "verification routing/lifecycle does not match captured workflow",
                ));
            }
        }
        if self.program.is_empty() || self.program.len() > MAX_MANUFACTURING_BUNDLE_BYTES as usize {
            return Err(resource("manufacturing program size is invalid"));
        }
        let mut object_count = 0usize;
        let mut support_count = 0usize;
        for path in self.plan.layers.iter().flat_map(|l| &l.paths) {
            if matches!(
                path.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            ) {
                support_count += path.points_mm.len() - 1;
            } else {
                object_count += path.points_mm.len() - 1;
            }
        }
        if self.verification.deposition_segments as usize != object_count
            || self.verification.support_deposition_segments as usize != support_count
        {
            return Err(invalid(
                "verification/plan object or support segment count mismatch",
            ));
        }
        self.summary().validate()?;
        json_bytes(self, MAX_MANUFACTURING_BUNDLE_BYTES as usize)?;
        Ok(())
    }
    /// Call after validation; measurements describe stored replay, not physical support.
    pub fn summary(&self) -> ManufacturingSummary {
        ManufacturingSummary {
            layers: self.plan.layers.len() as u32,
            paths: self.plan.layers.iter().map(|l| l.paths.len() as u32).sum(),
            deposition_segments: self.verification.deposition_segments,
            deposited_volume_mm3: self.verification.deposited_volume_mm3,
            filament_length_mm: self.verification.filament_length_mm,
            support_deposition_segments: self.verification.support_deposition_segments,
            support_deposited_volume_mm3: self.verification.support_deposited_volume_mm3,
            support_filament_length_mm: self.verification.support_filament_length_mm,
            routing: self.verification.routing.clone(),
            software_only: true,
            surface_resolution: SurfaceResolutionTotals::from_records(
                &self.plan.surface_resolution,
            ),
            native_geometry: self.plan.native_geometry.clone(),
            process_resolution: self.plan.process_resolution.clone(),
        }
    }
    /// Match metadata on an already validated bundle; caller checks exact byte hash/size.
    pub fn validate_record(
        &self,
        record: &ManufacturingArtifactRecord,
    ) -> Result<(), ManufacturingError> {
        record.validate()?;
        if self.provenance.input_hash != record.input_hash || self.summary() != record.summary {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::CorruptArtifact,
                "artifact record disagrees with bundle",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingArtifactRecord {
    pub hash: SourceHash,
    pub input_hash: SourceHash,
    pub byte_count: u32,
    pub summary: ManufacturingSummary,
}
impl ManufacturingArtifactRecord {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.byte_count == 0 || self.byte_count > MAX_MANUFACTURING_BUNDLE_BYTES {
            return Err(resource("manufacturing artifact size limit exceeded"));
        }
        self.summary.validate()
    }
}

/// Bounded inert JSON admission. This does not execute TypeScript or replay G-code.
pub fn decode_intent(bytes: &[u8]) -> Result<ManufacturingIntent, ManufacturingError> {
    if bytes.len() > MAX_PROFILE_JSON_BYTES as usize {
        return Err(resource("intent JSON byte limit exceeded"));
    }
    let intent: ManufacturingIntent =
        serde_json::from_slice(bytes).map_err(|e| invalid(&e.to_string()))?;
    intent.validate()?;
    Ok(intent)
}
pub fn decode_bundle(bytes: &[u8]) -> Result<ManufacturingBundle, ManufacturingError> {
    if bytes.is_empty() || bytes.len() > MAX_MANUFACTURING_BUNDLE_BYTES as usize {
        return Err(resource("bundle byte limit exceeded"));
    }
    #[derive(Deserialize)]
    struct BundleVersion {
        schema_version: u32,
    }
    let version: BundleVersion = serde_json::from_slice(bytes).map_err(|e| {
        ManufacturingError::new(ManufacturingErrorCode::CorruptArtifact, e.to_string())
    })?;
    if version.schema_version != MANUFACTURING_SCHEMA_VERSION {
        return Err(invalid("unsupported manufacturing bundle schema"));
    }
    let bundle: ManufacturingBundle = serde_json::from_slice(bytes).map_err(|e| {
        ManufacturingError::new(ManufacturingErrorCode::CorruptArtifact, e.to_string())
    })?;
    bundle.validate()?;
    Ok(bundle)
}

/// Canonical compact JSON of explicit semantic input fields sorted by identity.
/// Revision, allocator, derived artifacts and timing are intentionally excluded.
pub fn input_fingerprint(
    project_id: &ProjectId,
    definitions: &[StoredDefinition],
    occurrences: &[OccurrenceRecord],
    intent: &ManufacturingIntent,
    surface_overlays: &SurfaceOverlays,
    process_document: &ProcessDocument,
) -> Result<SourceHash, ManufacturingError> {
    intent.validate()?;
    surface_overlays
        .validate_targets(definitions, occurrences, u32::MAX)
        .map_err(|e| {
            if e.code == crate::project::ProjectErrorCode::ResourceLimit {
                resource(&e.message)
            } else {
                invalid(&e.message)
            }
        })?;
    process_document
        .validate_targets(definitions, occurrences, u32::MAX, u32::MAX)
        .map_err(|e| {
            if e.code == crate::project::ProjectErrorCode::ResourceLimit {
                resource(&e.message)
            } else {
                invalid(&e.message)
            }
        })?;
    if definitions.len() > MAX_DEFINITIONS as usize || occurrences.len() > MAX_OCCURRENCES as usize
    {
        return Err(resource("input record budget exceeded"));
    }
    let mut defs: Vec<_> = definitions.iter().collect();
    let mut occs: Vec<_> = occurrences.iter().collect();
    defs.sort_unstable_by(|a, b| a.definition_id.cmp(&b.definition_id));
    occs.sort_unstable_by_key(|o| o.occurrence_id);
    let mut ids = BTreeSet::new();
    for def in &defs {
        def.provenance.validate().map_err(|e| invalid(&e.message))?;
        let mut digest = [0u8; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&def.provenance.source_hash.as_str()[i * 2..i * 2 + 2], 16)
                .map_err(|_| invalid("invalid source hash"))?;
        }
        if DefinitionId::from_source_sha256(&digest) != def.definition_id
            || !ids.insert(&def.definition_id)
        {
            return Err(invalid("duplicate or source-inconsistent definition"));
        }
    }
    let mut used = BTreeSet::new();
    let mut previous = None;
    for occ in &occs {
        occ.pose.validate().map_err(|e| invalid(&e.message))?;
        if !ids.contains(&occ.definition_id) || previous == Some(occ.occurrence_id) {
            return Err(invalid(
                "invalid occurrence reference or duplicate identity",
            ));
        }
        previous = Some(occ.occurrence_id);
        used.insert(&occ.definition_id);
    }
    if ids != used {
        return Err(invalid("unreferenced input definition"));
    }
    #[derive(Serialize)]
    struct SemanticInput<'a> {
        schema_version: u32,
        project_id: &'a ProjectId,
        units: ProjectUnits,
        frame: ProjectFrame,
        definitions: Vec<&'a StoredDefinition>,
        occurrences: Vec<&'a OccurrenceRecord>,
        intent: &'a ManufacturingIntent,
        surface_overlays: &'a SurfaceOverlays,
        process_document: &'a ProcessDocument,
    }
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let semantic = SemanticInput {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
        project_id,
        units: ProjectUnits::Millimetres,
        frame: ProjectFrame::RightHanded,
        definitions: defs,
        occurrences: occs,
        intent,
        surface_overlays,
        process_document,
    };
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, &semantic).map_err(|e| invalid(&e.to_string()))?;
    let hash: [u8; 32] = writer.0.finalize().into();
    Ok(SourceHash::from_digest(&hash))
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManufacturingCommand {
    Get {
        session_id: SessionId,
    },
    SetIntent {
        session_id: SessionId,
        base_revision: ProjectRevision,
        intent: Arc<ManufacturingIntent>,
    },
    Compile {
        session_id: SessionId,
        base_revision: ProjectRevision,
    },
    Preflight {
        session_id: SessionId,
        base_revision: ProjectRevision,
    },
    Inspect {
        session_id: SessionId,
        base_revision: ProjectRevision,
        output: VerificationOutput,
    },
}
#[expect(
    clippy::large_enum_variant,
    reason = "Bounded control replies stay inline; boxing adds an allocation to each operation observation"
)]
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManufacturingResponse {
    Status {
        info: ProjectInfo,
        intent: Option<Arc<ManufacturingIntent>>,
        artifact: Option<ManufacturingArtifactRecord>,
        resource: Option<crate::ArtifactView>,
    },
    OperationAccepted {
        operation: crate::NativeOperationView,
    },
    Verified {
        record: ManufacturingArtifactRecord,
        report: VerificationReport,
        resource: crate::ArtifactView,
        provenance: ManufacturingInspectionProvenance,
        replay_display: Option<ReplayDisplayDescriptor>,
    },
    Preflight {
        report: PreflightReport,
    },
    Error {
        error: ManufacturingError,
    },
}

#[cfg(test)]
mod tests;
