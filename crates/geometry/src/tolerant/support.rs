// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{TolerantModel, unavailable};
use crate::{GeometryError, GeometryErrorCode, Interval};
use std::{
    collections::HashMap,
    ops::ControlFlow,
    sync::atomic::{AtomicBool, Ordering},
};

/// Conservative XY outer operands for the complete closed requested interval.
/// These are tolerant-source enclosures, not exact native certificates.
pub struct TolerantSupportProjection {
    pub pieces: Vec<TolerantSupportPiece>,
}

pub struct TolerantSupportPiece {
    pub points: Vec<[f64; 2]>,
    pub holes: Vec<Vec<[f64; 2]>>,
    pub height_mm: [f64; 2],
}

/// All-height projection of one connected section-hole boundary. `bottom`
/// describes its first opening; horizontal `floor` triangles can close it.
/// Consumers must prove full floor coverage before choosing a bounded scope.
pub struct TolerantVoidProjection {
    pub polygon: Vec<[f64; 2]>,
    pub height_mm: [f64; 2],
    pub bottom: Vec<[f64; 2]>,
    pub floor: Vec<[[f64; 2]; 3]>,
    /// Height enclosure of a downward-facing terminal cap stitched to the
    /// hole band, when all its free edges above the lower mouth close. This
    /// is not a clearance or withdrawal certificate.
    pub roof: Option<TolerantVoidRoof>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct TolerantVoidRoof {
    pub height_mm: [f64; 2],
    pub facets: Vec<[[f64; 2]; 3]>,
}

type MeshEdge = (u32, u32);

fn facet_edges(triangle: [u32; 3]) -> [MeshEdge; 3] {
    std::array::from_fn(|index| {
        let from = triangle[index];
        let to = triangle[(index + 1) % 3];
        (from.min(to), from.max(to))
    })
}

struct RoofEdge {
    edge: MeshEdge,
    cap: Option<usize>,
}

fn roof_resource_limit() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "terminal roof discovery exceeds the remaining allocated-byte budget",
    )
}

fn roof_buffer<Item>(capacity: usize) -> Result<Vec<Item>, GeometryError> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(capacity)
        .map_err(|_| roof_resource_limit())?;
    Ok(buffer)
}

fn cancelled(cancel: &AtomicBool) -> Result<(), GeometryError> {
    if cancel.load(Ordering::Relaxed) {
        Err(GeometryError::new(
            GeometryErrorCode::Cancelled,
            "support projection cancelled",
        ))
    } else {
        Ok(())
    }
}

fn hull(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let mut result = Vec::with_capacity(points.len() + 1);
    for p in &points {
        while result.len() >= 2
            && cross(result[result.len() - 2], result[result.len() - 1], *p) <= 0.0
        {
            result.pop();
        }
        result.push(*p);
    }
    let lower = result.len();
    for p in points.iter().rev().skip(1) {
        while result.len() > lower
            && cross(result[result.len() - 2], result[result.len() - 1], *p) <= 0.0
        {
            result.pop();
        }
        result.push(*p);
    }
    result.pop();
    result
}

fn padded(points: Vec<[f64; 2]>, margin: f64) -> Vec<[f64; 2]> {
    hull(
        points
            .into_iter()
            .flat_map(|p| {
                [-margin, margin].into_iter().flat_map(move |dx| {
                    [-margin, margin].into_iter().map(move |dy| {
                        [
                            if dx < 0.0 {
                                (p[0] + dx).next_down()
                            } else {
                                (p[0] + dx).next_up()
                            },
                            if dy < 0.0 {
                                (p[1] + dy).next_down()
                            } else {
                                (p[1] + dy).next_up()
                            },
                        ]
                    })
                })
            })
            .collect(),
    )
}

/// Exact bounds of `padded(points, margin)`: hull extremes are input extremes.
fn padded_bounds(points: &[[f64; 2]], margin: f64) -> [[f64; 2]; 2] {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in points {
        for axis in 0..2 {
            lo[axis] = lo[axis].min((p[axis] + -margin).next_down());
            hi[axis] = hi[axis].max((p[axis] + margin).next_up());
        }
    }
    [lo, hi]
}

fn bounds(points: &[[f64; 2]]) -> [[f64; 2]; 2] {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in points {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(p[axis]);
            hi[axis] = hi[axis].max(p[axis]);
        }
    }
    [lo, hi]
}

fn clip(triangle: [[f64; 3]; 3], interval: [f64; 2]) -> Vec<[f64; 3]> {
    let mut points = triangle.to_vec();
    for (z, above) in [(interval[0], true), (interval[1], false)] {
        let input = std::mem::take(&mut points);
        if input.is_empty() {
            break;
        }
        let inside = |p: [f64; 3]| if above { p[2] >= z } else { p[2] <= z };
        let mut a = *input.last().unwrap();
        for b in input {
            if inside(a) != inside(b) {
                let t = (z - a[2]) / (b[2] - a[2]);
                points.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), z]);
            }
            if inside(b) {
                points.push(b);
            }
            a = b;
        }
    }
    points
}

fn root(parents: &mut [usize], mut i: usize) -> usize {
    while parents[i] != i {
        parents[i] = parents[parents[i]];
        i = parents[i];
    }
    i
}
fn join(parents: &mut [usize], a: usize, b: usize) {
    let (a, b) = (root(parents, a), root(parents, b));
    if a != b {
        parents[a] = b;
    }
}

impl TolerantModel {
    /// Captured source/chain approximation enclosure used for support clearance.
    pub fn support_margin(&self) -> f64 {
        self.support_enclosure_mm
    }

    /// Projection of boundary facets clipped to the entire slab, plus one raw
    /// interior section. A vertical line intersecting solid in the slab either
    /// meets a boundary there or remains inside at the section. This is not a
    /// union of a few sampled cross-sections. Approximation is enclosed in XYZ.
    pub fn support_projection(
        &self,
        interval: [f64; 2],
        cancel: &AtomicBool,
    ) -> Result<TolerantSupportProjection, GeometryError> {
        let mut pieces = Vec::new();
        let _: ControlFlow<std::convert::Infallible> =
            self.visit_support_projection(interval, cancel, |piece| {
                pieces.push(piece);
                ControlFlow::Continue(())
            })?;
        Ok(TolerantSupportProjection { pieces })
    }

    /// Visit one owned piece at a time, without retaining the expanded source
    /// per layer. Breaking drops the current piece and stops source traversal.
    pub fn visit_support_projection<E>(
        &self,
        interval: [f64; 2],
        cancel: &AtomicBool,
        visit: impl FnMut(TolerantSupportPiece) -> ControlFlow<E>,
    ) -> Result<ControlFlow<E>, GeometryError> {
        self.visit_support_projection_screened(
            interval,
            cancel,
            |_, _| ControlFlow::Continue(true),
            visit,
        )
    }

