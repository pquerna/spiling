// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Immutable byte content held as independently compressed fixed-size frames.
//!
//! Large derived outputs such as emitted programs are written once, read
//! sequentially and then retained. [`ChunkWriter`] compresses each full
//! 256 KiB frame as it is produced and hashes the raw stream; the sealed
//! [`ChunkSource`] is cheap to clone and decodes one frame at a time. The frame
//! size and zstd level match PTAC's stored chunk profile. Frames never leave
//! the process in this form: storage and transfer receive the raw bytes.

use sha2::{Digest, Sha256};
use std::io::{self, BufRead, Read, Write};
use std::sync::Arc;

/// Raw bytes per frame; the last frame may be shorter.
pub const FRAME_BYTES: usize = 256 * 1024;
const LEVEL: i32 = 3;
/// Keep a frame raw unless compression saves at least this much.
const MIN_SAVING: usize = 64;

struct Frame {
    raw_len: u32,
    compressed: bool,
    data: Box<[u8]>,
}

struct Content {
    frames: Box<[Frame]>,
    len: u64,
    digest: [u8; 32],
    stored: usize,
}

/// Sealed immutable content. Clones share frames.
#[derive(Clone)]
pub struct ChunkSource(Arc<Content>);

impl std::fmt::Debug for ChunkSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkSource")
            .field("len", &self.0.len)
            .field("frames", &self.0.frames.len())
            .field("stored", &self.0.stored)
            .finish()
    }
}

impl ChunkSource {
    /// Raw content length in bytes.
    pub fn len(&self) -> u64 {
        self.0.len
    }
    pub fn is_empty(&self) -> bool {
        self.0.len == 0
    }
    /// SHA-256 of the raw content, computed while it was written.
    pub fn digest(&self) -> &[u8; 32] {
        &self.0.digest
    }
    /// Heap bytes owned by this content, shared by all clones.
    pub fn retained_bytes(&self) -> usize {
        self.0.stored + self.0.frames.len() * std::mem::size_of::<Frame>()
    }
    /// Identity for deduplicating shared retention accounting.
    pub fn ptr(&self) -> *const () {
        Arc::as_ptr(&self.0).cast()
    }
    /// Sequential raw reader that holds at most one decoded frame.
    pub fn reader(&self) -> ChunkReader {
        ChunkReader {
            source: self.clone(),
            next: 0,
            buf: Vec::new(),
            pos: 0,
        }
    }
    /// Copy raw content, refusing anything longer than `limit`.
    pub fn from_slice(bytes: &[u8], limit: u64) -> io::Result<Self> {
        let mut writer = ChunkWriter::new(limit);
        writer.write_all(bytes)?;
        writer.finish()
    }
    /// Consume raw content from `reader`, refusing anything longer than `limit`.
    pub fn read_from(reader: impl Read, limit: u64) -> io::Result<Self> {
        let mut writer = ChunkWriter::new(limit);
        io::copy(&mut reader.take(limit.saturating_add(1)), &mut writer)?;
        writer.finish()
    }
    /// Materialize the raw content with exact capacity.
    pub fn to_vec(&self) -> io::Result<Vec<u8>> {
        let len = usize::try_from(self.0.len).map_err(|_| too_large())?;
        let mut out = Vec::new();
        out.try_reserve_exact(len).map_err(io::Error::other)?;
        self.reader().read_to_end(&mut out)?;
        Ok(out)
    }
    /// Copy the raw content into `writer` one frame at a time.
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<u64> {
        io::copy(&mut self.reader(), writer)
    }
}

impl PartialEq for ChunkSource {
    fn eq(&self, other: &Self) -> bool {
        self.0.len == other.0.len && self.0.digest == other.0.digest
    }
}
impl Eq for ChunkSource {}

/// Append-only writer producing a [`ChunkSource`].
pub struct ChunkWriter {
    frames: Vec<Frame>,
    pending: Vec<u8>,
    hasher: Sha256,
    len: u64,
    limit: u64,
    stored: usize,
}

impl ChunkWriter {
    /// A writer refusing content longer than `limit` raw bytes.
    pub fn new(limit: u64) -> Self {
        Self {
            frames: Vec::new(),
            pending: Vec::new(),
            hasher: Sha256::new(),
            len: 0,
            limit,
            stored: 0,
        }
    }
    /// Raw bytes accepted so far.
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn seal_frame(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let raw_len = u32::try_from(self.pending.len()).map_err(|_| too_large())?;
        let compressed = zstd::bulk::compress(&self.pending, LEVEL)?;
        let (compressed, data) = if compressed.len() + MIN_SAVING <= self.pending.len() {
            (true, compressed.into_boxed_slice())
        } else {
            (false, self.pending.as_slice().into())
        };
        self.stored += data.len();
        self.frames.try_reserve(1).map_err(io::Error::other)?;
        self.frames.push(Frame {
            raw_len,
            compressed,
            data,
        });
        self.pending.clear();
        Ok(())
    }
    /// Seal the content. The remaining partial frame becomes the last frame.
    pub fn finish(mut self) -> io::Result<ChunkSource> {
        self.seal_frame()?;
        let digest = self.hasher.finalize().into();
        Ok(ChunkSource(Arc::new(Content {
            frames: self.frames.into_boxed_slice(),
            len: self.len,
            digest,
            stored: self.stored,
        })))
    }
}

