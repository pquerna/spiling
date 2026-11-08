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
fn admission_bounds_padding_and_overflow() {
    assert!(Shape::admit(0, 0).is_none());
    assert!(Shape::admit(1, MIN_BATCH_PAIRS - 1).is_none());
    assert!(Shape::admit(1, MIN_BATCH_PAIRS).is_some());
    assert!(Shape::admit(128, MAX_BATCH_ITEMS).is_some());
    assert!(Shape::admit(129, MAX_BATCH_ITEMS).is_none());
    assert!(Shape::admit(MAX_BATCH_ITEMS, 128).is_some());
    assert!(Shape::admit(MAX_BATCH_ITEMS, 129).is_none());
    assert!(Shape::admit(usize::MAX, usize::MAX).is_none());
}

#[test]
fn declines_cancellation_and_invalid_inputs_are_distinct() {
    let cancel = AtomicBool::new(false);
    assert_eq!(row_crossings(&[[0.0, 1.0]], &[0.5], &cancel).unwrap(), None);
    assert_eq!(box_overlaps(&[], &[], &cancel).unwrap(), None);
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
