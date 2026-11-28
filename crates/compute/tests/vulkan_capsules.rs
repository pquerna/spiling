// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! The capsule kernel on a Vulkan adapter equals the exact host reference.
//! Skips when no Vulkan adapter is present.

use spiling_compute::{ComputeBackend, ComputeSettings};
use std::sync::atomic::AtomicBool;

#[test]
fn capsule_masks_match_the_exact_reference_on_vulkan() {
    if spiling_compute::configure(ComputeSettings {
        default_backend: ComputeBackend::Vulkan,
        fea_backend: None,
    })
    .is_err()
    {
        eprintln!("no Vulkan adapter; skipped");
        return;
    }
    let cancel = AtomicBool::new(false);
    let (capsules, rows) = spiling_compute::capsule_cases::capsule_cases(11, 400);
    let columns = 300;
    let group_ends = [7, 7, 200, 400];
    let masks =
        spiling_compute::capsule_masks(&capsules, &rows, &group_ends, 128, columns, &cancel)
            .unwrap();
    spiling_compute::capsule_cases::assert_masks_match_reference(
        &capsules,
        &rows,
        &group_ends,
        &masks,
        128,
        columns,
    );
    // The 64-bit path, and the narrow one at the far edge of the range.
    for (capsules, rows, columns) in [
        {
            let (capsules, rows) = spiling_compute::capsule_cases::wide_capsule_cases(3, 40);
            (capsules, rows, 300)
        },
        spiling_compute::capsule_cases::far_capsule_cases(5, 200),
    ] {
        let group_ends = [capsules.len() / 2, capsules.len()];
        let masks =
            spiling_compute::capsule_masks(&capsules, &rows, &group_ends, 128, columns, &cancel)
                .unwrap();
        spiling_compute::capsule_cases::assert_masks_match_reference(
            &capsules,
            &rows,
            &group_ends,
            &masks,
            128,
            columns,
        );
    }
}
