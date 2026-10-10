// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{OFFSET_TOLERANCE_MM, boolean_budget, forward};
use crate::{check_cancel, error};
use geo::algorithm::buffer::{BufferStyle, LineJoin};
use geo::{
    Area, BooleanOps, BoundingRect, Buffer, Contains, Coord, LineString, MultiPolygon, Point,
    Polygon,
};
use spiling_contracts::geometry::RigidPoseMm;
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) fn from_native(
    native: spiling_geometry::NativeSection,
    pose: &RigidPoseMm,
    sample: f64,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut outer = Vec::<Polygon<f64>>::new();
    let mut holes = Vec::new();
    let mut points = 0usize;
    for boundary in native.loops {
        check_cancel(cancel)?;
        points += boundary.points_mm.len();
        if points > MAX_DEPOSITION_SEGMENTS as usize {
            return Err(resource("native section point limit exceeded"));
        }
        let mut coordinates = Vec::with_capacity(boundary.points_mm.len());
        for p in boundary.points_mm {
            check_cancel(cancel)?;
            let world = forward(pose, p);
            if world.iter().any(|v| !v.is_finite()) || (world[2] - sample).abs() > 0.0001 {
                return Err(unsupported(
                    "native placed loop does not lie in declared horizontal sample plane",
                ));
            }
            coordinates.push(Coord {
                x: zero(world[0]),
                y: zero(world[1]),
            });
        }
        if coordinates.len() < 4 || coordinates.first() != coordinates.last() {
            return Err(unsupported("native section loop is not closed"));
        }
        let ring = LineString(coordinates);
        if boundary.is_hole {
            holes.push(ring);
        } else {
            outer.push(Polygon::new(ring, Vec::new()));
        }
    }
    for hole in holes {
        let point = Point(hole.0[0]);
        let owner = outer
            .iter()
            .enumerate()
            .filter(|(_, p)| p.contains(&point))
            .min_by(|(_, a), (_, b)| a.unsigned_area().total_cmp(&b.unsigned_area()))
            .map(|(i, _)| i)
            .ok_or_else(|| unsupported("native hole has no containing island"))?;
        outer[owner].interiors_push(hole);
    }
    let mut result = MultiPolygon(outer);
    check_size(&result)?;
    canonicalize(&mut result);
    Ok(result)
}

pub(super) fn point_count(region: &MultiPolygon<f64>) -> usize {
    region
        .0
        .iter()
        .map(|p| p.exterior().0.len() + p.interiors().iter().map(|h| h.0.len()).sum::<usize>())
        .sum()
}
pub(super) fn check_size(region: &MultiPolygon<f64>) -> Result<(), ManufacturingError> {
    if point_count(region) > MAX_DEPOSITION_SEGMENTS as usize {
        return Err(resource("planar region point limit exceeded"));
    }
    if region.0.iter().any(|p| {
        !p.unsigned_area().is_finite()
            || p.unsigned_area() <= 0.0
            || p.exterior()
                .0
                .iter()
                .chain(p.interiors().iter().flat_map(|h| h.0.iter()))
                .any(|c| !c.x.is_finite() || !c.y.is_finite())
    }) {
        return Err(unsupported(
            "planar operation returned nonfinite or degenerate region",
        ));
    }
    Ok(())
}
pub(super) fn offset(
    region: &MultiPolygon<f64>,
    distance: f64,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    check_size(region)?;
    let radius = distance.abs();
    // LineJoin::Round takes maximum chord length / radius, not segment count.
    let ratio = if radius <= OFFSET_TOLERANCE_MM {
        1.0
    } else {
        2.0 * (2.0 * OFFSET_TOLERANCE_MM / radius - (OFFSET_TOLERANCE_MM / radius).powi(2)).sqrt()
    };
    if !ratio.is_finite() || ratio <= 0.0 || (std::f64::consts::TAU / ratio).ceil() > 4096.0 {
        return Err(resource("offset arc subdivision exceeds work limit"));
    }
    boolean_budget(region, region)?;
    let mut expanded_points = point_count(region);
    for polygon in &region.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let n = ring.0.len() - 1;
            for i in 0..n {
                check_cancel(cancel)?;
                let a = ring.0[(i + n - 1) % n];
                let b = ring.0[i];
                let c = ring.0[(i + 1) % n];
                let incoming = [b.x - a.x, b.y - a.y];
                let outgoing = [c.x - b.x, c.y - b.y];
                let turn = (incoming[0] * outgoing[1] - incoming[1] * outgoing[0])
                    .atan2(incoming[0] * outgoing[0] + incoming[1] * outgoing[1]);
                if turn * distance > 0.0 {
                    let additional = (turn.abs() / ratio.min(1.0)).ceil() as usize + 1;
                    expanded_points = expanded_points
                        .checked_add(additional)
                        .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                        .ok_or_else(|| {
                            resource("offset expanded point budget exceeded before buffering")
                        })?;
                }
            }
        }
    }
    let style = || BufferStyle::new(distance).line_join(LineJoin::Round(ratio.min(1.0)));
    let mut result = if distance < 0.0 {
        // Erosion cannot join disconnected material islands. Buffer each island in
        // its own coordinate range so distant placements do not reduce the
        // integer overlay's precision and introduce spurious contour fragments.
        let mut polygons = Vec::with_capacity(region.0.len());
        for polygon in &region.0 {
            check_cancel(cancel)?;
            let mut eroded = polygon.buffer_with_style(style());
            check_size(&eroded)?;
            polygons.append(&mut eroded.0);
        }
        MultiPolygon(polygons)
    } else {
        region.buffer_with_style(style())
    };
    check_cancel(cancel)?;
    check_size(&result)?;
    canonicalize(&mut result);
    Ok(result)
}

