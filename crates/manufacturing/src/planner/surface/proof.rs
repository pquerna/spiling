// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Carrier-independent closed-material proofs and cumulative bounded work.
use crate::{check_cancel, error};
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::line_intersection::{LineIntersection, line_intersection};
use geo::{Coord, Line, LineString, MultiPolygon};
use spiling_contracts::manufacturing::{ManufacturingError, ManufacturingErrorCode};
use std::sync::atomic::AtomicBool;
pub(super) fn charge(work: &mut usize, amount: usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(amount)
        .filter(|v| *v <= super::super::MAX_BOOLEAN_PAIRS)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "cumulative surface boundary proof work budget exceeded",
            )
        })?;
    Ok(())
}
fn on_open_segment(a: Coord<f64>, b: Coord<f64>, p: Coord<f64>) -> bool {
    p != a
        && p != b
        && p.x >= a.x.min(b.x)
        && p.x <= a.x.max(b.x)
        && p.y >= a.y.min(b.y)
        && p.y <= a.y.max(b.y)
        && RobustKernel::orient2d(a, b, p) == Orientation::Collinear
}

// Inclusion of vertices alone is not a missing-material proof. Reject every
// proper source/derived crossing and every vertex-on-edge crossing into the
// original material. Collinear shared bands and outward-only valley tangencies
// remain valid. Together with exact source-vertex inclusion, simple positive
// rings and identical holes this retains the ENTIRE original closed region.
pub(super) fn certify_retention(
    original: &LineString<f64>,
    derived: &LineString<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let old_count = original.0.len() - 1;
    let new_count = derived.0.len() - 1;
    charge(
        work,
        old_count
            .checked_mul(new_count)
            .and_then(|v| v.checked_mul(8))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native boundary retention work overflow",
                )
            })?,
    )?;
    for (old_index, old) in original.0.windows(2).enumerate() {
        for (new_index, new) in derived.0.windows(2).enumerate() {
            check_cancel(cancel)?;
            let proper = matches!(
                line_intersection(Line::new(old[0], old[1]), Line::new(new[0], new[1])),
                Some(LineIntersection::SinglePoint {
                    is_proper: true,
                    ..
                })
            );
            let new_crosses = on_open_segment(old[0], old[1], new[0])
                && [derived.0[(new_index + new_count - 1) % new_count], new[1]]
                    .iter()
                    .any(|p| {
                        RobustKernel::orient2d(old[0], old[1], *p) == Orientation::CounterClockwise
                    });
            let old_crosses = on_open_segment(new[0], new[1], old[0])
                && [original.0[(old_index + old_count - 1) % old_count], old[1]]
                    .iter()
                    .any(|p| RobustKernel::orient2d(new[0], new[1], *p) == Orientation::Clockwise);
            if proper || new_crosses || old_crosses {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "derived boundary crosses original material; missing native slivers are not accepted",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn certify_simple(
    material: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let rings = || {
        material
            .0
            .iter()
            .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
    };
    let segments = rings().map(|ring| ring.0.len() - 1).sum::<usize>();
    charge(
        work,
        segments.checked_mul(segments).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "surface intersection proof work overflow",
            )
        })?,
    )?;
    for (owner, ring) in rings().enumerate() {
        for (index, points) in ring.0.windows(2).enumerate() {
            check_cancel(cancel)?;
            if points[0] == points[1] {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "surface construction produced a zero-length segment",
                ));
            }
            for (other_owner, other) in rings().enumerate().skip(owner) {
                for (other_index, other_points) in other.0.windows(2).enumerate() {
                    check_cancel(cancel)?;
                    if owner == other_owner && other_index <= index {
                        continue;
                    }
                    let adjacent = owner == other_owner
                        && (other_index == index + 1
                            || (index == 0 && other_index + 2 == ring.0.len()));
                    match line_intersection(
                        Line::new(points[0], points[1]),
                        Line::new(other_points[0], other_points[1]),
                    ) {
                        None => {}
                        Some(LineIntersection::SinglePoint {
                            intersection,
                            is_proper: false,
                        }) if adjacent
                            && (intersection == points[0] || intersection == points[1])
                            && (intersection == other_points[0]
                                || intersection == other_points[1]) => {}
                        _ => {
                            return Err(error(
                                ManufacturingErrorCode::UnsupportedGeometry,
                                "surface construction changes topology through intersection or ring contact",
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
