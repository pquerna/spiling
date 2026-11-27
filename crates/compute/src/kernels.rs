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
/// Vulkan tiles use up to 64 cubes of 64 units. Readback drains every bounded tile.
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
                CubeCount::Static(
                    if self.cpu {
                        1
                    } else {
                        word_count.div_ceil(64) as u32
                    },
                    1,
                    1,
                ),
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

/// Distance from `value` to the closed interval `[low, high]`.
#[cube]
fn interval_gap(value: i64, low: i64, high: i64) -> i64 {
    (low - value).max(value - high).max(i64::new(0))
}

/// Whether `(px, py)` is within `sqrt(r2)` of segment `a`–`b`, exactly.
#[cube]
fn point_within(ax: i64, ay: i64, bx: i64, by: i64, px: i64, py: i64, r2: i64) -> bool {
    let dx = bx - ax;
    let dy = by - ay;
    let wx = px - ax;
    let wy = py - ay;
    let along = wx * dx + wy * dy;
    let length2 = dx * dx + dy * dy;
    let ex = px - bx;
    let ey = py - by;
    let cross = dx * wy - dy * wx;
    let before = along <= 0 && wx * wx + wy * wy <= r2;
    let after = along > 0 && along >= length2 && ex * ex + ey * ey <= r2;
    let beside = along > 0 && along < length2 && cross * cross <= r2 * length2;
    before || after || beside
}

/// Whether the closed square `[x0, x1] × [y0, y1]` meets the closed capsule
/// of squared radius `r2` around segment `a`–`b`, exactly in integers.
#[cube]
fn cell_meets(
    ax: i64,
    ay: i64,
    bx: i64,
    by: i64,
    r2: i64,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
) -> bool {
    let gax = interval_gap(ax, x0, x1);
    let gay = interval_gap(ay, y0, y1);
    let gbx = interval_gap(bx, x0, x1);
    let gby = interval_gap(by, y0, y1);
    let mut meets = gax * gax + gay * gay <= r2 || gbx * gbx + gby * gby <= r2;
    if !meets {
        let overlaps = ax.max(bx) >= x0 && ax.min(bx) <= x1 && ay.max(by) >= y0 && ay.min(by) <= y1;
        if overlaps {
            let dx = bx - ax;
            let dy = by - ay;
            let s0 = dx * (y0 - ay) - dy * (x0 - ax);
            let s1 = dx * (y0 - ay) - dy * (x1 - ax);
            let s2 = dx * (y1 - ay) - dy * (x0 - ax);
            let s3 = dx * (y1 - ay) - dy * (x1 - ax);
            let above = s0 > 0 && s1 > 0 && s2 > 0 && s3 > 0;
            let below = s0 < 0 && s1 < 0 && s2 < 0 && s3 < 0;
            meets = !above && !below;
        }
    }
    if !meets {
        meets = point_within(ax, ay, bx, by, x0, y0, r2)
            || point_within(ax, ay, bx, by, x1, y0, r2)
            || point_within(ax, ay, bx, by, x0, y1, r2)
            || point_within(ax, ay, bx, by, x1, y1, r2);
    }
    meets
}

/// The estimated x-range, relative to the capsule's start, of the capsule's
/// cross-section on the line `y` (relative too): the hull of both end disks'
/// and the side band's sections, or `low > high` when the line misses.
/// Single precision: only a starting guess for exact tests.
#[cube]
fn section(dx: f32, dy: f32, radius: f32, y: f32) -> (f32, f32) {
    let mut low = f32::new(1.0e30_f32);
    let mut high = f32::new(-1.0e30_f32);
    let r2 = radius * radius;
    let start = r2 - y * y;
    if start >= 0.0 {
        let half = start.sqrt();
        low = low.min(-half);
        high = high.max(half);
    }
    let end = r2 - (y - dy) * (y - dy);
    if end >= 0.0 {
        let half = end.sqrt();
        low = low.min(dx - half);
        high = high.max(dx + half);
    }
    let length2 = dx * dx + dy * dy;
    if length2 > 0.0 {
        let length = length2.sqrt();
        // 0 <= x dx + y dy <= L² and |dx y - dy x| <= r L.
        let mut band_low = f32::new(-1.0e30_f32);
        let mut band_high = f32::new(1.0e30_f32);
        let mut inside = true;
        if dx != 0.0 {
            let a = -y * dy / dx;
            let b = (length2 - y * dy) / dx;
            band_low = band_low.max(a.min(b));
            band_high = band_high.min(a.max(b));
        } else {
            let along = y * dy;
            inside = along >= 0.0 && along <= length2;
        }
        if dy != 0.0 {
            let a = (dx * y - radius * length) / dy;
            let b = (dx * y + radius * length) / dy;
            band_low = band_low.max(a.min(b));
            band_high = band_high.min(a.max(b));
        } else {
            inside = inside && (dx * y).abs() <= radius * length;
        }
        if inside && band_low <= band_high {
            low = low.min(band_low);
            high = high.max(band_high);
        }
    }
    (low, high)
}

