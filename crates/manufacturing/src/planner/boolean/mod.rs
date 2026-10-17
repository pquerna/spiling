// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Carrier-independent conservative overlay work admission. Coarse safe inputs
//! allocate nothing; sampled boundary neighborhoods use an explicitly charged
//! edge sweep, not whole-capsule Cartesian products. No material is changed.
use super::MAX_BOOLEAN_PAIRS;
use crate::{check_cancel, error};
use geo::{MultiPolygon, Polygon};
use spiling_contracts::manufacturing::{
    MAX_DEPOSITION_SEGMENTS, ManufacturingError, ManufacturingErrorCode,
};
use std::sync::atomic::AtomicBool;

struct EdgeBounds {
    min: [f64; 2],
    max: [f64; 2],
}
fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}
fn charge(work: &mut usize, amount: usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(amount)
        .filter(|n| *n <= MAX_BOOLEAN_PAIRS)
        .ok_or_else(|| resource("planar Boolean work budget exceeded"))?;
    Ok(())
}

// Pinned geo0.32/i_overlay4/i_float1.15 uses bbox midpoint and
// 2^(29-round(log2(half_span))) for float->integer conversion. Thus a grid
// step is <= span/2^29, and endpoint rounding <= half a step. This deliberately
// wider work-only pad includes conversion/intersection rounding and midpoint /
// subtraction arithmetic. Directed rounding encloses its own arithmetic too.
// It is NOT a geometric tolerance or permission to move original coordinates.
pub(super) fn overlay_padding(min: [f64; 2], max: [f64; 2]) -> Result<f64, ManufacturingError> {
    let span = (max[0] - min[0]).max(max[1] - min[1]);
    let scale = min
        .into_iter()
        .chain(max)
        .fold(1.0f64, |a, b| a.max(b.abs()));
    let padding = (span / 134_217_728.0 + 64.0 * f64::EPSILON * scale).next_up();
    if !span.is_finite() || !padding.is_finite() || (span > 0.0 && span * 0.5 == 0.0) {
        return Err(resource("Boolean coordinate conversion is not conditioned"));
    }
    Ok(padding)
}

// Explicit heapsort charges every key comparison and swap and can stop safely
// without invoking overlay on an incompletely sorted buffer. There is no
// panic/comparator short circuit and no assumed standard-sort comparison count.
fn sift<T>(
    edges: &mut [T],
    mut root: usize,
    end: usize,
    key: &impl Fn(&T) -> f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    loop {
        check_cancel(cancel)?;
        let Some(mut child) = root.checked_mul(2).and_then(|v| v.checked_add(1)) else {
            return Err(resource("Boolean heap index overflow"));
        };
        if child >= end {
            return Ok(());
        }
        if child + 1 < end {
            charge(work, 1)?;
            if key(&edges[child])
                .total_cmp(&key(&edges[child + 1]))
                .is_lt()
            {
                child += 1;
            }
        }
        charge(work, 1)?;
        if !key(&edges[root]).total_cmp(&key(&edges[child])).is_lt() {
            return Ok(());
        }
        charge(work, 1)?;
        edges.swap(root, child);
        root = child;
    }
}
pub(super) fn sort_by<T>(
    edges: &mut [T],
    key: impl Fn(&T) -> f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let count = edges.len();
    for root in (0..count / 2).rev() {
        sift(edges, root, count, &key, work, cancel)?;
    }
    for end in (1..count).rev() {
        check_cancel(cancel)?;
        charge(work, 1)?;
        edges.swap(0, end);
        sift(edges, 0, end, &key, work, cancel)?;
    }
    Ok(())
}

pub(super) fn budget(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    // Unary unions/buffer admission pass the SAME owner twice. Borrow it once;
    // all its intra-operand pairs are still inspected by the common sweep.
    let other = if std::ptr::eq(a, b) {
        &b.0[..0]
    } else {
        &b.0[..]
    };
    polygon_budget(
        a.0.iter().chain(other.iter()),
        &mut 0,
        MAX_BOOLEAN_PAIRS,
        cancel,
    )
}

/// Charge a sequence of overlays against one ceiling without billing remote
/// boundary pairs as though every pair intersected at every layer.
pub(super) fn budget_accumulated(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let other = if std::ptr::eq(a, b) {
        &b.0[..0]
    } else {
        &b.0[..]
    };
    polygon_budget(a.0.iter().chain(other.iter()), work, 256, cancel)
}

pub(super) fn budget_polygon_accumulated(
    polygon: &Polygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    polygon_budget(std::iter::once(polygon), work, 256, cancel)
}

/// Borrowed required-material pieces share the ordinary Boolean admission proof.
pub(super) fn budget_parts(
    parts: &[&Polygon<f64>],
    other: &MultiPolygon<f64>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    polygon_budget(
        parts.iter().copied().chain(other.0.iter()),
        &mut 0,
        MAX_BOOLEAN_PAIRS,
        cancel,
    )
}

