// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{CHUNK_BYTES, Result, invalid, limit, model::hash};

pub(crate) struct Encoded {
    pub hash: [u8; 32],
    pub raw_length: usize,
    pub codec: u8,
    pub data: Vec<u8>,
}

pub(crate) fn encode(raw: &[u8]) -> Result<Encoded> {
    if raw.is_empty() || raw.len() > CHUNK_BYTES {
        return Err(limit("chunk length"));
    }
    let compressed = zstd::bulk::compress(raw, 3)?;
    let (codec, data) = if compressed
        .len()
        .checked_add(64)
        .is_some_and(|size| size <= raw.len())
    {
        (1, compressed)
    } else {
        (0, raw.to_vec())
    };
    Ok(Encoded {
        hash: hash(raw),
        raw_length: raw.len(),
        codec,
        data,
    })
}

pub(crate) fn decode(
    codec: i64,
    raw_length: usize,
    data: &[u8],
    expected: &[u8; 32],
) -> Result<Vec<u8>> {
    if raw_length == 0 || raw_length > CHUNK_BYTES || data.is_empty() || data.len() > CHUNK_BYTES {
        return Err(limit("encoded chunk length"));
    }
    let raw = match codec {
        0 if data.len() == raw_length => data.to_vec(),
        1 => {
            if data.get(..4) != Some(&[0x28, 0xb5, 0x2f, 0xfd]) {
                return Err(invalid("not a standard ZSTD frame"));
            }
            let descriptor = *data
                .get(4)
                .ok_or_else(|| invalid("truncated ZSTD header"))?;
            if descriptor & 0x1b != 0 {
                return Err(invalid("reserved ZSTD bits/dictionary are forbidden"));
            }
            if descriptor & 0x20 == 0 {
                let window = *data.get(5).ok_or_else(|| invalid("missing ZSTD window"))?;
                let base = 1u64 << (10 + (window >> 3));
                let size = base + (base >> 3) * u64::from(window & 7);
                if size > CHUNK_BYTES as u64 {
                    return Err(invalid("ZSTD window exceeds 256 KiB"));
                }
            }
            if zstd::zstd_safe::get_frame_content_size(data).ok().flatten()
                != Some(raw_length as u64)
                || zstd::zstd_safe::find_frame_compressed_size(data).ok() != Some(data.len())
                || zstd::zstd_safe::get_dict_id_from_frame(data).is_some()
            {
                return Err(invalid("ZSTD frame profile/length/dictionary mismatch"));
            }
            let mut decoder = zstd::bulk::Decompressor::new()?;
            decoder.set_parameter(zstd::zstd_safe::DParameter::WindowLogMax(18))?;
            decoder
                .decompress(data, raw_length)
                .map_err(|_| invalid("invalid or oversized ZSTD frame"))?
        }
        _ => return Err(invalid("unsupported chunk codec/RAW length")),
    };
    if raw.len() != raw_length || &hash(&raw) != expected {
        return Err(invalid("chunk hash/length mismatch"));
    }
    Ok(raw)
}