/// The column holding relative position `offset` from absolute tick
/// `origin`, without losing the origin's precision.
#[cube]
fn column_of(origin: i64, offset: f32, cell: i64) -> i64 {
    let mut base = origin / cell;
    if base * cell > origin {
        base -= 1;
    }
    let rest = f32::cast_from(origin - base * cell) + offset;
    base + i64::cast_from((rest / f32::cast_from(cell)).floor())
}

/// The last capsule whose first job is at or before `job`.
#[cube]
fn capsule_of(starts: &[u32], capsule_count: u32, job: u32) -> u32 {
    let mut first = u32::new(0);
    let mut last = capsule_count - 1;
    while first < last {
        let middle = (first + last).div_ceil(2);
        if starts[middle as usize] <= job {
            first = middle;
        } else {
            last = middle - 1;
        }
    }
    first
}

/// The first and last column whose cell meets capsule `index` on `row`, or
/// `(-1, -2)` when none does. A single-precision guess of the interval is
/// settled by exact tests at its two ends, usually four tests in all; the
/// cells that meet form one interval, so walking from any hit is exact, and
/// a missed guess falls back to bisection from the capsule's seed column,
/// which is a hit whenever the row is.
#[cube]
fn row_span(capsules: &[i64], index: usize, row: i64, cell: i64, columns: i64) -> (i64, i64) {
    let capsule = index * 5;
    let ax = capsules[capsule];
    let ay = capsules[capsule + 1];
    let bx = capsules[capsule + 2];
    let by = capsules[capsule + 3];
    let radius = capsules[capsule + 4];
    let r2 = radius * radius;
    let y0 = row * cell;
    let y1 = y0 + cell;
    // The capsule's axis point nearest the row: on the segment where it
    // crosses the row's middle, else its endpoint nearest the row.
    let middle = y0 + cell / 2;
    let low_y = ay.min(by);
    let high_y = ay.max(by);
    let mut seed_x = ax;
    if middle <= low_y {
        if by < ay {
            seed_x = bx;
        }
    } else if middle >= high_y {
        if by > ay {
            seed_x = bx;
        }
    } else {
        seed_x = ax + (middle - ay) * (bx - ax) / (by - ay);
    }
    let first_column = ((ax.min(bx) - radius - 1).max(0) / cell).min(columns - 1);
    let last_column = ((ax.max(bx) + radius).max(0) / cell).min(columns - 1);
    let seed = (seed_x.max(0) / cell).max(first_column).min(last_column);
    let mut low = i64::new(-1);
    let mut high = i64::new(-2);
    // A single-precision guess of the row's interval, settled by exact
    // tests at its two ends: usually four tests in all. The cells that
    // meet form one interval, so walking from any hit is exact.
    let fdx = f32::cast_from(bx - ax);
    let fdy = f32::cast_from(by - ay);
    let fr = f32::cast_from(radius);
    let floor = f32::cast_from(low_y - ay) - fr;
    let ceiling = f32::cast_from(high_y - ay) + fr;
    let band_low = f32::cast_from(y0 - ay).max(floor);
    let band_high = f32::cast_from(y1 - ay).min(ceiling);
    let mut guessed = false;
    if band_low <= band_high {
        // The section's ends are concave in y, so each is extreme where
        // the extreme end point's height is clamped into the band.
        let mut right_y = f32::new(0.0_f32);
        let mut left_y = fdy;
        if bx > ax {
            right_y = fdy;
            left_y = f32::new(0.0_f32);
        }
        let left = section(fdx, fdy, fr, left_y.max(band_low).min(band_high)).0;
        let right = section(fdx, fdy, fr, right_y.max(band_low).min(band_high)).1;
        if left <= right {
            let guess_low = column_of(ax, left, cell).max(first_column).min(last_column);
            let guess_high = column_of(ax, right, cell).max(guess_low).min(last_column);
            if cell_meets(
                ax,
                ay,
                bx,
                by,
                r2,
                guess_low * cell,
                y0,
                guess_low * cell + cell,
                y1,
            ) {
                guessed = true;
                let mut lo = guess_low;
                while lo > first_column
                    && cell_meets(ax, ay, bx, by, r2, lo * cell - cell, y0, lo * cell, y1)
                {
                    lo -= 1;
                }
                low = lo;
                let mut hi = guess_high;
                if cell_meets(ax, ay, bx, by, r2, hi * cell, y0, hi * cell + cell, y1) {
                    while hi < last_column
                        && cell_meets(
                            ax,
                            ay,
                            bx,
                            by,
                            r2,
                            hi * cell + cell,
                            y0,
                            hi * cell + cell + cell,
                            y1,
                        )
                    {
                        hi += 1;
                    }
                } else {
                    hi -= 1;
                    while !cell_meets(ax, ay, bx, by, r2, hi * cell, y0, hi * cell + cell, y1) {
                        hi -= 1;
                    }
                }
                high = hi;
            }
        }
    }
    if !guessed && cell_meets(ax, ay, bx, by, r2, seed * cell, y0, seed * cell + cell, y1) {
        // First hit in [first_column, seed].
        let mut lo = first_column;
        let mut hi = seed;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if cell_meets(ax, ay, bx, by, r2, mid * cell, y0, mid * cell + cell, y1) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        low = lo;
        // Last hit in [seed, last_column].
        lo = seed;
        hi = last_column;
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if cell_meets(ax, ay, bx, by, r2, mid * cell, y0, mid * cell + cell, y1) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        high = lo;
    }
    (low, high)
}

