// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use cubecl::prelude::*;

use crate::{ComputeError, zeroed_words};

#[cube]
fn ordered_le(left_hi: u32, left_lo: u32, right_hi: u32, right_lo: u32) -> bool {
    left_hi < right_hi || (left_hi == right_hi && left_lo <= right_lo)
}

#[cube]
fn ordered_lt(left_hi: u32, left_lo: u32, right_hi: u32, right_lo: u32) -> bool {
    left_hi < right_hi || (left_hi == right_hi && left_lo < right_lo)
}

/// Mask words of many independent groups in one launch. `table` holds the
/// `entry_count + 1` word starts, then four words per entry: entry `e` owns
/// output words `table[e]..table[e + 1]`, row-major with `words_per_row`
/// words per row; its padded four-lane input fields start at vector
/// `value_base` and its encoded query rows at `query_base`. A word reads only
/// its own entry, so every group's mask is the same however groups are batched.
#[cube(launch)]
fn masks<N: Size>(
    values: &[Vector<u32, N>],
    queries: &[u32],
    table: &[u32],
    output: &mut [u32],
    entry_count: u32,
    #[comptime] kind: u32,
) {
    let query_fields = if kind == 1 { 8usize } else { 2usize };
    let fields = entry_count as usize + 1;
    for position in range_stepped(ABSOLUTE_POS, output.len(), CUBE_DIM as usize * CUBE_COUNT) {
        let entry = capsule_of(table, entry_count, position as u32) as usize;
        let value_base = table[fields + entry * 4] as usize;
        let query_base = table[fields + entry * 4 + 1] as usize;
        let columns = table[fields + entry * 4 + 2] as usize;
        let words_per_row = table[fields + entry * 4 + 3] as usize;
        let local = position - table[entry] as usize;
        let row = local / words_per_row;
        let row_word = local % words_per_row;
        let field_stride = words_per_row * 8;
        let query = query_base + row * query_fields;
        let mut mask = 0u32;
        for group in 0..8usize {
            let base = value_base + row_word * 8 + group;
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
                        queries[query + 4],
                        queries[query + 5],
                    ) && ordered_le(
                        queries[query],
                        queries[query + 1],
                        third_hi.extract(lane),
                        third_lo.extract(lane),
                    ) && ordered_le(
                        second_hi.extract(lane),
                        second_lo.extract(lane),
                        queries[query + 6],
                        queries[query + 7],
                    ) && ordered_le(
                        queries[query + 2],
                        queries[query + 3],
                        fourth_hi.extract(lane),
                        fourth_lo.extract(lane),
                    );
                    if column < columns && hit {
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
                        queries[query],
                        queries[query + 1],
                    );
                    let reverse_start = ordered_le(
                        second_hi.extract(lane),
                        second_lo.extract(lane),
                        queries[query],
                        queries[query + 1],
                    );
                    let mut forward_end = ordered_lt(
                        queries[query],
                        queries[query + 1],
                        second_hi.extract(lane),
                        second_lo.extract(lane),
                    );
                    let mut reverse_end = ordered_lt(
                        queries[query],
                        queries[query + 1],
                        first_hi.extract(lane),
                        first_lo.extract(lane),
                    );
                    if kind == 2 {
                        forward_end = ordered_le(
                            queries[query],
                            queries[query + 1],
                            second_hi.extract(lane),
                            second_lo.extract(lane),
                        );
                        reverse_end = ordered_le(
                            queries[query],
                            queries[query + 1],
                            first_hi.extract(lane),
                            first_lo.extract(lane),
                        );
                    }
                    if column < columns
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

/// Units per cube for a CPU launch: the CPU runtime runs one worker per unit
/// of a cube, so one cube of one unit per core uses every core.
pub(crate) fn cpu_units(client: &Client) -> u32 {
    std::thread::available_parallelism()
        .map_or(1, |cores| cores.get())
        .min(client.properties().hardware.max_units_per_cube as usize)
        .max(1) as u32
}

/// Cubes and units for `work` grid-stride items: one cube of every core on
/// the CPU, 64-unit cubes on a GPU.
fn launch_shape(client: &Client, cpu: bool, work: usize) -> (CubeCount, CubeDim) {
    if cpu {
        (
            CubeCount::Static(1, 1, 1),
            CubeDim::new_1d(cpu_units(client)),
        )
    } else {
        let cubes = work.div_ceil(64).clamp(1, 65_535) as u32;
        (CubeCount::Static(cubes, 1, 1), CubeDim::new_1d(64))
    }
}

/// One packed mask segment: the sole unsafe adapter for `masks`. Handles are
/// allocated from these exact u32 slices; lengths are scalar element counts,
/// four-lane inputs are padded, and checked launches retain bounds checks.
/// One grid-stride launch with no barriers, read back once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn masks_dispatch(
    client: &Client,
    cpu: bool,
    kind: u32,
    values: &[u32],
    queries: &[u32],
    entries: &[u32],
    starts: &[u32],
) -> Result<Vec<u32>, ComputeError> {
    let entry_count = entries.len() / 4;
    let words = starts.last().map_or(0, |&words| words as usize);
    if kind > 2
        || entry_count == 0
        || !entries.len().is_multiple_of(4)
        || starts.len() != entry_count + 1
        || words == 0
        || !values.len().is_multiple_of(4)
    {
        return Err(ComputeError::Execution(
            "invalid internal mask segment".into(),
        ));
    }
    let mut table = Vec::new();
    table
        .try_reserve_exact(starts.len() + entries.len())
        .map_err(|_| ComputeError::Allocation)?;
    table.extend_from_slice(starts);
    table.extend_from_slice(entries);
    let value_handle = client.create_from_slice(u32::as_bytes(values));
    let query_handle = client.create_from_slice(u32::as_bytes(queries));
    let table_handle = client.create_from_slice(u32::as_bytes(&table));
    let output_handle = client.empty(words * size_of::<u32>());
    let (cubes, units) = launch_shape(client, cpu, words);
    unsafe {
        masks::launch(
            client,
            cubes,
            units,
            4,
            BufferArg::from_raw_parts(value_handle, values.len()),
            BufferArg::from_raw_parts(query_handle, queries.len()),
            BufferArg::from_raw_parts(table_handle, table.len()),
            BufferArg::from_raw_parts(output_handle.clone(), words),
            entry_count as u32,
            kind,
        );
    }
    let bytes = client
        .read_one(output_handle)
        .map_err(|error| ComputeError::Execution(crate::runtime::bounded_message(error)))?;
    if bytes.len() != words * size_of::<u32>() {
        return Err(ComputeError::Execution(
            "readback length differs from the dispatch".into(),
        ));
    }
    let mut output = zeroed_words(words)?;
    for (destination, source) in output.iter_mut().zip(bytes.as_chunks::<4>().0) {
        *destination = u32::from_ne_bytes(*source);
    }
    Ok(output)
}

/// Distance from `value` to the closed interval `[low, high]`.
#[cube]
fn interval_gap<N: Int>(value: N, low: N, high: N) -> N {
    (low - value).max(value - high).max(N::new(0))
}

/// Whether `(px, py)` is within `sqrt(r2)` of segment `a`–`b`, exactly.
#[cube]
fn point_within<N: Int>(ax: N, ay: N, bx: N, by: N, px: N, py: N, r2: N) -> bool {
    let dx = bx - ax;
    let dy = by - ay;
    let wx = px - ax;
    let wy = py - ay;
    let along = wx * dx + wy * dy;
    let length2 = dx * dx + dy * dy;
    let ex = px - bx;
    let ey = py - by;
    let cross = dx * wy - dy * wx;
    let zero = N::new(0);
    let before = along <= zero && wx * wx + wy * wy <= r2;
    let after = along > zero && along >= length2 && ex * ex + ey * ey <= r2;
    // The only products wider than 32 bits on the narrow path.
    let wide_cross = i64::cast_from(cross);
    let beside = along > zero
        && along < length2
        && wide_cross * wide_cross <= i64::cast_from(r2) * i64::cast_from(length2);
    before || after || beside
}

/// Whether the closed square `[x0, x1] × [y0, y1]` meets the closed capsule
/// of squared radius `r2` around segment `a`–`b`, exactly in integers.
#[cube]
fn cell_meets<N: Int>(ax: N, ay: N, bx: N, by: N, r2: N, x0: N, y0: N, x1: N, y1: N) -> bool {
    let gax = interval_gap::<N>(ax, x0, x1);
    let gay = interval_gap::<N>(ay, y0, y1);
    let gbx = interval_gap::<N>(bx, x0, x1);
    let gby = interval_gap::<N>(by, y0, y1);
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
            let zero = N::new(0);
            let above = s0 > zero && s1 > zero && s2 > zero && s3 > zero;
            let below = s0 < zero && s1 < zero && s2 < zero && s3 < zero;
            meets = !above && !below;
        }
    }
    if !meets {
        meets = point_within::<N>(ax, ay, bx, by, x0, y0, r2)
            || point_within::<N>(ax, ay, bx, by, x1, y0, r2)
            || point_within::<N>(ax, ay, bx, by, x0, y1, r2)
            || point_within::<N>(ax, ay, bx, by, x1, y1, r2);
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
fn column_of<N: Int>(origin: N, offset: f32, cell: N, shift: N) -> N {
    // An arithmetic shift is floor division by the power-of-two cell.
    let base = origin >> shift;
    let rest = f32::cast_from(origin - base * cell) + offset;
    base + N::cast_from((rest / f32::cast_from(cell)).floor())
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
fn row_span<N: Int>(
    capsules: &[i64],
    index: usize,
    row: N,
    cell: N,
    shift: N,
    columns: N,
) -> (N, N) {
    let capsule = index * 5;
    let ax = N::cast_from(capsules[capsule]);
    let ay = N::cast_from(capsules[capsule + 1]);
    let bx = N::cast_from(capsules[capsule + 2]);
    let by = N::cast_from(capsules[capsule + 3]);
    let radius = N::cast_from(capsules[capsule + 4]);
    let one = N::new(1);
    let r2 = radius * radius;
    let y0 = row * cell;
    let y1 = y0 + cell;
    // The capsule's axis point nearest the row: on the segment where it
    // crosses the row's middle, else its endpoint nearest the row.
    let middle = y0 + (cell >> one);
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
    let zero = N::new(0);
    let first_column = ((ax.min(bx) - radius - one).max(zero) >> shift).min(columns - one);
    let last_column = ((ax.max(bx) + radius).max(zero) >> shift).min(columns - one);
    let seed = (seed_x.max(zero) >> shift)
        .max(first_column)
        .min(last_column);
    let mut low = N::new(-1);
    let mut high = N::new(-2);
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
            let guess_low = column_of::<N>(ax, left, cell, shift)
                .max(first_column)
                .min(last_column);
            let guess_high = column_of::<N>(ax, right, cell, shift)
                .max(guess_low)
                .min(last_column);
            if cell_meets::<N>(
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
                    && cell_meets::<N>(ax, ay, bx, by, r2, lo * cell - cell, y0, lo * cell, y1)
                {
                    lo -= one;
                }
                low = lo;
                let mut hi = guess_high;
                if cell_meets::<N>(ax, ay, bx, by, r2, hi * cell, y0, hi * cell + cell, y1) {
                    while hi < last_column
                        && cell_meets::<N>(
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
                        hi += one;
                    }
                } else {
                    hi -= one;
                    while !cell_meets::<N>(ax, ay, bx, by, r2, hi * cell, y0, hi * cell + cell, y1)
                    {
                        hi -= one;
                    }
                }
                high = hi;
            }
        }
    }
    if !guessed && cell_meets::<N>(ax, ay, bx, by, r2, seed * cell, y0, seed * cell + cell, y1) {
        // First hit in [first_column, seed].
        let mut lo = first_column;
        let mut hi = seed;
        while lo < hi {
            let mid = (lo + hi) >> one;
            if cell_meets::<N>(ax, ay, bx, by, r2, mid * cell, y0, mid * cell + cell, y1) {
                hi = mid;
            } else {
                lo = mid + one;
            }
        }
        low = lo;
        // Last hit in [seed, last_column].
        lo = seed;
        hi = last_column;
        while lo < hi {
            let mid = (lo + hi + one) >> one;
            if cell_meets::<N>(ax, ay, bx, by, r2, mid * cell, y0, mid * cell + cell, y1) {
                lo = mid;
            } else {
                hi = mid - one;
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
    shift: i64,
    columns: i64,
    job_offset: u32,
    job_count: u32,
    #[comptime] contiguous: bool,
    #[comptime] narrow: bool,
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
        // A row beyond the capsule's reach meets no cell; skipping it also
        // bounds every operand of the narrow path.
        let ay = capsules[index * 5 + 1];
        let by = capsules[index * 5 + 3];
        let radius = capsules[index * 5 + 4];
        let y0 = i64::cast_from(row) * cell;
        let mut low = i64::new(-1);
        let mut high = i64::new(-2);
        if y0 + cell >= ay.min(by) - radius && y0 <= ay.max(by) + radius {
            if narrow {
                let (l, h) = row_span::<i32>(
                    capsules,
                    index,
                    i32::cast_from(row),
                    i32::cast_from(cell),
                    i32::cast_from(shift),
                    i32::cast_from(columns),
                );
                low = i64::cast_from(l);
                high = i64::cast_from(h);
            } else {
                let (l, h) =
                    row_span::<i64>(capsules, index, i64::cast_from(row), cell, shift, columns);
                low = l;
                high = h;
            }
        }
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

/// Zero `output` on the device, so no zeroed buffer is uploaded.
#[cube(launch)]
fn clear(output: &mut [u32]) {
    for position in range_stepped(ABSOLUTE_POS, output.len(), CUBE_DIM as usize * CUBE_COUNT) {
        output[position] = 0u32;
    }
}

/// One uploaded capsule segment: its jobs launched in tiles over one output
/// of `words` 32-bit words zeroed on the device, read back once as 64-bit
/// words (each the low then the high 32-bit word).
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
) -> Result<Vec<u64>, ComputeError> {
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
    // Radii up to NARROW_RADIUS_TICKS keep every operand but one product
    // within 32 bits (see `row_span`), which the GPU runs natively.
    let narrow = capsules
        .as_chunks::<5>()
        .0
        .iter()
        .all(|capsule| capsule[4] <= NARROW_RADIUS_TICKS);
    let cpu_units = cpu_units(client);
    let capsule_handle = client.create_from_slice(i64::as_bytes(capsules));
    let start_handle = client.create_from_slice(u32::as_bytes(starts));
    let row_handle = client.create_from_slice(u32::as_bytes(first_rows));
    let capsule_window_handle = client.create_from_slice(u32::as_bytes(capsule_windows));
    let window_handle = client.create_from_slice(u32::as_bytes(windows));
    if !words.is_multiple_of(2) {
        return Err(ComputeError::Execution(
            "capsule output is not whole 64-bit words".into(),
        ));
    }
    let output_handle = client.empty(words * size_of::<u32>());
    {
        let (cubes, units) = launch_shape(client, cpu, words);
        unsafe {
            clear::launch(
                client,
                cubes,
                units,
                BufferArg::from_raw_parts(output_handle.clone(), words),
            );
        }
    }
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
                i64::from(cell.trailing_zeros()),
                columns,
                tile_offset,
                tile_jobs,
                cpu,
                narrow,
            );
        }
        offset += tile;
    }
    let read = |handle, length: usize| -> Result<cubecl::bytes::Bytes, ComputeError> {
        let bytes = client
            .read_one(handle)
            .map_err(|error| ComputeError::Execution(crate::runtime::bounded_message(error)))?;
        if bytes.len() != length * size_of::<u32>() {
            return Err(ComputeError::Execution(
                "readback length differs from the dispatch".into(),
            ));
        }
        Ok(bytes)
    };
    if read(overflow_handle, 1)?[..] != 0u32.to_ne_bytes() {
        return Err(ComputeError::Execution(
            "a capsule hit fell outside its window".into(),
        ));
    }
    let bytes = read(output_handle, words)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(words / 2)
        .map_err(|_| ComputeError::Allocation)?;
    values.extend(bytes.as_chunks::<8>().0.iter().map(|pair| {
        let low = u32::from_ne_bytes([pair[0], pair[1], pair[2], pair[3]]);
        let high = u32::from_ne_bytes([pair[4], pair[5], pair[6], pair[7]]);
        u64::from(low) | u64::from(high) << 32
    }));
    Ok(values)
}

/// Largest radius the 32-bit kernel path takes. Coordinates are within
/// `MAX_CAPSULE_TICKS` (2^30) and segments within 2^12 ticks, so with
/// radius at most 2^13 every offset a reachable cell has from either end
/// stays under 2^14 and each product under 2^29; only the side test's
/// squared cross product and `r2 * length2` take 64 bits.
const NARROW_RADIUS_TICKS: i64 = 1 << 13;

/// Jobs per capsule dispatch tile.
const CAPSULE_TILE_JOBS: usize = 1 << 21;
