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
    if old_count
        .checked_mul(new_count)
        .is_some_and(|pairs| pairs <= 256)
    {
        for old in 0..old_count {
            for new in 0..new_count {
                check_cancel(cancel)?;
                charge(work, 8)?;
                retention_pair(original, old, derived, new)?;
            }
        }
        return Ok(());
    }
    let count = old_count
        .checked_add(new_count)
        .ok_or_else(proof_overflow)?;
    admit_index(count, work)?;
    let mut edges = Vec::with_capacity(count);
    append_edges(&mut edges, original, 0, 0, work, cancel)?;
    append_edges(&mut edges, derived, 0, 1, work, cancel)?;
    sort(&mut edges, work, cancel)?;
    sweep(&edges, true, work, cancel)
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
    let count = rings()
        .try_fold(0usize, |total, ring| total.checked_add(ring.0.len() - 1))
        .ok_or_else(proof_overflow)?;
    if count <= 16 {
        for (owner, ring) in rings().enumerate() {
            for index in 0..ring.0.len() - 1 {
                check_cancel(cancel)?;
                if ring.0[index] == ring.0[index + 1] {
                    return Err(zero_segment());
                }
                for (other_owner, other) in rings().enumerate().skip(owner) {
                    for other_index in 0..other.0.len() - 1 {
                        check_cancel(cancel)?;
                        if owner == other_owner && other_index <= index {
                            continue;
                        }
                        charge(work, 1)?;
                        simple_pair(ring, index, owner, other, other_index, other_owner)?;
                    }
                }
            }
        }
        return Ok(());
    }
    admit_index(count, work)?;
    let mut edges = Vec::with_capacity(count);
    for (owner, ring) in rings().enumerate() {
        append_edges(&mut edges, ring, owner, 0, work, cancel)?;
    }
    sort(&mut edges, work, cancel)?;
    sweep(&edges, false, work, cancel)
}

fn proof_overflow() -> ManufacturingError {
    error(
        ManufacturingErrorCode::ResourceLimit,
        "surface boundary proof index/work overflow",
    )
}
fn zero_segment() -> ManufacturingError {
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        "surface construction produced a zero-length segment",
    )
}

// These are the original robust predicates. The sweep only removes pairs whose
// CLOSED exact endpoint boxes are disjoint. Unlike overlay admission, no integer
// conversion, snapping or tolerance enters this proof, so no padding is needed.
fn retention_pair(
    original: &LineString<f64>,
    old_index: usize,
    derived: &LineString<f64>,
    new_index: usize,
) -> Result<(), ManufacturingError> {
    let old_count = original.0.len() - 1;
    let new_count = derived.0.len() - 1;
    let old = &original.0[old_index..=old_index + 1];
    let new = &derived.0[new_index..=new_index + 1];
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
            .any(|point| {
                RobustKernel::orient2d(old[0], old[1], *point) == Orientation::CounterClockwise
            });
    let old_crosses = on_open_segment(new[0], new[1], old[0])
        && [original.0[(old_index + old_count - 1) % old_count], old[1]]
            .iter()
            .any(|point| RobustKernel::orient2d(new[0], new[1], *point) == Orientation::Clockwise);
    if proper || new_crosses || old_crosses {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "derived boundary crosses original material; missing native slivers are not accepted",
        ));
    }
    Ok(())
}
fn simple_pair(
    ring: &LineString<f64>,
    index: usize,
    owner: usize,
    other: &LineString<f64>,
    other_index: usize,
    other_owner: usize,
) -> Result<(), ManufacturingError> {
    let points = &ring.0[index..=index + 1];
    let other_points = &other.0[other_index..=other_index + 1];
    let adjacent = owner == other_owner
        && (index.abs_diff(other_index) == 1
            || (index == 0 && other_index + 2 == ring.0.len())
            || (other_index == 0 && index + 2 == ring.0.len()));
    match line_intersection(
        Line::new(points[0], points[1]),
        Line::new(other_points[0], other_points[1]),
    ) {
        None => Ok(()),
        Some(LineIntersection::SinglePoint {
            intersection,
            is_proper: false,
        }) if adjacent
            && (intersection == points[0] || intersection == points[1])
            && (intersection == other_points[0] || intersection == other_points[1]) =>
        {
            Ok(())
        }
        _ => Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "surface construction changes topology through intersection or ring contact",
        )),
    }
}

