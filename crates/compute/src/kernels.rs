// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use cubecl::prelude::*;

use crate::{ComputeError, TILE_WORDS, zeroed_words};

#[cube]
fn ordered_le(left_hi: u32, left_lo: u32, right_hi: u32, right_lo: u32) -> bool {
    left_hi < right_hi || (left_hi == right_hi && left_lo <= right_lo)
}

#[cube]
fn ordered_lt(left_hi: u32, left_lo: u32, right_hi: u32, right_lo: u32) -> bool {
    left_hi < right_hi || (left_hi == right_hi && left_lo < right_lo)
}

#[cube(launch)]
fn masks<N: Size>(
    values: &[Vector<u32, N>],
    queries: &[u32],
    output: &mut [u32],
    columns: u32,
    word_offset: u32,
    #[comptime] kind: u32,
) {
    let words_per_row = (columns as usize).div_ceil(32);
    let field_stride = words_per_row * 8;
    for position in range_stepped(ABSOLUTE_POS, output.len(), CUBE_DIM as usize * CUBE_COUNT) {
        let global_word = position + word_offset as usize;
        let row = global_word / words_per_row;
        let row_word = global_word % words_per_row;
        let mut mask = 0u32;
        for group in 0..8usize {
            let base = row_word * 8 + group;
            let first_hi = values[base];
            let first_lo = values[field_stride + base];
            let second_hi = values[field_stride * 2 + base];
            let second_lo = values[field_stride * 3 + base];
            if kind == 1 {
                let third_hi = values[field_stride * 4 + base];
                let third_lo = values[field_stride * 5 + base];
                let fourth_hi = values[field_stride * 6 + base];
                let fourth_lo = values[field_stride * 7 + base];
                #[unroll]
                for lane in 0..4usize {
                    let column = row_word * 32 + group * 4 + lane;
                    let hit = ordered_le(
                        first_hi.extract(lane),
                        first_lo.extract(lane),
                        queries[row * 8 + 4],
                        queries[row * 8 + 5],
                    ) && ordered_le(
                        queries[row * 8],
                        queries[row * 8 + 1],
                        third_hi.extract(lane),
                        third_lo.extract(lane),
                    ) && ordered_le(
                        second_hi.extract(lane),
                        second_lo.extract(lane),
                        queries[row * 8 + 6],
                        queries[row * 8 + 7],
                    ) && ordered_le(
                        queries[row * 8 + 2],
                        queries[row * 8 + 3],
                        fourth_hi.extract(lane),
                        fourth_lo.extract(lane),
                    );
                    if column < columns as usize && hit {
                        mask |= 1u32 << (group * 4 + lane) as u32;
                    }
                }
            } else {
                #[unroll]
                for lane in 0..4usize {
                    let column = row_word * 32 + group * 4 + lane;
                    let forward_start = ordered_le(
                        first_hi.extract(lane),
                        first_lo.extract(lane),
                        queries[row * 2],
                        queries[row * 2 + 1],
                    );
                    let reverse_start = ordered_le(
                        second_hi.extract(lane),
                        second_lo.extract(lane),
                        queries[row * 2],
                        queries[row * 2 + 1],
                    );
                    let mut forward_end = ordered_lt(
                        queries[row * 2],
                        queries[row * 2 + 1],
                        second_hi.extract(lane),
                        second_lo.extract(lane),
                    );
                    let mut reverse_end = ordered_lt(
                        queries[row * 2],
                        queries[row * 2 + 1],
                        first_hi.extract(lane),
                        first_lo.extract(lane),
                    );
                    if kind == 2 {
                        forward_end = ordered_le(
                            queries[row * 2],
                            queries[row * 2 + 1],
                            second_hi.extract(lane),
                            second_lo.extract(lane),
                        );
                        reverse_end = ordered_le(
                            queries[row * 2],
                            queries[row * 2 + 1],
                            first_hi.extract(lane),
                            first_lo.extract(lane),
                        );
                    }
                    if column < columns as usize
                        && ((forward_start && forward_end) || (reverse_start && reverse_end))
                    {
                        mask |= 1u32 << (group * 4 + lane) as u32;
                    }
                }
            }
        }
        output[position] = mask;
    }
}