/// Every job's cells, OR-ed into its capsule's group window. Capsules are
/// `[ax, ay, bx, by, r]` in integer ticks of `cell` per grid cell. Capsule
/// `i` owns jobs `starts[i]..starts[i + 1]`, for rows from `first_rows[i]`
/// up; a job finds its capsule by bisection, so no per-job input exists.
/// Window `w` is `windows[5w..5w + 5]`: its first output word, first row,
/// rows, first word column and width, in 32-cell words. A hit outside its
/// window sets `overflow` instead of being written.
#[cube(launch)]
fn capsule_masks(
    capsules: &[i64],
    starts: &[u32],
    first_rows: &[u32],
    capsule_windows: &[u32],
    windows: &[u32],
    output: &mut [Atomic<u32>],
    overflow: &mut [Atomic<u32>],
    cell: i64,
    columns: i64,
    job_offset: u32,
    job_count: u32,
    #[comptime] contiguous: bool,
) {
    let capsule_count = first_rows.len() as u32;
    let count = job_count as usize;
    let workers = CUBE_DIM as usize * CUBE_COUNT;
    // GPU units stride through the jobs; CPU units each take one
    // contiguous block, stepping from capsule to capsule.
    let mut begin = ABSOLUTE_POS;
    let mut end = count;
    let mut step = workers;
    if contiguous {
        let block = count.div_ceil(workers);
        begin = (ABSOLUTE_POS * block).min(count);
        end = (begin + block).min(count);
        step = 1;
    }
    let mut first = u32::new(0);
    for position in range_stepped(begin, end, step) {
        let job = position as u32 + job_offset;
        if contiguous {
            if position == begin {
                first = capsule_of(starts, capsule_count, job);
            } else {
                while starts[first as usize + 1] <= job {
                    first += 1;
                }
            }
        } else {
            first = capsule_of(starts, capsule_count, job);
        }
        let index = first as usize;
        let row = first_rows[index] + (job - starts[index]);
        let (low, high) = row_span(capsules, index, row as i64, cell, columns);
        if low >= 0 {
            let window = capsule_windows[index] as usize * 5;
            let base = windows[window];
            let row0 = windows[window + 1];
            let rows = windows[window + 2];
            let word0 = windows[window + 3];
            let width = windows[window + 4];
            let lo = low as u32;
            let hi = high as u32;
            let first_word = lo / 32;
            let last_word = hi / 32;
            if row < row0 || row - row0 >= rows || first_word < word0 || last_word >= word0 + width
            {
                overflow[0].fetch_or(1u32);
            } else {
                let line = base + (row - row0) * width - word0;
                let mut word = first_word;
                while word <= last_word {
                    let from = lo.max(word * 32) - word * 32;
                    let to = hi.min(word * 32 + 31) - word * 32;
                    let mask = (u32::MAX << from) & (u32::MAX >> (31 - to));
                    output[(line + word) as usize].fetch_or(mask);
                    word += 1;
                }
            }
        }
    }
}

