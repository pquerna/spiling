// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded software-only manufacturing data. Profile declarations certify no machine.
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
pub use process::*;
mod resolution;
pub use resolution::*;

pub const MANUFACTURING_SCHEMA_VERSION: u32 = 5;
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
    HeightMm,
    SectionZMm,
    Paths,
    RegionCoverage,
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
                "height_mm",
                "section_z_mm",
                "paths",
                "region_coverage",
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
        let mut height_mm = None;
        let mut section_z_mm = None;
        let mut paths = None;
        let mut region_coverage = None;
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
            }
        }
        Ok(PrintLayer {
            index: index.ok_or_else(|| serde::de::Error::missing_field("index"))?,
            z_mm: z_mm.ok_or_else(|| serde::de::Error::missing_field("z_mm"))?,
            height_mm: height_mm.ok_or_else(|| serde::de::Error::missing_field("height_mm"))?,
            section_z_mm: section_z_mm
                .ok_or_else(|| serde::de::Error::missing_field("section_z_mm"))?,
            paths: paths.ok_or_else(|| serde::de::Error::missing_field("paths"))?,
            region_coverage: region_coverage
                .ok_or_else(|| serde::de::Error::missing_field("region_coverage"))?,
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
    Marlin2SingleExtruderGcodeV1,
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
        if self.extruders != 1 {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::UnsupportedCapability,
                "only one extruder is implemented",
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
    pub workflow: JobWorkflow,
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
        match (&self.workflow, self.capabilities.output_dialect) {
            (JobWorkflow::DiagnosticBodyOnly, OutputDialect::CartesianAbsoluteGcodeV1) => {}
            (JobWorkflow::CompleteMarlin { job }, OutputDialect::Marlin2SingleExtruderGcodeV1) => {
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
    pub top_skin_thickness_mm: f64,
    pub bottom_skin_thickness_mm: f64,
    pub print_speed_mm_s: f64,
    pub travel_speed_mm_s: f64,
    pub flow_multiplier: f64,
    pub process: ProcessPolicy,
    pub supports: SupportPolicy,
}
impl PlanarPrintRecipe {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        label(&self.id)?;
        label(&self.revision)?;
        label(&self.material_id)?;
        label(&self.material_revision)?;
        self.process.validate()?;
        self.supports.validate()?;
        positive(&[
            self.filament_diameter_mm,
            self.layer_height_mm,
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
    pub requested_infill_fraction: f64,
}
impl PlanarRegionCoverage {
    pub fn validate(&self, recipe: &PlanarPrintRecipe) -> Result<(), ManufacturingError> {
        let values = [
            self.section_area_mm2,
            self.source_section_area_mm2,
            self.surface_added_area_mm2,
            self.wall_area_mm2,
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
            + self.sparse_interior_area_mm2;
        let tolerance = 1e-7 * self.section_area_mm2.max(1.0);
        if values.iter().any(|v| !v.is_finite() || *v < 0.0)
            || (self.section_area_mm2 - self.source_section_area_mm2 - self.surface_added_area_mm2)
                .abs()
                > tolerance
            || (self.section_area_mm2 == 0.0
                && (self.nominal_deposition_volume_mm3 != 0.0
                    || self.source_section_area_mm2 != 0.0
                    || self.surface_added_area_mm2 != 0.0
                    || partition != 0.0
                    || self.protected_hole_area_mm2 != 0.0
                    || self.intentional_void_area_mm2 != 0.0))
            || (self.support_area_mm2 == 0.0) != (self.support_nominal_deposition_volume_mm3 == 0.0)
            || (matches!(recipe.supports, SupportPolicy::Disabled)
                && (self.support_area_mm2 != 0.0
                    || self.support_nominal_deposition_volume_mm3 != 0.0))
            || self.requested_infill_fraction != recipe.infill_fraction
            || !partition.is_finite()
            || (partition - self.section_area_mm2).abs() > tolerance
            || (self.intentional_void_area_mm2
                - self.sparse_interior_area_mm2 * (1.0 - recipe.infill_fraction))
                .abs()
                > tolerance
            || (recipe.infill_fraction == 1.0 && self.sparse_interior_area_mm2 != 0.0)
            || (recipe.infill_fraction < 1.0 && self.solid_interior_area_mm2 != 0.0)
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
    pub height_mm: f64,
    pub section_z_mm: f64,
    #[serde(deserialize_with = "paths")]
    pub paths: Vec<DepositionPath>,
    pub region_coverage: PlanarRegionCoverage,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedPrintPlan {
    pub schema_version: u32,
    #[serde(deserialize_with = "layers")]
    pub layers: Vec<PrintLayer>,
    pub sampling_tolerance_mm: f64,
    pub surface_tolerance_mm: f64,
    pub offset_tolerance_mm: f64,
    pub layer_quantization_mm: f64,
    #[serde(deserialize_with = "resolution::records")]
    pub surface_resolution: Vec<SurfaceResolution>,
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
        let envelope = &intent.printer.build_envelope;
        let mut lower_z = envelope.min[2];
        let mut segments = 0usize;
        let mut object_segments = 0usize;
        let complete = matches!(intent.printer.workflow, JobWorkflow::CompleteMarlin { .. });
        let mut object_xy = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
        let mut gap_height = 0.0;
        for (index, layer) in self.layers.iter().enumerate() {
            layer.region_coverage.validate(&intent.recipe)?;
            let height = layer.height_mm;
            if layer.index != index as u32
                || !layer.z_mm.is_finite()
                || !layer.section_z_mm.is_finite()
                || !height.is_finite()
                || height <= 0.0
                || height > intent.recipe.layer_height_mm + self.layer_quantization_mm
                || height > intent.recipe.bead_width_mm
                || (layer.z_mm - lower_z - height).abs() > 1e-9 * height.max(1.0)
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
            if layer.paths.is_empty() {
                let SupportPolicy::Breakaway { z_gap_mm, .. } = intent.recipe.supports else {
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
                    || (gap_height == 0.0
                        && !self.layers[index - 1]
                            .paths
                            .iter()
                            .any(|p| p.kind == DepositionKind::SupportInterface))
                {
                    return Err(invalid(
                        "contact gap requires bed-anchored support and preceding support interface",
                    ));
                }
                gap_height += layer.height_mm;
                if gap_height > z_gap_mm + self.layer_quantization_mm {
                    return Err(invalid("empty contact gap exceeds declared physical Z gap"));
                }
            } else if gap_height > 0.0 {
                if !layer.paths.iter().any(|p| {
                    !matches!(
                        p.kind,
                        DepositionKind::Support | DepositionKind::SupportInterface
                    )
                }) {
                    return Err(invalid("contact gap must end at actual object deposition"));
                }
                gap_height = 0.0;
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
            let rounded_start = job
                .prime_start_mm
                .map(|value| (value * 1_000_000.0).round() / 1_000_000.0);
            let rounded_end = job
                .prime_end_mm
                .map(|value| (value * 1_000_000.0).round() / 1_000_000.0);
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
    pub lifecycle_motion_count: u32,
    pub priming_filament_length_mm: f64,
    pub support_deposition_segments: u32,
    pub support_deposited_volume_mm3: f64,
    pub support_filament_length_mm: f64,
    pub retraction_count: u32,
    pub retracted_filament_mm: f64,
}
impl VerificationReport {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self
            .deposition_segments
            .checked_add(self.support_deposition_segments)
            .is_none_or(|n| n > MAX_DEPOSITION_SEGMENTS)
            || self.retraction_count > 50_000
            || self.travel_segments > MAX_DEPOSITION_SEGMENTS
        {
            return Err(resource("verification segment limit exceeded"));
        }
        if (self.lifecycle_motion_count != 0 && self.lifecycle_motion_count != 4)
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
}
impl ManufacturingSummary {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        self.surface_resolution.validate()?;
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
        resolution::validate_attribution(
            &self.plan.surface_resolution,
            &self.provenance.surface_overlays,
            &self.provenance.occurrences,
        )?;
        self.verification.validate()?;
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
            software_only: true,
            surface_resolution: SurfaceResolutionTotals::from_records(
                &self.plan.surface_resolution,
            ),
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