/// The sole unsafe adapter. All handles are allocated from these exact u32 slices;
/// lengths are scalar element counts, four-lane inputs are padded, and checked launches
/// retain bounds checks. CPU launches have one unit and one cube, with no barriers;
/// Vulkan launches have at most 64 units. Readback drains every bounded tile.
pub(crate) struct UploadedBatch {
    client: Client,
    cpu: bool,
    input: cubecl::server::Handle,
    queries: cubecl::server::Handle,
    input_length: usize,
    query_length: usize,
    columns: u32,
    total_words: usize,
    kind: u32,
}

impl UploadedBatch {
    pub(crate) fn new(
        client: &Client,
        cpu: bool,
        values: &[u32],
        queries: &[u32],
        columns: usize,
        kind: u32,
    ) -> Result<Self, ComputeError> {
        if columns == 0 || columns > crate::MAX_BATCH_ITEMS || kind > 2 {
            return Err(ComputeError::Execution(
                "invalid internal dispatch shape".into(),
            ));
        }
        let fields = if kind == 1 { 8 } else { 4 };
        let query_fields = if kind == 1 { 8 } else { 2 };
        let words_per_row = columns.div_ceil(32);
        let rows = queries.len() / query_fields;
        if rows == 0 || rows > crate::MAX_BATCH_ITEMS {
            return Err(ComputeError::Execution(
                "invalid internal query count".into(),
            ));
        }
        let total_words = rows
            .checked_mul(words_per_row)
            .ok_or_else(|| ComputeError::Execution("internal mask shape overflow".into()))?;
        if values.len() != words_per_row * 32 * fields
            || !queries.len().is_multiple_of(query_fields)
            || total_words > crate::MAX_MASK_WORDS
        {
            return Err(ComputeError::Execution(
                "invalid internal buffer lengths".into(),
            ));
        }
        Ok(Self {
            client: client.clone(),
            cpu,
            input: client.create_from_slice(u32::as_bytes(values)),
            queries: client.create_from_slice(u32::as_bytes(queries)),
            input_length: values.len(),
            query_length: queries.len(),
            columns: u32::try_from(columns)
                .map_err(|_| ComputeError::Execution("column index overflow".into()))?,
            total_words,
            kind,
        })
    }

    pub(crate) fn dispatch(
        &self,
        word_offset: usize,
        word_count: usize,
    ) -> Result<Vec<u32>, ComputeError> {
        if word_count == 0
            || word_count > TILE_WORDS
            || word_offset
                .checked_add(word_count)
                .is_none_or(|end| end > self.total_words)
        {
            return Err(ComputeError::Execution(
                "invalid internal tile range".into(),
            ));
        }
        let word_offset = u32::try_from(word_offset)
            .map_err(|_| ComputeError::Execution("word offset overflow".into()))?;
        let output_handle = self.client.empty(word_count * size_of::<u32>());
        unsafe {
            masks::launch(
                &self.client,
                CubeCount::Static(1, 1, 1),
                CubeDim::new_1d(if self.cpu { 1 } else { 64 }),
                4,
                BufferArg::from_raw_parts(self.input.clone(), self.input_length),
                BufferArg::from_raw_parts(self.queries.clone(), self.query_length),
                BufferArg::from_raw_parts(output_handle.clone(), word_count),
                self.columns,
                word_offset,
                self.kind,
            );
        }
        let bytes = self
            .client
            .read_one(output_handle)
            .map_err(|error| ComputeError::Execution(crate::runtime::bounded_message(error)))?;
        if bytes.len() != word_count * size_of::<u32>() {
            return Err(ComputeError::Execution(
                "readback length differs from the dispatch".into(),
            ));
        }
        let mut words = zeroed_words(word_count)?;
        for (destination, source) in words.iter_mut().zip(bytes.as_chunks::<4>().0) {
            *destination = u32::from_ne_bytes(*source);
        }
        Ok(words)
    }
}
