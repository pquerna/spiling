// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

#[test]
fn keys_preserve_independent_f64_order_and_signed_zero() {
    let mut coordinates = vec![
        -f64::MAX,
        -f64::MIN_POSITIVE,
        -f64::from_bits(1),
        -0.0,
        0.0,
        f64::from_bits(1),
        f64::MIN_POSITIVE,
        1.0,
        f64::from_bits(1.0f64.to_bits() + 1),
        f64::MAX,
    ];
    let mut state = 0x3d84_aa91_8f77_11a5u64;
    for _ in 0..1_024 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let value = f64::from_bits(state);
        if value.is_finite() {
            coordinates.push(value);
        }
    }
    for left in &coordinates {
        for right in &coordinates {
            assert_eq!(
                left.partial_cmp(right),
                Some(ordered_key(*left).cmp(&ordered_key(*right)))
            );
        }
    }
    assert_eq!(ordered_key(-0.0), ordered_key(0.0));
}

#[test]
fn masks_bound_access_and_scan_only_selected_columns() {
    let rows = MaskRows {
        rows: 2,
        columns: 35,
        words_per_row: 2,
        words: vec![0x8000_0001, 5, 0, 2],
    };
    assert_eq!(
        rows.selected_columns(0).unwrap().collect::<Vec<_>>(),
        [0, 31, 32, 34]
    );
    assert_eq!(rows.selected_columns(1).unwrap().collect::<Vec<_>>(), [33]);
    assert!(rows.selected_columns(2).is_none());
    assert!(rows.selected_columns(usize::MAX).is_none());
    assert_eq!(rows.get(0, 34), Some(true));
    assert_eq!(rows.get(1, 34), Some(false));
    assert_eq!(rows.get(0, 35), None);
    assert_eq!(rows.get(usize::MAX, usize::MAX), None);
}

#[test]
fn shapes_pad_rows_and_refuse_overflow() {
    let shape = Shape::new(3, 33).unwrap();
    assert_eq!((shape.words_per_row, shape.words), (2, 6));
    assert_eq!(Shape::new(5, 0).unwrap().words, 0);
    assert!(Shape::new(usize::MAX, usize::MAX).is_err());
}

#[test]
fn every_size_runs_and_cancellation_and_invalid_inputs_are_distinct() {
    crate::configure(crate::ComputeSettings {
        default_backend: crate::ComputeBackend::Cpu,
        fea_backend: None,
    })
    .unwrap();
    let cancel = AtomicBool::new(false);
    let single = row_crossings(&[[0.0, 1.0]], &[0.5], &cancel).unwrap();
    assert_eq!(single.get(0, 0), Some(true));
    let empty = box_overlaps(&[], &[[0.0, 0.0, 1.0, 1.0]], &cancel).unwrap();
    assert_eq!((empty.rows(), empty.columns()), (1, 0));
    assert_eq!(empty.selected_columns(0).unwrap().count(), 0);
    let none = interval_membership(&[[0.0, 1.0]], &[], &cancel).unwrap();
    assert_eq!((none.rows(), none.columns()), (0, 1));
    assert!(matches!(
        row_crossings(&[[f64::NAN, 0.0]], &[0.0], &cancel),
        Err(ComputeError::InvalidInput(_))
    ));
    assert!(matches!(
        interval_membership(&[[0.0, 1.0]], &[f64::INFINITY], &cancel),
        Err(ComputeError::InvalidInput(_))
    ));
    assert!(matches!(
        box_overlaps(&[[2.0, 0.0, 1.0, 1.0]], &[], &cancel),
        Err(ComputeError::InvalidInput(_))
    ));
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        row_crossings(&[], &[], &cancel),
        Err(ComputeError::Cancelled)
    );
    assert_eq!(
        box_overlaps(&[], &[], &cancel),
        Err(ComputeError::Cancelled)
    );
    assert_eq!(
        interval_membership(&[], &[], &cancel),
        Err(ComputeError::Cancelled)
    );
}

#[test]
fn masks_span_several_segments() {
    crate::configure(crate::ComputeSettings {
        default_backend: crate::ComputeBackend::Cpu,
        fea_backend: None,
    })
    .unwrap();
    let cancel = AtomicBool::new(false);
    let columns = 1 << 16;
    let rows = MASK_SEGMENT_WORDS / (columns / 32) * 2 + 5;
    let ranges = vec![[0.0, 1.0]; columns];
    let coordinates: Vec<f64> = (0..rows)
        .map(|row| if row % 3 == 0 { 0.5 } else { 2.0 })
        .collect();
    let mask = row_crossings(&ranges, &coordinates, &cancel).unwrap();
    assert_eq!(mask.words.len(), rows * columns / 32);
    for (row, words) in mask.words.chunks(columns / 32).enumerate() {
        let expected = if row % 3 == 0 { u32::MAX } else { 0 };
        assert!(words.iter().all(|&word| word == expected), "row {row}");
    }
}

#[test]
fn capsule_masks_match_the_exact_reference_on_the_cpu() {
    crate::configure(crate::ComputeSettings {
        default_backend: crate::ComputeBackend::Cpu,
        fea_backend: None,
    })
    .unwrap();
    let cancel = AtomicBool::new(false);
    let (capsules, rows) = crate::capsule_cases::capsule_cases(7, 300);
    let columns = 300;
    // Uneven groups, an empty one among them.
    let group_ends = [0, 50, 50, 121, 300];
    let masks = crate::capsule_masks(&capsules, &rows, &group_ends, 128, columns, &cancel).unwrap();
    let set = crate::capsule_cases::assert_masks_match_reference(
        &capsules,
        &rows,
        &group_ends,
        &masks,
        128,
        columns,
    );
    assert!(set > 1_000);
    assert_eq!(masks[0], crate::CellMask::default());
}

#[test]
fn wide_and_far_capsules_match_the_exact_reference_on_the_cpu() {
    crate::configure(crate::ComputeSettings {
        default_backend: crate::ComputeBackend::Cpu,
        fea_backend: None,
    })
    .unwrap();
    let cancel = AtomicBool::new(false);
    for (capsules, rows, columns) in [
        {
            let (capsules, rows) = crate::capsule_cases::wide_capsule_cases(3, 40);
            (capsules, rows, 300)
        },
        crate::capsule_cases::far_capsule_cases(5, 200),
    ] {
        let group_ends = [capsules.len() / 2, capsules.len()];
        let masks =
            crate::capsule_masks(&capsules, &rows, &group_ends, 128, columns, &cancel).unwrap();
        let set = crate::capsule_cases::assert_masks_match_reference(
            &capsules,
            &rows,
            &group_ends,
            &masks,
            128,
            columns,
        );
        assert!(set > 1_000);
    }
}
