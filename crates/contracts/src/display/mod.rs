// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Explicit little-endian packed geometry. Validators borrow bytes, never cast Rust layouts.

mod mesh;
mod section;
use crate::geometry::{GeometryError, invalid};
pub use mesh::*;
pub use section::*;

pub const MESH_SCHEMA_VERSION: u16 = 1;
pub const MESH_HEADER_BYTES: usize = 64;
pub const SECTION_SCHEMA_VERSION: u16 = 1;
pub const SECTION_HEADER_BYTES: usize = 64;

pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        bytes[offset..offset + 2]
            .try_into()
            .expect("checked layout"),
    )
}
pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("checked layout"),
    )
}
pub(crate) fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("checked layout"),
    )
}
pub(crate) fn f64_at(bytes: &[u8], offset: usize) -> f64 {
    f64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("checked layout"),
    )
}
pub(crate) fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_f64(bytes: &mut [u8], offset: usize, value: f64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
pub(crate) fn checked_size(
    base: usize,
    count: usize,
    stride: usize,
) -> Result<usize, GeometryError> {
    count
        .checked_mul(stride)
        .and_then(|size| base.checked_add(size))
        .ok_or_else(|| invalid("packed layout arithmetic overflow"))
}

/// An endian-independent scalar view. Every iterator reads directly from the caller's bytes.
#[derive(Debug, Clone, Copy)]
pub struct U32View<'a> {
    bytes: &'a [u8],
}
impl<'a> U32View<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
    pub fn len(self) -> usize {
        self.bytes.len() / 4
    }
    pub fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }
    pub fn get(self, index: usize) -> Option<u32> {
        if index < self.len() {
            Some(u32_at(self.bytes, index * 4))
        } else {
            None
        }
    }
    pub fn iter(self) -> impl ExactSizeIterator<Item = u32> + 'a {
        self.bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| u32::from_le_bytes(*v))
    }
    pub fn bytes(self) -> &'a [u8] {
        self.bytes
    }
}
#[derive(Debug, Clone, Copy)]
pub struct F32x3View<'a> {
    bytes: &'a [u8],
}
impl<'a> F32x3View<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
    pub fn len(self) -> usize {
        self.bytes.len() / 12
    }
    pub fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }
    pub fn get(self, index: usize) -> Option<[f32; 3]> {
        if index < self.len() {
            Some(std::array::from_fn(|axis| {
                f32_at(self.bytes, index * 12 + axis * 4)
            }))
        } else {
            None
        }
    }
    pub fn iter(self) -> impl ExactSizeIterator<Item = [f32; 3]> + 'a {
        self.bytes
            .as_chunks::<12>()
            .0
            .iter()
            .map(|v| std::array::from_fn(|axis| f32_at(v, axis * 4)))
    }
    pub fn bytes(self) -> &'a [u8] {
        self.bytes
    }
}
#[derive(Debug, Clone, Copy)]
pub struct F64x3View<'a> {
    bytes: &'a [u8],
}
impl<'a> F64x3View<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
    pub fn len(self) -> usize {
        self.bytes.len() / 24
    }
    pub fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }
    pub fn get(self, index: usize) -> Option<[f64; 3]> {
        if index < self.len() {
            Some(std::array::from_fn(|axis| {
                f64_at(self.bytes, index * 24 + axis * 8)
            }))
        } else {
            None
        }
    }
    pub fn iter(self) -> impl ExactSizeIterator<Item = [f64; 3]> + 'a {
        self.bytes
            .as_chunks::<24>()
            .0
            .iter()
            .map(|v| std::array::from_fn(|axis| f64_at(v, axis * 8)))
    }
    pub fn bytes(self) -> &'a [u8] {
        self.bytes
    }
}

#[cfg(test)]
mod tests;
