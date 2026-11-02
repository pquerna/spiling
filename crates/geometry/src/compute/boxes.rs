// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{GeometryError, GeometryWorkBudget, compute};
use spiling_contracts::geometry::GeometryErrorCode;
use std::{mem::size_of, sync::atomic::AtomicBool};

struct Entry {
    minimum: [f64; 2],
    maximum: [f64; 2],
    index: usize,
}

fn resource(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}

fn allocate<T>(count: usize, bytes: &mut usize) -> Result<Vec<T>, GeometryError> {
    let expected = count
        .checked_mul(size_of::<T>())
        .ok_or_else(|| resource("box intersection storage overflow"))?;
    if expected > *bytes {
        return Err(resource(
            "box intersection live storage allowance exhausted",
        ));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| resource("box intersection allocation failed"))?;
    let actual = values
        .capacity()
        .checked_mul(size_of::<T>())
        .ok_or_else(|| resource("box intersection capacity overflow"))?;
    *bytes = bytes
        .checked_sub(actual)
        .ok_or_else(|| resource("box intersection allocated capacity exceeds allowance"))?;
    Ok(values)
}

fn update(
    tree: &mut [f64],
    mut node: usize,
    value: f64,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    tree[node] = value;
    while node > 1 {
        work.charge(1, cancel)?;
        node /= 2;
        tree[node] = tree[node * 2].max(tree[node * 2 + 1]);
    }
    Ok(())
}

pub(crate) fn pairs(
    count: usize,
    bounds: impl Fn(usize) -> Result<([f64; 2], [f64; 2]), GeometryError>,
    mut bytes: usize,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
    mut visit: impl FnMut(usize, usize) -> Result<(), GeometryError>,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    if count == 0 {
        return Ok(());
    }
    let base = count
        .checked_next_power_of_two()
        .ok_or_else(|| resource("box intersection tree overflow"))?;
    let tree_count = base
        .checked_mul(2)
        .ok_or_else(|| resource("box intersection tree overflow"))?;
    let expected = count
        .checked_mul(size_of::<Entry>() + 2 * size_of::<usize>())
        .and_then(|storage| {
            tree_count
                .checked_mul(size_of::<f64>())?
                .checked_add(storage)
        })
        .ok_or_else(|| resource("box intersection storage overflow"))?;
    if expected > bytes {
        return Err(resource(
            "box intersection live storage allowance exhausted",
        ));
    }
    let mut entries = allocate::<Entry>(count, &mut bytes)?;
    let mut starts = allocate::<usize>(count, &mut bytes)?;
    let mut ends = allocate::<usize>(count, &mut bytes)?;
    let mut tree = allocate::<f64>(tree_count, &mut bytes)?;
    let mut finite = true;
    for index in 0..count {
        work.charge(1, cancel)?;
        let (minimum, maximum) = bounds(index)?;
        if (0..2).any(|axis| {
            minimum[axis].is_nan() || maximum[axis].is_nan() || minimum[axis] > maximum[axis]
        }) {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "box intersection requires ordered non-NaN bounds",
            ));
        }
        finite &= minimum.into_iter().chain(maximum).all(f64::is_finite);
        entries.push(Entry {
            minimum,
            maximum,
            index,
        });
        starts.push(index);
        ends.push(index);
    }
    if !finite {
        for first in 0..count {
            work.charge(1, cancel)?;
            for second in first + 1..count {
                work.charge(1, cancel)?;
                visit(first, second)?;
            }
        }
        return Ok(());
    }
    for _ in 0..tree_count {
        work.charge(1, cancel)?;
        tree.push(f64::NEG_INFINITY);
    }
    compute::sort_unstable_by(
        &mut entries,
        |left, right| {
            left.minimum[1]
                .total_cmp(&right.minimum[1])
                .then(left.index.cmp(&right.index))
        },
        work,
        cancel,
    )?;
    for (order, maximum) in [(&mut starts, false), (&mut ends, true)] {
        compute::sort_unstable_by(
            order,
            |&left, &right| {
                let coordinate = |index: usize| {
                    if maximum {
                        entries[index].maximum[0]
                    } else {
                        entries[index].minimum[0]
                    }
                };
                coordinate(left)
                    .total_cmp(&coordinate(right))
                    .then(left.cmp(&right))
            },
            work,
            cancel,
        )?;
    }
    let mut expired = 0;
    for &current in &starts {
        work.charge(1, cancel)?;
        let entry = &entries[current];
        while expired < ends.len() && entries[ends[expired]].maximum[0] < entry.minimum[0] {
            update(
                &mut tree,
                base + ends[expired],
                f64::NEG_INFINITY,
                work,
                cancel,
            )?;
            expired += 1;
        }
        let mut lo = 0;
        let mut hi = entries.len();
        while lo < hi {
            work.charge(1, cancel)?;
            let mid = lo + (hi - lo) / 2;
            if entries[mid].minimum[1] <= entry.maximum[1] {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut node = 1_usize;
        'query: loop {
            work.charge(1, cancel)?;
            let depth = node.ilog2();
            let first_leaf = (node - (1 << depth)) * (base >> depth);
            if first_leaf >= lo {
                break;
            }
            if tree[node] >= entry.minimum[1] {
                if node < base {
                    node *= 2;
                    continue;
                }
                let other = &entries[node - base];
                visit(entry.index.min(other.index), entry.index.max(other.index))?;
            }
            while node % 2 == 1 {
                work.charge(1, cancel)?;
                if node == 1 {
                    break 'query;
                }
                node /= 2;
            }
            node += 1;
        }
        update(&mut tree, base + current, entry.maximum[1], work, cancel)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonfinite_expansion_keeps_complete_cancellable_fallback() {
        let mut actual = Vec::new();
        pairs(
            3,
            |_| Ok(([f64::NEG_INFINITY; 2], [f64::INFINITY; 2])),
            usize::MAX,
            &GeometryWorkBudget::query(),
            &AtomicBool::new(false),
            |first, second| {
                actual.push((first, second));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(actual, [(0, 1), (0, 2), (1, 2)]);
        let cancel = AtomicBool::new(false);
        let mut visits = 0;
        assert_eq!(
            pairs(
                3,
                |_| Ok(([f64::NEG_INFINITY; 2], [f64::INFINITY; 2])),
                usize::MAX,
                &GeometryWorkBudget::query(),
                &cancel,
                |_, _| {
                    visits += 1;
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                    Ok(())
                }
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(visits, 1);
    }
}