pub(super) fn canonicalize(region: &mut MultiPolygon<f64>) {
    for p in &mut region.0 {
        p.exterior_mut(|r| canonical_ring(r, true));
        p.interiors_mut(|rings| {
            for r in rings.iter_mut() {
                canonical_ring(r, false);
            }
            rings.sort_by(ring_compare);
        });
    }
    region
        .0
        .sort_by(|a, b| ring_compare(a.exterior(), b.exterior()));
}
fn canonical_ring(ring: &mut LineString<f64>, ccw: bool) {
    if ring.0.len() < 4 {
        return;
    }
    ring.0.pop();
    for c in &mut ring.0 {
        c.x = zero(c.x);
        c.y = zero(c.y);
    }
    let signed: f64 = (0..ring.0.len())
        .map(|i| {
            let a = ring.0[i];
            let b = ring.0[(i + 1) % ring.0.len()];
            a.x * b.y - b.x * a.y
        })
        .sum();
    if (signed > 0.0) != ccw {
        ring.0.reverse();
    }
    let start = (0..ring.0.len())
        .min_by(|&a, &b| coord_compare(&ring.0[a], &ring.0[b]))
        .unwrap_or(0);
    ring.0.rotate_left(start);
    ring.0.push(ring.0[0]);
}
fn coord_compare(a: &Coord<f64>, b: &Coord<f64>) -> std::cmp::Ordering {
    a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y))
}
fn ring_compare(a: &LineString<f64>, b: &LineString<f64>) -> std::cmp::Ordering {
    for (a, b) in a.0.iter().zip(&b.0) {
        let c = coord_compare(a, b);
        if c != std::cmp::Ordering::Equal {
            return c;
        }
    }
    a.0.len().cmp(&b.0.len())
}

