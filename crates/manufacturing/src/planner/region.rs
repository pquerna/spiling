// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{MAX_BOOLEAN_PAIRS, OFFSET_TOLERANCE_MM, boolean_budget, forward};
use crate::{check_cancel, error};
use geo::algorithm::buffer::{BufferStyle, LineJoin};
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{
    Area, BooleanOps, BoundingRect, Buffer, Contains, ConvexHull, Coord, LineString, MultiPoint,
    MultiPolygon, Point, Polygon, unary_union,
};
use spiling_contracts::geometry::RigidPoseMm;
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

/// Borrowed mandatory coverage operand. Requested relief may contain fine
/// additions, but these source/ordinary material pieces must remain reachable.
pub(super) struct RequiredMaterial<'a> {
    pub(super) polygons: Vec<&'a Polygon<f64>>,
}
impl<'a> RequiredMaterial<'a> {
    pub(super) fn from_section(section: &'a MultiPolygon<f64>) -> Self {
        Self {
            polygons: section.0.iter().collect(),
        }
    }
}
impl BooleanOps for RequiredMaterial<'_> {
    type Scalar = f64;
    fn rings(&self) -> impl Iterator<Item = &LineString<f64>> {
        self.polygons
            .iter()
            .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
    }
}

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
    if region.0.is_empty() || distance == 0.0 {
        return Ok(region.clone());
    }
    if !distance.is_finite() {
        return Err(unsupported("offset distance is not finite"));
    }
    let radius = distance.abs();
    // A circumscribed disk reserves half the existing offset error budget for
    // ordinary Boolean arithmetic. Fourfold symmetry retains cardinal supports.
    let radial_error = OFFSET_TOLERANCE_MM * 0.5;
    let angle = (radius / (radius + radial_error)).acos();
    let required = (std::f64::consts::PI / angle).ceil();
    if !required.is_finite() || required > 4096.0 {
        return Err(resource("offset disk subdivision exceeds work limit"));
    }
    let sides = (required as usize).max(4).div_ceil(4) * 4;
    let circumradius = radius / (std::f64::consts::PI / sides as f64).cos();
    if !circumradius.is_finite() || circumradius - radius > radial_error {
        return Err(resource("offset disk cannot meet the radial error budget"));
    }
    let disk = (0..sides)
        .map(|index| {
            let angle = std::f64::consts::TAU * (index as f64 + 0.5) / sides as f64;
            Coord {
                x: circumradius * angle.cos(),
                y: circumradius * angle.sin(),
            }
        })
        .collect::<Vec<_>>();
    let mut result = if distance < 0.0 {
        // S eroded by a disk is S minus the disk-neighborhood of its boundary.
        // Build that neighborhood from convex segment capsules, not independently
        // shifted finite edges whose disappearing supports can form false islands.
        // Each island retains its own Boolean coordinate range.
        let mut polygons = Vec::with_capacity(region.0.len());
        for polygon in &region.0 {
            check_cancel(cancel)?;
            let tube = boundary_tube(polygon, &disk, cancel)?;
            let source = MultiPolygon(vec![polygon.clone()]);
            boolean_budget(&source, &tube, cancel)?;
            let mut eroded = source.difference(&tube);
            check_cancel(cancel)?;
            check_size(&eroded)?;
            polygons.append(&mut eroded.0);
        }
        MultiPolygon(polygons)
    } else {
        let mut expanded = region.clone();
        for polygon in &region.0 {
            let tube = boundary_tube(polygon, &disk, cancel)?;
            boolean_budget(&expanded, &tube, cancel)?;
            expanded = expanded.union(&tube);
            check_cancel(cancel)?;
            check_size(&expanded)?;
        }
        expanded
    };
    check_cancel(cancel)?;
    check_size(&result)?;
    canonicalize(&mut result);
    Ok(result)
}