    /// As [`Self::visit_support_projection`], but first offers each piece's
    /// exact XY bounds and height scope to `screen`; `Continue(false)` skips
    /// constructing that piece. The skipped piece is still part of the
    /// projection: only a consumer that provably ignores it may decline it.
    pub fn visit_support_projection_screened<E>(
        &self,
        interval: [f64; 2],
        cancel: &AtomicBool,
        mut screen: impl FnMut([[f64; 2]; 2], [f64; 2]) -> ControlFlow<E, bool>,
        mut visit: impl FnMut(TolerantSupportPiece) -> ControlFlow<E>,
    ) -> Result<ControlFlow<E>, GeometryError> {
        if !interval.iter().all(|z| z.is_finite()) || interval[0] > interval[1] {
            return Err(unavailable("invalid support projection interval"));
        }
        let mut checked_visit = |piece: TolerantSupportPiece| {
            if piece
                .points
                .iter()
                .flatten()
                .chain(piece.holes.iter().flatten().flatten())
                .chain(piece.height_mm.iter())
                .any(|v| !v.is_finite())
            {
                return Err(unavailable("support projection is not finite"));
            }
            Ok(visit(piece))
        };
        cancelled(cancel)?;
        let margin = self.support_margin();
        let closed = [
            (interval[0] - margin).next_down(),
            (interval[1] + margin).next_up(),
        ];

        // An interior section covers vertical lines with no boundary inside
        // this slab. Each boundary edge retains the same complete slab scope.
        let section =
            self.section_with((interval[0] + interval[1]) * 0.5, false, Some(cancel), None)?;
        let mut owners = vec![None; section.loops.len()];
        for (index, hole) in section
            .loops
            .iter()
            .enumerate()
            .filter(|(_, ring)| ring.area < 0.0)
        {
            for (outer_index, outer) in section
                .loops
                .iter()
                .enumerate()
                .filter(|(_, ring)| ring.area > 0.0)
            {
                cancelled(cancel)?;
                if super::contains(&outer.points, hole.points[0])
                    && owners[index]
                        .is_none_or(|owner: usize| outer.area < section.loops[owner].area)
                {
                    owners[index] = Some(outer_index);
                }
            }
            if owners[index].is_none() {
                return Err(unavailable(
                    "support projection hole has no containing material boundary",
                ));
            }
        }
        for (index, l) in section.loops.iter().enumerate() {
            cancelled(cancel)?;
            if l.area > 0.0 {
                // Try the coarse interior exclusion first. One contour copy lets
                // a stopped consumer avoid constructing all boundary expansions;
                // no per-layer facet copies are retained by the planner.
                match screen(bounds(&l.points), interval) {
                    ControlFlow::Break(result) => return Ok(ControlFlow::Break(result)),
                    ControlFlow::Continue(false) => {}
                    ControlFlow::Continue(true) => {
                        if let ControlFlow::Break(result) = checked_visit(TolerantSupportPiece {
                            points: l.points.clone(),
                            holes: section
                                .loops
                                .iter()
                                .zip(&owners)
                                .filter(|(_, owner)| **owner == Some(index))
                                .map(|(ring, _)| ring.points.clone())
                                .collect(),
                            height_mm: interval,
                        })? {
                            return Ok(ControlFlow::Break(result));
                        }
                    }
                }
            }
            for i in 0..l.points.len() {
                cancelled(cancel)?;
                let edge = [l.points[i], l.points[(i + 1) % l.points.len()]];
                match screen(padded_bounds(&edge, margin), interval) {
                    ControlFlow::Break(result) => return Ok(ControlFlow::Break(result)),
                    ControlFlow::Continue(false) => continue,
                    ControlFlow::Continue(true) => {}
                }
                if let ControlFlow::Break(result) = checked_visit(TolerantSupportPiece {
                    points: padded(edge.to_vec(), margin),
                    holes: Vec::new(),
                    height_mm: interval,
                })? {
                    return Ok(ControlFlow::Break(result));
                }
            }
        }
        let first = (((closed[0] - self.z_min) / super::BUCKET_MM)
            .floor()
            .max(0.0) as usize)
            .min(self.buckets.len());
        let end = (((closed[1] - self.z_min) / super::BUCKET_MM)
            .floor()
            .max(0.0) as usize)
            .saturating_add(1)
            .min(self.buckets.len());
        for bucket in first..end {
            for &index in &self.buckets[bucket] {
                cancelled(cancel)?;
                let vertices = self.triangles[index as usize].map(|i| self.positions[i as usize]);
                let low = vertices.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
                let owner = (((low - self.z_min) / super::BUCKET_MM).floor() as usize).max(first);
                if owner != bucket
                    || vertices.iter().all(|v| v[2] < closed[0])
                    || vertices.iter().all(|v| v[2] > closed[1])
                {
                    continue;
                }
                let points = clip(vertices, closed);
                if !points.is_empty() {
                    let low = points.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
                    let high = points
                        .iter()
                        .map(|p| p[2])
                        .fold(f64::NEG_INFINITY, f64::max);
                    let scope = [
                        (low - margin).next_down().max(interval[0]),
                        (high + margin).next_up().min(interval[1]),
                    ];
                    if scope[0] <= scope[1] {
                        let xy: Vec<[f64; 2]> = points.into_iter().map(|p| [p[0], p[1]]).collect();
                        match screen(padded_bounds(&xy, margin), scope) {
                            ControlFlow::Break(result) => return Ok(ControlFlow::Break(result)),
                            ControlFlow::Continue(false) => continue,
                            ControlFlow::Continue(true) => {}
                        }
                        if let ControlFlow::Break(result) = checked_visit(TolerantSupportPiece {
                            points: padded(xy, margin),
                            holes: Vec::new(),
                            height_mm: scope,
                        })? {
                            return Ok(ControlFlow::Break(result));
                        }
                    }
                }
            }
        }
        Ok(ControlFlow::Continue(()))
    }

