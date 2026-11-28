// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Shared generator for capsule kernel reference tests.
/// Pseudo-random capsules and, for each, every row it can reach as
/// `[first row, row count]`.
pub fn capsule_cases(seed: u64, count: usize) -> (Vec<[i64; 5]>, Vec<[u32; 2]>) {
    let mut state = seed;
    let mut next = move |bound: i64| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as i64).rem_euclid(bound)
    };
    let cell = 128;
    let mut capsules = Vec::new();
    let mut rows = Vec::new();
    for _ in 0..count {
        let ax = next(200 * cell);
        let ay = next(200 * cell);
        let bx = (ax + next(8193) - 4096).max(0);
        let by = (ay + next(8193) - 4096).max(0);
        let radius = match next(5) {
            0 => 0,
            1 => next(4),
            // The narrow kernel path's largest radius.
            2 => 1 << 13,
            _ => next(3000),
        };
        capsules.push([ax, ay, bx, by, radius]);
        let low = ((ay.min(by) - radius - 1).max(0) / cell) as u32;
        let high = ((ay.max(by) + radius) / cell + 1) as u32;
        rows.push([low, high - low + 1]);
    }
    (capsules, rows)
}

/// Capsules past the narrow kernel path, in batches of their own: radii
/// above 2^13 ticks, so the 64-bit path decides them.
pub fn wide_capsule_cases(seed: u64, count: usize) -> (Vec<[i64; 5]>, Vec<[u32; 2]>) {
    let (mut capsules, _) = capsule_cases(seed, count);
    let cell = 128;
    let mut rows = Vec::new();
    for (index, capsule) in capsules.iter_mut().enumerate() {
        capsule[4] = (1 << 13) + 1 + (index as i64 * 7_919) % 40_000;
        let [_, ay, _, by, radius] = *capsule;
        let low = ((ay.min(by) - radius - 1).max(0) / cell) as u32;
        let high = ((ay.max(by) + radius) / cell + 1) as u32;
        rows.push([low, high - low + 1]);
    }
    (capsules, rows)
}

/// Narrow-path capsules at the far edge of the coordinate range, where
/// absolute ticks approach 2^30, on a grid wide enough to hold them.
pub fn far_capsule_cases(seed: u64, count: usize) -> (Vec<[i64; 5]>, Vec<[u32; 2]>, i64) {
    let (mut capsules, _) = capsule_cases(seed, count);
    let cell = 128;
    let far = crate::MAX_CAPSULE_TICKS - 300 * cell;
    let mut rows = Vec::new();
    for capsule in &mut capsules {
        for value in &mut capsule[..4] {
            *value += far;
        }
        let [_, ay, _, by, radius] = *capsule;
        let low = ((ay.min(by) - radius - 1).max(0) / cell) as u32;
        let high = ((ay.max(by) + radius) / cell + 1) as u32;
        rows.push([low, high - low + 1]);
    }
    (capsules, rows, crate::MAX_CAPSULE_TICKS / cell + 1)
}

/// Asserts that `masks` hold exactly the exact host reference's cells for
/// `capsule_cases` split into groups at `group_ends`, and returns how many
/// cells are set.
pub fn assert_masks_match_reference(
    capsules: &[[i64; 5]],
    rows: &[[u32; 2]],
    group_ends: &[usize],
    masks: &[crate::CellMask],
    cell: i64,
    columns: i64,
) -> usize {
    assert_eq!(masks.len(), group_ends.len());
    let mut set = 0;
    let mut first = 0;
    for (&end, mask) in group_ends.iter().zip(masks) {
        let mut want = std::collections::BTreeSet::new();
        for (&capsule, &[row0, count]) in capsules[first..end].iter().zip(&rows[first..end]) {
            for row in row0..row0 + count {
                let [from, to] = crate::capsule_rows_reference(capsule, row, cell, columns);
                for column in from.max(0)..=to {
                    want.insert((row, column as u32));
                }
            }
        }
        let mut got = std::collections::BTreeSet::new();
        assert_eq!(mask.words.len(), mask.rows as usize * mask.width as usize);
        for (index, &word) in mask.words.iter().enumerate() {
            let row = mask.row0 + (index / mask.width as usize) as u32;
            let column0 = (mask.word0 as usize + index % mask.width as usize) * 64;
            for bit in 0..64 {
                if word >> bit & 1 == 1 {
                    got.insert((row, (column0 + bit) as u32));
                }
            }
        }
        assert_eq!(got, want, "group ending at capsule {end}");
        set += got.len();
        first = end;
    }
    set
}
