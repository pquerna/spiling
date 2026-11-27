// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! An Auto session (a hardware GPU plus the CPU helper when one is present)
//! returns exact results under concurrent callers, whichever device leads
//! each round.

use spiling_compute::{ComputeBackend, ComputeSettings};
use std::sync::atomic::AtomicBool;

#[test]
fn concurrent_callers_get_exact_results_on_every_device() {
    spiling_compute::configure(ComputeSettings {
        default_backend: ComputeBackend::Auto,
        fea_backend: None,
    })
    .unwrap();
    let cancel = AtomicBool::new(false);
    let ranges: Vec<[f64; 2]> = (0..300)
        .map(|i| [f64::from(i) * 0.5, f64::from(i) * 0.5 + 7.25])
        .collect();
    std::thread::scope(|scope| {
        for thread in 0..16u64 {
            let (cancel, ranges) = (&cancel, &ranges);
            scope.spawn(move || {
                for round in 0..12u64 {
                    let seed = thread * 100 + round;
                    let (capsules, rows) = spiling_compute::capsule_cases::capsule_cases(seed, 120);
                    let group_ends = [40, 40, 120];
                    let masks = spiling_compute::capsule_masks(
                        &capsules,
                        &rows,
                        &group_ends,
                        128,
                        300,
                        cancel,
                    )
                    .unwrap();
                    spiling_compute::capsule_cases::assert_masks_match_reference(
                        &capsules,
                        &rows,
                        &group_ends,
                        &masks,
                        128,
                        300,
                    );
                    let coordinates: Vec<f64> = (0..50)
                        .map(|i| (i as f64 + seed as f64 * 0.01) * 3.0)
                        .collect();
                    let mask =
                        spiling_compute::row_crossings(ranges, &coordinates, cancel).unwrap();
                    for (row, coordinate) in coordinates.iter().enumerate() {
                        for (column, [start, end]) in ranges.iter().enumerate() {
                            let expected = (start <= coordinate && coordinate < end)
                                || (end <= coordinate && coordinate < start);
                            assert_eq!(mask.get(row, column), Some(expected));
                        }
                    }
                }
            });
        }
    });
}
