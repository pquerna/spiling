// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Kernel-independent geometry identities, provenance and bounded display metadata.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

mod runtime;
pub use runtime::*;

pub const IMPORT_PROFILE: &str = "step-planar-cylindrical-v1";
pub const MESH_PROFILE: &str = "mesh-mm-0.05-v1";
pub const MAX_SOURCE_BYTES: u32 = 16 * 1024 * 1024;
pub const MAX_LIVE_SOURCE_BYTES: u32 = 64 * 1024 * 1024;
pub const MAX_STEP_ENTITIES: u32 = 100_000;
pub const MAX_NATIVE_FACES: u32 = 50_000;
pub const MAX_DEFINITIONS: u32 = 32;
pub const MAX_OCCURRENCES: u32 = 256;
pub const MAX_DISPLAY_VERTICES: u32 = 1_000_000;
pub const MAX_DISPLAY_TRIANGLES: u32 = 1_000_000;
pub const MAX_SCENE_MESH_BYTES: u32 = 64 * 1024 * 1024;
pub const MAX_SECTION_BYTES: u32 = 16 * 1024 * 1024;
pub const MAX_ENGINE_ARTIFACT_BYTES: u32 = 128 * 1024 * 1024;
pub const MAX_GEOMETRY_CHUNK_BYTES: u32 = 1024 * 1024;
pub const MAX_DISPLAY_BUFFER_BYTES: u32 = 128 * 1024 * 1024;
pub const MAX_LOOP_POINTS: u32 = 40_000;
pub const MAX_NATIVE_PATH_UNITS: u32 = 8_192;
pub const MAX_DISPLAY_LABEL_BYTES: u32 = 256;
pub const MAX_ERROR_MESSAGE_BYTES: u32 = 1_024;
pub const SCENE_PAGE_SIZE: u32 = 64;
pub const FACE_PAGE_SIZE: u32 = 256;
pub const ARTIFACT_PAGE_SIZE: u32 = 64;
pub const MAX_TERMINAL_JOBS: u32 = 64;
pub const JOB_POLL_INTERVAL_MS: u32 = 100;
pub const JOB_CANCEL_AFTER_MS: u32 = 60_000;
pub const JOB_CANCEL_GRACE_MS: u32 = 10_000;
pub const MAX_ACTIVE_GEOMETRY_JOBS: u32 = 1;
pub const MAX_OUTSTANDING_CHUNKS: u32 = 1;
pub const MESH_CARRIER_TOLERANCE_MM: f64 = 0.025;
pub const MESH_QUANTIZATION_TOLERANCE_MM: f64 = 0.025;
pub const MESH_SURFACE_TOLERANCE_MM: f64 = 0.05;
pub const SECTION_SAMPLING_TOLERANCE_MM: f64 = 0.005;
pub const SECTION_BOOLEAN_TOLERANCE_MM: f64 = 0.00001;
pub const SECTION_PLANE_TOLERANCE_MM: f64 = 0.0001;
pub const POSE_NORM_TOLERANCE: f64 = 1e-10;
pub const NORMAL_NORM_TOLERANCE: f64 = 1e-4;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GeometryErrorCode {
    InvalidGeometry,
    UnsupportedGeometry,
    UnsupportedUnits,
    SourceIo,
    SourceChanged,
    InvalidPose,
    StaleRevision,
    UnknownHandle,
    Busy,
    ResourceLimit,
    Cancelled,
    DegenerateSection,
    KernelFailure,
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {message}")]
#[serde(deny_unknown_fields)]
pub struct GeometryError {
    pub code: GeometryErrorCode,
    pub message: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGeometryError {
    code: GeometryErrorCode,
    message: String,
}
impl TryFrom<RawGeometryError> for GeometryError {
    type Error = &'static str;
    fn try_from(raw: RawGeometryError) -> Result<Self, Self::Error> {
        if raw.message.len() > MAX_ERROR_MESSAGE_BYTES as usize {
            return Err("geometry error message too long");
        }
        Ok(Self {
            code: raw.code,
            message: raw.message,
        })
    }
}
impl<'de> Deserialize<'de> for GeometryError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_checked::<D, RawGeometryError, Self>(deserializer)
    }
}
impl GeometryError {
    pub fn new(code: GeometryErrorCode, message: impl AsRef<str>) -> Self {
        Self {
            code,
            message: bounded_text(message.as_ref(), MAX_ERROR_MESSAGE_BYTES as usize),
        }
    }
}
pub(crate) fn invalid(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::InvalidGeometry, message)
}
pub(crate) fn resource(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}
pub fn bounded_text(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

macro_rules! hash_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[serde(transparent)]
        #[ts(type = "string")]
        pub struct $name(String);
        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, GeometryError> {
                let value = value.into();
                if !valid_hash(&value) {
                    return Err(invalid(concat!(
                        stringify!($name),
                        " must be lowercase SHA-256 hex"
                    )));
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = GeometryError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserialize_checked::<D, String, Self>(deserializer)
            }
        }
    };
}
hash_id!(DefinitionId);
hash_id!(SourceHash);
impl SourceHash {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(lower_hex(&Sha256::digest(bytes)))
    }
    pub fn from_digest(bytes: &[u8; 32]) -> Self {
        Self(lower_hex(bytes))
    }
    /// Hash borrowed bytes and compare directly with canonical hex, without allocating.
    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        Sha256::digest(bytes)
            .iter()
            .zip(self.0.as_bytes().as_chunks::<2>().0)
            .all(|(byte, pair)| {
                pair[0] == DIGITS[(byte >> 4) as usize] && pair[1] == DIGITS[(byte & 15) as usize]
            })
    }
}
impl DefinitionId {
    pub fn from_source_sha256(source: &[u8; 32]) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"spiling:definition:step-planar-cylindrical-v1\0");
        hash.update(source);
        Self(lower_hex(&hash.finalize()))
    }
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
#[ts(type = "string")]
pub struct SessionId(String);
impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }
    pub fn parse(value: impl Into<String>) -> Result<Self, GeometryError> {
        let value = value.into();
        let uuid = uuid::Uuid::parse_str(&value).map_err(|_| invalid("invalid session UUID"))?;
        let mut canonical = [0; 36];
        if uuid.hyphenated().encode_lower(&mut canonical).as_bytes() != value.as_bytes()
            || uuid.is_nil()
        {
            return Err(invalid("session UUID must be canonical and nonnil"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}
impl TryFrom<String> for SessionId {
    type Error = GeometryError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}
impl From<SessionId> for String {
    fn from(value: SessionId) -> Self {
        value.0
    }
}
impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_checked::<D, String, Self>(deserializer)
    }
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
#[ts(type = "string")]
pub struct FaceId(String);
impl FaceId {
    pub fn from_step_entity(entity: u64) -> Result<Self, GeometryError> {
        if entity == 0 {
            return Err(invalid("STEP face entity must be nonzero"));
        }
        Ok(Self(format!("step:{entity}")))
    }
    pub fn parse(value: impl Into<String>) -> Result<Self, GeometryError> {
        let value = value.into();
        let digits = value
            .strip_prefix("step:")
            .ok_or_else(|| invalid("invalid source face ID"))?;
        let entity = digits
            .parse::<u64>()
            .map_err(|_| invalid("invalid source face entity"))?;
        if entity == 0 || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid(
                "source face entity must be canonical nonzero decimal",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn step_entity(&self) -> u64 {
        self.0[5..].parse().expect("validated FaceId")
    }
}
impl TryFrom<String> for FaceId {
    type Error = GeometryError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}
impl From<FaceId> for String {
    fn from(value: FaceId) -> Self {
        value.0
    }
}
impl<'de> Deserialize<'de> for FaceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_checked::<D, String, Self>(deserializer)
    }
}

macro_rules! counter_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[serde(transparent)]
        #[ts(type = "number")]
        pub struct $name(u32);
        impl $name {
            pub fn new(value: u32) -> Result<Self, GeometryError> {
                if value == 0 {
                    Err(invalid(concat!(stringify!($name), " must be nonzero")))
                } else {
                    Ok(Self(value))
                }
            }
            pub fn get(self) -> u32 {
                self.0
            }
            pub fn next(self) -> Result<Self, GeometryError> {
                self.0
                    .checked_add(1)
                    .map(Self)
                    .ok_or_else(|| resource(concat!(stringify!($name), " exhausted")))
            }
        }
        impl TryFrom<u32> for $name {
            type Error = GeometryError;
            fn try_from(value: u32) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
        impl From<$name> for u32 {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserialize_checked::<D, u32, Self>(deserializer)
            }
        }
    };
}
counter_id!(OccurrenceId);
counter_id!(ArtifactId);

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
)]
#[serde(transparent)]
#[ts(type = "number")]
pub struct SceneRevision(pub u32);
impl SceneRevision {
    pub const ZERO: Self = Self(0);
    pub fn get(self) -> u32 {
        self.0
    }
    pub fn next(self) -> Result<Self, GeometryError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or_else(|| resource("scene revision exhausted"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AabbMm {
    pub min: [f64; 3],
    pub max: [f64; 3],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAabb {
    min: [f64; 3],
    max: [f64; 3],
}
impl TryFrom<RawAabb> for AabbMm {
    type Error = GeometryError;
    fn try_from(value: RawAabb) -> Result<Self, Self::Error> {
        Self::new(value.min, value.max)
    }
}
impl<'de> Deserialize<'de> for AabbMm {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_checked::<D, RawAabb, Self>(deserializer)
    }
}
impl AabbMm {
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Result<Self, GeometryError> {
        let result = Self { min, max };
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<(), GeometryError> {
        if (0..3).any(|axis| {
            !self.min[axis].is_finite()
                || !self.max[axis].is_finite()
                || self.min[axis] > self.max[axis]
        }) {
            return Err(invalid("bounds must be finite and ordered"));
        }
        Ok(())
    }
    pub fn include(&mut self, point: [f64; 3]) -> Result<(), GeometryError> {
        if !finite3(point) {
            return Err(invalid("nonfinite point"));
        }
        for (axis, value) in point.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(value);
            self.max[axis] = self.max[axis].max(value);
        }
        Ok(())
    }
}
pub(crate) fn finite3(value: [f64; 3]) -> bool {
    value.into_iter().all(f64::is_finite)
}
pub(crate) fn norm3(value: [f64; 3]) -> f64 {
    value[0].hypot(value[1]).hypot(value[2])
}

#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RigidPoseMm {
    pub translation_mm: [f64; 3],
    pub rotation_xyzw: [f64; 4],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPose {
    translation_mm: [f64; 3],
    rotation_xyzw: [f64; 4],
}
impl TryFrom<RawPose> for RigidPoseMm {
    type Error = GeometryError;
    fn try_from(raw: RawPose) -> Result<Self, Self::Error> {
        let pose = Self {
            translation_mm: raw.translation_mm,
            rotation_xyzw: raw.rotation_xyzw,
        };
        pose.validate()?;
        Ok(pose)
    }
}
impl<'de> Deserialize<'de> for RigidPoseMm {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_checked::<D, RawPose, Self>(deserializer)
    }
}
impl RigidPoseMm {
    pub const IDENTITY: Self = Self {
        translation_mm: [0.0; 3],
        rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
    };
    pub fn validate(&self) -> Result<(), GeometryError> {
        let norm = self
            .rotation_xyzw
            .iter()
            .fold(0.0_f64, |sum, value| sum.hypot(*value));
        if !finite3(self.translation_mm)
            || !norm.is_finite()
            || (norm - 1.0).abs() > POSE_NORM_TOLERANCE
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidPose,
                "pose requires finite translation and a unit quaternion",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlaneMm {
    pub origin_mm: [f64; 3],
    pub normal: [f64; 3],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlane {
    origin_mm: [f64; 3],
    normal: [f64; 3],
}
impl TryFrom<RawPlane> for PlaneMm {
    type Error = GeometryError;
    fn try_from(raw: RawPlane) -> Result<Self, Self::Error> {
        let plane = Self {
            origin_mm: raw.origin_mm,
            normal: raw.normal,
        };
        plane.validate()?;
        Ok(plane)
    }
}
impl<'de> Deserialize<'de> for PlaneMm {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_checked::<D, RawPlane, Self>(deserializer)
    }
}
impl PlaneMm {
    pub fn validate(&self) -> Result<(), GeometryError> {
        if !finite3(self.origin_mm)
            || !finite3(self.normal)
            || self.normal.iter().all(|value| *value == 0.0)
        {
            return Err(invalid(
                "plane requires finite origin and nonzero finite normal",
            ));
        }
        Ok(())
    }
    pub fn normalized(self) -> Result<Self, GeometryError> {
        self.validate()?;
        // Scaling first handles both overflowing finite lengths and subnormal normals.
        let scale = self
            .normal
            .iter()
            .fold(0.0_f64, |largest, value| largest.max(value.abs()));
        let scaled = self.normal.map(|value| value / scale);
        let norm = norm3(scaled);
        Ok(Self {
            origin_mm: self.origin_mm,
            normal: scaled.map(|value| canonical_zero(value / norm)),
        })
    }
}
fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlaneFrameMm {
    pub origin_mm: [f64; 3],
    pub x_axis: [f64; 3],
    pub y_axis: [f64; 3],
    pub z_axis: [f64; 3],
}
impl PlaneFrameMm {
    pub fn from_plane(plane: PlaneMm) -> Result<Self, GeometryError> {
        let plane = plane.normalized()?;
        let z = plane.normal;
        let mut least = 0;
        for axis in 1..3 {
            if z[axis].abs() < z[least].abs() {
                least = axis;
            }
        }
        let mut axis = [0.0; 3];
        axis[least] = 1.0;
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let x = cross(axis, z);
        let norm = norm3(x);
        let x = x.map(|v| canonical_zero(v / norm));
        Ok(Self {
            origin_mm: plane.origin_mm,
            x_axis: x,
            y_axis: cross(z, x).map(canonical_zero),
            z_axis: z,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
pub enum SourceUnit {
    #[serde(rename = "millimetre")]
    Millimetre,
    #[serde(rename = "metre")]
    Metre,
    #[serde(rename = "inch")]
    Inch,
}
impl SourceUnit {
    pub fn scale_to_mm(self) -> f64 {
        match self {
            Self::Millimetre => 1.0,
            Self::Metre => 1000.0,
            Self::Inch => 25.4,
        }
    }
}
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenance {
    pub source_hash: SourceHash,
    pub source_name: String,
    pub source_unit: SourceUnit,
    /// Declared source uncertainty, normalized to mm; not a program-created accuracy guarantee.
    pub uncertainty_mm: Option<f64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvenance {
    source_hash: SourceHash,
    source_name: String,
    source_unit: SourceUnit,
    uncertainty_mm: Option<f64>,
}
impl TryFrom<RawProvenance> for SourceProvenance {
    type Error = GeometryError;
    fn try_from(raw: RawProvenance) -> Result<Self, Self::Error> {
        let value = Self {
            source_hash: raw.source_hash,
            source_name: raw.source_name,
            source_unit: raw.source_unit,
            uncertainty_mm: raw.uncertainty_mm,
        };
        value.validate()?;
        Ok(value)
    }
}
impl<'de> Deserialize<'de> for SourceProvenance {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_checked::<D, RawProvenance, Self>(deserializer)
    }
}
impl SourceProvenance {
    pub fn validate(&self) -> Result<(), GeometryError> {
        if self.source_name.len() > MAX_DISPLAY_LABEL_BYTES as usize
            || self
                .uncertainty_mm
                .is_some_and(|v| !v.is_finite() || v < 0.0)
        {
            return Err(invalid("invalid source provenance"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
pub enum DisplayProfile {
    #[serde(rename = "mesh-mm-0.05-v1")]
    MeshMm005V1,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FaceRef {
    pub session_id: SessionId,
    pub scene_revision: SceneRevision,
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub face_id: FaceId,
}
impl FaceRef {
    /// Validate against live records and the definition's authoritative source-face table.
    pub fn validate_current(
        &self,
        scene: &SceneSummary,
        occurrence: &OccurrenceRecord,
        definition: &DefinitionRecord,
        faces: &[FaceIndexRow],
    ) -> Result<(), GeometryError> {
        if self.session_id != scene.session_id || self.scene_revision != scene.revision {
            return Err(GeometryError::new(
                GeometryErrorCode::StaleRevision,
                "face reference session or revision is stale",
            ));
        }
        if self.occurrence_id != occurrence.occurrence_id
            || self.definition_id != occurrence.definition_id
            || self.definition_id != definition.definition_id
            || !faces.iter().any(|row| row.face_id == self.face_id)
        {
            return Err(GeometryError::new(
                GeometryErrorCode::UnknownHandle,
                "face reference is not in the live occurrence",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FaceCarrier {
    Plane {
        origin_mm: [f64; 3],
        normal: [f64; 3],
    },
    Cylinder {
        axis_origin_mm: [f64; 3],
        axis_direction: [f64; 3],
        radius_mm: f64,
    },
}
impl FaceCarrier {
    pub fn validate(&self) -> Result<(), GeometryError> {
        let (origin, direction) = match self {
            Self::Plane { origin_mm, normal } => (*origin_mm, *normal),
            Self::Cylinder {
                axis_origin_mm,
                axis_direction,
                radius_mm,
            } => {
                if !radius_mm.is_finite() || *radius_mm <= 0.0 {
                    return Err(invalid("invalid cylinder radius"));
                }
                (*axis_origin_mm, *axis_direction)
            }
        };
        if !finite3(origin)
            || !finite3(direction)
            || (norm3(direction) - 1.0).abs() > POSE_NORM_TOLERANCE
        {
            return Err(invalid("invalid native face carrier"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FaceInfo {
    pub face_id: FaceId,
    /// Decimal string preserves STEP entity IDs above JavaScript's exact integer range.
    pub surface_face_entity: String,
    pub source_entity_kind: String,
    pub orientation: bool,
    pub carrier: FaceCarrier,
}
impl FaceInfo {
    pub fn validate(&self) -> Result<(), GeometryError> {
        let entity = self
            .surface_face_entity
            .parse::<u64>()
            .map_err(|_| invalid("invalid surface-face source entity"))?;
        if entity == 0
            || self.surface_face_entity.starts_with('0')
            || !self.surface_face_entity.bytes().all(|b| b.is_ascii_digit())
            || self.source_entity_kind.is_empty()
            || self.source_entity_kind.len() > MAX_DISPLAY_LABEL_BYTES as usize
        {
            return Err(invalid("invalid face source metadata"));
        }
        self.carrier.validate()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FaceInspection {
    pub reference: FaceRef,
    pub face: FaceInfo,
    pub pose: RigidPoseMm,
    pub provenance: SourceProvenance,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FaceIndexRow {
    pub ordinal: u32,
    pub face_id: FaceId,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SceneSummary {
    pub session_id: SessionId,
    pub revision: SceneRevision,
    pub definition_count: u32,
    pub occurrence_count: u32,
    pub bounds_mm: Option<AabbMm>,
    pub unique_mesh_bytes: u32,
}
impl SceneSummary {
    pub fn validate(&self) -> Result<(), GeometryError> {
        if self.definition_count > MAX_DEFINITIONS
            || self.occurrence_count > MAX_OCCURRENCES
            || self.unique_mesh_bytes > MAX_SCENE_MESH_BYTES
        {
            return Err(resource("scene exceeds support limits"));
        }
        if self.definition_count > self.occurrence_count
            || (self.definition_count == 0) != (self.occurrence_count == 0)
            || (self.occurrence_count == 0) != self.bounds_mm.is_none()
            || (self.occurrence_count == 0) != (self.unique_mesh_bytes == 0)
        {
            return Err(invalid("inconsistent live scene summary"));
        }
        if let Some(bounds) = self.bounds_mm {
            bounds.validate()?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DefinitionRecord {
    pub definition_id: DefinitionId,
    pub provenance: SourceProvenance,
    pub face_count: u32,
    pub bounds_mm: AabbMm,
    pub mesh_artifact_id: ArtifactId,
}
impl DefinitionRecord {
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.provenance.validate()?;
        self.bounds_mm.validate()?;
        if self.face_count == 0 || self.face_count > MAX_NATIVE_FACES {
            return Err(resource("definition face count exceeds support limits"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OccurrenceRecord {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub pose: RigidPoseMm,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativePath {
    UnixBytes { hex: String },
    WindowsWide { units: Vec<u16> },
}
impl NativePath {
    pub fn validate(&self) -> Result<(), GeometryError> {
        let invalid_path = || {
            GeometryError::new(
                GeometryErrorCode::SourceIo,
                "native path is invalid or for a different host",
            )
        };
        match self {
            Self::UnixBytes { hex } => {
                if !cfg!(unix) || hex.is_empty() || hex.len() % 2 != 0 {
                    return Err(invalid_path());
                }
                if hex.len() / 2 > MAX_NATIVE_PATH_UNITS as usize {
                    return Err(resource("native path exceeds limit"));
                }
                for pair in hex.as_bytes().as_chunks::<2>().0 {
                    let digit = |b: u8| (b as char).to_digit(16);
                    let byte = digit(pair[0])
                        .zip(digit(pair[1]))
                        .map(|(a, b)| a * 16 + b)
                        .ok_or_else(invalid_path)?;
                    if byte == 0 {
                        return Err(invalid_path());
                    }
                }
            }
            Self::WindowsWide { units } => {
                if !cfg!(windows) || units.is_empty() || units.contains(&0) {
                    return Err(invalid_path());
                }
                if units.len() > MAX_NATIVE_PATH_UNITS as usize {
                    return Err(resource("native path exceeds limit"));
                }
            }
        }
        // Hex needs two bytes per Unix byte; decimal UTF-16 units need at most
        // five digits plus a separator. Both supported caps fit bounded control JSON.
        let max_json_bytes = match self {
            Self::UnixBytes { hex } => hex.len() + 64,
            Self::WindowsWide { units } => units.len() * 6 + 64,
        };
        if max_json_bytes > crate::MAX_CONTROL_BYTES as usize {
            return Err(resource("native path exceeds control limit"));
        }
        Ok(())
    }
    pub fn from_os_str(path: &std::ffi::OsStr) -> Result<Self, GeometryError> {
        #[cfg(unix)]
        let result = {
            use std::os::unix::ffi::OsStrExt;
            let bytes = path.as_bytes();
            if bytes.len() > MAX_NATIVE_PATH_UNITS as usize {
                return Err(resource("native path exceeds limit"));
            }
            Self::UnixBytes {
                hex: lower_hex(bytes),
            }
        };
        #[cfg(windows)]
        let result = {
            use std::os::windows::ffi::OsStrExt;
            Self::WindowsWide {
                units: path
                    .encode_wide()
                    .take(MAX_NATIVE_PATH_UNITS as usize + 1)
                    .collect(),
            }
        };
        #[cfg(not(any(unix, windows)))]
        return Err(GeometryError::new(
            GeometryErrorCode::SourceIo,
            "unsupported native host",
        ));
        #[cfg(any(unix, windows))]
        {
            result.validate()?;
            Ok(result)
        }
    }
    pub fn to_os_string(&self) -> Result<std::ffi::OsString, GeometryError> {
        self.validate()?;
        match self {
            #[cfg(unix)]
            Self::UnixBytes { hex } => {
                use std::os::unix::ffi::OsStringExt;
                let bytes = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("validated hex"))
                    .collect();
                Ok(std::ffi::OsString::from_vec(bytes))
            }
            #[cfg(windows)]
            Self::WindowsWide { units } => {
                use std::os::windows::ffi::OsStringExt;
                Ok(std::ffi::OsString::from_wide(units))
            }
            #[allow(unreachable_patterns)]
            _ => Err(GeometryError::new(
                GeometryErrorCode::SourceIo,
                "native path host mismatch",
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MeshChunkMetadata {
    pub session_id: SessionId,
    pub artifact_id: ArtifactId,
    pub definition_id: DefinitionId,
    pub schema_version: u16,
    pub mesh_profile: DisplayProfile,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub byte_count: u32,
    pub sha256: SourceHash,
    pub face_count: u32,
    pub bounds_mm: AabbMm,
    pub carrier_deviation_mm: f64,
    pub quantization_error_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SectionChunkMetadata {
    pub session_id: SessionId,
    pub artifact_id: ArtifactId,
    pub schema_version: u16,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub byte_count: u32,
    pub sha256: SourceHash,
    pub first_loop_ordinal: u32,
    pub loop_count: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SectionLoopMetadata {
    pub ordinal: u32,
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub is_hole: bool,
    pub point_count: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SectionSummary {
    pub session_id: SessionId,
    pub artifact_id: ArtifactId,
    pub revision: SceneRevision,
    pub schema_version: u16,
    pub plane: PlaneMm,
    pub frame: PlaneFrameMm,
    pub total_loop_count: u32,
    pub chunk_count: u32,
    pub total_bytes: u32,
    pub sampling_tolerance_mm: f64,
    pub boolean_tolerance_mm: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GeometryLimits {
    pub source_bytes: u32,
    pub live_source_bytes: u32,
    pub step_entities: u32,
    pub native_faces: u32,
    pub definitions: u32,
    pub occurrences: u32,
    pub display_vertices: u32,
    pub display_triangles: u32,
    pub scene_mesh_bytes: u32,
    pub section_bytes: u32,
    pub engine_artifact_bytes: u32,
    pub geometry_chunk_bytes: u32,
    pub display_buffer_bytes: u32,
    pub loop_points: u32,
    pub active_jobs: u32,
    pub outstanding_chunks: u32,
}
impl GeometryLimits {
    pub const FROZEN: Self = Self {
        source_bytes: MAX_SOURCE_BYTES,
        live_source_bytes: MAX_LIVE_SOURCE_BYTES,
        step_entities: MAX_STEP_ENTITIES,
        native_faces: MAX_NATIVE_FACES,
        definitions: MAX_DEFINITIONS,
        occurrences: MAX_OCCURRENCES,
        display_vertices: MAX_DISPLAY_VERTICES,
        display_triangles: MAX_DISPLAY_TRIANGLES,
        scene_mesh_bytes: MAX_SCENE_MESH_BYTES,
        section_bytes: MAX_SECTION_BYTES,
        engine_artifact_bytes: MAX_ENGINE_ARTIFACT_BYTES,
        geometry_chunk_bytes: MAX_GEOMETRY_CHUNK_BYTES,
        display_buffer_bytes: MAX_DISPLAY_BUFFER_BYTES,
        loop_points: MAX_LOOP_POINTS,
        active_jobs: MAX_ACTIVE_GEOMETRY_JOBS,
        outstanding_chunks: MAX_OUTSTANDING_CHUNKS,
    };
}

#[cfg(test)]
mod tests;

fn deserialize_checked<'de, D, Raw, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    Raw: Deserialize<'de>,
    T: TryFrom<Raw>,
    T::Error: std::fmt::Display,
{
    T::try_from(Raw::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}
