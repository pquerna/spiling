use super::*;
use std::{
    cell::{Cell, RefCell},
    sync::atomic::Ordering,
};

fn checked_sift<T>(
    values: &mut [T],
    mut root: usize,
    end: usize,
    key: &impl Fn(&T) -> f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    loop {
        check_cancel(cancel)?;
        let Some(mut child) = root.checked_mul(2).and_then(|value| value.checked_add(1)) else {
            return Err(resource("Boolean heap index overflow"));
        };
        if child >= end {
            return Ok(());
        }
        if child + 1 < end {
            charge(work, 1)?;
            if key(&values[child])
                .total_cmp(&key(&values[child + 1]))
                .is_lt()
            {
                child += 1;
            }
        }
        charge(work, 1)?;
        if !key(&values[root]).total_cmp(&key(&values[child])).is_lt() {
            return Ok(());
        }
        charge(work, 1)?;
        values.swap(root, child);
        root = child;
    }
}

fn checked_sort<T>(
    values: &mut [T],
    key: impl Fn(&T) -> f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let count = values.len();
    for root in (0..count / 2).rev() {
        checked_sift(values, root, count, &key, work, cancel)?;
    }
    for end in (1..count).rev() {
        check_cancel(cancel)?;
        charge(work, 1)?;
        values.swap(0, end);
        checked_sift(values, 0, end, &key, work, cancel)?;
    }
    Ok(())
}

fn identities(values: &[(usize, f64)]) -> Vec<usize> {
    values.iter().map(|value| value.0).collect()
}

#[test]
fn batched_sift_matches_checked_work_order_and_every_refusal_boundary() {
    let cancel = AtomicBool::new(false);
    for count in [0, 1, 2, 3, 8, 31, 64] {
        let source: Vec<_> = (0..count)
            .map(|index| {
                let key = match index % 11 {
                    0 => -0.0,
                    1 => 0.0,
                    2 => f64::INFINITY,
                    3 => f64::NEG_INFINITY,
                    4 => f64::NAN,
                    _ => ((index * 37) % 13) as f64 - 6.0,
                };
                (index, key)
            })
            .collect();
        let mut full = source.clone();
        let mut full_work = 0;
        checked_sort(&mut full, |value| value.1, &mut full_work, &cancel).unwrap();
        assert!(
            full.windows(2)
                .all(|pair| pair[0].1.total_cmp(&pair[1].1).is_le())
        );
        for remaining in 0..=full_work + 1 {
            let mut expected = source.clone();
            let mut actual = source.clone();
            let mut expected_work = usize::MAX - remaining;
            let mut actual_work = expected_work;
            let expected_trace = RefCell::new(Vec::new());
            let actual_trace = RefCell::new(Vec::new());
            let expected_result = checked_sort(
                &mut expected,
                |value| {
                    expected_trace.borrow_mut().push(value.0);
                    value.1
                },
                &mut expected_work,
                &cancel,
            );
            let actual_result = sort_by(
                &mut actual,
                |value| {
                    actual_trace.borrow_mut().push(value.0);
                    value.1
                },
                &mut actual_work,
                &cancel,
            );
            assert_eq!(actual_result.is_ok(), expected_result.is_ok());
            if let (Err(actual), Err(expected)) = (actual_result, expected_result) {
                assert_eq!(actual.code, expected.code);
                assert_eq!(actual.message, expected.message);
            }
            assert_eq!(actual_work, expected_work);
            assert_eq!(actual_trace.into_inner(), expected_trace.into_inner());
            assert_eq!(identities(&actual), identities(&expected));
        }
        let mut actual = source;
        let mut actual_work = 0;
        sort_by(&mut actual, |value| value.1, &mut actual_work, &cancel).unwrap();
        assert_eq!(actual_work, full_work);
        assert_eq!(identities(&actual), identities(&full));
    }
}

#[test]
fn batched_sift_settles_exact_work_on_mid_sift_cancellation() {
    let source: Vec<_> = (0..256).map(|index| (index, (index % 23) as f64)).collect();
    for (after, initial_work) in [0, 1, 2, 3, 10, 100, 1_000].into_iter().flat_map(|after| {
        [0, usize::MAX - 192, usize::MAX - 2, usize::MAX]
            .into_iter()
            .map(move |work| (after, work))
    }) {
        let expected_cancel = AtomicBool::new(after == 0);
        let actual_cancel = AtomicBool::new(after == 0);
        let expected_calls = Cell::new(0);
        let actual_calls = Cell::new(0);
        let key = |value: &(usize, f64), calls: &Cell<usize>, cancel: &AtomicBool| {
            calls.set(calls.get() + 1);
            if calls.get() >= after {
                cancel.store(true, Ordering::Relaxed);
            }
            value.1
        };
        let mut expected = source.clone();
        let mut actual = source.clone();
        let mut expected_work = initial_work;
        let mut actual_work = initial_work;
        let expected_error = checked_sort(
            &mut expected,
            |value| key(value, &expected_calls, &expected_cancel),
            &mut expected_work,
            &expected_cancel,
        )
        .unwrap_err();
        let actual_error = sort_by(
            &mut actual,
            |value| key(value, &actual_calls, &actual_cancel),
            &mut actual_work,
            &actual_cancel,
        )
        .unwrap_err();
        if initial_work == 0 || after == 0 {
            assert_eq!(actual_error.code, ManufacturingErrorCode::Cancelled);
        }
        assert_eq!(actual_error.code, expected_error.code);
        assert_eq!(actual_error.message, expected_error.message);
        assert_eq!(actual_calls.get(), expected_calls.get());
        assert_eq!(actual_work, expected_work);
        assert_eq!(identities(&actual), identities(&expected));
    }
}

#[test]
fn batched_sift_allowance_and_arithmetic_failure_preserve_checked_behavior() {
    let cancel = AtomicBool::new(false);
    for count in 1usize..=128 {
        for root in 0..count {
            let mut values: Vec<_> = (0..count).map(|index| ((index * 37) % 53) as f64).collect();
            let mut consumed = 0;
            sift_inner::<_, true>(
                &mut values,
                root,
                count,
                &|value| *value,
                &mut consumed,
                &cancel,
            )
            .unwrap();
            assert!(consumed <= 3 * (usize::BITS - count.leading_zeros()) as usize);
        }
    }
    for initial_work in [0, usize::MAX] {
        let mut expected_work = initial_work;
        let mut actual_work = initial_work;
        let expected = checked_sift::<f64>(
            &mut [],
            usize::MAX,
            usize::MAX,
            &|value| *value,
            &mut expected_work,
            &cancel,
        )
        .unwrap_err();
        let actual = sift::<f64>(
            &mut [],
            usize::MAX,
            usize::MAX,
            &|value| *value,
            &mut actual_work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(actual.code, expected.code);
        assert_eq!(actual.message, expected.message);
        assert_eq!(actual_work, expected_work);
    }
}