pub(super) fn paths(
    section: &MultiPolygon<f64>,
    z: f64,
    index: u32,
    recipe: &PlanarPrintRecipe,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<DepositionPath>, ManufacturingError> {
    let width = recipe.bead_width_mm;
    let mut paths = Vec::new();
    for wall in 0..recipe.perimeter_count {
        let centers = offset(section, -(wall as f64 + 0.5) * width, cancel)?;
        if centers.0.len() != section.0.len() {
            return Err(unsupported(
                "requested perimeter offset erases or splits an island; thin geometry is unsupported",
            ));
        }
        for original in &section.0 {
            check_cancel(cancel)?;
            let mut descendants = centers
                .0
                .iter()
                .filter(|p| original.contains(&Point(p.exterior().0[0])));
            let Some(descendant) = descendants.next() else {
                return Err(unsupported("requested perimeter offset erases an island"));
            };
            if descendants.next().is_some()
                || descendant.interiors().len() != original.interiors().len()
            {
                return Err(unsupported(
                    "requested perimeter offset changes island/hole topology",
                ));
            }
        }
        if wall == 0 {
            // Conservative miter dilation preserves square corners while exposing erased
            // narrow branches. Tolerance accounts for section and offset approximation.
            if point_count(&centers) > MAX_DEPOSITION_SEGMENTS as usize / 2 {
                return Err(resource("miter dilation expanded point budget exceeded"));
            }
            boolean_budget(&centers, &centers)?;
            let reachable = centers.buffer_with_style(
                BufferStyle::new(width * 0.5 + 2.0 * OFFSET_TOLERANCE_MM)
                    .line_join(LineJoin::Miter(0.01)),
            );
            check_cancel(cancel)?;
            check_size(&reachable)?;
            boolean_budget(section, &reachable)?;
            let unreachable_area = section.difference(&reachable).unsigned_area();
            check_cancel(cancel)?;
            if unreachable_area > OFFSET_TOLERANCE_MM.powi(2) {
                return Err(unsupported(
                    "section contains thin regions unreachable by nominal bead centers",
                ));
            }
        }
        for p in &centers.0 {
            for ring in std::iter::once(p.exterior()).chain(p.interiors()) {
                check_cancel(cancel)?;
                reserve_segments(segments, ring.0.len() - 1)?;
                let points_mm = ring.0.iter().map(|c| [c.x, c.y, z]).collect();
                push_path(&mut paths, DepositionKind::Perimeter, points_mm)?;
            }
        }
    }
    let fill = offset(section, -(recipe.perimeter_count as f64) * width, cancel)?;
    let h = recipe.layer_height_mm;
    let spacing = ((width - h) * h + std::f64::consts::PI * (h * 0.5).powi(2)) / h;
    for polygon in &fill.0 {
        check_cancel(cancel)?;
        let rect = polygon
            .bounding_rect()
            .ok_or_else(|| unsupported("fill has no finite bounds"))?;
        let swap = index % 2 == 1;
        let (min, max) = if swap {
            (rect.min().x, rect.max().x)
        } else {
            (rect.min().y, rect.max().y)
        };
        let row_count = ((max - min) / spacing).ceil().max(1.0);
        if !row_count.is_finite()
            || row_count > (MAX_DEPOSITION_SEGMENTS as usize - *segments) as f64
        {
            return Err(resource("solid hatch row limit exceeded"));
        }
        let start = min + ((max - min) - (row_count - 1.0) * spacing) * 0.5;
        let mut hits = Vec::new();
        for row in 0..row_count as usize {
            check_cancel(cancel)?;
            hits.clear();
            let coordinate = start + row as f64 * spacing;
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for edge in ring.0.windows(2) {
                    let a = if swap {
                        [edge[0].y, edge[0].x]
                    } else {
                        [edge[0].x, edge[0].y]
                    };
                    let b = if swap {
                        [edge[1].y, edge[1].x]
                    } else {
                        [edge[1].x, edge[1].y]
                    };
                    // Half-open crossing avoids double-counting a shared vertex.
                    if (a[1] <= coordinate && coordinate < b[1])
                        || (b[1] <= coordinate && coordinate < a[1])
                    {
                        hits.push(a[0] + (coordinate - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                    }
                }
            }
            hits.sort_by(f64::total_cmp);
            if hits.len() % 2 != 0 {
                return Err(unsupported("solid hatch clipping has odd boundary parity"));
            }
            for pair in hits.as_chunks::<2>().0 {
                if pair[1] - pair[0] <= 0.000002 {
                    return Err(unsupported(
                        "solid hatch creates unresolved thin or zero-length deposition",
                    ));
                }
                reserve_segments(segments, 1)?;
                let mut a = if swap {
                    [coordinate, pair[0], z]
                } else {
                    [pair[0], coordinate, z]
                };
                let mut b = if swap {
                    [coordinate, pair[1], z]
                } else {
                    [pair[1], coordinate, z]
                };
                if row % 2 == 1 {
                    std::mem::swap(&mut a, &mut b);
                }
                push_path(&mut paths, DepositionKind::SolidFill, vec![a, b])?;
            }
        }
    }
    if paths.is_empty() {
        return Err(unsupported(
            "section has no printable nominal deposition paths",
        ));
    }
    Ok(paths)
}
fn push_path(
    paths: &mut Vec<DepositionPath>,
    kind: DepositionKind,
    points_mm: Vec<[f64; 3]>,
) -> Result<(), ManufacturingError> {
    if points_mm
        .windows(2)
        .any(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]) <= 0.000002)
    {
        return Err(unsupported(
            "offset creates unresolved or zero-length deposition segment",
        ));
    }
    paths.push(DepositionPath { kind, points_mm });
    Ok(())
}
fn reserve_segments(count: &mut usize, additional: usize) -> Result<(), ManufacturingError> {
    if additional > MAX_DEPOSITION_SEGMENTS as usize - *count {
        return Err(resource("retained deposition segment limit exceeded"));
    }
    *count += additional;
    Ok(())
}
fn unsupported(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::UnsupportedGeometry, message)
}
fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}
fn zero(x: f64) -> f64 {
    if x == 0.0 { 0.0 } else { x }
}
