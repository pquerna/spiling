// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::{cancelled, invalid};
use spiling_contracts::{
    geometry::SourceHash,
    project::{ProjectError, ProjectErrorCode},
    surface::SurfaceImageRecord,
};
use std::sync::atomic::AtomicBool;

const MAX_ENCODED: usize = 1024 * 1024;
fn dimensions(w: u32, h: u32) -> Result<usize, ProjectError> {
    if !(2..=256).contains(&w) || !(2..=256).contains(&h) {
        return Err(invalid("image dimensions must be 2..256"));
    }
    Ok(w as usize * h as usize)
}
/// Decode captured bytes only; no source path or storage authority crosses here.
pub fn normalize(
    encoded: &[u8],
    source_name: String,
    canonical_budget: usize,
    cancel: &AtomicBool,
) -> Result<(SurfaceImageRecord, Vec<u8>), ProjectError> {
    cancelled(cancel)?;
    if encoded.len() > MAX_ENCODED {
        return Err(ProjectError {
            code: ProjectErrorCode::ResourceLimit,
            message: "encoded image exceeds 1 MiB".into(),
        });
    }
    if source_name.len() > 256 {
        return Err(invalid("image source name exceeds 256 UTF-8 bytes"));
    }
    let (width, height, bytes) = if encoded.starts_with(b"P5\n") {
        pgm(encoded, canonical_budget)?
    } else {
        png(encoded, canonical_budget, cancel)?
    };
    cancelled(cancel)?;
    let record = SurfaceImageRecord {
        image_hash: SourceHash::from_bytes(&bytes),
        width,
        height,
        byte_count: bytes.len() as u64,
        source_hash: SourceHash::from_bytes(encoded),
        source_name,
    };
    record.validate_content(&bytes)?;
    Ok((record, bytes))
}
fn pgm(encoded: &[u8], canonical_budget: usize) -> Result<(u32, u32, Vec<u8>), ProjectError> {
    let end = encoded[3..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|n| n + 3)
        .ok_or_else(|| invalid("invalid canonical PGM header"))?;
    let line =
        std::str::from_utf8(&encoded[3..end]).map_err(|_| invalid("invalid PGM dimensions"))?;
    let (w, h) = line
        .split_once(' ')
        .ok_or_else(|| invalid("invalid PGM dimensions"))?;
    let width = w.parse::<u32>().map_err(|_| invalid("invalid PGM width"))?;
    let height = h
        .parse::<u32>()
        .map_err(|_| invalid("invalid PGM height"))?;
    let count = dimensions(width, height)?;
    let header = format!("P5\n{width} {height}\n255\n");
    if !encoded.starts_with(header.as_bytes()) || encoded.len() != header.len() + count {
        return Err(invalid(
            "PGM must have exact canonical P5 header and pixel length",
        ));
    }
    admit_canonical(header.len(), count, canonical_budget)?;
    Ok((width, height, encoded.to_vec()))
}
fn admit_canonical(header: usize, pixels: usize, budget: usize) -> Result<usize, ProjectError> {
    header
        .checked_add(pixels)
        .filter(|size| {
            *size <= budget && *size <= spiling_contracts::surface::MAX_SURFACE_IMAGE_BYTES
        })
        .ok_or_else(|| ProjectError {
            code: ProjectErrorCode::ResourceLimit,
            message: "canonical image exceeds remaining retained allocation budget".into(),
        })
}
#[cfg(not(feature = "png"))]
fn png(_: &[u8], _: usize, _: &AtomicBool) -> Result<(u32, u32, Vec<u8>), ProjectError> {
    Err(invalid("PNG normalization requires the native png feature"))
}
#[cfg(feature = "png")]
fn png(
    encoded: &[u8],
    canonical_budget: usize,
    cancel: &AtomicBool,
) -> Result<(u32, u32, Vec<u8>), ProjectError> {
    use std::io::Cursor;
    if !encoded.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(invalid("expected static PNG or canonical PGM"));
    }
    // Inspect the complete bounded stream, including metadata after IDAT. A
    // decoder that returns just frame zero must not accidentally admit APNG.
    let mut offset = 8;
    let mut ended = false;
    while offset < encoded.len() {
        cancelled(cancel)?;
        let chunk = encoded
            .get(offset..offset + 8)
            .ok_or_else(|| invalid("truncated PNG chunk"))?;
        let len = u32::from_be_bytes(chunk[..4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(len)
            .and_then(|v| v.checked_add(12))
            .filter(|v| *v <= encoded.len())
            .ok_or_else(|| invalid("invalid PNG chunk length"))?;
        if matches!(&chunk[4..8], b"acTL" | b"fcTL" | b"fdAT") {
            return Err(invalid("animated PNG is unsupported"));
        }
        if &chunk[4..8] == b"IEND" {
            if len != 0 || end != encoded.len() {
                return Err(invalid("PNG trailing bytes or malformed end"));
            }
            ended = true;
        }
        offset = end;
    }
    if !ended {
        return Err(invalid("PNG has no complete IEND"));
    }
    let ihdr = encoded
        .get(8..29)
        .filter(|v| &v[..8] == b"\0\0\0\rIHDR")
        .ok_or_else(|| invalid("PNG must begin with IHDR"))?;
    dimensions(
        u32::from_be_bytes(ihdr[8..12].try_into().unwrap()),
        u32::from_be_bytes(ihdr[12..16].try_into().unwrap()),
    )?;
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(encoded),
        png::Limits {
            bytes: 8 * 1024 * 1024,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| invalid(&format!("invalid PNG: {e}")))?;
    let width = reader.info().width;
    let height = reader.info().height;
    let count = dimensions(width, height)?;
    let header = format!("P5\n{width} {height}\n255\n");
    let canonical_size = admit_canonical(header.len(), count, canonical_budget)?;
    let size = reader
        .output_buffer_size()
        .filter(|n| *n <= 262144)
        .ok_or_else(|| invalid("PNG decoded byte limit exceeded"))?;
    let mut decoded = vec![0; size];
    cancelled(cancel)?;
    let info = reader
        .next_frame(&mut decoded)
        .map_err(|e| invalid(&format!("PNG decode failed: {e}")))?;
    reader
        .finish()
        .map_err(|e| invalid(&format!("PNG completion failed: {e}")))?;
    cancelled(cancel)?;
    if info.width != width || info.height != height || info.bit_depth != png::BitDepth::Eight {
        return Err(invalid("unexpected PNG frame layout"));
    }
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return Err(invalid("PNG palette was not expanded")),
    };
    if info.buffer_size() != count * channels {
        return Err(invalid("unexpected PNG decoded size"));
    }
    let mut canonical = Vec::with_capacity(canonical_size);
    canonical.extend_from_slice(header.as_bytes());
    for pixel in decoded[..info.buffer_size()].chunks_exact(channels) {
        cancelled(cancel)?;
        let (luma, alpha) = match channels {
            1 => (pixel[0] as u32, 255),
            2 => (pixel[0] as u32, pixel[1] as u32),
            3 | 4 => (
                (299 * pixel[0] as u32 + 587 * pixel[1] as u32 + 114 * pixel[2] as u32 + 500)
                    / 1000,
                if channels == 4 { pixel[3] as u32 } else { 255 },
            ),
            _ => unreachable!(),
        };
        canonical.push(((luma * alpha + 127) / 255) as u8);
    }
    Ok((width, height, canonical))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pgm_golden_preserves_samples_and_hash() {
        let mut bytes = b"P5\n3 2\n255\n".to_vec();
        bytes.extend([0, 1, 255, 128, 64, 17]);
        let (record, canonical) = normalize(
            &bytes,
            "golden.pgm".into(),
            bytes.len(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(canonical, bytes);
        assert_eq!(
            record.image_hash.as_str(),
            "3e3fa3680ccd7a4a3e93904f48f14661d07486d5c835cf2ffffb9e9427a6051c"
        );
        assert_eq!(
            normalize(
                &bytes,
                "golden.pgm".into(),
                bytes.len() - 1,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ProjectErrorCode::ResourceLimit
        );
        assert!(canonical.capacity() <= bytes.len());
        bytes.push(0);
        assert!(
            normalize(
                &bytes,
                "bad.pgm".into(),
                MAX_ENCODED,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    #[cfg(feature = "png")]
    fn encode(
        color: png::ColorType,
        depth: png::BitDepth,
        pixels: &[u8],
        palette: Option<Vec<u8>>,
        trns: Option<Vec<u8>>,
    ) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let mut e = png::Encoder::new(&mut output, 2, 2);
            e.set_color(color);
            e.set_depth(depth);
            if let Some(v) = palette {
                e.set_palette(v);
            }
            if let Some(v) = trns {
                e.set_trns(v);
            }
            let mut writer = e.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
        }
        output
    }
    #[cfg(feature = "png")]
    #[test]
    fn png_integer_luma_alpha_palette_and_high_bytes_are_frozen() {
        let c = AtomicBool::new(false);
        let rgba = encode(
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[
                255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 255, 255, 255, 255,
            ],
            None,
            None,
        );
        let (_, bytes) = normalize(&rgba, "rgba.png".into(), 15, &c).unwrap();
        assert_eq!(&bytes[bytes.len() - 4..], &[76, 75, 0, 255]);
        assert_eq!(bytes.len(), 15);
        assert_eq!(bytes.capacity(), 15);
        assert_eq!(
            normalize(&rgba, "rgba.png".into(), 14, &c)
                .unwrap_err()
                .code,
            ProjectErrorCode::ResourceLimit
        );
        let palette = encode(
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            &[0, 1, 0, 1],
            Some(vec![255, 0, 0, 0, 255, 0]),
            Some(vec![255, 128]),
        );
        let (_, bytes) = normalize(&palette, "palette.png".into(), MAX_ENCODED, &c).unwrap();
        assert_eq!(&bytes[bytes.len() - 4..], &[76, 75, 76, 75]);
        let gray = encode(
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Sixteen,
            &[
                128, 255, 255, 0, 255, 255, 128, 255, 0, 255, 255, 255, 255, 0, 0, 255,
            ],
            None,
            None,
        );
        let (_, bytes) = normalize(&gray, "gray16.png".into(), MAX_ENCODED, &c).unwrap();
        assert_eq!(&bytes[bytes.len() - 4..], &[128, 128, 0, 0]);
    }
    #[cfg(feature = "png")]
    #[test]
    fn animated_metadata_oversized_dimensions_and_cancel_are_refused() {
        let mut animated = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut animated, 2, 2);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(1, 0).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0, 17, 128, 255]).unwrap();
        }
        assert!(
            normalize(
                &animated,
                "animated.png".into(),
                MAX_ENCODED,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        let mut oversized = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut oversized, 257, 2);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![0; 514]).unwrap();
        }
        assert!(
            normalize(
                &oversized,
                "large.png".into(),
                MAX_ENCODED,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(
            normalize(
                b"not decoded",
                "cancel.png".into(),
                MAX_ENCODED,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            ProjectErrorCode::Cancelled
        );
        assert_eq!(
            normalize(
                &vec![0; MAX_ENCODED + 1],
                "oversized.png".into(),
                MAX_ENCODED,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ProjectErrorCode::ResourceLimit
        );
    }
}
