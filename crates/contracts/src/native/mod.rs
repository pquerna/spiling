// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Checked protobuf adapters. Persisted JSON and packed display bytes remain separate schemas.
use crate::{ArtifactView, geometry::*, rpc};
mod error;
pub mod geometry;
pub mod manufacturing;
mod operation;
pub mod project;
pub use error::*;
pub use geometry::*;
pub use manufacturing::*;
pub use operation::*;
pub use project::*;

pub fn required<T>(value: Option<T>, field: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("missing {field}"))
}
pub fn session_parent(parent: &str) -> Result<SessionId, String> {
    SessionId::parse(
        parent
            .strip_prefix("sessions/")
            .ok_or("invalid session parent")?,
    )
    .map_err(|e| e.to_string())
}
pub fn request_id(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    SessionId::parse(value)
        .map(|_| ())
        .map_err(|_| "request_id must be canonical nonnil UUID".into())
}
fn artifact_fields(
    name: &str,
    size: u64,
    sha256: &str,
    media_type: &str,
    max: u64,
) -> Result<(), String> {
    if name.strip_prefix("artifacts/") != Some(sha256)
        || size == 0
        || size > max
        || sha256.len() != 64
        || !sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || media_type.is_empty()
        || media_type.len() > 128
        || media_type.chars().any(char::is_control)
    {
        return Err("invalid artifact descriptor".into());
    }
    Ok(())
}
pub fn validate_artifact(value: &rpc::Artifact, max: u64) -> Result<(), String> {
    artifact_fields(
        &value.name,
        value.size_bytes,
        &value.sha256,
        &value.media_type,
        max,
    )
}
pub fn validate_artifact_view(value: &ArtifactView, max: u64) -> Result<u64, String> {
    let size: u64 = value
        .size_bytes
        .parse()
        .map_err(|_| "invalid artifact size")?;
    if value.size_bytes.is_empty()
        || value.size_bytes.starts_with('0')
        || !value.size_bytes.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("noncanonical artifact size".into());
    }
    artifact_fields(&value.name, size, &value.sha256, &value.media_type, max)?;
    Ok(size)
}
pub fn validate_operation_name(name: &str) -> Result<(), String> {
    let (parent, id) = name
        .rsplit_once("/operations/")
        .ok_or("invalid operation name")?;
    session_parent(parent)?;
    SessionId::parse(id).map_err(|_| "invalid operation UUID".to_owned())?;
    Ok(())
}
pub fn artifact_to_view(value: rpc::Artifact, max: u64) -> Result<ArtifactView, String> {
    validate_artifact(&value, max)?;
    Ok(ArtifactView {
        name: value.name,
        size_bytes: value.size_bytes.to_string(),
        sha256: value.sha256,
        media_type: value.media_type,
    })
}
pub fn artifact_from_view(value: ArtifactView, max: u64) -> Result<rpc::Artifact, String> {
    let size_bytes = validate_artifact_view(&value, max)?;
    Ok(rpc::Artifact {
        name: value.name,
        size_bytes,
        sha256: value.sha256,
        media_type: value.media_type,
    })
}
impl TryFrom<rpc::Vec3> for [f64; 3] {
    type Error = String;
    fn try_from(v: rpc::Vec3) -> Result<Self, String> {
        let v = [v.x, v.y, v.z];
        if v.iter().any(|n| !n.is_finite()) {
            Err("nonfinite vector".into())
        } else {
            Ok(v)
        }
    }
}
impl TryFrom<[f64; 3]> for rpc::Vec3 {
    type Error = String;
    fn try_from(v: [f64; 3]) -> Result<Self, String> {
        if v.iter().any(|n| !n.is_finite()) {
            return Err("nonfinite vector".into());
        }
        Ok(Self {
            x: v[0],
            y: v[1],
            z: v[2],
        })
    }
}
impl TryFrom<rpc::AabbMm> for AabbMm {
    type Error = String;
    fn try_from(v: rpc::AabbMm) -> Result<Self, String> {
        Self::new(
            required(v.min, "bounds.min")?.try_into()?,
            required(v.max, "bounds.max")?.try_into()?,
        )
        .map_err(|e| e.to_string())
    }
}
impl TryFrom<AabbMm> for rpc::AabbMm {
    type Error = String;
    fn try_from(v: AabbMm) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            min: Some(v.min.try_into()?),
            max: Some(v.max.try_into()?),
        })
    }
}
impl TryFrom<rpc::RigidPoseMm> for RigidPoseMm {
    type Error = String;
    fn try_from(v: rpc::RigidPoseMm) -> Result<Self, String> {
        let q = required(v.rotation_xyzw, "rotation")?;
        let pose = Self {
            translation_mm: required(v.translation_mm, "translation")?.try_into()?,
            rotation_xyzw: [q.x, q.y, q.z, q.w],
        };
        pose.validate().map_err(|e| e.to_string())?;
        Ok(pose)
    }
}
impl TryFrom<RigidPoseMm> for rpc::RigidPoseMm {
    type Error = String;
    fn try_from(v: RigidPoseMm) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        let q = v.rotation_xyzw;
        Ok(Self {
            translation_mm: Some(v.translation_mm.try_into()?),
            rotation_xyzw: Some(rpc::Quaternion {
                x: q[0],
                y: q[1],
                z: q[2],
                w: q[3],
            }),
        })
    }
}
impl TryFrom<rpc::PlaneMm> for PlaneMm {
    type Error = String;
    fn try_from(v: rpc::PlaneMm) -> Result<Self, String> {
        let plane = Self {
            origin_mm: required(v.origin_mm, "plane.origin")?.try_into()?,
            normal: required(v.normal, "plane.normal")?.try_into()?,
        };
        plane.validate().map_err(|e| e.to_string())?;
        Ok(plane)
    }
}
impl TryFrom<PlaneMm> for rpc::PlaneMm {
    type Error = String;
    fn try_from(v: PlaneMm) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            origin_mm: Some(v.origin_mm.try_into()?),
            normal: Some(v.normal.try_into()?),
        })
    }
}
impl TryFrom<rpc::NativePath> for NativePath {
    type Error = String;
    fn try_from(v: rpc::NativePath) -> Result<Self, String> {
        let path = match required(v.value, "native path")? {
            rpc::native_path::Value::UnixBytes(bytes) => {
                if bytes.len() > MAX_NATIVE_PATH_UNITS as usize {
                    return Err("native path exceeds limit".into());
                }
                let mut hex = String::with_capacity(bytes.len() * 2);
                const DIGITS: &[u8; 16] = b"0123456789abcdef";
                for b in bytes {
                    hex.push(DIGITS[(b >> 4) as usize] as char);
                    hex.push(DIGITS[(b & 15) as usize] as char);
                }
                Self::UnixBytes { hex }
            }
            rpc::native_path::Value::WindowsUtf16(wide) => {
                if wide.units.len() > MAX_NATIVE_PATH_UNITS as usize {
                    return Err("native path exceeds limit".into());
                }
                Self::WindowsWide {
                    units: wide
                        .units
                        .into_iter()
                        .map(|u| u16::try_from(u).map_err(|_| "invalid UTF16 path unit".to_owned()))
                        .collect::<Result<_, _>>()?,
                }
            }
        };
        path.validate().map_err(|e| e.to_string())?;
        Ok(path)
    }
}
impl TryFrom<NativePath> for rpc::NativePath {
    type Error = String;
    fn try_from(v: NativePath) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        let value = match v {
            NativePath::UnixBytes { hex } => rpc::native_path::Value::UnixBytes(
                (0..hex.len())
                    .step_by(2)
                    .map(|i| {
                        u8::from_str_radix(&hex[i..i + 2], 16)
                            .map_err(|_| "invalid path hex".to_owned())
                    })
                    .collect::<Result<_, _>>()?,
            ),
            NativePath::WindowsWide { units } => {
                rpc::native_path::Value::WindowsUtf16(rpc::Utf16Path {
                    units: units.into_iter().map(u32::from).collect(),
                })
            }
        };
        Ok(Self { value: Some(value) })
    }
}