fn polygon_budget<'a>(
    polygons: impl Iterator<Item = &'a Polygon<f64>> + Clone,
    work: &mut usize,
    coarse_pair_limit: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    let mut count = 0usize;
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for polygon in polygons.clone() {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            count = count
                .checked_add(ring.0.len().saturating_sub(1))
                .ok_or_else(|| resource("Boolean boundary count overflow"))?;
            for point in &ring.0 {
                check_cancel(cancel)?;
                charge(work, 1)?;
                if !point.x.is_finite() || !point.y.is_finite() {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        "Boolean boundary is nonfinite",
                    ));
                }
                for (axis, v) in [point.x, point.y].into_iter().enumerate() {
                    min[axis] = min[axis].min(v);
                    max[axis] = max[axis].max(v);
                }
            }
        }
    }
    if count == 0 {
        return Ok(());
    }
    let padding = overlay_padding(min, max)?;
    if let Some(pairs) = count.checked_mul(count).filter(|pairs| {
        *pairs <= coarse_pair_limit
            && work
                .checked_add(*pairs)
                .is_some_and(|total| total <= MAX_BOOLEAN_PAIRS)
    }) {
        charge(work, pairs)?;
        return Ok(()); // Every unordered candidate is covered by this coarse bound.
    }
    if count > MAX_DEPOSITION_SEGMENTS as usize {
        return Err(resource("Boolean broad-phase staging point limit exceeded"));
    }
    // Two bounded scratch arrays only: edge bounds and active indices. Their
    // total retained entries, byte products and construction/sort allowance are
    // admitted BEFORE growth. Heap depth <= ceil(log2(count)); at most three
    // charged operations per level plus extraction, with generous linear slack.
    count
        .checked_mul(std::mem::size_of::<EdgeBounds>() + std::mem::size_of::<usize>())
        .ok_or_else(|| resource("Boolean broad-phase scratch byte overflow"))?;
    let levels = usize::BITS as usize - count.leading_zeros() as usize;
    let sort_bound = count
        .checked_mul(6 * levels + 8)
        .ok_or_else(|| resource("Boolean broad-phase sort work overflow"))?;
    if work
        .checked_add(sort_bound)
        .is_none_or(|v| v > MAX_BOOLEAN_PAIRS)
    {
        return Err(resource(
            "Boolean broad-phase construction/sort work limit exceeded",
        ));
    }
    let mut edges = Vec::with_capacity(count);
    for polygon in polygons.clone() {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            for edge in ring.0.windows(2) {
                check_cancel(cancel)?;
                charge(work, 2)?; // Construct the box and its eventual active index.
                let [p, q] = [edge[0], edge[1]];
                let lo = [
                    (p.x.min(q.x) - padding).next_down(),
                    (p.y.min(q.y) - padding).next_down(),
                ];
                let hi = [
                    (p.x.max(q.x) + padding).next_up(),
                    (p.y.max(q.y) + padding).next_up(),
                ];
                if lo.into_iter().chain(hi).any(|v| !v.is_finite()) {
                    return Err(resource("Boolean broad-phase box overflow"));
                }
                edges.push(EdgeBounds { min: lo, max: hi });
            }
        }
    }
    sort_by(&mut edges, |edge| edge.min[0], work, cancel)?;
    let mut active = Vec::<usize>::with_capacity(count);
    for (index, edge) in edges.iter().enumerate() {
        let mut position = 0;
        while position < active.len() {
            check_cancel(cancel)?;
            charge(work, 1)?; // Includes expiration and every rejected box.
            let previous = &edges[active[position]];
            if previous.max[0] < edge.min[0] {
                charge(work, 1)?;
                active.swap_remove(position);
                continue;
            }
            if previous.min[1] <= edge.max[1] && edge.min[1] <= previous.max[1] {
                charge(work, 1)?; // EVERY intra- and cross-operand predicate candidate.
            }
            position += 1;
        }
        active.push(index);
    }
    check_cancel(cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Coord, LineString, Polygon};

    #[test]
    fn boolean_broad_phase_refuses_dense_intra_operand_and_quantized_candidate_work() {
        for (spacing, width) in [(0.0, 1.0), (1e-12, 0.5e-12)] {
            // Slightly separated thin boxes collapse/overlap on the consuming
            // overlay's common grid. Floating X separation must not hide work.
            let dense = MultiPolygon(
                (0..6000)
                    .map(|i| {
                        let x = i as f64 * spacing;
                        Polygon::new(
                            LineString(vec![
                                Coord { x, y: 0.0 },
                                Coord {
                                    x: x + width,
                                    y: 0.5,
                                },
                                Coord { x, y: 1.0 },
                                Coord { x, y: 0.0 },
                            ]),
                            Vec::new(),
                        )
                    })
                    .chain(std::iter::once(Polygon::new(
                        LineString(vec![
                            Coord { x: -10.0, y: -10.0 },
                            Coord { x: 10.0, y: -10.0 },
                            Coord { x: 10.0, y: 10.0 },
                            Coord { x: -10.0, y: -10.0 },
                        ]),
                        Vec::new(),
                    )))
                    .collect(),
            );
            assert_eq!(
                budget(&dense, &MultiPolygon(Vec::new()), &AtomicBool::new(false))
                    .unwrap_err()
                    .code,
                ManufacturingErrorCode::ResourceLimit
            );
            assert_eq!(
                budget(&dense, &dense, &AtomicBool::new(true))
                    .unwrap_err()
                    .code,
                ManufacturingErrorCode::Cancelled
            );
        }
    }
}
