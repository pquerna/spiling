use super::*;
use std::{cell::RefCell, sync::atomic::Ordering};

fn checked_sweep(
    active: &mut Vec<usize>,
    edge: EdgeBounds,
    bounds: impl Fn(&usize) -> EdgeBounds,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut position = 0;
    while position < active.len() {
        check_cancel(cancel)?;
        charge(work, 1)?;
        let previous = bounds(&active[position]);
        if previous.max[0] < edge.min[0] {
            charge(work, 1)?;
            active.swap_remove(position);
            continue;
        }
        if previous.min[1] <= edge.max[1] && edge.min[1] <= previous.max[1] {
            charge(work, 1)?;
        }
        position += 1;
    }
    Ok(())
}

fn inventory(count: usize) -> Vec<EdgeBounds> {
    let examples = [
        EdgeBounds {
            min: [-3.0, -1.0],
            max: [-1.0, 1.0],
        },
        EdgeBounds {
            min: [-0.0, -1.0],
            max: [0.0, 1.0],
        },
        EdgeBounds {
            min: [-1.0, 1.0],
            max: [2.0, 3.0],
        },
        EdgeBounds {
            min: [-1.0, 2.0],
            max: [2.0, 3.0],
        },
        EdgeBounds {
            min: [-1.0, -3.0],
            max: [2.0, -1.0],
        },
        EdgeBounds {
            min: [-1.0, -3.0],
            max: [2.0, -2.0],
        },
        EdgeBounds {
            min: [-1.0, -1.0],
            max: [2.0, 1.0],
        },
    ];
    (0..count)
        .map(|index| examples[index % examples.len()])
        .collect()
}

fn same_result(actual: Result<(), ManufacturingError>, expected: Result<(), ManufacturingError>) {
    assert_eq!(actual.is_ok(), expected.is_ok());
    if let (Err(actual), Err(expected)) = (actual, expected) {
        assert_eq!(actual.code, expected.code);
        assert_eq!(actual.message, expected.message);
    }
}

#[test]
fn batched_sweep_matches_independent_checked_scans_and_every_refusal_boundary() {
    let cancel = AtomicBool::new(false);
    let edge = EdgeBounds {
        min: [0.0, -1.0],
        max: [1.0, 1.0],
    };
    for count in [0, 1, 2, 31, 256, 257] {
        let boxes = inventory(count);
        for remaining in 0..=2 * count + 1 {
            let mut expected: Vec<_> = (0..count).collect();
            let mut actual = expected.clone();
            let expected_trace = RefCell::new(Vec::new());
            let actual_trace = RefCell::new(Vec::new());
            let mut expected_work = MAX_BOOLEAN_PAIRS - remaining;
            let mut actual_work = expected_work;
            let expected_result = checked_sweep(
                &mut expected,
                edge,
                |index| {
                    expected_trace.borrow_mut().push(*index);
                    boxes[*index]
                },
                &mut expected_work,
                &cancel,
            );
            let actual_result = sweep_active(
                &mut actual,
                edge,
                |index| {
                    actual_trace.borrow_mut().push(*index);
                    boxes[*index]
                },
                &mut actual_work,
                &cancel,
            );
            same_result(actual_result, expected_result);
            assert_eq!(actual_work, expected_work);
            assert_eq!(actual, expected);
            assert_eq!(actual_trace.into_inner(), expected_trace.into_inner());
        }
        let mut active: Vec<_> = (0..count).collect();
        let mut consumed = 0;
        sweep_active_inner::<_, true>(
            &mut active,
            edge,
            |index| boxes[*index],
            &mut consumed,
            &cancel,
        )
        .unwrap();
        assert!(consumed <= 2 * count);
    }
}

#[test]
fn prepared_batched_sweep_matches_index_oracle_counts_and_partial_boxes() {
    let cancel = AtomicBool::new(false);
    let edge = EdgeBounds {
        min: [0.0, -1.0],
        max: [1.0, 1.0],
    };
    let bits = |bounds: &EdgeBounds| (bounds.min.map(f64::to_bits), bounds.max.map(f64::to_bits));
    for count in [0, 1, 31, 257] {
        let boxes = inventory(count);
        for initial_work in [
            0,
            MAX_BOOLEAN_PAIRS,
            MAX_BOOLEAN_PAIRS - 1,
            MAX_BOOLEAN_PAIRS - 2 * count,
            usize::MAX,
        ] {
            let mut expected: Vec<_> = (0..count).collect();
            let mut actual = boxes.clone();
            let mut expected_work = initial_work;
            let mut actual_work = initial_work;
            let expected_result = checked_sweep(
                &mut expected,
                edge,
                |index| boxes[*index],
                &mut expected_work,
                &cancel,
            );
            let actual_result = sweep_active(
                &mut actual,
                edge,
                |bounds| *bounds,
                &mut actual_work,
                &cancel,
            );
            same_result(actual_result, expected_result);
            assert_eq!(actual_work, expected_work);
            assert_eq!(
                actual.iter().map(bits).collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|index| bits(&boxes[*index]))
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn batched_sweep_settles_cancellation_and_overflow_fallback_exactly() {
    let count = 512;
    let boxes = inventory(count);
    let edge = EdgeBounds {
        min: [0.0, -1.0],
        max: [1.0, 1.0],
    };
    for after in [0, 1, 2, 3, 256, 512] {
        for initial_work in [
            0,
            MAX_BOOLEAN_PAIRS - 1024,
            MAX_BOOLEAN_PAIRS - 1,
            usize::MAX,
        ] {
            let expected_cancel = AtomicBool::new(after == 0);
            let actual_cancel = AtomicBool::new(after == 0);
            let expected_trace = RefCell::new(Vec::new());
            let actual_trace = RefCell::new(Vec::new());
            let bounds = |index: &usize, trace: &RefCell<Vec<usize>>, cancel: &AtomicBool| {
                trace.borrow_mut().push(*index);
                if trace.borrow().len() >= after {
                    cancel.store(true, Ordering::Relaxed);
                }
                boxes[*index]
            };
            let mut expected: Vec<_> = (0..count).collect();
            let mut actual = expected.clone();
            let mut expected_work = initial_work;
            let mut actual_work = initial_work;
            let expected_result = checked_sweep(
                &mut expected,
                edge,
                |index| bounds(index, &expected_trace, &expected_cancel),
                &mut expected_work,
                &expected_cancel,
            );
            let actual_result = sweep_active(
                &mut actual,
                edge,
                |index| bounds(index, &actual_trace, &actual_cancel),
                &mut actual_work,
                &actual_cancel,
            );
            same_result(actual_result, expected_result);
            assert_eq!(actual_work, expected_work);
            assert_eq!(actual, expected);
            assert_eq!(actual_trace.into_inner(), expected_trace.into_inner());
        }
    }
}
