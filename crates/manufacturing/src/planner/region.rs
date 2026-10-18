// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{MAX_BOOLEAN_PAIRS, OFFSET_TOLERANCE_MM, boolean_budget, forward};
use crate::{check_cancel, error};
use geo::algorithm::buffer::{BufferStyle, LineJoin};
use geo::algorithm::convex_hull::quick_hull;
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{
    Area, BooleanOps, BoundingRect, Buffer, Contains, Coord, LineString, MultiPolygon, Point,
    Polygon, unary_union,
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
    native: &spiling_geometry::NativeSection,
    pose: &RigidPoseMm,
    sample: f64,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut outer = Vec::<Polygon<f64>>::new();
    let mut holes = Vec::new();
    let mut points = 0usize;
    for boundary in native.loops() {
        check_cancel(cancel)?;
        points += boundary.points_mm.len();
        if points > MAX_DEPOSITION_SEGMENTS as usize {
            return Err(resource("native section point limit exceeded"));
        }
        let ring = native_ring(boundary, pose, sample, cancel)?;
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

fn native_ring(
    boundary: &spiling_geometry::SectionLoopMm,
    pose: &RigidPoseMm,
    sample: f64,
    cancel: &AtomicBool,
) -> Result<LineString<f64>, ManufacturingError> {
    if boundary.points_mm.len() > MAX_DEPOSITION_SEGMENTS as usize {
        return Err(resource("native loop point limit exceeded"));
    }
    let mut coordinates = Vec::with_capacity(boundary.points_mm.len());
    for &p in &boundary.points_mm {
        check_cancel(cancel)?;
        let world = forward(pose, p);
        if world.iter().any(|v| !v.is_finite())
            || (world[2] - sample).abs() > spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM
        {
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
    Ok(LineString(coordinates))
}

/// A geometry-certified WORLD-XY outer mask, never authoritative material.
pub(super) fn from_world_outer(
    boundary: &spiling_geometry::SectionLoopMm,
    cancel: &AtomicBool,
) -> Result<Polygon<f64>, ManufacturingError> {
    if boundary.is_hole {
        return Err(unsupported("native outer envelope was marked as a hole"));
    }
    let result = Polygon::new(
        native_ring(boundary, &RigidPoseMm::IDENTITY, 0.0, cancel)?,
        Vec::new(),
    );
    check_polygons(std::slice::from_ref(&result))?;
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
    check_polygons(&region.0)
}

fn check_polygons(polygons: &[Polygon<f64>]) -> Result<(), ManufacturingError> {
    let points: usize = polygons
        .iter()
        .map(|p| {
            p.exterior().0.len() + p.interiors().iter().map(|hole| hole.0.len()).sum::<usize>()
        })
        .sum();
    if points > MAX_DEPOSITION_SEGMENTS as usize {
        return Err(resource("planar region point limit exceeded"));
    }
    if polygons.iter().any(|p| {
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
    offset_admitted(
        region,
        distance,
        |_, overlay| {
            if let Some((a, b)) = overlay {
                boolean_budget(a, b, cancel)?;
            }
            Ok(())
        },
        cancel,
    )
}

/// Admit known live application-owned construction/input coordinates before
/// growth, and returned overlay output immediately after the call. The caller
/// owns combined retention and cumulative overlay work. Upstream transient
/// allocation is not bounded by this callback; host-process limits remain separate.
pub(super) fn offset_admitted(
    region: &MultiPolygon<f64>,
    distance: f64,
    mut admit: impl FnMut(
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    offset_polygons(&region.0, distance, &mut admit, cancel)
}

/// Offset one certified outer operand without normalizing overlapping masks.
pub(super) fn offset_polygon(
    polygon: &Polygon<f64>,
    distance: f64,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    offset_polygons(
        std::slice::from_ref(polygon),
        distance,
        &mut |_, overlay| {
            if let Some((a, b)) = overlay {
                boolean_budget(a, b, cancel)?;
            }
            Ok(())
        },
        cancel,
    )
}

fn offset_polygons(
    polygons: &[Polygon<f64>],
    distance: f64,
    admit: &mut impl FnMut(
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    check_polygons(polygons)?;
    if polygons.is_empty() || distance == 0.0 {
        admit(polygons.iter().map(polygon_points).sum(), None)?;
        return Ok(MultiPolygon(polygons.to_vec()));
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
    admit(sides, None)?;
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
        let mut eroded_polygons = MultiPolygon(Vec::new());
        for polygon in polygons {
            check_cancel(cancel)?;
            let retained = sides + point_count(&eroded_polygons);
            let tube = boundary_tube(polygon, &disk, retained, admit, cancel)?;
            let source_points = polygon_points(polygon);
            let scratch = retained + point_count(&tube) + source_points;
            admit(scratch, None)?;
            let source = MultiPolygon(vec![polygon.clone()]);
            admit(scratch, Some((&source, &tube)))?;
            let mut eroded = source.difference(&tube);
            check_cancel(cancel)?;
            check_size(&eroded)?;
            admit(scratch + point_count(&eroded), None)?;
            eroded_polygons.0.append(&mut eroded.0);
        }
        eroded_polygons
    } else {
        admit(
            sides + polygons.iter().map(polygon_points).sum::<usize>(),
            None,
        )?;
        let mut expanded = MultiPolygon(polygons.to_vec());
        for polygon in polygons {
            let retained = sides + point_count(&expanded);
            let tube = boundary_tube(polygon, &disk, retained, admit, cancel)?;
            let scratch = retained + point_count(&tube);
            admit(scratch, Some((&expanded, &tube)))?;
            let replacement = expanded.union(&tube);
            admit(scratch + point_count(&replacement), None)?;
            expanded = replacement;
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

fn polygon_points(polygon: &Polygon<f64>) -> usize {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(|ring| ring.0.len())
        .sum()
}

fn boundary_tube(
    polygon: &Polygon<f64>,
    disk: &[Coord<f64>],
    external: usize,
    admit: &mut impl FnMut(
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut primitives = MultiPolygon(Vec::new());
    let mut retained_points = 0usize;
    let mut retained_capacity = 0usize;
    let construction_points = disk
        .len()
        .checked_mul(2)
        .ok_or_else(|| resource("offset capsule construction point overflow"))?;
    // Pinned quick_hull partitions its input in place and grows only its output
    // Vec. At most every input point plus closure enters that output; account
    // its power-of-two capacity, not merely its final coordinate count.
    let hull_capacity = construction_points
        .checked_add(1)
        .and_then(usize::checked_next_power_of_two)
        .ok_or_else(|| resource("offset capsule hull capacity overflow"))?;
    admit(external + construction_points, None)?;
    let mut construction = Vec::new();
    construction
        .try_reserve_exact(construction_points)
        .map_err(|_| resource("offset capsule construction allocation refused"))?;
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
            // Reuse one admitted input buffer. Calling ConvexHull on MultiPoint
            // would retain it while allocating another complete input copy.
            admit(
                external + retained_capacity + construction.capacity() + hull_capacity,
                None,
            )?;
            construction.clear();
            construction.extend(segment.iter().flat_map(|endpoint| {
                disk.iter().map(move |delta| Coord {
                    x: endpoint.x + delta.x,
                    y: endpoint.y + delta.y,
                })
            }));
            let capsule = Polygon::new(quick_hull(&mut construction), Vec::new());
            retained_points = retained_points
                .checked_add(capsule.exterior().0.len())
                .filter(|count| *count <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| resource("offset primitive point limit exceeded"))?;
            retained_capacity = retained_capacity
                .checked_add(capsule.exterior().0.capacity())
                .ok_or_else(|| resource("offset primitive capacity overflow"))?;
            let count = primitives.0.len() + 1;
            if count
                .checked_mul(count)
                .is_none_or(|pairs| pairs > MAX_BOOLEAN_PAIRS)
            {
                return Err(resource("offset primitive pair admission limit exceeded"));
            }
            admit(external + construction.capacity() + retained_capacity, None)?;
            primitives.0.push(capsule);
        }
    }
    drop(construction);
    check_size(&primitives)?;
    admit(
        external + retained_capacity,
        Some((&primitives, &primitives)),
    )?;
    check_cancel(cancel)?;
    // One overlay coordinate frame for all boundary capsules. Incremental
    // unions repeatedly quantize prior results and can create sub-grid edges.
    let tube = unary_union(&primitives.0);
    check_cancel(cancel)?;
    check_size(&tube)?;
    admit(external + retained_capacity + point_count(&tube), None)?;
    Ok(tube)
}

pub(super) fn canonicalize(region: &mut MultiPolygon<f64>) {
    for p in &mut region.0 {
        canonicalize_polygon(p);
    }
    region
        .0
        .sort_by(|a, b| ring_compare(a.exterior(), b.exterior()));
}

pub(super) fn canonicalize_polygon(polygon: &mut Polygon<f64>) {
    polygon.exterior_mut(|ring| canonical_ring(ring, true));
    polygon.interiors_mut(|rings| {
        for ring in rings.iter_mut() {
            canonical_ring(ring, false);
        }
        rings.sort_by(ring_compare);
    });
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
        z: _,
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
            row_intersections(polygon, coordinate, axis, &mut hits, cancel)?;
            append_hatch_row(
                &hits,
                coordinate,
                kind,
                layer,
                row % 2 == 1,
                paths,
                segments,
            )?;
        }
    }
    Ok(())
}

/// Clip the positive perimeter-owned fill at actual classified hatch rows.
/// Classification islands and densities may have different centered lattices;
/// bridge ownership must follow emitted rows, not invent a second lattice.
pub(super) fn bridge_rows(
    fill: &MultiPolygon<f64>,
    required: &[DepositionPath],
    layer: &HatchLayer<'_>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<DepositionPath>, ManufacturingError> {
    let fixed = if layer.axis == HatchAxis::X { 1 } else { 0 };
    let along = 1 - fixed;
    let grid = |value: f64| (value / super::LAYER_QUANTIZATION_MM).round_ties_even();
    // Index along-coordinate starts first: disjoint islands must not each scan
    // every classified path. Row-local views below retain only bbox candidates.
    let mut intervals = Vec::new();
    for path in required {
        check_cancel(cancel)?;
        if path.kind != DepositionKind::Perimeter
            && path.points_mm.len() == 2
            && grid(path.points_mm[0][fixed]) == grid(path.points_mm[1][fixed])
        {
            if intervals.len() >= MAX_DEPOSITION_SEGMENTS as usize - *segments {
                return Err(resource("bridge row ownership scratch limit exceeded"));
            }
            intervals.push((
                grid(path.points_mm[0][along].min(path.points_mm[1][along])),
                grid(path.points_mm[0][along].max(path.points_mm[1][along])),
                grid(path.points_mm[0][fixed]),
                path.points_mm[0][fixed],
            ));
        }
    }
    let mut work = 0usize;
    charge_bridge_rows(
        &mut work,
        intervals.len() * (usize::BITS - intervals.len().leading_zeros()) as usize,
    )?;
    intervals.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    check_cancel(cancel)?;
    let mut rows = Vec::new();
    let mut hits = Vec::new();
    let mut local = Vec::new();
    for polygon in &fill.0 {
        check_cancel(cancel)?;
        let rect = polygon
            .bounding_rect()
            .ok_or_else(|| unsupported("fill has no finite bounds"))?;
        let (min, max, low, high) = if fixed == 0 {
            (rect.min().x, rect.max().x, rect.min().y, rect.max().y)
        } else {
            (rect.min().y, rect.max().y, rect.min().x, rect.max().x)
        };
        let begin = intervals.partition_point(|piece| piece.0 < grid(low));
        let end = intervals.partition_point(|piece| piece.0 <= grid(high));
        local.clear();
        charge_bridge_rows(&mut work, end - begin + 1)?;
        for &piece in &intervals[begin..end] {
            check_cancel(cancel)?;
            if piece.1 <= grid(high) && piece.3 >= min && piece.3 < max {
                if intervals.len() + local.len() >= MAX_DEPOSITION_SEGMENTS as usize - *segments {
                    return Err(resource("bridge row ownership scratch limit exceeded"));
                }
                local.push(piece);
            }
        }
        charge_bridge_rows(
            &mut work,
            local.len() * (usize::BITS - local.len().leading_zeros()) as usize,
        )?;
        local.sort_unstable_by(|a, b| a.2.total_cmp(&b.2).then(a.0.total_cmp(&b.0)));
        let mut first = 0;
        while first < local.len() {
            check_cancel(cancel)?;
            let key = local[first].2;
            let stop = first + local[first..].partition_point(|piece| piece.2 <= key);
            let owners = &local[first..stop];
            let coordinate = owners[0].3;
            hits.clear();
            let edges = polygon.exterior().0.len()
                + polygon
                    .interiors()
                    .iter()
                    .map(|ring| ring.0.len())
                    .sum::<usize>();
            charge_bridge_rows(&mut work, edges + owners.len())?;
            row_intersections(polygon, coordinate, layer.axis, &mut hits, cancel)?;
            for pair in hits.as_chunks::<2>().0 {
                check_cancel(cancel)?;
                let lo = grid(pair[0]);
                let hi = grid(pair[1]);
                let begin = owners.partition_point(|piece| piece.0 < lo);
                let end = owners.partition_point(|piece| piece.0 <= hi);
                charge_bridge_rows(&mut work, end - begin + 1)?;
                // Auxiliary bounds are relevant only when this exact positive
                // interval owns a complete required piece. Unrequested islands,
                // tangent points and tiny slivers are not deposition proposals.
                if !owners[begin..end].iter().any(|piece| piece.1 <= hi) {
                    continue;
                }
                if intervals.len() + local.len() + hits.len() + rows.len() * 2 + 2
                    > MAX_DEPOSITION_SEGMENTS as usize - *segments
                {
                    return Err(resource("bridge row ownership scratch limit exceeded"));
                }
                append_hatch_row(
                    pair,
                    coordinate,
                    DepositionKind::Bridge,
                    layer,
                    false,
                    &mut rows,
                    segments,
                )?;
            }
            first = stop;
        }
    }
    Ok(rows)
}

fn charge_bridge_rows(work: &mut usize, amount: usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(amount)
        .filter(|value| *value <= MAX_BOOLEAN_PAIRS)
        .ok_or_else(|| resource("bridge row ownership work limit exceeded"))?;
    Ok(())
}

fn row_intersections(
    polygon: &Polygon<f64>,
    coordinate: f64,
    axis: HatchAxis,
    hits: &mut Vec<f64>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let swap = axis == HatchAxis::Y;
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
    Ok(())
}

fn append_hatch_row(
    hits: &[f64],
    coordinate: f64,
    kind: DepositionKind,
    layer: &HatchLayer<'_>,
    reverse: bool,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
) -> Result<(), ManufacturingError> {
    for pair in hits.as_chunks::<2>().0 {
        if pair[1] - pair[0] <= 0.000002 {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                format!(
                    "rectilinear hatch creates unresolved thin or zero-length deposition; role {kind:?}, Z {:.9}, fixed {:.12}, interval [{:.12}, {:.12}], length {:.12} mm",
                    layer.z,
                    coordinate,
                    pair[0],
                    pair[1],
                    pair[1] - pair[0],
                ),
            ));
        }
        reserve_segments(segments, 1)?;
        let mut a = if layer.axis == HatchAxis::Y {
            [coordinate, pair[0], layer.z]
        } else {
            [pair[0], coordinate, layer.z]
        };
        let mut b = if layer.axis == HatchAxis::Y {
            [coordinate, pair[1], layer.z]
        } else {
            [pair[1], coordinate, layer.z]
        };
        if reverse {
            std::mem::swap(&mut a, &mut b);
        }
        push_path(paths, kind, vec![a, b])?;
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