fn boundary_tube(
    polygon: &Polygon<f64>,
    disk: &[Coord<f64>],
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut primitives = MultiPolygon(Vec::new());
    let mut retained_points = 0usize;
    for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
        let mut start = 0;
        while start + 1 < ring.0.len() {
            check_cancel(cancel)?;
            let a = ring.0[start];
            let mut end = start + 1;
            // Collinear monotone partitions have exactly the same disk
            // neighborhood as their full segment. Coalesce only construction
            // capsules; never simplify or round the authoritative material ring.
            while end + 1 < ring.0.len() {
                check_cancel(cancel)?;
                let b = ring.0[end];
                let c = ring.0[end + 1];
                if RobustKernel::orient2d(a, b, c) != Orientation::Collinear
                    || b.x < a.x.min(c.x)
                    || b.x > a.x.max(c.x)
                    || b.y < a.y.min(c.y)
                    || b.y > a.y.max(c.y)
                {
                    break;
                }
                end += 1;
            }
            let segment = [a, ring.0[end]];
            start = end;
            // Only this convex construction primitive is hulled; authoritative
            // material rings are never simplified, snapped or replaced by a hull.
            let capsule = MultiPoint(
                segment
                    .iter()
                    .flat_map(|endpoint| {
                        disk.iter().map(move |delta| {
                            Point(Coord {
                                x: endpoint.x + delta.x,
                                y: endpoint.y + delta.y,
                            })
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .convex_hull();
            retained_points = retained_points
                .checked_add(capsule.exterior().0.len())
                .filter(|count| *count <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| resource("offset primitive point limit exceeded"))?;
            let count = primitives.0.len() + 1;
            if count
                .checked_mul(count)
                .is_none_or(|pairs| pairs > MAX_BOOLEAN_PAIRS)
            {
                return Err(resource("offset primitive pair admission limit exceeded"));
            }
            primitives.0.push(capsule);
        }
    }
    check_size(&primitives)?;
    boolean_budget(&primitives, &primitives, cancel)?;
    check_cancel(cancel)?;
    // One overlay coordinate frame for all boundary capsules. Incremental
    // unions repeatedly quantize prior results and can create sub-grid edges.
    let tube = unary_union(&primitives.0);
    check_cancel(cancel)?;
    check_size(&tube)?;
    Ok(tube)
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

pub(super) fn wall_centers(
    section: &MultiPolygon<f64>,
    required_material: &RequiredMaterial<'_>,
    recipe: &PlanarPrintRecipe,
    retained_native_points: usize,
    cancel: &AtomicBool,
) -> Result<Vec<MultiPolygon<f64>>, ManufacturingError> {
    if required_material.polygons.is_empty() && !section.0.is_empty() {
        return Err(unsupported(
            "nonempty requested material requires a mandatory coverage operand",
        ));
    }
    let width = recipe.bead_width_mm;
    let mut walls = Vec::with_capacity(recipe.perimeter_count as usize);
    let mut retained_points = retained_native_points;
    for wall in 0..recipe.perimeter_count {
        let centers = offset(section, -(wall as f64 + 0.5) * width, cancel)?;
        if centers.0.len() != section.0.len() {
            return Err(unsupported(&format!(
                "requested perimeter offset erases or splits an island; thin geometry is unsupported (source islands {}, offset islands {}, section area {} mm², bead width {} mm)",
                section.0.len(),
                centers.0.len(),
                section.unsigned_area(),
                width
            )));
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
            boolean_budget(&centers, &centers, cancel)?;
            let reachable = centers.buffer_with_style(
                BufferStyle::new(width * 0.5 + 2.0 * OFFSET_TOLERANCE_MM)
                    .line_join(LineJoin::Miter(0.01)),
            );
            check_cancel(cancel)?;
            check_size(&reachable)?;
            super::boolean::budget_parts(&required_material.polygons, &reachable, cancel)?;
            let unreachable = required_material.difference(&reachable);
            check_size(&unreachable)?;
            let unreachable_area = unreachable.unsigned_area();
            check_cancel(cancel)?;
            if unreachable_area > OFFSET_TOLERANCE_MM.powi(2) {
                return Err(unsupported(
                    "section contains thin regions unreachable by nominal bead centers",
                ));
            }
        }
        retained_points = retained_points
            .checked_add(point_count(&centers))
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| resource("retained wall center point limit exceeded"))?;
        walls.push(centers);
    }
    Ok(walls)
}

pub(super) fn wall_paths(
    centers: &MultiPolygon<f64>,
    z: f64,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for p in &centers.0 {
        for ring in std::iter::once(p.exterior()).chain(p.interiors()) {
            check_cancel(cancel)?;
            reserve_segments(segments, ring.0.len() - 1)?;
            let points_mm = ring.0.iter().map(|c| [c.x, c.y, z]).collect();
            push_path(paths, DepositionKind::Perimeter, points_mm)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HatchAxis {
    X,
    Y,
}
impl HatchAxis {
    pub(super) fn preferred(index: u32) -> Self {
        if index.is_multiple_of(2) {
            Self::X
        } else {
            Self::Y
        }
    }
    pub(super) fn orthogonal(self) -> Self {
        match self {
            Self::X => Self::Y,
            Self::Y => Self::X,
        }
    }
}

pub(super) struct HatchLayer<'a> {
    pub z: f64,
    pub height: f64,
    pub axis: HatchAxis,
    pub recipe: &'a PlanarPrintRecipe,
}

// Fill endpoints belong to the eroded interior boundary, half a bead from
// the final wall center. No wall-extension strokes or duplicate outlines.
pub(super) fn hatch(
    fill: &MultiPolygon<f64>,
    kind: DepositionKind,
    fraction: f64,
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let HatchLayer {
        z,
        height: h,
        axis,
        recipe,
    } = *layer;
    let width = recipe.bead_width_mm;
    let solid_spacing = ((width - h) * h + std::f64::consts::PI * (h * 0.5).powi(2)) / h;
    for polygon in &fill.0 {
        check_cancel(cancel)?;
        let rect = polygon
            .bounding_rect()
            .ok_or_else(|| unsupported("fill has no finite bounds"))?;
        let swap = axis == HatchAxis::Y;
        let (min, max) = if swap {
            (rect.min().x, rect.max().x)
        } else {
            (rect.min().y, rect.max().y)
        };
        // Multiplying density before dividing avoids overflow for tiny finite
        // fractions; a single centered sparse row needs no finite huge pitch.
        let row_count = (((max - min) / solid_spacing) * fraction).ceil().max(1.0);
        if !row_count.is_finite()
            || row_count > (MAX_DEPOSITION_SEGMENTS as usize - *segments) as f64
        {
            return Err(resource("rectilinear hatch row limit exceeded"));
        }
        let spacing = if row_count == 1.0 {
            0.0
        } else {
            solid_spacing / fraction
        };
        let start = min + ((max - min) - (row_count - 1.0) * spacing) * 0.5;
        let mut hits = Vec::new();
        for row in 0..row_count as usize {
            check_cancel(cancel)?;
            hits.clear();
            let coordinate = start + row as f64 * spacing;
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for edge in ring.0.windows(2) {
                    check_cancel(cancel)?;
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
                return Err(unsupported(
                    "rectilinear hatch clipping has odd boundary parity",
                ));
            }
            for pair in hits.as_chunks::<2>().0 {
                if pair[1] - pair[0] <= 0.000002 {
                    return Err(unsupported(
                        "rectilinear hatch creates unresolved thin or zero-length deposition",
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
                push_path(paths, kind, vec![a, b])?;
            }
        }
    }
    Ok(())
}
fn push_path(
    paths: &mut Vec<DepositionPath>,
    kind: DepositionKind,
    points_mm: Vec<[f64; 3]>,
) -> Result<(), ManufacturingError> {
    if let Some(edge) = points_mm
        .windows(2)
        .find(|edge| (edge[1][0] - edge[0][0]).hypot(edge[1][1] - edge[0][1]) <= 0.000002)
    {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            format!(
                "offset creates unresolved or zero-length deposition segment; role {kind:?}, {:?} to {:?}, length {:.12} mm",
                edge[0],
                edge[1],
                (edge[1][0] - edge[0][0]).hypot(edge[1][1] - edge[0][1])
            ),
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
