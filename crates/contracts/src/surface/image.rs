// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::{invalid, resource};
use crate::{geometry::SourceHash, project::ProjectError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;

pub const MAX_SURFACE_IMAGE_BYTES: usize = 65551;
pub const MAX_SURFACE_IMAGES: usize = 16;
pub const MAX_SURFACE_IMAGE_TOTAL_BYTES: usize = 1024 * 1024;
pub const MAX_SURFACE_IMAGE_RETAINED_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SURFACE_IMAGE_SOURCE_NAME_BYTES: usize = 256;
pub const SURFACE_IMAGE_MEDIA_TYPE: &str = "image/x-portable-graymap";

fn canonical_header(width: u32, height: u32) -> ([u8; 15], usize) {
    let mut header = [0; 15];
    let length = {
        use std::io::Write;
        let mut cursor = std::io::Cursor::new(header.as_mut_slice());
        write!(&mut cursor, "P5\n{width} {height}\n255\n")
            .expect("validated image dimensions fit canonical header");
        cursor.position() as usize
    };
    (header, length)
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceImageRecord {
    pub image_hash: SourceHash,
    pub width: u32,
    pub height: u32,
    #[ts(type = "number")]
    pub byte_count: u64,
    pub source_hash: SourceHash,
    pub source_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSurfaceImageRecord {
    image_hash: SourceHash,
    width: u32,
    height: u32,
    byte_count: u64,
    source_hash: SourceHash,
    #[serde(deserialize_with = "source_name")]
    source_name: String,
}
fn source_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    struct Name;
    impl<'de> serde::de::Visitor<'de> for Name {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded image source name")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
            if value.is_empty()
                || value.len() > MAX_SURFACE_IMAGE_SOURCE_NAME_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(E::custom("image source name must be bounded nonempty text"));
            }
            Ok(value.to_owned())
        }
    }
    d.deserialize_str(Name)
}
impl<'de> Deserialize<'de> for SurfaceImageRecord {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawSurfaceImageRecord::deserialize(d)?;
        let record = Self {
            image_hash: raw.image_hash,
            width: raw.width,
            height: raw.height,
            byte_count: raw.byte_count,
            source_hash: raw.source_hash,
            source_name: raw.source_name,
        };
        record.validate().map_err(serde::de::Error::custom)?;
        Ok(record)
    }
}
#[derive(Debug, Clone, Copy)]
pub struct CanonicalImageView<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
    pub canonical_bytes: &'a [u8],
}
impl SurfaceImageRecord {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if !(2..=256).contains(&self.width) || !(2..=256).contains(&self.height) {
            return Err(invalid("image dimensions must be in 2..=256"));
        }
        let digits = |n: u32| {
            if n >= 100 {
                3
            } else if n >= 10 {
                2
            } else {
                1
            }
        };
        let expected = (9 + digits(self.width) + digits(self.height)) as u64
            + u64::from(self.width) * u64::from(self.height);
        if self.byte_count != expected || self.byte_count > MAX_SURFACE_IMAGE_BYTES as u64 {
            return Err(invalid(
                "image byte count disagrees with canonical dimensions",
            ));
        }
        if self.source_name.is_empty()
            || self.source_name.len() > MAX_SURFACE_IMAGE_SOURCE_NAME_BYTES
            || self.source_name.chars().any(char::is_control)
        {
            return Err(invalid("image source name must be bounded nonempty text"));
        }
        Ok(())
    }
    pub fn validate_content<'a>(
        &self,
        bytes: &'a [u8],
    ) -> Result<CanonicalImageView<'a>, ProjectError> {
        self.validate()?;
        if bytes.len() != self.byte_count as usize || bytes.len() > MAX_SURFACE_IMAGE_BYTES {
            return Err(crate::project::ProjectError::new(
                crate::project::ProjectErrorCode::CorruptAsset,
                "canonical image content size mismatch",
            ));
        }
        let (header, length) = canonical_header(self.width, self.height);
        if !bytes.starts_with(&header[..length]) || !self.image_hash.matches_bytes(bytes) {
            return Err(crate::project::ProjectError::new(
                crate::project::ProjectErrorCode::CorruptAsset,
                "canonical image header or hash mismatch",
            ));
        }
        Ok(CanonicalImageView {
            width: self.width,
            height: self.height,
            pixels: &bytes[length..],
            canonical_bytes: bytes,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceImageContent {
    pub image_hash: SourceHash,
    #[serde(deserialize_with = "bounded_image_bytes")]
    pub bytes: Arc<Vec<u8>>,
}
fn bounded_image_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Arc<Vec<u8>>, D::Error> {
    struct Bytes;
    impl<'de> serde::de::Visitor<'de> for Bytes {
        type Value = Vec<u8>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded canonical image bytes")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut bytes = Vec::new();
            while let Some(byte) = seq.next_element()? {
                if bytes.len() == MAX_SURFACE_IMAGE_BYTES {
                    return Err(serde::de::Error::custom(
                        "canonical image byte limit exceeded",
                    ));
                }
                if bytes.len() == bytes.capacity() {
                    bytes.reserve_exact((MAX_SURFACE_IMAGE_BYTES - bytes.len()).min(1024));
                }
                bytes.push(byte);
            }
            Ok(bytes)
        }
    }
    d.deserialize_seq(Bytes).map(Arc::new)
}
pub fn bounded_image_contents<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SurfaceImageContent>, D::Error> {
    struct Contents;
    impl<'de> serde::de::Visitor<'de> for Contents {
        type Value = Vec<SurfaceImageContent>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded unique canonical image content table")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut entries = Vec::new();
            let mut bytes = 0usize;
            while let Some(entry) = seq.next_element::<SurfaceImageContent>()? {
                bytes += entry.bytes.capacity();
                if entries.len() == MAX_SURFACE_IMAGES || bytes > MAX_SURFACE_IMAGE_TOTAL_BYTES {
                    return Err(serde::de::Error::custom(
                        "canonical image table budget exceeded",
                    ));
                }
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    d.deserialize_seq(Contents)
}
pub fn validate_image_contents(
    records: &[SurfaceImageRecord],
    contents: &[SurfaceImageContent],
) -> Result<(), ProjectError> {
    if contents.len() != records.len() || contents.len() > MAX_SURFACE_IMAGES {
        return Err(invalid("image content table must exactly match metadata"));
    }
    let mut total = 0usize;
    for (record, content) in records.iter().zip(contents) {
        if record.image_hash != content.image_hash {
            return Err(invalid("image content order or identity mismatch"));
        }
        record.validate_content(content.bytes.as_slice())?;
        if content.bytes.capacity() > MAX_SURFACE_IMAGE_BYTES {
            return Err(resource("image content allocation limit exceeded"));
        }
        total = total
            .checked_add(content.bytes.capacity())
            .ok_or_else(|| resource("image content budget overflow"))?;
    }
    if total > MAX_SURFACE_IMAGE_TOTAL_BYTES {
        return Err(resource("image content table byte budget exceeded"));
    }
    Ok(())
}
/// Periodic pixel-node relief in captured source coordinates, never world placement.
/// Depth/margin/period/Z phase/vertical error are millimetres; U phase is a repeat-cycle fraction.
/// Increasing source Z traverses image rows in reverse; phases advance the sampled coordinate.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImageReliefParameters {
    pub depth_mm: f64,
    pub u_repeats: u32,
    pub u_phase_fraction: f64,
    pub z_period_mm: f64,
    pub z_phase_mm: f64,
    pub edge_margin_mm: f64,
    pub vertical_error_mm: f64,
}
impl ImageReliefParameters {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if [self.depth_mm, self.z_period_mm, self.vertical_error_mm]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
            || !(1..=256).contains(&self.u_repeats)
            || !self.u_phase_fraction.is_finite()
            || !(0.0..1.0).contains(&self.u_phase_fraction)
            || !self.z_phase_mm.is_finite()
            || self.z_phase_mm < 0.0
            || self.z_phase_mm >= self.z_period_mm
            || !self.edge_margin_mm.is_finite()
            || self.edge_margin_mm < 0.0
        {
            return Err(invalid(
                "image relief requires finite positive depth, period and vertical error, repeats 1..=256, explicit bounded phases and nonnegative margin",
            ));
        }
        Ok(())
    }
}