impl Write for ChunkWriter {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let accepted = bytes.len();
        let len = self
            .len
            .checked_add(accepted as u64)
            .filter(|len| *len <= self.limit)
            .ok_or_else(too_large)?;
        self.hasher.update(bytes);
        while !bytes.is_empty() {
            if self.pending.capacity() == 0 {
                self.pending
                    .try_reserve_exact(FRAME_BYTES)
                    .map_err(io::Error::other)?;
            }
            let take = (FRAME_BYTES - self.pending.len()).min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() == FRAME_BYTES {
                self.seal_frame()?;
            }
        }
        self.len = len;
        Ok(accepted)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Sequential reader over a [`ChunkSource`].
pub struct ChunkReader {
    source: ChunkSource,
    next: usize,
    buf: Vec<u8>,
    pos: usize,
}

impl ChunkReader {
    fn load(&mut self) -> io::Result<bool> {
        let Some(frame) = self.source.0.frames.get(self.next) else {
            return Ok(false);
        };
        self.next += 1;
        self.pos = 0;
        self.buf.clear();
        let raw_len = frame.raw_len as usize;
        if frame.compressed {
            let decoded = zstd::bulk::decompress(&frame.data, raw_len)?;
            if decoded.len() != raw_len {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "frame length differs from its record",
                ));
            }
            self.buf = decoded;
        } else {
            self.buf.extend_from_slice(&frame.data);
        }
        Ok(true)
    }
}

impl Read for ChunkReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for ChunkReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.pos == self.buf.len() {
            if !self.load()? {
                return Ok(&[]);
            }
        }
        Ok(&self.buf[self.pos..])
    }
    fn consume(&mut self, amount: usize) {
        self.pos = (self.pos + amount).min(self.buf.len());
    }
}

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        "content exceeds its byte limit",
    )
}

/// True when `error` came from a writer's byte limit.
pub fn is_limit(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::FileTooLarge
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| b"G1 X12.345678 Y9.876543 E1.00000000\n"[i % 36])
            .collect()
    }
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    #[test]
    fn round_trips_across_frame_boundaries() {
        for len in [
            0,
            1,
            FRAME_BYTES - 1,
            FRAME_BYTES,
            FRAME_BYTES + 1,
            3 * FRAME_BYTES + 17,
        ] {
            let raw = text(len);
            let mut writer = ChunkWriter::new(u64::MAX);
            for piece in raw.chunks(4093) {
                writer.write_all(piece).unwrap();
            }
            let source = writer.finish().unwrap();
            assert_eq!(source.len(), len as u64);
            assert_eq!(source.to_vec().unwrap(), raw);
            let digest: [u8; 32] = Sha256::digest(&raw).into();
            assert_eq!(source.digest(), &digest);
            assert_eq!(source.0.frames.len(), len.div_ceil(FRAME_BYTES));
        }
    }

    #[test]
    fn compresses_text_and_keeps_incompressible_frames_raw() {
        let compressible = ChunkSource::from_slice(&text(4 * FRAME_BYTES), u64::MAX).unwrap();
        assert!(compressible.retained_bytes() < FRAME_BYTES / 4);
        assert!(compressible.0.frames.iter().all(|f| f.compressed));
        let random = noise(2 * FRAME_BYTES + 5);
        let incompressible = ChunkSource::from_slice(&random, u64::MAX).unwrap();
        assert!(incompressible.0.frames.iter().all(|f| !f.compressed));
        assert_eq!(incompressible.to_vec().unwrap(), random);
    }

    #[test]
    fn refuses_content_beyond_its_limit() {
        let mut writer = ChunkWriter::new(10);
        writer.write_all(b"0123456789").unwrap();
        let error = writer.write_all(b"x").unwrap_err();
        assert!(is_limit(&error));
        let error = ChunkSource::read_from(&text(11)[..], 10).unwrap_err();
        assert!(is_limit(&error));
        assert_eq!(ChunkSource::read_from(&text(10)[..], 10).unwrap().len(), 10);
    }

    #[test]
    fn buffered_lines_match_the_raw_text() {
        let raw = text(2 * FRAME_BYTES + 100);
        let source = ChunkSource::from_slice(&raw, u64::MAX).unwrap();
        let read: Vec<_> = source.reader().lines().map(Result::unwrap).collect();
        let expected: Vec<_> = std::str::from_utf8(&raw).unwrap().lines().collect();
        assert_eq!(read, expected);
        let clone = source.clone();
        assert_eq!(clone, source);
        assert_eq!(clone.ptr(), source.ptr());
    }
}
