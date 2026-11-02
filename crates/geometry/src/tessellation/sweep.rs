// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{invalid, resource};
use crate::{GeometryWorkBudget, compute};
use spiling_contracts::geometry::GeometryError;
use std::sync::atomic::AtomicBool;

pub(super) fn pairs(
    count: usize,
    endpoints: impl Fn(usize) -> [[f64; 2]; 2],
    outside: usize,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
    visit: impl FnMut(usize, usize) -> Result<(), GeometryError>,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let bytes = super::PROOF_BYTES
        .checked_sub(outside)
        .ok_or_else(|| resource("display boundary sweep exceeds live storage allowance"))?;
    compute::box_pairs(
        count,
        |index| {
            let points = endpoints(index);
            if points.into_iter().flatten().any(|value| !value.is_finite()) {
                return Err(invalid(
                    "display boundary sweep requires finite UV endpoints",
                ));
            }
            Ok((
                std::array::from_fn(|axis| points[0][axis].min(points[1][axis])),
                std::array::from_fn(|axis| points[0][axis].max(points[1][axis])),
            ))
        },
        bytes,
        work,
        cancel,
        visit,
    )
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

    #[test]
    fn sweep_prunes_axis_degenerate_and_rotated_boxes() {
        let count = 32_768;
        for axis in 0..2 {
            let work = GeometryWorkBudget::new(10_000_000);
            pairs(
                count,
                |index| {
                    let mut endpoints = [
                        [-100_000.0, index as f64 * 4.0],
                        [100_000.0, index as f64 * 4.0 + 1.0],
                    ];
                    if axis == 1 {
                        for point in &mut endpoints {
                            point.swap(0, 1);
                        }
                    }
                    endpoints
                },
                0,
                &work,
                &AtomicBool::new(false),
                |_, _| panic!("disjoint boxes cannot need exact pair checks"),
            )
            .unwrap();
            assert!(work.consumed() < count as u64 * (count as u64 - 1) / 100);
            eprintln!(
                "axis-degenerate boxes={count} axis={axis} work={}",
                work.consumed()
            );
        }
    }

    #[test]
    fn sweep_retains_dense_contacts_and_cancellation_during_publication() {
        for count in [0, 1, 2, 3, 17, 128] {
            let mut actual = Vec::new();
            pairs(
                count,
                |_| [[-0.0, 0.0], [0.0, 0.0]],
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
            let expected: Vec<_> = (0..count)
                .flat_map(|left| (left + 1..count).map(move |right| (left, right)))
                .collect();
            assert_eq!(actual, expected);
        }
        let cancel = AtomicBool::new(false);
        let mut visits = 0;
        let failure = pairs(
            128,
            |_| [[0.0, 0.0]; 2],
            0,
            &GeometryWorkBudget::query(),
            &cancel,
            |_, _| {
                visits += 1;
                cancel.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::Cancelled);
        assert_eq!(visits, 1);
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                pairs(
                    1,
                    |_| [[invalid, 0.0], [0.0, 0.0]],
                    0,
                    &GeometryWorkBudget::query(),
                    &AtomicBool::new(false),
                    |_, _| Ok(())
                )
                .is_err()
            );
        }
    }
}