    /// Boundary facets crossing the slab between `interval`'s heights, each
    /// projected to XY and padded by the support enclosure, limited to those
    /// whose padded bounds meet `within`. Facets lying wholly at or beyond
    /// either height (a flat face bounding the slab) do not cross it. A
    /// vertical line inside a section of the slab and outside every piece
    /// meets no boundary there, so it stays inside material over the whole
    /// slab: the slab's full-height inner material is that section minus the
    /// union of these pieces. Approximation is enclosed in XY.
    pub fn slab_crossings(
        &self,
        interval: [f64; 2],
        within: [[f64; 2]; 2],
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<[f64; 2]>>, GeometryError> {
        if !interval.iter().all(|z| z.is_finite()) || interval[0] >= interval[1] {
            return Err(unavailable("invalid inner-material slab interval"));
        }
        cancelled(cancel)?;
        let margin = self.support_margin();
        let mut pieces = Vec::new();
        if self.buckets.is_empty() {
            return Ok(pieces);
        }
        let bucket = |z: f64| {
            (((z - self.z_min) / super::BUCKET_MM).floor().max(0.0) as usize)
                .min(self.buckets.len() - 1)
        };
        let first = bucket(interval[0]);
        for index in first..=bucket(interval[1]) {
            for &triangle in &self.buckets[index] {
                cancelled(cancel)?;
                let vertices =
                    self.triangles[triangle as usize].map(|i| self.positions[i as usize]);
                let low = vertices.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
                let high = vertices
                    .iter()
                    .map(|p| p[2])
                    .fold(f64::NEG_INFINITY, f64::max);
                // Each triangle once: in its lowest bucket within the range.
                if bucket(low).max(first) != index || low >= interval[1] || high <= interval[0] {
                    continue;
                }
                let xy: Vec<[f64; 2]> = clip(vertices, interval)
                    .into_iter()
                    .map(|p| [p[0], p[1]])
                    .collect();
                if xy.is_empty() {
                    continue;
                }
                let [lo, hi] = padded_bounds(&xy, margin);
                if lo[0] > within[1][0]
                    || hi[0] < within[0][0]
                    || lo[1] > within[1][1]
                    || hi[1] < within[0][1]
                {
                    continue;
                }
                let piece = padded(xy, margin);
                if piece.iter().flatten().any(|v| !v.is_finite()) {
                    return Err(unavailable("inner-material crossing is not finite"));
                }
                pieces.push(piece);
            }
        }
        Ok(pieces)
    }

    /// Heights of the vertices where a horizontal section can change
    /// topology: extrema, saddles, vertices with a neighbour at the same
    /// height and vertices whose link is not one cycle; plus the bounds.
    fn critical_heights(&self, cancel: &AtomicBool) -> Result<Vec<f64>, GeometryError> {
        let count = self.positions.len();
        let mut start = vec![0usize; count + 1];
        for triangle in self.triangles.iter() {
            for v in triangle {
                start[*v as usize + 1] += 1;
            }
        }
        for v in 0..count {
            start[v + 1] += start[v];
        }
        let mut fill = start.clone();
        // Each corner's opposite edge in the triangle's winding.
        let mut link = vec![[0u32; 2]; start[count]];
        for t in self.triangles.iter() {
            for k in 0..3 {
                let v = t[k] as usize;
                link[fill[v]] = [t[(k + 1) % 3], t[(k + 2) % 3]];
                fill[v] += 1;
            }
        }
        drop(fill);
        let mut heights = Vec::new();
        for v in 0..count {
            if v % 4096 == 0 {
                cancelled(cancel)?;
            }
            let corners = &link[start[v]..start[v + 1]];
            if corners.is_empty() {
                continue;
            }
            let z = self.positions[v][2];
            let mut flat = false;
            let mut mixed = 0;
            for [p, q] in corners {
                let (zp, zq) = (
                    self.positions[*p as usize][2],
                    self.positions[*q as usize][2],
                );
                flat |= zp == z || zq == z;
                mixed += usize::from((zp < z) != (zq < z));
            }
            let mut single = false;
            if !flat && mixed == 2 {
                // Follow the link from the first corner around one cycle.
                let mut current = corners[0][1];
                let mut steps = 1;
                while current != corners[0][0] && steps <= corners.len() {
                    match corners.iter().find(|c| c[0] == current) {
                        Some(c) => current = c[1],
                        None => break,
                    }
                    steps += 1;
                }
                single = current == corners[0][0] && steps == corners.len();
            }
            if !single {
                heights.push(z);
            }
        }
        let (min, max) = self.bounds();
        heights.extend([min[2], max[2]]);
        heights.sort_by(f64::total_cmp);
        heights.dedup();
        Ok(heights)
    }

    fn void_roof(
        &self,
        [mouth, top]: [f64; 2],
        sides: impl Fn(usize) -> bool,
        remaining_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Option<TolerantVoidRoof>, GeometryError> {
        let margin = self.support_margin();
        if top - mouth <= 2.0 * margin {
            return Ok(None);
        }
        let cap_band = [top - margin, top + margin];
        let mouth_roundoff = 512.0 * f64::EPSILON * mouth.abs().max(1.0);
        let above_mouth = |&(a, b): &MeshEdge| {
            self.positions[a as usize][2].max(self.positions[b as usize][2])
                > mouth + mouth_roundoff
        };
        let cap_candidate = |facet: [u32; 3]| {
            let vertices = facet.map(|vertex| self.positions[vertex as usize]);
            if !vertices
                .iter()
                .all(|p| cap_band[0] <= p[2] && p[2] <= cap_band[1])
            {
                return false;
            }
            let [start, middle, end] = vertices.map(|p| p.map(Interval::point));
            let normal = middle[0]
                .sub(start[0])
                .mul(end[1].sub(start[1]))
                .sub(middle[1].sub(start[1]).mul(end[0].sub(start[0])));
            normal.hi < 0.0
        };
        let mut side_count = 0usize;
        let mut cap_count = 0usize;
        for (index, &facet) in self.triangles.iter().enumerate() {
            cancelled(cancel)?;
            if sides(index) {
                side_count = side_count.saturating_add(
                    facet_edges(facet)
                        .iter()
                        .filter(|edge| above_mouth(edge))
                        .count(),
                );
            }
            cap_count += usize::from(cap_candidate(facet));
        }
        if side_count == 0 || cap_count == 0 {
            return Ok(None);
        }
        let edge_count = side_count.saturating_add(cap_count.saturating_mul(3));
        let scratch = edge_count
            .saturating_mul(size_of::<RoofEdge>())
            .saturating_add(cap_count.saturating_mul(2 * size_of::<usize>() + size_of::<u8>()));
        let remaining_bytes = remaining_bytes
            .checked_sub(scratch)
            .ok_or_else(roof_resource_limit)?;
        let mut edges = roof_buffer(edge_count)?;
        let mut candidates = roof_buffer(cap_count)?;
        for (index, &facet) in self.triangles.iter().enumerate() {
            cancelled(cancel)?;
            if sides(index) {
                edges.extend(
                    facet_edges(facet)
                        .into_iter()
                        .filter(above_mouth)
                        .map(|edge| RoofEdge { edge, cap: None }),
                );
            }
            if cap_candidate(facet) {
                let cap = Some(candidates.len());
                candidates.push(index);
                edges.extend(facet_edges(facet).map(|edge| RoofEdge { edge, cap }));
            }
        }
        edges.sort_unstable_by_key(|entry| entry.edge);
        let adjacent = |edge: MeshEdge| {
            let first = edges.partition_point(|entry| entry.edge < edge);
            let last = edges.partition_point(|entry| entry.edge <= edge);
            &edges[first..last]
        };
        let is_fringe =
            |group: &[RoofEdge]| group.iter().filter(|entry| entry.cap.is_none()).count() % 2 != 0;
        let mut seen = roof_buffer::<u8>(cap_count)?;
        seen.resize(cap_count, 0);
        let mut pending = roof_buffer(cap_count)?;
        let push = |cap, seen: &mut [u8], pending: &mut Vec<usize>| {
            if seen[cap] == 0 {
                seen[cap] = 1;
                pending.push(cap);
            }
        };
        for group in edges.chunk_by(|left, right| left.edge == right.edge) {
            cancelled(cancel)?;
            if is_fringe(group) {
                let Some(cap) = group
                    .iter()
                    .filter_map(|entry| entry.cap)
                    .find(|cap| !sides(candidates[*cap]))
                else {
                    return Ok(None);
                };
                push(cap, &mut seen, &mut pending);
            }
        }
        if pending.is_empty() {
            return Ok(None);
        }
        let mut cursor = 0;
        while let Some(&cap) = pending.get(cursor) {
            cancelled(cancel)?;
            cursor += 1;
            for edge in facet_edges(self.triangles[candidates[cap]]) {
                let group = adjacent(edge);
                if !is_fringe(group) {
                    let Some(other) = group
                        .iter()
                        .filter_map(|entry| entry.cap)
                        .find(|other| *other != cap && !sides(candidates[*other]))
                    else {
                        return Ok(None);
                    };
                    push(other, &mut seen, &mut pending);
                }
            }
        }
        cursor = 0;
        while let Some(&cap) = pending.get(cursor) {
            cancelled(cancel)?;
            cursor += 1;
            for edge in facet_edges(self.triangles[candidates[cap]]) {
                for other in adjacent(edge).iter().filter_map(|entry| entry.cap) {
                    push(other, &mut seen, &mut pending);
                }
            }
        }
        if pending.len().saturating_mul(size_of::<[[f64; 2]; 3]>()) > remaining_bytes {
            return Err(roof_resource_limit());
        }
        let mut facets = roof_buffer(pending.len())?;
        let mut roof = [f64::INFINITY, f64::NEG_INFINITY];
        for (index, included) in candidates.into_iter().zip(seen) {
            cancelled(cancel)?;
            if included != 0 {
                facets.push(self.triangles[index].map(|vertex| {
                    let [x, y, z] = self.positions[vertex as usize];
                    roof[0] = roof[0].min(z);
                    roof[1] = roof[1].max(z);
                    [x, y]
                }));
            }
        }
        Ok(Some(TolerantVoidRoof {
            height_mm: [(roof[0] - margin).next_down(), (roof[1] + margin).next_up()],
            facets,
        }))
    }

    /// Exhaust every mesh topology interval. Raw traces retain triangle IDs,
    /// so complete swept hole boundaries are grouped independently of print
    /// layer sampling, analytic face labels or the presence of a sample hole.
    pub fn support_voids(
        &self,
        cancel: &AtomicBool,
    ) -> Result<Vec<TolerantVoidProjection>, GeometryError> {
        self.support_voids_with_roof_budget(usize::MAX, cancel)
    }

    /// The roof budget covers the live void results, cap discovery scratch,
    /// and all retained cap footprints. Earlier void enumeration is unchanged.
    pub fn support_voids_with_roof_budget(
        &self,
        roof_budget_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<TolerantVoidProjection>, GeometryError> {
        // A tolerant point join at one height is not an all-height closure.
        // Require the derived facet complex itself to close, after exact
        // coordinate welding, before asserting complete hole inventory.
        let mut edges = HashMap::<(u32, u32), u32>::new();
        for triangle in self.triangles.iter() {
            cancelled(cancel)?;
            for [a, b] in [
                [triangle[0], triangle[1]],
                [triangle[1], triangle[2]],
                [triangle[2], triangle[0]],
            ] {
                if a == b {
                    return Err(unavailable(
                        "support projection has a collapsed source facet",
                    ));
                }
                *edges
                    .entry(if a < b { (a, b) } else { (b, a) })
                    .or_default() += 1;
            }
        }
        let open = edges.values().filter(|n| **n != 2).count();
        if open > 0 {
            let mut faces: Vec<u32> = self
                .open_edges()
                .into_iter()
                .flat_map(|(_, faces)| faces)
                .collect();
            faces.sort_unstable();
            faces.dedup();
            let more = faces.len().saturating_sub(16);
            faces.truncate(16);
            return Err(unavailable(format!(
                "support projection requires a closed source facet boundary; sampled chain joins do not establish complete protected holes ({open} open mesh edges on source face ordinals {faces:?} and {more} more)",
            )));
        }
        drop(edges);
        struct Record {
            triangle: usize,
            points: Vec<[f64; 2]>,
            bottom: Vec<[f64; 2]>,
            height: [f64; 2],
        }
        let mut events: Vec<f64> = self.positions.iter().map(|p| p[2]).collect();
        events.sort_by(f64::total_cmp);
        events.dedup();
        // Vertex slabs with a representable midpoint are the ones a section
        // can sample; `sampled[k]` counts them among the first `k` slabs.
        let mut sampled = Vec::with_capacity(events.len());
        sampled.push(0usize);
        for pair in events.windows(2) {
            let mid = pair[0] + (pair[1] - pair[0]) * 0.5;
            let last = *sampled.last().expect("seeded");
            sampled.push(last + usize::from(pair[0] < mid && mid < pair[1]));
        }
        // Whether some sampled slab lies within the closed height range.
        let crosses_sampled = |low: f64, high: f64| {
            let first = events.partition_point(|e| *e < low);
            let last = events.partition_point(|e| *e <= high);
            last > first + 1 && sampled[last - 1] > sampled[first]
        };
        // The level sets of a closed mesh change topology only at critical
        // vertices. Between consecutive critical heights each connected band
        // of crossed triangles sweeps exactly one section loop, so one section
        // classifies every band, and the union of a band's per-slab clips is
        // its clip over the whole interval. Threads and other helical faces
        // put almost every vertex at its own height but few are critical.
        let critical = self.critical_heights(cancel)?;
        let heights: Vec<[f64; 2]> = self
            .triangles
            .iter()
            .map(|t| {
                let zs = t.map(|v| self.positions[v as usize][2]);
                [
                    zs.iter().copied().fold(f64::INFINITY, f64::min),
                    zs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                ]
            })
            .collect();
        let mut order: Vec<usize> = (0..self.triangles.len()).collect();
        order.sort_by(|a, b| heights[*a][0].total_cmp(&heights[*b][0]));
        let mut next = 0;
        let mut active: Vec<usize> = Vec::new();
        let mut parents: Vec<usize> = (0..self.triangles.len()).collect();
        let mut selected = vec![false; self.triangles.len()];
        let mut local = vec![usize::MAX; self.triangles.len()];
        let mut records = Vec::new();
        for pair in critical.windows(2) {
            cancelled(cancel)?;
            let [lo, hi] = [pair[0], pair[1]];
            while next < order.len() && heights[order[next]][0] < hi {
                active.push(order[next]);
                next += 1;
            }
            active.retain(|t| heights[*t][1] > lo);
            // The first and last sampled vertex slabs bound what the
            // per-slab sections would have seen.
            let first = events.partition_point(|e| *e < lo);
            let last = events.partition_point(|e| *e <= hi);
            let Some(low) = (first..last.saturating_sub(1)).find(|k| sampled[k + 1] > sampled[*k])
            else {
                continue;
            };
            let high = (first..last - 1)
                .rev()
                .find(|k| sampled[k + 1] > sampled[*k])
                .expect("a first sampled slab is also a last");
            let mid = events[low] + (events[low + 1] - events[low]) * 0.5;
            let [lo, hi] = [events[low], events[high + 1]];
            let holes: Vec<_> = self
                .section_with(mid, false, Some(cancel), None)?
                .loops
                .into_iter()
                .filter(|l| l.area < 0.0)
                .collect();
            if holes.is_empty() {
                continue;
            }
            if low == high {
                // One sampled slab: its section is the whole record. Keep only
                // the faces the section crossed; a slab a few ulps thick
                // (rounding noise on a flat face) must not enrol every
                // nearly flat triangle it touches.
                for l in holes {
                    let first = l.faces[0] as usize;
                    let mut points = l.points;
                    let mut bottom = Vec::new();
                    for index in l.faces {
                        let index = index as usize;
                        selected[index] = true;
                        join(&mut parents, first, index);
                        let vertices = self.triangles[index].map(|i| self.positions[i as usize]);
                        points.extend(clip(vertices, [lo, hi]).into_iter().map(|p| [p[0], p[1]]));
                        bottom.extend(clip(vertices, [lo, lo]).into_iter().map(|p| [p[0], p[1]]));
                    }
                    records.push(Record {
                        triangle: first,
                        points: hull(points),
                        bottom: hull(bottom),
                        height: [lo, hi],
                    });
                }
                continue;
            }
            // Bands: triangles crossed in a sampled slab, joined by edges
            // crossed there.
            let members: Vec<usize> = active
                .iter()
                .copied()
                .filter(|t| crosses_sampled(heights[*t][0].max(lo), heights[*t][1].min(hi)))
                .collect();
            for (i, &t) in members.iter().enumerate() {
                local[t] = i;
            }
            let member = |face: u32| {
                let i = local[face as usize];
                if members.get(i) == Some(&(face as usize)) {
                    Ok(i)
                } else {
                    Err(unavailable(
                        "support void section crossed a triangle outside its interval",
                    ))
                }
            };
            let mut bands: Vec<usize> = (0..members.len()).collect();
            let mut crossed = HashMap::<(u32, u32), usize>::new();
            for (i, &t) in members.iter().enumerate() {
                cancelled(cancel)?;
                let triangle = self.triangles[t];
                for [a, b] in [
                    [triangle[0], triangle[1]],
                    [triangle[1], triangle[2]],
                    [triangle[2], triangle[0]],
                ] {
                    let (za, zb) = (self.positions[a as usize][2], self.positions[b as usize][2]);
                    if za.min(zb) < hi
                        && za.max(zb) > lo
                        && crosses_sampled(za.min(zb).max(lo), za.max(zb).min(hi))
                    {
                        match crossed.entry(if a < b { (a, b) } else { (b, a) }) {
                            std::collections::hash_map::Entry::Occupied(o) => {
                                join(&mut bands, i, *o.get());
                            }
                            std::collections::hash_map::Entry::Vacant(v) => {
                                v.insert(i);
                            }
                        }
                    }
                }
            }
            drop(crossed);
            let mut band_records = HashMap::<usize, Record>::new();
            for l in &holes {
                let first = member(l.faces[0])?;
                for &face in &l.faces {
                    join(&mut bands, first, member(face)?);
                }
            }
            for l in holes {
                let band = root(&mut bands, member(l.faces[0])?);
                band_records
                    .entry(band)
                    .or_insert_with(|| Record {
                        triangle: l.faces[0] as usize,
                        points: Vec::new(),
                        bottom: Vec::new(),
                        height: [lo, hi],
                    })
                    .points
                    .extend(l.points);
            }
            for (i, &t) in members.iter().enumerate() {
                let band = root(&mut bands, i);
                let Some(record) = band_records.get_mut(&band) else {
                    continue;
                };
                selected[t] = true;
                join(&mut parents, record.triangle, t);
                let vertices = self.triangles[t].map(|v| self.positions[v as usize]);
                record
                    .points
                    .extend(clip(vertices, [lo, hi]).into_iter().map(|p| [p[0], p[1]]));
                record
                    .bottom
                    .extend(clip(vertices, [lo, lo]).into_iter().map(|p| [p[0], p[1]]));
            }
            let mut band_records: Vec<_> = band_records.into_values().collect();
            band_records.sort_by_key(|record| record.triangle);
            for mut record in band_records {
                record.points = hull(std::mem::take(&mut record.points));
                record.bottom = hull(std::mem::take(&mut record.bottom));
                records.push(record);
            }
        }
        let mut vertices = HashMap::new();
        for (i, triangle) in self
            .triangles
            .iter()
            .enumerate()
            .filter(|(i, _)| selected[*i])
        {
            cancelled(cancel)?;
            for vertex in triangle {
                if let Some(previous) = vertices.insert(*vertex, i) {
                    join(&mut parents, previous, i);
                }
            }
        }
        let margin = self.support_margin();
        let mut groups: HashMap<usize, TolerantVoidProjection> = HashMap::new();
        for record in records {
            cancelled(cancel)?;
            let group = groups
                .entry(root(&mut parents, record.triangle))
                .or_insert_with(|| TolerantVoidProjection {
                    polygon: Vec::new(),
                    height_mm: record.height,
                    bottom: Vec::new(),
                    floor: Vec::new(),
                    roof: None,
                });
            if record.height[0] < group.height_mm[0] {
                group.height_mm[0] = record.height[0];
                group.bottom.clear();
            }
            if record.height[0] == group.height_mm[0] {
                group.bottom.extend(record.bottom);
            }
            group.height_mm[1] = group.height_mm[1].max(record.height[1]);
            group.polygon.extend(record.points);
            group.polygon = hull(std::mem::take(&mut group.polygon));
        }
        let mut result: Vec<_> = groups.into_iter().collect();
        result.sort_by_key(|(index, _)| *index);
        for (index, &selected) in selected.iter().enumerate() {
            cancelled(cancel)?;
            if selected {
                let owner = root(&mut parents, index);
                parents[index] = owner;
            }
        }
        drop((
            events, sampled, critical, heights, order, active, local, vertices,
        ));
        for (_, group) in &mut result {
            cancelled(cancel)?;
            group.bottom = hull(std::mem::take(&mut group.bottom));
            for triangle in self.triangles.iter() {
                cancelled(cancel)?;
                let vertices = triangle.map(|i| self.positions[i as usize]);
                if vertices.iter().all(|p| p[2] == group.height_mm[0]) {
                    group.floor.push(vertices.map(|p| [p[0], p[1]]));
                }
            }
            group.polygon = padded(std::mem::take(&mut group.polygon), margin);
        }
        let void_bytes = result.iter().fold(
            result
                .capacity()
                .saturating_mul(size_of::<(usize, TolerantVoidProjection)>())
                .saturating_add(parents.capacity().saturating_mul(size_of::<usize>()))
                .saturating_add(selected.capacity().saturating_mul(size_of::<bool>())),
            |bytes, (_, group)| {
                bytes
                    .saturating_add(
                        group
                            .polygon
                            .capacity()
                            .saturating_mul(size_of::<[f64; 2]>()),
                    )
                    .saturating_add(
                        group
                            .bottom
                            .capacity()
                            .saturating_mul(size_of::<[f64; 2]>()),
                    )
                    .saturating_add(
                        group
                            .floor
                            .capacity()
                            .saturating_mul(size_of::<[[f64; 2]; 3]>()),
                    )
            },
        );
        let mut roof_bytes = roof_budget_bytes.saturating_sub(void_bytes);
        for (index, group) in &mut result {
            cancelled(cancel)?;
            let roof_header = size_of::<TolerantVoidRoof>() + 2 * size_of::<usize>();
            group.roof = self.void_roof(
                group.height_mm,
                |facet| selected[facet] && parents[facet] == *index,
                roof_bytes.saturating_sub(roof_header),
                cancel,
            )?;
            if let Some(roof) = &group.roof {
                roof_bytes = roof_bytes
                    .checked_sub(
                        roof_header.saturating_add(
                            roof.facets
                                .capacity()
                                .saturating_mul(size_of::<[[f64; 2]; 3]>()),
                        ),
                    )
                    .ok_or_else(roof_resource_limit)?;
            }
            group.height_mm = [
                (group.height_mm[0] - margin).next_down(),
                (group.height_mm[1] + margin).next_up(),
            ];
        }
        Ok(result.into_iter().map(|(_, group)| group).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{TolerantPolicy, contains};
    use super::*;
    use std::collections::BTreeMap;

    #[cfg(test)]
    impl TolerantModel {
        /// The per-vertex-slab algorithm `support_voids` replaced: one section
        /// between every pair of consecutive vertex heights. Test oracle.
        fn reference_support_voids(
            &self,
            cancel: &AtomicBool,
        ) -> Result<Vec<TolerantVoidProjection>, GeometryError> {
            struct Record {
                triangle: usize,
                points: Vec<[f64; 2]>,
                bottom: Vec<[f64; 2]>,
                height: [f64; 2],
            }
            let mut events: Vec<f64> = self.positions.iter().map(|p| p[2]).collect();
            events.sort_by(f64::total_cmp);
            events.dedup();
            let mut parents: Vec<usize> = (0..self.triangles.len()).collect();
            let mut selected = vec![false; self.triangles.len()];
            let mut records = Vec::new();
            for pair in events.windows(2) {
                cancelled(cancel)?;
                let mid = pair[0] + (pair[1] - pair[0]) * 0.5;
                if !(pair[0] < mid && mid < pair[1]) {
                    continue;
                }
                for l in self
                    .section_with(mid, false, Some(cancel), None)?
                    .loops
                    .into_iter()
                    .filter(|l| l.area < 0.0)
                {
                    let first = l.faces[0] as usize;
                    let mut points = l.points;
                    let mut bottom = Vec::new();
                    for index in l.faces {
                        let index = index as usize;
                        selected[index] = true;
                        join(&mut parents, first, index);
                        let vertices = self.triangles[index].map(|i| self.positions[i as usize]);
                        points.extend(
                            clip(vertices, [pair[0], pair[1]])
                                .into_iter()
                                .map(|p| [p[0], p[1]]),
                        );
                        bottom.extend(
                            clip(vertices, [pair[0], pair[0]])
                                .into_iter()
                                .map(|p| [p[0], p[1]]),
                        );
                    }
                    records.push(Record {
                        triangle: first,
                        points: hull(points),
                        bottom: hull(bottom),
                        height: [pair[0], pair[1]],
                    });
                }
            }
            if std::env::var("VOID_DEBUG").is_ok() {
                let list: Vec<usize> = (0..selected.len()).filter(|i| selected[*i]).collect();
                eprintln!("SELECTED {} {:?}", list.len(), list);
            }
            let mut vertices = HashMap::new();
            for (i, triangle) in self
                .triangles
                .iter()
                .enumerate()
                .filter(|(i, _)| selected[*i])
            {
                cancelled(cancel)?;
                for vertex in triangle {
                    if let Some(previous) = vertices.insert(*vertex, i) {
                        join(&mut parents, previous, i);
                    }
                }
            }
            let margin = self.support_margin();
            let mut groups: HashMap<usize, TolerantVoidProjection> = HashMap::new();
            for record in records {
                cancelled(cancel)?;
                let group = groups
                    .entry(root(&mut parents, record.triangle))
                    .or_insert_with(|| TolerantVoidProjection {
                        polygon: Vec::new(),
                        height_mm: record.height,
                        bottom: Vec::new(),
                        floor: Vec::new(),
                        roof: None,
                    });
                if record.height[0] < group.height_mm[0] {
                    group.height_mm[0] = record.height[0];
                    group.bottom.clear();
                }
                if record.height[0] == group.height_mm[0] {
                    group.bottom.extend(record.bottom);
                }
                group.height_mm[1] = group.height_mm[1].max(record.height[1]);
                group.polygon.extend(record.points);
                group.polygon = hull(std::mem::take(&mut group.polygon));
            }
            let mut result: Vec<_> = groups.into_iter().collect();
            result.sort_by_key(|(index, _)| *index);
            let mut result: Vec<_> = result.into_iter().map(|(_, value)| value).collect();
            for group in &mut result {
                cancelled(cancel)?;
                group.bottom = hull(std::mem::take(&mut group.bottom));
                for triangle in self.triangles.iter() {
                    cancelled(cancel)?;
                    let vertices = triangle.map(|i| self.positions[i as usize]);
                    if vertices.iter().all(|p| p[2] == group.height_mm[0]) {
                        group.floor.push(vertices.map(|p| [p[0], p[1]]));
                    }
                }
                group.polygon = padded(std::mem::take(&mut group.polygon), margin);
                group.height_mm = [
                    (group.height_mm[0] - margin).next_down(),
                    (group.height_mm[1] + margin).next_up(),
                ];
            }
            Ok(result)
        }
    }

    fn boxes(bounds: &[[[f64; 3]; 2]]) -> TolerantModel {
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for [min, max] in bounds {
            let base = positions.len() as u32;
            for bits in 0..8 {
                positions.push(std::array::from_fn(|a| {
                    if bits & (1 << a) == 0 { min[a] } else { max[a] }
                }));
            }
            for indices in [
                [0, 2, 3, 1],
                [4, 5, 7, 6],
                [0, 1, 5, 4],
                [2, 6, 7, 3],
                [0, 4, 6, 2],
                [1, 3, 7, 5],
            ] {
                let [a, b, c, d] = indices.map(|i| base + i);
                triangles.extend([[a, b, c], [a, c, d]]);
            }
        }
        let faces = vec![0; triangles.len()];
        TolerantModel::indexed(
            positions,
            triangles,
            faces,
            vec![None],
            TolerantPolicy::default(),
            0.01,
            0,
        )
    }

    /// A tube whose every vertex sits at its own height, like a modelled
    /// thread: each ring rises by a fraction of a layer around its turn.
    fn stepped_tube(segments: usize, layers: usize) -> TolerantModel {
        let ring =
            |k: usize, side: usize, j: usize| ((k * 2 + side) * segments + j % segments) as u32;
        let mut positions = Vec::new();
        for k in 0..=layers {
            for (side, radius) in [(0, 10.0), (1, 4.0)] {
                for j in 0..segments {
                    let angle =
                        std::f64::consts::TAU * j as f64 / segments as f64 + 0.05 * k as f64;
                    let z = k as f64 + 0.37 * j as f64 / segments as f64 + 0.011 * side as f64;
                    positions.push([radius * angle.cos(), radius * angle.sin(), z]);
                }
            }
        }
        let mut triangles = Vec::new();
        for j in 0..segments {
            for k in 0..layers {
                let (a, b, c, d) = (
                    ring(k, 0, j),
                    ring(k, 0, j + 1),
                    ring(k + 1, 0, j + 1),
                    ring(k + 1, 0, j),
                );
                triangles.extend([[a, b, c], [a, c, d]]);
                let (a, b, c, d) = (
                    ring(k, 1, j),
                    ring(k, 1, j + 1),
                    ring(k + 1, 1, j + 1),
                    ring(k + 1, 1, j),
                );
                triangles.extend([[a, c, b], [a, d, c]]);
            }
            let top = [
                ring(layers, 0, j),
                ring(layers, 0, j + 1),
                ring(layers, 1, j + 1),
                ring(layers, 1, j),
            ];
            triangles.extend([[top[0], top[1], top[2]], [top[0], top[2], top[3]]]);
            let bottom = [
                ring(0, 0, j),
                ring(0, 0, j + 1),
                ring(0, 1, j + 1),
                ring(0, 1, j),
            ];
            triangles.extend([
                [bottom[0], bottom[2], bottom[1]],
                [bottom[0], bottom[3], bottom[2]],
            ]);
        }
        let faces = vec![0; triangles.len()];
        TolerantModel::indexed(
            positions,
            triangles,
            faces,
            vec![None],
            TolerantPolicy::default(),
            0.01,
            0,
        )
    }

    fn area(points: &[[f64; 2]]) -> f64 {
        (0..points.len())
            .map(|i| {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            * 0.5
    }

    fn rectangular_recess(ceiling: bool, upside_down: bool) -> TolerantModel {
        let mut positions = Vec::new();
        for (radius, height) in [
            (5.0, 0.0),
            (5.0, 6.0),
            (3.0, 0.0),
            (3.0, if ceiling { 4.0 } else { 6.0 }),
        ] {
            for [x, y] in [
                [-radius, -radius],
                [radius, -radius],
                [radius, radius],
                [-radius, radius],
            ] {
                positions.push([x, y, height]);
            }
        }
        let mut triangles = Vec::new();
        let mut face = |[start, right, diagonal, left]: [u32; 4]| {
            triangles.extend([[start, right, diagonal], [start, diagonal, left]])
        };
        for index in 0..4 {
            let next = (index + 1) % 4;
            face([index, next, next + 4, index + 4]);
            face([index + 8, index + 12, next + 12, next + 8]);
            face([next, index, index + 8, next + 8]);
            if !ceiling {
                face([index + 4, next + 4, next + 12, index + 12]);
            }
        }
        if ceiling {
            face([4, 5, 6, 7]);
            face([15, 14, 13, 12]);
        }
        if upside_down {
            for position in &mut positions {
                position[2] = 6.0 - position[2];
            }
            for triangle in &mut triangles {
                triangle.swap(1, 2);
            }
        }
        let faces = vec![0; triangles.len()];
        TolerantModel::indexed(
            positions,
            triangles,
            faces,
            vec![None],
            TolerantPolicy::default(),
            0.01,
            0,
        )
    }

    #[test]
    fn only_bottom_open_capped_recess_has_a_terminal_roof() {
        let cancel = AtomicBool::new(false);
        for (ceiling, upside_down) in [(true, false), (false, false), (true, true)] {
            let model = rectangular_recess(ceiling, upside_down);
            let voids = model.support_voids(&cancel).unwrap();
            assert_eq!(voids.len(), 1);
            let roof = voids[0].roof.as_ref().map(|roof| roof.height_mm);
            if ceiling && !upside_down {
                assert!(roof.is_some_and(|[lower, upper]| lower < 4.0 && upper > 4.0));
            } else {
                assert_eq!(roof, None);
            }
        }
    }

    #[test]
    fn cap_discovery_refuses_before_unadmitted_scratch_or_footprint() {
        let cancel = AtomicBool::new(false);
        let model = rectangular_recess(true, false);
        let sides = |facet| facet < 24 && matches!(facet % 6, 2 | 3);
        let scratch = 26 * size_of::<RoofEdge>() + 2 * (2 * size_of::<usize>() + size_of::<u8>());
        let footprint = 2 * size_of::<[[f64; 2]; 3]>();
        for budget in [scratch - 1, scratch + footprint - 1] {
            let failure = model
                .void_roof([0.0, 4.0], sides, budget, &cancel)
                .err()
                .unwrap();
            assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
        }
        let roof = model
            .void_roof([0.0, 4.0], sides, scratch + footprint, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(roof.facets.len(), 2);
        let failure = model
            .support_voids_with_roof_budget(1, &cancel)
            .err()
            .unwrap();
        assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
    }

    fn block_with_void(empty: impl Fn([i32; 3]) -> bool) -> TolerantModel {
        let size = [7, 5, 5];
        let occupied = |cell: [i32; 3]| {
            (0..3).all(|axis| (0..size[axis]).contains(&cell[axis])) && !empty(cell)
        };
        let mut indices = BTreeMap::new();
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for x in 0..size[0] {
            for y in 0..size[1] {
                for z in 0..size[2] {
                    let cell = [x, y, z];
                    if !occupied(cell) {
                        continue;
                    }
                    for axis in 0..3 {
                        let along = (axis + 1) % 3;
                        let across = (axis + 2) % 3;
                        for direction in [-1, 1] {
                            let mut neighbor = cell;
                            neighbor[axis] += direction;
                            if occupied(neighbor) {
                                continue;
                            }
                            let corners = [[0, 0], [1, 0], [1, 1], [0, 1]].map(|[u, v]| {
                                let mut corner = cell;
                                corner[axis] += i32::from(direction > 0);
                                corner[along] += u;
                                corner[across] += v;
                                *indices.entry(corner).or_insert_with(|| {
                                    let index = positions.len() as u32;
                                    positions.push(corner.map(f64::from));
                                    index
                                })
                            });
                            let [first, mut next, opposite, mut previous] = corners;
                            if direction < 0 {
                                std::mem::swap(&mut next, &mut previous);
                            }
                            triangles
                                .extend([[first, next, opposite], [first, opposite, previous]]);
                        }
                    }
                }
            }
        }
        let faces = vec![0; triangles.len()];
        TolerantModel::indexed(
            positions,
            triangles,
            faces,
            vec![None],
            TolerantPolicy::default(),
            0.01,
            0,
        )
    }

    #[test]
    fn a_through_branch_or_side_exit_is_not_a_terminal_roof() {
        let cancel = AtomicBool::new(false);
        let in_mouth = |[x, y, _]: [i32; 3]| (1..6).contains(&x) && (1..4).contains(&y);
        for (branches, side_exit) in [(false, false), (true, false), (false, true)] {
            let model = block_with_void(|cell @ [x, y, z]| {
                let interior = in_mouth(cell) && if branches { z < 2 || x != 3 } else { z < 4 };
                interior || (side_exit && x == 0 && y == 2 && (2..4).contains(&z))
            });
            let voids = model.support_voids(&cancel).unwrap();
            assert!(!voids.is_empty(), "the mouth remains protected");
            assert_eq!(
                voids.iter().any(|void| void.roof.is_some()),
                !branches && !side_exit,
                "branches={branches}, side_exit={side_exit}"
            );
        }
    }

    /// Few vertices of a helical surface are critical, so the voids need a
    /// section per critical interval, not per vertex height; the result is
    /// the per-vertex-slab one.
    #[test]
    fn helical_voids_sample_critical_intervals_and_match_every_vertex_slab() {
        let model = stepped_tube(24, 12);
        let cancel = AtomicBool::new(false);
        let critical = model.critical_heights(&cancel).unwrap();
        let mut heights: Vec<f64> = model.positions.iter().map(|p| p[2]).collect();
        heights.sort_by(f64::total_cmp);
        heights.dedup();
        assert!(
            critical.len() * 4 < heights.len(),
            "{} critical of {} heights",
            critical.len(),
            heights.len()
        );
        let voids = model.support_voids(&cancel).unwrap();
        let reference = model.reference_support_voids(&cancel).unwrap();
        assert_eq!(voids.len(), 1);
        assert_eq!(reference.len(), 1);
        let (v, r) = (&voids[0], &reference[0]);
        assert_eq!(v.height_mm, r.height_mm);
        assert!((area(&v.polygon) - area(&r.polygon)).abs() < 1e-9);
        assert!((area(&v.bottom) - area(&r.bottom)).abs() < 1e-9);
        assert_eq!(v.floor, r.floor);
        // The hole is the inner radius-4 bore, not the radius-10 outside.
        let hole = area(&v.polygon);
        assert!(hole > 45.0 && hole < 100.0, "{hole}");
    }

    /// Rounding noise on a flat face makes slabs an ulp thick that no
    /// section can sample. The nearly flat triangles there must not join a
    /// pocket's void, nor separate pockets into one.
    #[test]
    fn ulp_slabs_on_a_flat_face_keep_separate_pockets() {
        // Two square pockets opening on the top face, the top face split
        // between them by a vertex one ulp low.
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        let ring = [[0.0, 0.0], [10.0, 0.0], [10.0, 4.0], [0.0, 4.0]];
        let pockets = [
            [[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]],
            [[7.0, 1.0], [9.0, 1.0], [9.0, 3.0], [7.0, 3.0]],
        ];
        let top = 2.0;
        let low = 2.0f64.next_down();
        // 0..4 outer top corners, 4..8 outer bottom corners.
        for p in ring {
            positions.push([p[0], p[1], top]);
        }
        for p in ring {
            positions.push([p[0], p[1], 0.0]);
        }
        let mut pocket_base = Vec::new();
        for (index, pocket) in pockets.iter().enumerate() {
            pocket_base.push(positions.len() as u32);
            // The first pocket's first rim corner sits one ulp low: it stays
            // a regular vertex between its wall and the top face.
            for (corner, p) in pocket.iter().enumerate() {
                let z = if index == 0 && corner == 0 { low } else { top };
                positions.push([p[0], p[1], z]);
            }
            for p in pocket {
                positions.push([p[0], p[1], 1.0]);
            }
        }
        let middle = positions.len() as u32;
        positions.push([5.0, 2.0, top]);
        // Outer walls and bottom.
        for i in 0..4u32 {
            let j = (i + 1) % 4;
            triangles.extend([[i, i + 4, j + 4], [i, j + 4, j]]);
        }
        triangles.extend([[4, 6, 5], [4, 7, 6]]);
        // Pocket walls and floors.
        for &base in &pocket_base {
            for i in 0..4u32 {
                let j = (i + 1) % 4;
                triangles.extend([
                    [base + i, base + j, base + j + 4],
                    [base + i, base + j + 4, base + i + 4],
                ]);
            }
            triangles.extend([
                [base + 4, base + 5, base + 6],
                [base + 4, base + 6, base + 7],
            ]);
        }
        // Top face: fan from the outer ring and pocket rims to the middle
        // vertex and the corners, covering the ring minus both pockets.
        let [a, b] = [pocket_base[0], pocket_base[1]];
        triangles.extend([
            [0, 1, b],
            [0, b, a],
            [1, 2, b + 1],
            [b, 1, b + 1],
            [2, b + 2, b + 1],
            [2, 3, a + 3],
            [2, a + 3, b + 2],
            [3, 0, a],
            [3, a, a + 3],
            [a, b, middle],
            [b, b + 3, middle],
            [b + 3, b + 2, middle],
            [b + 2, a + 2, middle],
            [a + 2, a + 1, middle],
            [a + 1, a, middle],
            [a + 2, b + 2, a + 3],
        ]);
        let faces = vec![0; triangles.len()];
        let model = TolerantModel::indexed(
            positions,
            triangles,
            faces,
            vec![None],
            TolerantPolicy::default(),
            0.01,
            0,
        );
        let cancel = AtomicBool::new(false);
        let voids = model.support_voids(&cancel).unwrap();
        let reference = model.reference_support_voids(&cancel).unwrap();
        assert_eq!(voids.len(), 2);
        assert_eq!(reference.len(), 2);
        for v in &voids {
            let a = area(&v.polygon);
            assert!(a < 9.0, "a pocket void spans {a} mm2");
        }
    }

    #[test]
    fn occupied_projection_preserves_cavities_and_covers_the_complete_material_slab() {
        let original = stepped_tube(24, 4);
        let cancel = AtomicBool::new(false);
        for shift in [0.0, 0.37] {
            let model = original.placed(&spiling_contracts::geometry::RigidPoseMm {
                translation_mm: [shift, 0.0, 0.0],
                ..spiling_contracts::geometry::RigidPoseMm::IDENTITY
            });
            let projection = model.support_projection([2.1, 2.9], &cancel).unwrap();
            let covered = |point| {
                projection.pieces.iter().any(|piece| {
                    contains(&piece.points, point)
                        && !piece.holes.iter().any(|hole| contains(hole, point))
                })
            };
            assert!(!covered([shift, 0.0]));
            for height_mm in [2.1, 2.5, 2.9] {
                let section = model.section(height_mm).unwrap();
                for radial in [4.5, 6.0, 8.5] {
                    for step in 0..12 {
                        let angle = f64::from(step) * std::f64::consts::TAU / 12.0;
                        let point = [shift + radial * angle.cos(), radial * angle.sin()];
                        assert!(
                            section
                                .loops
                                .iter()
                                .filter(|ring| contains(&ring.points, point))
                                .map(|ring| if ring.area > 0.0 { 1 } else { -1 })
                                .sum::<i32>()
                                > 0
                        );
                        assert!(covered(point), "lost material at {point:?}, Z={height_mm}");
                    }
                }
            }
        }
    }

    #[test]
    fn closed_slab_projection_catches_material_between_all_print_samples() {
        let model = boxes(&[
            [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            [[3.0, 0.0, 0.30], [4.0, 1.0, 0.31]],
        ]);
        let cancel = AtomicBool::new(false);
        // The old lower/middle/upper samples all miss the separate thin plate.
        for z in [0.01, 0.5, 0.99] {
            assert!(
                !model
                    .section(z)
                    .unwrap()
                    .loops
                    .iter()
                    .any(|l| contains(&l.points, [3.5, 0.5]))
            );
        }
        let envelope = model.support_projection([0.0, 1.0], &cancel).unwrap();
        assert!(
            envelope
                .pieces
                .iter()
                .any(|p| contains(&p.points, [3.5, 0.5]))
        );
        assert!(
            envelope
                .pieces
                .iter()
                .filter(|p| contains(&p.points, [3.5, 0.5]))
                .all(|p| p.height_mm[0] >= 0.279 && p.height_mm[1] <= 0.331),
            "the thin plate's projection must retain its physical height scope"
        );
        assert!(
            !envelope
                .pieces
                .iter()
                .any(|p| contains(&p.points, [2.0, 0.5])),
            "disjoint components must not become one convex box"
        );
        let mut visited = 0;
        let stopped = model
            .visit_support_projection([0.0, 1.0], &cancel, |_| {
                visited += 1;
                ControlFlow::Break("consumer stopped")
            })
            .unwrap();
        assert_eq!(stopped, ControlFlow::Break("consumer stopped"));
        assert_eq!(
            visited, 1,
            "a stopped consumer must not traverse or retain the remaining source pieces"
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            model
                .support_projection([0.0, 1.0], &cancel)
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn slab_crossings_exclude_every_boundary_inside_the_slab_only() {
        let model = boxes(&[
            [[0.0, 0.0, 0.0], [4.0, 4.0, 1.0]],
            [[0.0, 0.0, 1.0], [2.0, 4.0, 2.0]],
        ]);
        let cancel = AtomicBool::new(false);
        let everywhere = [[-10.0, -10.0], [10.0, 10.0]];
        let hit =
            |pieces: &[Vec<[f64; 2]>], p: [f64; 2]| pieces.iter().any(|piece| contains(piece, p));
        // Below the step: only the outer walls cross; the flat faces at 0
        // and 1 bound the slab and do not.
        let lower = model
            .slab_crossings([0.0, 1.0], everywhere, &cancel)
            .unwrap();
        assert!(!hit(&lower, [3.0, 2.0]) && !hit(&lower, [1.0, 2.0]));
        assert!(hit(&lower, [4.0, 2.0]) && hit(&lower, [0.0, 2.0]));
        // Across the step the lower box's top at 1 lies inside the slab and
        // excludes the region it caps; the upper box's wall at x = 2 crosses.
        let across = model
            .slab_crossings([0.5, 1.5], everywhere, &cancel)
            .unwrap();
        assert!(hit(&across, [3.0, 2.0]) && hit(&across, [2.0, 2.0]));
        // Pieces outside the requested XY window are not built.
        let window = model
            .slab_crossings([0.0, 1.0], [[3.5, 1.0], [4.5, 3.0]], &cancel)
            .unwrap();
        assert!(!hit(&window, [0.0, 2.0]) && hit(&window, [4.0, 2.0]));
    }

    #[test]
    fn swept_inclined_face_is_enclosed_between_vertices() {
        let mut model = boxes(&[[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]]);
        for p in &mut model.positions {
            p[0] += 2.0 * p[2];
        }
        let model = TolerantModel::indexed(
            model.positions,
            model.triangles,
            model.faces,
            vec![None],
            model.policy,
            model.reach,
            0,
        );
        let envelope = model
            .support_projection([0.25, 0.75], &AtomicBool::new(false))
            .unwrap();
        for z in [0.25, 0.31, 0.49, 0.68, 0.75] {
            for x in [0.0, 0.4, 1.0] {
                assert!(
                    envelope
                        .pieces
                        .iter()
                        .any(|p| contains(&p.points, [x + 2.0 * z, 0.5])),
                    "missed the continuous face at {z}"
                );
            }
        }
    }
}
