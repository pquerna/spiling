// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{TolerantModel, unavailable};
use crate::{GeometryError, GeometryErrorCode};
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
        for l in self
            .section_with((interval[0] + interval[1]) * 0.5, false)?
            .loops
        {
            cancelled(cancel)?;
            if l.area > 0.0 {
                // Try the coarse interior exclusion first. One contour copy lets
                // a stopped consumer avoid constructing all boundary expansions;
                // no per-layer facet copies are retained by the planner.
                if let ControlFlow::Break(result) = checked_visit(TolerantSupportPiece {
                    points: l.points.clone(),
                    height_mm: interval,
                })? {
                    return Ok(ControlFlow::Break(result));
                }

                for i in 0..l.points.len() {
                    cancelled(cancel)?;
                    if let ControlFlow::Break(result) = checked_visit(TolerantSupportPiece {
                        points: padded(
                            vec![l.points[i], l.points[(i + 1) % l.points.len()]],
                            margin,
                        ),
                        height_mm: interval,
                    })? {
                        return Ok(ControlFlow::Break(result));
                    }
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
                    if scope[0] <= scope[1]
                        && let ControlFlow::Break(result) = checked_visit(TolerantSupportPiece {
                            points: padded(
                                points.into_iter().map(|p| [p[0], p[1]]).collect(),
                                margin,
                            ),
                            height_mm: scope,
                        })?
                    {
                        return Ok(ControlFlow::Break(result));
                    }
                }
            }
        }
        Ok(ControlFlow::Continue(()))
    }

    /// Exhaust every mesh topology interval. Raw traces retain triangle IDs,
    /// so complete swept hole boundaries are grouped independently of print
    /// layer sampling, analytic face labels or the presence of a sample hole.
    pub fn support_voids(
        &self,
        cancel: &AtomicBool,
    ) -> Result<Vec<TolerantVoidProjection>, GeometryError> {
        // A tolerant point join at one height is not an all-height closure.
        // Require the derived facet complex itself to close, after exact
        // coordinate welding, before asserting complete hole inventory.
        let mut edges = HashMap::<(u32, u32), u32>::new();
        for triangle in &self.triangles {
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
        if edges.values().any(|n| *n != 2) {
            return Err(unavailable(
                "support projection requires a closed source facet boundary; sampled chain joins do not establish complete protected holes",
            ));
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
                .section_with(mid, false)?
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
            for triangle in &self.triangles {
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

#[cfg(test)]
mod tests {
    use super::super::{TolerantPolicy, contains};
    use super::*;

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
