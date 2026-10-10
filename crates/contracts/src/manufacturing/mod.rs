// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded software-only manufacturing data. Profile declarations certify no machine.
use crate::{geometry::*, project::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};
use ts_rs::TS;

pub const MANUFACTURING_SCHEMA_VERSION: u32 = 1;
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
struct SegmentBudget {
    remaining: u32,
}
impl SegmentBudget {
    fn new() -> Self {
        Self {
            remaining: MAX_DEPOSITION_SEGMENTS,
        }
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
            if self.0.remaining == 0 {
                return Err(serde::de::Error::custom(
                    "deposition segment limit exceeded",
                ));
            }
            if !points.is_empty() {
                self.0.remaining -= 1;
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

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum PathField {
    Kind,
    PointsMm,
}
struct PathSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for PathSeed<'_> {
    type Value = DepositionPath;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_struct("DepositionPath", &["kind", "points_mm"], self)
    }
}
impl<'de> serde::de::Visitor<'de> for PathSeed<'_> {
    type Value = DepositionPath;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a deposition path")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut kind = None;
        let mut points_mm = None;
        while let Some(field) = map.next_key::<PathField>()? {
            match field {
                PathField::Kind => {
                    if kind.is_some() {
                        return Err(serde::de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                PathField::PointsMm => {
                    if points_mm.is_some() {
                        return Err(serde::de::Error::duplicate_field("points_mm"));
                    }
                    points_mm = Some(map.next_value_seed(PointsSeed(&mut *self.0))?);
                }
            }
        }
        Ok(DepositionPath {
            kind: kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?,
            points_mm: points_mm.ok_or_else(|| serde::de::Error::missing_field("points_mm"))?,
        })
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
        f.write_str("a nonempty bounded array of deposition paths")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut paths = Vec::new();
        while paths.len() < MAX_DEPOSITION_SEGMENTS as usize {
            match seq.next_element_seed(PathSeed(&mut *self.0))? {
                Some(path) => paths.push(path),
                None if paths.is_empty() => {
                    return Err(serde::de::Error::custom("print layer has no deposition"));
                }
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
    SectionZMm,
    Paths,
}
struct LayerSeed<'a>(&'a mut SegmentBudget);
impl<'de> serde::de::DeserializeSeed<'de> for LayerSeed<'_> {
    type Value = PrintLayer;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_struct(
            "PrintLayer",
            &["index", "z_mm", "section_z_mm", "paths"],
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
        let mut section_z_mm = None;
        let mut paths = None;
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
            }
        }
        Ok(PrintLayer {
            index: index.ok_or_else(|| serde::de::Error::missing_field("index"))?,
            z_mm: z_mm.ok_or_else(|| serde::de::Error::missing_field("z_mm"))?,
            section_z_mm: section_z_mm
                .ok_or_else(|| serde::de::Error::missing_field("section_z_mm"))?,
            paths: paths.ok_or_else(|| serde::de::Error::missing_field("paths"))?,
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

fn points<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<[f64; 3]>, D::Error> {
    serde::de::DeserializeSeed::deserialize(PointsSeed(&mut SegmentBudget::new()), d)
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
        if self.extruders != 1 || self.generated_supports {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::UnsupportedCapability,
                "only one extruder and no generated supports are implemented",
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
    pub nozzle_diameter_mm: f64,
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
        self.capabilities.validate()?;
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
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanarPrintRecipe {
    pub id: String,
    pub revision: String,
    pub material_id: String,
    pub material_revision: String,
    pub filament_diameter_mm: f64,
    pub layer_height_mm: f64,
    pub bead_width_mm: f64,
    pub perimeter_count: u32,
    pub infill_fraction: f64,
    pub print_speed_mm_s: f64,
    pub travel_speed_mm_s: f64,
    pub flow_multiplier: f64,
}
impl PlanarPrintRecipe {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        label(&self.id)?;
        label(&self.revision)?;
        label(&self.material_id)?;
        label(&self.material_revision)?;
        positive(&[
            self.filament_diameter_mm,
            self.layer_height_mm,
            self.bead_width_mm,
            self.print_speed_mm_s,
            self.travel_speed_mm_s,
            self.flow_multiplier,
        ])?;
        if !(1..=8).contains(&self.perimeter_count) || self.layer_height_mm > self.bead_width_mm {
            return Err(invalid(
                "recipe requires 1..8 perimeters and layer height <= bead width",
            ));
        }
        if self.infill_fraction != 1.0 {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::UnsupportedCapability,
                "only solid infill is implemented",
            ));
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
        {
            return Err(invalid(
                "recipe exceeds printer feed/rounded-bead flow limits or numeric range",
            ));
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
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DepositionPath {
    pub kind: DepositionKind,
    #[serde(deserialize_with = "points")]
    pub points_mm: Vec<[f64; 3]>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrintLayer {
    pub index: u32,
    pub z_mm: f64,
    pub section_z_mm: f64,
    #[serde(deserialize_with = "paths")]
    pub paths: Vec<DepositionPath>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedPrintPlan {
    pub schema_version: u32,
    #[serde(deserialize_with = "layers")]
    pub layers: Vec<PrintLayer>,
    pub sampling_tolerance_mm: f64,
    pub offset_tolerance_mm: f64,
    pub layer_quantization_mm: f64,
}
impl NormalizedPrintPlan {
    pub fn validate(&self, intent: &ManufacturingIntent) -> Result<(), ManufacturingError> {
        intent.validate()?;
        if self.schema_version != MANUFACTURING_SCHEMA_VERSION {
            return Err(invalid("unsupported plan schema"));
        }
        positive(&[
            self.sampling_tolerance_mm,
            self.offset_tolerance_mm,
            self.layer_quantization_mm,
        ])?;
        if self.layers.is_empty() {
            return Err(invalid("print plan is empty"));
        }
        if self.layers.len() > MAX_PRINT_LAYERS as usize {
            return Err(resource("print layer limit exceeded"));
        }
        let envelope = &intent.printer.build_envelope;
        let height = intent.recipe.layer_height_mm;
        let mut segments = 0usize;
        for (index, layer) in self.layers.iter().enumerate() {
            let expected_z = envelope.min[2] + (index + 1) as f64 * height;
            let lower_z = envelope.min[2] + index as f64 * height;
            if layer.index != index as u32
                || !layer.z_mm.is_finite()
                || !layer.section_z_mm.is_finite()
                || (layer.z_mm - expected_z).abs() > self.layer_quantization_mm
                || layer.z_mm <= lower_z
                || layer.z_mm > envelope.max[2]
                || layer.section_z_mm <= lower_z
                || layer.section_z_mm >= layer.z_mm
            {
                return Err(invalid(
                    "invalid fixed-height layer index, deposition height or interior section height",
                ));
            }
            if layer.paths.is_empty() {
                return Err(invalid("print layer has no deposition"));
            }
            for path in &layer.paths {
                if path.points_mm.len() < 2 {
                    return Err(invalid("deposition path requires at least two points"));
                }
                segments = segments
                    .checked_add(path.points_mm.len() - 1)
                    .ok_or_else(|| resource("segment count overflow"))?;
                if segments > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("deposition segment limit exceeded"));
                }
                if path.kind == DepositionKind::Perimeter
                    && path.points_mm.first() != path.points_mm.last()
                {
                    return Err(invalid("perimeter closure must be explicit"));
                }
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
        if segments == 0 {
            return Err(invalid("print plan has no deposition"));
        }
        Ok(())
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
    pub kernel: KernelIdentity,
    pub planner_version: String,
    pub emitter_version: String,
    pub verifier_version: String,
}
impl ManufacturingProvenance {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
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
}
impl VerificationReport {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.deposition_segments > MAX_DEPOSITION_SEGMENTS
            || self.travel_segments > MAX_DEPOSITION_SEGMENTS
        {
            return Err(resource("verification segment limit exceeded"));
        }
        if [
            self.deposited_volume_mm3,
            self.filament_length_mm,
            self.max_position_error_mm,
            self.max_extrusion_error_mm,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(invalid(
                "verification measurements must be finite and nonnegative",
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
    pub software_only: bool,
}
impl ManufacturingSummary {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.layers == 0
            || self.layers > MAX_PRINT_LAYERS
            || self.paths == 0
            || self.paths > self.deposition_segments
            || self.deposition_segments > MAX_DEPOSITION_SEGMENTS
        {
            return Err(invalid("invalid manufacturing summary counts"));
        }
        positive(&[self.deposited_volume_mm3, self.filament_length_mm])?;
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
        self.verification.validate()?;
        if self.program.is_empty() || self.program.len() > MAX_MANUFACTURING_BUNDLE_BYTES as usize {
            return Err(resource("manufacturing program size is invalid"));
        }
        let count: usize = self
            .plan
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .map(|p| p.points_mm.len() - 1)
            .sum();
        if self.verification.deposition_segments as usize != count {
            return Err(invalid("verification/plan segment count mismatch"));
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
            software_only: true,
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
) -> Result<SourceHash, ManufacturingError> {
    intent.validate()?;
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
    Inspect {
        session_id: SessionId,
    },
    ReadArtifactChunk {
        session_id: SessionId,
        hash: SourceHash,
        offset: u32,
        max_bytes: u32,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManufacturingResponse {
    Status {
        info: ProjectInfo,
        intent: Option<Arc<ManufacturingIntent>>,
        artifact: Option<ManufacturingArtifactRecord>,
    },
    JobAccepted {
        job_id: JobId,
    },
    Verified {
        record: ManufacturingArtifactRecord,
        report: VerificationReport,
    },
    ArtifactChunk {
        hash: SourceHash,
        offset: u32,
        total_bytes: u32,
        byte_count: u32,
    },
    Error {
        error: ManufacturingError,
    },
}

#[cfg(test)]
mod tests;
