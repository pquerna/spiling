// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Runtime-independent B0 control framing and synthetic diagnostic data.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use ts_rs::TS;

pub const PROTOCOL_VERSION: u16 = 1;
pub const FRAME_HEADER_BYTES: usize = 16;
pub const MAX_CONTROL_BYTES: u32 = 65_536;
pub const MAX_BINARY_BYTES: u32 = 4_194_304;
pub const TRIANGLE_SCHEMA_VERSION: u16 = 1;
pub const TRIANGLE_HEADER_BYTES: usize = 16;
pub const TRIANGLE_VERTEX_COUNT: u32 = 3;
pub const TRIANGLE_INDEX_COUNT: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Hello {
        protocol_version: u16,
        client_build: String,
    },
    Ping {},
    Triangle {},
    Shutdown {},
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol_version: u16,
    pub engine_build: String,
    /// Configured future default, not evidence of a linked kernel.
    pub kernel: String,
    pub geometry_capabilities: Vec<String>,
    pub max_control_bytes: u32,
    pub max_binary_bytes: u32,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Hello(Hello),
    Pong {},
    Bye {},
    Error { code: String, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FrameKind {
    Control = 1,
    Triangle = 2,
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("frame I/O: {0}")]
    Io(#[from] io::Error),
    #[error("invalid frame magic")]
    Magic,
    #[error("unsupported frame version {0}; expected {PROTOCOL_VERSION}")]
    Version(u16),
    #[error("unknown frame kind {0}")]
    Kind(u16),
    #[error("request ID must be nonzero")]
    RequestId,
    #[error("payload length {length} exceeds {limit}")]
    Length { length: usize, limit: u32 },
    #[error("frame length does not match its header")]
    Size,
    #[error("control JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub kind: FrameKind,
    pub request_id: u32,
    pub payload_len: u32,
}

impl FrameHeader {
    pub fn new(kind: FrameKind, request_id: u32, payload_len: usize) -> Result<Self, WireError> {
        if request_id == 0 {
            return Err(WireError::RequestId);
        }
        let limit = match kind {
            FrameKind::Control => MAX_CONTROL_BYTES,
            FrameKind::Triangle => MAX_BINARY_BYTES,
        };
        if payload_len > limit as usize {
            return Err(WireError::Length {
                length: payload_len,
                limit,
            });
        }
        Ok(Self {
            kind,
            request_id,
            payload_len: payload_len as u32,
        })
    }

    /// Validate all untrusted metadata before a payload allocation.
    pub fn decode(bytes: &[u8; FRAME_HEADER_BYTES]) -> Result<Self, WireError> {
        if &bytes[..4] != b"SPLG" {
            return Err(WireError::Magic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != PROTOCOL_VERSION {
            return Err(WireError::Version(version));
        }
        let kind = match u16::from_le_bytes([bytes[6], bytes[7]]) {
            1 => FrameKind::Control,
            2 => FrameKind::Triangle,
            value => return Err(WireError::Kind(value)),
        };
        let request_id = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let payload_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        Self::new(kind, request_id, payload_len as usize)
    }

    pub fn encode(self) -> Result<[u8; FRAME_HEADER_BYTES], WireError> {
        Self::new(self.kind, self.request_id, self.payload_len as usize)?;
        let mut bytes = [0; FRAME_HEADER_BYTES];
        bytes[..4].copy_from_slice(b"SPLG");
        bytes[4..6].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&(self.kind as u16).to_le_bytes());
        bytes[8..12].copy_from_slice(&self.request_id.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.payload_len.to_le_bytes());
        Ok(bytes)
    }
}

#[derive(Debug)]
pub struct Frame {
    pub header: FrameHeader,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: FrameKind, request_id: u32, payload: Vec<u8>) -> Result<Self, WireError> {
        Ok(Self {
            header: FrameHeader::new(kind, request_id, payload.len())?,
            payload,
        })
    }

    pub fn control<T: Serialize>(request_id: u32, value: &T) -> Result<Self, WireError> {
        Self::new(FrameKind::Control, request_id, serde_json::to_vec(value)?)
    }

    /// EOF between frames is clean; EOF inside either header or payload is not.
    pub fn read(reader: &mut impl Read) -> Result<Option<Self>, WireError> {
        let mut bytes = [0; FRAME_HEADER_BYTES];
        loop {
            match reader.read(&mut bytes[..1]) {
                Ok(0) => return Ok(None),
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        reader.read_exact(&mut bytes[1..])?;
        let header = FrameHeader::decode(&bytes)?;
        let mut payload = vec![0; header.payload_len as usize];
        reader.read_exact(&mut payload)?;
        Ok(Some(Self { header, payload }))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() < FRAME_HEADER_BYTES {
            return Err(WireError::Size);
        }
        let header = FrameHeader::decode(bytes[..FRAME_HEADER_BYTES].try_into().unwrap())?;
        if bytes.len() != FRAME_HEADER_BYTES + header.payload_len as usize {
            return Err(WireError::Size);
        }
        Ok(Self {
            header,
            payload: bytes[FRAME_HEADER_BYTES..].to_vec(),
        })
    }

    pub fn write(&self, writer: &mut impl Write) -> Result<(), WireError> {
        let validated =
            FrameHeader::new(self.header.kind, self.header.request_id, self.payload.len())?;
        if validated != self.header {
            return Err(WireError::Size);
        }
        writer.write_all(&self.header.encode()?)?;
        writer.write_all(&self.payload)?;
        writer.flush()?;
        Ok(())
    }

    /// Write borrowed binary data without allocating an owned frame payload.
    pub fn write_payload(
        kind: FrameKind,
        request_id: u32,
        payload: &[u8],
        writer: &mut impl Write,
    ) -> Result<(), WireError> {
        let header = FrameHeader::new(kind, request_id, payload.len())?;
        writer.write_all(&header.encode()?)?;
        writer.write_all(payload)?;
        writer.flush()?;
        Ok(())
    }
}

/// Explicit little-endian schema, never Rust memory layout or CAD geometry.
pub fn synthetic_triangle() -> [u8; 64] {
    let mut bytes = [0; 64];
    bytes[..4].copy_from_slice(b"SPLT");
    bytes[4..6].copy_from_slice(&TRIANGLE_SCHEMA_VERSION.to_le_bytes());
    bytes[8..12].copy_from_slice(&TRIANGLE_VERTEX_COUNT.to_le_bytes());
    bytes[12..16].copy_from_slice(&TRIANGLE_INDEX_COUNT.to_le_bytes());
    let positions: [f32; 9] = [-0.75, -0.6, 0.0, 0.75, -0.6, 0.0, 0.0, 0.75, 0.0];
    for (index, value) in positions.iter().enumerate() {
        bytes[16 + index * 4..20 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    for (index, value) in [0_u32, 1, 2].iter().enumerate() {
        bytes[52 + index * 4..56 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
