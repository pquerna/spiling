// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{check_memory, invalid, resource};
use crate::{GeometryWorkBudget, compute};
use spiling_contracts::geometry::GeometryError;
use std::{mem::size_of, sync::atomic::AtomicBool};

struct Entry {
    minimum: [f64; 2],
    maximum: [f64; 2],
    index: usize,
}

pub(super) fn pairs(
    count: usize,
    endpoints: impl Fn(usize) -> [[f64; 2]; 2],
    outside: usize,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
    mut visit: impl FnMut(usize, usize) -> Result<(), GeometryError>,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let bytes = count
        .checked_mul(size_of::<Entry>())
        .and_then(|bytes| outside.checked_add(bytes))
        .ok_or_else(|| resource("display boundary sweep storage overflow"))?;
    check_memory(bytes)?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(count)
        .map_err(|_| resource("display boundary sweep allocation failed"))?;
    check_memory(outside.saturating_add(entries.capacity().saturating_mul(size_of::<Entry>())))?;
    for index in 0..count {
        work.charge(1, cancel)?;
        let points = endpoints(index);
        if points.into_iter().flatten().any(|value| !value.is_finite()) {
            return Err(invalid(
                "display boundary sweep requires finite UV endpoints",
            ));
        }
        entries.push(Entry {
            minimum: std::array::from_fn(|axis| points[0][axis].min(points[1][axis])),
            maximum: std::array::from_fn(|axis| points[0][axis].max(points[1][axis])),
            index,
        });
    }
    compute::sort_unstable_by(
        &mut entries,
        |left, right| {
            left.minimum[0]
                .total_cmp(&right.minimum[0])
                .then(left.index.cmp(&right.index))
        },
        work,
        cancel,
    )?;
    for (index, left) in entries.iter().enumerate() {
        work.charge(1, cancel)?;
        for right in &entries[index + 1..] {
            work.charge(1, cancel)?;
            if right.minimum[0] > left.maximum[0] {
                break;
            }
            if left.maximum[1] < right.minimum[1] || right.maximum[1] < left.minimum[1] {
                continue;
            }
            visit(left.index.min(right.index), left.index.max(right.index))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::geometry::GeometryErrorCode;

    #[test]
    fn sweep_matches_closed_box_all_pairs_including_degenerate_and_extreme_bounds() {
        let mut segments = vec![
            [[0.0, 0.0], [1.0, 1.0]],
            [[1.0, 1.0], [2.0, 0.0]],
            [[0.5, 0.5], [0.5, 0.5]],
            [[-0.0, -1.0], [0.0, 1.0]],
            [[-f64::MAX, -f64::MAX], [-f64::MAX, f64::MAX]],
            [[f64::MAX, -f64::MAX], [f64::MAX, f64::MAX]],
        ];
        for index in 0..200 {
            let x = (index * 37 % 101) as f64 - 50.0;
            let y = (index * 23 % 103) as f64 - 50.0;
            segments.push([[x, y], [x + (index % 9) as f64, y - (index % 7) as f64]]);
        }
        for reverse in [false, true] {
            if reverse {
                segments.reverse();
            }
            let mut expected = Vec::new();
            for (index, left) in segments.iter().enumerate() {
                for (other, right) in segments.iter().enumerate().skip(index + 1) {
                    if !(0..2).any(|axis| {
                        left[0][axis].max(left[1][axis]) < right[0][axis].min(right[1][axis])
                            || right[0][axis].max(right[1][axis]) < left[0][axis].min(left[1][axis])
                    }) {
                        expected.push((index, other));
                    }
                }
            }
            let mut actual = Vec::new();
            pairs(
                segments.len(),
                |index| segments[index],
                0,
                &GeometryWorkBudget::query(),
                &AtomicBool::new(false),
                |left, right| {
                    actual.push((left, right));
                    Ok(())
                },
            )
            .unwrap();
            actual.sort_unstable();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn sweep_admits_storage_and_checks_cancel_and_work_before_reading_endpoints() {
        for (count, outside, work, cancelled, code) in [
            (usize::MAX, 0, 10, false, GeometryErrorCode::ResourceLimit),
            (
                1,
                super::super::PROOF_BYTES,
                10,
                false,
                GeometryErrorCode::ResourceLimit,
            ),
            (1, 0, 0, false, GeometryErrorCode::ResourceLimit),
            (1, 0, 10, true, GeometryErrorCode::Cancelled),
        ] {
            let error = pairs(
                count,
                |_| panic!("storage/work/cancellation must be checked before inventory growth"),
                outside,
                &GeometryWorkBudget::new(work),
                &AtomicBool::new(cancelled),
                |_, _| panic!("no pair can be published"),
            )
            .unwrap_err();
            assert_eq!(error.code, code);
        }
    }

    #[test]
    fn sweep_prunes_separated_columns_and_retains_narrow_limit_refusal() {
        let count = 32_768;
        let endpoints = |index: usize| {
            let x = (index % 256) as f64 * 4.0;
            let y = (index / 256) as f64 * 4.0;
            [[x, y], [x + 1.0, y + 1.0]]
        };
        let work = GeometryWorkBudget::new(10_000_000);
        let cancel = AtomicBool::new(false);
        pairs(count, endpoints, 0, &work, &cancel, |_, _| {
            panic!("separated boxes cannot need exact pair checks")
        })
        .unwrap();
        assert!(work.consumed() < (count as u64 * (count as u64 - 1)) / 100);
        let failure = pairs(
            count,
            endpoints,
            0,
            &GeometryWorkBudget::new(work.consumed() - 1),
            &cancel,
            |_, _| Ok(()),
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
    }
}