struct Edge<'a> {
    ring: &'a LineString<f64>,
    index: usize,
    owner: usize,
    group: u8,
    min: [f64; 2],
    max: [f64; 2],
}
fn admit_index(count: usize, work: &usize) -> Result<(), ManufacturingError> {
    let bytes = count
        .checked_mul(std::mem::size_of::<Edge<'_>>() + std::mem::size_of::<usize>())
        .ok_or_else(proof_overflow)?;
    let allowance = (spiling_contracts::manufacturing::MAX_DEPOSITION_SEGMENTS as usize)
        .checked_mul(std::mem::size_of::<Coord<f64>>())
        .ok_or_else(proof_overflow)?;
    let levels = usize::BITS as usize - count.max(1).leading_zeros() as usize;
    let preparation = count
        .checked_mul(levels)
        .and_then(|n| n.checked_mul(6))
        .and_then(|n| n.checked_add(count.checked_mul(2)?))
        .ok_or_else(proof_overflow)?;
    if bytes > allowance
        || work
            .checked_add(preparation)
            .is_none_or(|n| n > super::super::MAX_BOOLEAN_PAIRS)
    {
        return Err(proof_overflow());
    }
    Ok(())
}
fn append_edges<'a>(
    edges: &mut Vec<Edge<'a>>,
    ring: &'a LineString<f64>,
    owner: usize,
    group: u8,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for (index, points) in ring.0.windows(2).enumerate() {
        check_cancel(cancel)?;
        if points
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "nonfinite surface boundary proof coordinate",
            ));
        }
        charge(work, 2)?; // Box construction and eventual active-index insertion.
        if points[0] == points[1] {
            return Err(zero_segment());
        }
        let min = [points[0].x.min(points[1].x), points[0].y.min(points[1].y)];
        let max = [points[0].x.max(points[1].x), points[0].y.max(points[1].y)];
        if min
            .into_iter()
            .chain(max)
            .any(|coordinate| !coordinate.is_finite())
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "nonfinite surface boundary proof coordinate",
            ));
        }
        edges.push(Edge {
            ring,
            index,
            owner,
            group,
            min,
            max,
        });
    }
    Ok(())
}
// Follow the existing bounded Boolean-admission heapsort: every comparison,
// swap and active scan is charged and cancellable, including rejected pairs.
fn sift(
    edges: &mut [Edge<'_>],
    mut root: usize,
    end: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    loop {
        check_cancel(cancel)?;
        let Some(mut child) = root.checked_mul(2).and_then(|n| n.checked_add(1)) else {
            return Err(proof_overflow());
        };
        if child >= end {
            return Ok(());
        }
        if child + 1 < end {
            charge(work, 1)?;
            if edges[child].min[0]
                .total_cmp(&edges[child + 1].min[0])
                .is_lt()
            {
                child += 1;
            }
        }
        charge(work, 1)?;
        if !edges[root].min[0].total_cmp(&edges[child].min[0]).is_lt() {
            return Ok(());
        }
        charge(work, 1)?;
        edges.swap(root, child);
        root = child;
    }
}
fn sort(
    edges: &mut [Edge<'_>],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let count = edges.len();
    for root in (0..count / 2).rev() {
        sift(edges, root, count, work, cancel)?;
    }
    for end in (1..count).rev() {
        check_cancel(cancel)?;
        charge(work, 1)?;
        edges.swap(0, end);
        sift(edges, 0, end, work, cancel)?;
    }
    Ok(())
}
fn sweep(
    edges: &[Edge<'_>],
    retention: bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut active = Vec::<usize>::with_capacity(edges.len());
    for (index, edge) in edges.iter().enumerate() {
        let mut position = 0;
        while position < active.len() {
            check_cancel(cancel)?;
            charge(work, 1)?;
            let previous = &edges[active[position]];
            if previous.max[0] < edge.min[0] {
                charge(work, 1)?;
                active.swap_remove(position);
                continue;
            }
            if (!retention || previous.group != edge.group)
                && previous.min[1] <= edge.max[1]
                && edge.min[1] <= previous.max[1]
            {
                if retention {
                    charge(work, 8)?;
                    let (old, new) = if previous.group == 0 {
                        (previous, edge)
                    } else {
                        (edge, previous)
                    };
                    retention_pair(old.ring, old.index, new.ring, new.index)?;
                } else {
                    charge(work, 1)?;
                    simple_pair(
                        previous.ring,
                        previous.index,
                        previous.owner,
                        edge.ring,
                        edge.index,
                        edge.owner,
                    )?;
                }
            }
            position += 1;
        }
        active.push(index);
    }
    check_cancel(cancel)
}

#[cfg(test)]
#[path = "proof_tests.rs"]
mod tests;
