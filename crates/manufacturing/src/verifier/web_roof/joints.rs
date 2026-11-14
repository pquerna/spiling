// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Reconstruct automatic joints from classified rings and the captured bead.
//! No planner construction, path labels or retained joint decisions are trusted.
use super::*;

#[expect(
    clippy::too_many_arguments,
    reason = "independent classified geometry and cumulative work"
)]
pub(super) fn reconstruct(
    polygon: &DiaphragmPolygonMm,
    roots: &[Vec<f64>; 2],
    min: [f64; 2],
    max: [f64; 2],
    width: f64,
    expected: &mut Vec<(usize, f64, f64, f64)>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let roomy = |axis: usize, i: usize| {
        let r = &roots[axis];
        let lower = if i == 0 { min[axis] } else { r[i - 1] };
        let upper = r.get(i + 1).copied().unwrap_or(max[axis]);
        r[i] - lower >= 8.0 * width && upper - r[i] >= 8.0 * width
    };
    let mut through = Vec::new();
    let mut offset = [Vec::new(), Vec::new()];
    for axis in 0..2 {
        for (i, &c) in roots[axis].iter().enumerate() {
            charge(work, cancel)?;
            if !roomy(axis, i) {
                continue;
            }
            crossing_intervals(polygon, axis, c, &mut through, work, cancel)?;
            let preferred = if i.is_multiple_of(2) { width } else { -width };
            let coordinates = [c + preferred, c - preferred];
            for side in 0..2 {
                crossing_intervals(
                    polygon,
                    axis,
                    coordinates[side],
                    &mut offset[side],
                    work,
                    cancel,
                )?;
            }
            for pair in through.as_chunks::<2>().0 {
                let along = &roots[1 - axis];
                let lo = along.partition_point(|v| *v < pair[0] + 3.0 * width);
                let hi = along.partition_point(|v| *v <= pair[1] - 3.0 * width);
                for (j, &position) in along.iter().enumerate().take(hi).skip(lo) {
                    charge(work, cancel)?;
                    if !roomy(1 - axis, j) {
                        continue;
                    }
                    let start = position - 3.0 * width;
                    let end = position + 3.0 * width;
                    for side in 0..2 {
                        charge_many(work, offset[side].len(), cancel)?;
                        if offset[side]
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .any(|chord| chord[0] <= start && end <= chord[1])
                        {
                            if expected.len() >= crate::working_set::units() {
                                return Err(failed(
                                    "web + roof automatic joint inventory budget exceeded",
                                ));
                            }
                            expected.push((axis, coordinates[side], start, end));
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
