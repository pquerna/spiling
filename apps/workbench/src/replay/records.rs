// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Indexed access to checked exact commands with one decoded page of scratch.
use super::*;
use std::{
    cell::RefCell,
    io::{Read, Seek, SeekFrom},
};

pub(super) struct Records {
    pub source: spiling_chunks::ChunkSource,
    pub count: usize,
    page: RefCell<(usize, Vec<u8>)>,
}
impl Records {
    pub fn new(source: spiling_chunks::ChunkSource, count: usize) -> Self {
        Self {
            source,
            count,
            page: RefCell::new((usize::MAX, Vec::new())),
        }
    }
    pub fn get(&self, index: usize) -> Result<[u8; REPLAY_RECORD_BYTES], String> {
        if index >= self.count {
            return Err("replay index outside admitted document".into());
        }
        let number = index / REPLAY_PAGE_EVENTS as usize;
        let mut cache = self.page.borrow_mut();
        if cache.0 != number {
            let first = number * REPLAY_PAGE_EVENTS as usize;
            let length =
                (self.count - first).min(REPLAY_PAGE_EVENTS as usize) * REPLAY_RECORD_BYTES;
            let mut reader = self.source.reader();
            reader
                .seek(SeekFrom::Start(
                    (REPLAY_HEADER_BYTES + first * REPLAY_RECORD_BYTES) as u64,
                ))
                .map_err(|e| e.to_string())?;
            cache.1.resize(length, 0);
            reader.read_exact(&mut cache.1).map_err(|e| e.to_string())?;
            cache.0 = number;
        }
        let offset = index % REPLAY_PAGE_EVENTS as usize * REPLAY_RECORD_BYTES;
        Ok(cache.1[offset..offset + REPLAY_RECORD_BYTES]
            .try_into()
            .unwrap())
    }
}