/// One uploaded capsule segment: its jobs launched in tiles over one zeroed
/// output of `words` words, read back once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn capsule_masks_dispatch(
    client: &Client,
    cpu: bool,
    capsules: &[i64],
    starts: &[u32],
    first_rows: &[u32],
    capsule_windows: &[u32],
    windows: &[u32],
    words: usize,
    cell: i64,
    columns: i64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Vec<u32>, ComputeError> {
    let count = first_rows.len();
    let job_count = starts.last().map_or(0, |&jobs| jobs as usize);
    if count == 0
        || capsules.len() != count * 5
        || starts.len() != count + 1
        || capsule_windows.len() != count
        || !windows.len().is_multiple_of(5)
        || words == 0
    {
        return Err(ComputeError::Execution(
            "invalid internal capsule batch".into(),
        ));
    }
    let cpu_units = std::thread::available_parallelism()
        .map_or(1, |cores| cores.get())
        .min(client.properties().hardware.max_units_per_cube as usize)
        .max(1) as u32;
    let capsule_handle = client.create_from_slice(i64::as_bytes(capsules));
    let start_handle = client.create_from_slice(u32::as_bytes(starts));
    let row_handle = client.create_from_slice(u32::as_bytes(first_rows));
    let capsule_window_handle = client.create_from_slice(u32::as_bytes(capsule_windows));
    let window_handle = client.create_from_slice(u32::as_bytes(windows));
    let zeros = zeroed_words(words)?;
    let output_handle = client.create_from_slice(u32::as_bytes(&zeros));
    drop(zeros);
    let overflow_handle = client.create_from_slice(u32::as_bytes(&[0u32]));
    let mut offset = 0usize;
    while offset < job_count {
        crate::check_cancel(cancel)?;
        let tile = (job_count - offset).min(CAPSULE_TILE_JOBS);
        // The CPU runtime runs one worker per unit of a cube.
        let units: u32 = if cpu { cpu_units } else { 64 };
        let cubes = if cpu { 1 } else { tile.div_ceil(64) as u32 };
        let tile_jobs = u32::try_from(tile)
            .map_err(|_| ComputeError::Execution("capsule tile overflow".into()))?;
        let tile_offset = u32::try_from(offset)
            .map_err(|_| ComputeError::Execution("job offset overflow".into()))?;
        unsafe {
            capsule_masks::launch(
                client,
                CubeCount::Static(cubes.max(1), 1, 1),
                CubeDim::new_1d(units),
                BufferArg::from_raw_parts(capsule_handle.clone(), capsules.len()),
                BufferArg::from_raw_parts(start_handle.clone(), starts.len()),
                BufferArg::from_raw_parts(row_handle.clone(), first_rows.len()),
                BufferArg::from_raw_parts(capsule_window_handle.clone(), capsule_windows.len()),
                BufferArg::from_raw_parts(window_handle.clone(), windows.len()),
                BufferArg::from_raw_parts(output_handle.clone(), words),
                BufferArg::from_raw_parts(overflow_handle.clone(), 1),
                cell,
                columns,
                tile_offset,
                tile_jobs,
                cpu,
            );
        }
        offset += tile;
    }
    let read = |handle, length: usize| -> Result<Vec<u32>, ComputeError> {
        let bytes = client
            .read_one(handle)
            .map_err(|error| ComputeError::Execution(crate::runtime::bounded_message(error)))?;
        if bytes.len() != length * size_of::<u32>() {
            return Err(ComputeError::Execution(
                "readback length differs from the dispatch".into(),
            ));
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(length)
            .map_err(|_| ComputeError::Allocation)?;
        values.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| u32::from_ne_bytes(*chunk)),
        );
        Ok(values)
    };
    if read(overflow_handle, 1)? != [0] {
        return Err(ComputeError::Execution(
            "a capsule hit fell outside its window".into(),
        ));
    }
    read(output_handle, words)
}

/// Jobs per capsule dispatch tile.
const CAPSULE_TILE_JOBS: usize = 1 << 21;
