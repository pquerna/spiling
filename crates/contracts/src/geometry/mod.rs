// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Kernel-independent geometry identities, provenance and bounded display metadata.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

mod runtime;
pub use runtime::*;

pub const IMPORT_PROFILE: &str = "step-native-trimmed-v2";
pub const MESH_PROFILE: &str = "mesh-trimmed-mm-0.05-v2";
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
pub const JOB_CANCEL_AFTER_MS: u32 = 300_000;
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
    ConditioningFailure,
    AnalysisIndeterminate,
    SingularGeometry,
    UnsupportedConversion,
    UnsupportedOperation,
    NativeIntersectionFailure,
    ParameterCorrespondenceUnavailable,
    ShapeErrorExceeded,
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
    pub fn validate(&self) -> Result<(), GeometryError> {
        if self.message.len() > MAX_ERROR_MESSAGE_BYTES as usize {
            return Err(invalid("geometry error message exceeds limit"));
        }
        Ok(())
    }
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
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.0.capacity()
    }
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
    pub fn allocated_bytes(&self) -> usize {
        self.0.capacity()
    }
    pub fn from_source_sha256(source: &[u8; 32]) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"spiling:definition:");
        hash.update(IMPORT_PROFILE.as_bytes());
        hash.update(b"\0");
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
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.0.capacity()
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
    pub fn allocated_bytes(&self) -> usize {
        self.0.capacity()
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
    pub fn validate(&self) -> Result<(), GeometryError> {
        let axes = [self.x_axis, self.y_axis, self.z_axis];
        if !finite3(self.origin_mm)
            || axes
                .iter()
                .any(|axis| !finite3(*axis) || (norm3(*axis) - 1.0).abs() > POSE_NORM_TOLERANCE)
        {
            return Err(invalid("invalid surface frame"));
        }
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let cross = [
            self.x_axis[1] * self.y_axis[2] - self.x_axis[2] * self.y_axis[1],
            self.x_axis[2] * self.y_axis[0] - self.x_axis[0] * self.y_axis[2],
            self.x_axis[0] * self.y_axis[1] - self.x_axis[1] * self.y_axis[0],
        ];
        if dot(self.x_axis, self.y_axis).abs() > POSE_NORM_TOLERANCE
            || dot(self.x_axis, self.z_axis).abs() > POSE_NORM_TOLERANCE
            || dot(self.y_axis, self.z_axis).abs() > POSE_NORM_TOLERANCE
            || cross
                .into_iter()
                .zip(self.z_axis)
                .any(|(a, b)| (a - b).abs() > POSE_NORM_TOLERANCE)
        {
            return Err(invalid(
                "surface frame must be orthonormal and right-handed",
            ));
        }
        Ok(())
    }
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
    #[serde(rename = "mesh-trimmed-mm-0.05-v2")]
    MeshTrimmedMm005V2,
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceFamily {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    BSpline,
    Nurbs,
    LinearExtrusion,
    Revolution,
    TSpline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TorusKind {
    RegularRing,
    Horn,
    Spindle,
}

/// Definition-local millimetres. These descriptions never authorize an operation.
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalyticSurfaceDescription {
    Plane {
        origin_mm: [f64; 3],
        normal: [f64; 3],
    },
    Cylinder {
        axis_origin_mm: [f64; 3],
        axis_direction: [f64; 3],
        radius_mm: f64,
    },
    /// At axial distance z, radius = reference_radius_mm + z * tan(semi_angle_radians).
    /// The signed angle is nonzero and strictly between -pi/2 and pi/2.
    Cone {
        frame: PlaneFrameMm,
        reference_radius_mm: f64,
        semi_angle_radians: f64,
    },
    /// The frame origin is the center; regular chart normals do not exist at the poles.
    Sphere { frame: PlaneFrameMm, radius_mm: f64 },
    Torus {
        frame: PlaneFrameMm,
        major_radius_mm: f64,
        minor_radius_mm: f64,
        torus_kind: TorusKind,
    },
}
#[derive(Deserialize)]
#[serde(
    remote = "AnalyticSurfaceDescription",
    tag = "kind",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum RawAnalyticSurfaceDescription {
    Plane {
        origin_mm: [f64; 3],
        normal: [f64; 3],
    },
    Cylinder {
        axis_origin_mm: [f64; 3],
        axis_direction: [f64; 3],
        radius_mm: f64,
    },
    Cone {
        frame: PlaneFrameMm,
        reference_radius_mm: f64,
        semi_angle_radians: f64,
    },
    Sphere {
        frame: PlaneFrameMm,
        radius_mm: f64,
    },
    Torus {
        frame: PlaneFrameMm,
        major_radius_mm: f64,
        minor_radius_mm: f64,
        torus_kind: TorusKind,
    },
}
impl<'de> Deserialize<'de> for AnalyticSurfaceDescription {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawAnalyticSurfaceDescription::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl AnalyticSurfaceDescription {
    pub fn family(&self) -> SurfaceFamily {
        match self {
            Self::Plane { .. } => SurfaceFamily::Plane,
            Self::Cylinder { .. } => SurfaceFamily::Cylinder,
            Self::Cone { .. } => SurfaceFamily::Cone,
            Self::Sphere { .. } => SurfaceFamily::Sphere,
            Self::Torus { .. } => SurfaceFamily::Torus,
        }
    }
    pub fn validate(&self) -> Result<(), GeometryError> {
        let positive = |r: f64| r.is_finite() && r > 0.0;
        let axis = |origin: [f64; 3], direction: [f64; 3]| {
            finite3(origin)
                && finite3(direction)
                && (norm3(direction) - 1.0).abs() <= POSE_NORM_TOLERANCE
        };
        let valid = match self {
            Self::Plane { origin_mm, normal } => axis(*origin_mm, *normal),
            Self::Cylinder {
                axis_origin_mm,
                axis_direction,
                radius_mm,
            } => axis(*axis_origin_mm, *axis_direction) && positive(*radius_mm),
            Self::Cone {
                frame,
                reference_radius_mm,
                semi_angle_radians,
            } => {
                frame.validate().is_ok()
                    && positive(*reference_radius_mm)
                    && semi_angle_radians.is_finite()
                    && *semi_angle_radians != 0.0
                    && semi_angle_radians.abs() < std::f64::consts::FRAC_PI_2
            }
            Self::Sphere { frame, radius_mm } => frame.validate().is_ok() && positive(*radius_mm),
            Self::Torus {
                frame,
                major_radius_mm,
                minor_radius_mm,
                torus_kind,
            } => {
                frame.validate().is_ok()
                    && positive(*major_radius_mm)
                    && positive(*minor_radius_mm)
                    && match torus_kind {
                        TorusKind::RegularRing => major_radius_mm > minor_radius_mm,
                        TorusKind::Horn => major_radius_mm == minor_radius_mm,
                        TorusKind::Spindle => major_radius_mm < minor_radius_mm,
                    }
            }
        };
        if !valid {
            return Err(invalid("invalid analytic surface description"));
        }
        Ok(())
    }
}

/// Descriptive trim topology only, not a UV polygon or an invertibility certificate.
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FaceDomainSummary {
    pub trim_loop_count: u32,
    pub inner_loop_count: u32,
    pub periodic_u: bool,
    pub periodic_v: bool,
    pub has_singular_boundary: bool,
}
#[derive(Deserialize)]
#[serde(remote = "FaceDomainSummary", deny_unknown_fields)]
struct RawFaceDomainSummary {
    trim_loop_count: u32,
    inner_loop_count: u32,
    periodic_u: bool,
    periodic_v: bool,
    has_singular_boundary: bool,
}
impl<'de> Deserialize<'de> for FaceDomainSummary {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawFaceDomainSummary::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl FaceDomainSummary {
    pub fn new(
        trim_loop_count: u32,
        inner_loop_count: u32,
        periodic_u: bool,
        periodic_v: bool,
        has_singular_boundary: bool,
    ) -> Result<Self, GeometryError> {
        let value = Self {
            trim_loop_count,
            inner_loop_count,
            periodic_u,
            periodic_v,
            has_singular_boundary,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), GeometryError> {
        if self.trim_loop_count > 2 * MAX_STEP_ENTITIES {
            return Err(resource("face trim loop count exceeds limit"));
        }
        if (self.trim_loop_count == 0 && self.inner_loop_count != 0)
            || (self.trim_loop_count != 0 && self.inner_loop_count >= self.trim_loop_count)
        {
            return Err(invalid("invalid face inner trim loop count"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FaceGeometryInfo {
    pub family: SurfaceFamily,
    pub analytic: Option<AnalyticSurfaceDescription>,
    pub domain: FaceDomainSummary,
}
#[derive(Deserialize)]
#[serde(remote = "FaceGeometryInfo", deny_unknown_fields)]
struct RawFaceGeometryInfo {
    family: SurfaceFamily,
    analytic: Option<AnalyticSurfaceDescription>,
    domain: FaceDomainSummary,
}
impl<'de> Deserialize<'de> for FaceGeometryInfo {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawFaceGeometryInfo::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl FaceGeometryInfo {
    pub fn new(
        family: SurfaceFamily,
        analytic: Option<AnalyticSurfaceDescription>,
        domain: FaceDomainSummary,
    ) -> Result<Self, GeometryError> {
        let value = Self {
            family,
            analytic,
            domain,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.domain.validate()?;
        if let Some(analytic) = &self.analytic {
            analytic.validate()?;
            // Recognized analytic metadata on a spline/sweep remains descriptive.
            if matches!(
                self.family,
                SurfaceFamily::Plane
                    | SurfaceFamily::Cylinder
                    | SurfaceFamily::Cone
                    | SurfaceFamily::Sphere
                    | SurfaceFamily::Torus
            ) && self.family != analytic.family()
            {
                return Err(invalid(
                    "analytic description contradicts native surface family",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FaceInfo {
    pub face_id: FaceId,
    /// Decimal string preserves STEP entity IDs above JavaScript's exact integer range.
    pub surface_face_entity: String,
    pub source_entity_kind: String,
    pub source_surface_entity_kind: String,
    /// Material-side direction relative to the regular analytic/native frame.
    /// At singular points, operations require an oriented limiting chart/fan.
    pub orientation: bool,
    pub geometry: FaceGeometryInfo,
}
#[derive(Deserialize)]
#[serde(remote = "FaceInfo", deny_unknown_fields)]
struct RawFaceInfo {
    face_id: FaceId,
    surface_face_entity: String,
    source_entity_kind: String,
    source_surface_entity_kind: String,
    orientation: bool,
    geometry: FaceGeometryInfo,
}
impl<'de> Deserialize<'de> for FaceInfo {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawFaceInfo::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl FaceInfo {
    pub fn validate(&self) -> Result<(), GeometryError> {
        let entity = self
            .surface_face_entity
            .parse::<u64>()
            .map_err(|_| invalid("invalid surface-face source entity"))?;
        let valid_kind = |kind: &str| {
            !kind.is_empty()
                && kind.len() <= MAX_DISPLAY_LABEL_BYTES as usize
                && kind
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        };
        if entity == 0
            || self.surface_face_entity.starts_with('0')
            || !self.surface_face_entity.bytes().all(|b| b.is_ascii_digit())
            || !valid_kind(&self.source_entity_kind)
            || !valid_kind(&self.source_surface_entity_kind)
        {
            return Err(invalid("invalid face source metadata"));
        }
        self.geometry.validate()
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
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SceneSummary {
    pub session_id: SessionId,
    pub revision: SceneRevision,
    pub definition_count: u32,
    pub occurrence_count: u32,
    pub bounds_mm: Option<AabbMm>,
    pub unique_mesh_bytes: u32,
}
#[derive(Deserialize)]
#[serde(remote = "SceneSummary", deny_unknown_fields)]
struct RawSceneSummary {
    session_id: SessionId,
    revision: SceneRevision,
    definition_count: u32,
    occurrence_count: u32,
    bounds_mm: Option<AabbMm>,
    unique_mesh_bytes: u32,
}
impl<'de> Deserialize<'de> for SceneSummary {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawSceneSummary::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
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
            || (self.occurrence_count == 0 && self.unique_mesh_bytes != 0)
        {
            return Err(invalid("inconsistent live scene summary"));
        }
        if let Some(bounds) = self.bounds_mm {
            bounds.validate()?;
        }
        Ok(())
    }
}
/// Display availability never changes source/native identity or face inspection authority.
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DefinitionDisplay {
    Mesh { artifact_id: ArtifactId },
    Unavailable { error: GeometryError },
}
#[derive(Deserialize)]
#[serde(
    remote = "DefinitionDisplay",
    tag = "kind",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum RawDefinitionDisplay {
    Mesh { artifact_id: ArtifactId },
    Unavailable { error: GeometryError },
}
impl<'de> Deserialize<'de> for DefinitionDisplay {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawDefinitionDisplay::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl DefinitionDisplay {
    pub fn validate(&self) -> Result<(), GeometryError> {
        match self {
            Self::Mesh { .. } => Ok(()),
            Self::Unavailable { error } => {
                error.validate()?;
                if matches!(
                    error.code,
                    GeometryErrorCode::Cancelled
                        | GeometryErrorCode::StaleRevision
                        | GeometryErrorCode::Busy
                        | GeometryErrorCode::UnknownHandle
                        | GeometryErrorCode::SourceIo
                        | GeometryErrorCode::SourceChanged
                        | GeometryErrorCode::UnsupportedUnits
                        | GeometryErrorCode::InvalidPose
                ) {
                    return Err(invalid(
                        "display availability cannot mask source, revision or cancellation failures",
                    ));
                }
                Ok(())
            }
        }
    }
}
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DefinitionRecord {
    pub definition_id: DefinitionId,
    pub provenance: SourceProvenance,
    pub face_count: u32,
    pub bounds_mm: AabbMm,
    pub display: DefinitionDisplay,
}
#[derive(Deserialize)]
#[serde(remote = "DefinitionRecord", deny_unknown_fields)]
struct RawDefinitionRecord {
    definition_id: DefinitionId,
    provenance: SourceProvenance,
    face_count: u32,
    bounds_mm: AabbMm,
    display: DefinitionDisplay,
}
impl<'de> Deserialize<'de> for DefinitionRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawDefinitionRecord::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl DefinitionRecord {
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.provenance.validate()?;
        self.bounds_mm.validate()?;
        self.display.validate()?;
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

#[derive(Debug, Clone, Serialize, TS, PartialEq)]
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
    /// Whole trimmed-domain triangle-chain approximation, independent of interpolation.
    pub trim_deviation_mm: f64,
    pub quantization_error_mm: f64,
}
#[derive(Deserialize)]
#[serde(remote = "MeshChunkMetadata", deny_unknown_fields)]
struct RawMeshChunkMetadata {
    session_id: SessionId,
    artifact_id: ArtifactId,
    definition_id: DefinitionId,
    schema_version: u16,
    mesh_profile: DisplayProfile,
    chunk_index: u32,
    chunk_count: u32,
    byte_count: u32,
    sha256: SourceHash,
    face_count: u32,
    bounds_mm: AabbMm,
    carrier_deviation_mm: f64,
    trim_deviation_mm: f64,
    quantization_error_mm: f64,
}
impl<'de> Deserialize<'de> for MeshChunkMetadata {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RawMeshChunkMetadata::deserialize(deserializer)?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
impl MeshChunkMetadata {
    /// Structural/error-budget admission only; this is not a native proof or manufacturing authority.
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.bounds_mm.validate()?;
        if self.schema_version != crate::display::MESH_SCHEMA_VERSION
            || self.chunk_count == 0
            || self.chunk_count > MAX_SCENE_MESH_BYTES / crate::display::MESH_HEADER_BYTES as u32
            || self.chunk_index >= self.chunk_count
            || self.face_count == 0
            || self.face_count > MAX_NATIVE_FACES
            || self.byte_count < crate::display::MESH_HEADER_BYTES as u32
            || self.byte_count > MAX_GEOMETRY_CHUNK_BYTES
        {
            return Err(invalid("invalid mesh descriptor counts, bytes or schema"));
        }
        if !self.carrier_deviation_mm.is_finite()
            || !self.trim_deviation_mm.is_finite()
            || !self.quantization_error_mm.is_finite()
            || self.carrier_deviation_mm < 0.0
            || self.trim_deviation_mm < 0.0
            || self.carrier_deviation_mm + self.trim_deviation_mm > MESH_CARRIER_TOLERANCE_MM
            || !(0.0..=MESH_QUANTIZATION_TOLERANCE_MM).contains(&self.quantization_error_mm)
            || self.carrier_deviation_mm + self.trim_deviation_mm + self.quantization_error_mm
                > MESH_SURFACE_TOLERANCE_MM
        {
            return Err(invalid("invalid mesh error budget"));
        }
        Ok(())
    }
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
