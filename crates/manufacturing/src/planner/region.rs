// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

#[cfg(test)]
use super::boolean_budget;
use super::{MAX_BOOLEAN_PAIRS, MAX_RETAINED_GEOMETRY_UNITS, OFFSET_TOLERANCE_MM, forward};
use crate::{check_cancel, error};
use geo::algorithm::convex_hull::quick_hull;
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{
    Area, BooleanOps, BoundingRect, Contains, Coord, LineString, MultiPolygon, Point, Polygon,
    unary_union,
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
        if points > MAX_RETAINED_GEOMETRY_UNITS {
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
    if boundary.points_mm.len() > MAX_RETAINED_GEOMETRY_UNITS {
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
    if points > MAX_RETAINED_GEOMETRY_UNITS {
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
#[cfg(test)]
pub(super) fn offset(
    region: &MultiPolygon<f64>,
    distance: f64,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    offset_admitted(
        region,
        distance,
        |_, _, overlay| {
            if let Some((a, b)) = overlay {
                boolean_budget(a, b, cancel)?;
            }
            Ok(())
        },
        cancel,
    )
}

/// Admit coordinate-equivalent units AND known application-owned bytes before
/// growth, including simultaneous old/new containers and returned overlay
/// output immediately after the call. Borrowed input belongs to the caller.
/// Upstream Boolean-kernel transient allocation remains separately bounded by
/// existing operation/work policy and observed process limits, not this receipt.
pub(super) fn offset_admitted(
    region: &MultiPolygon<f64>,
    distance: f64,
    mut admit: impl FnMut(
        usize,
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    offset_polygons(&region.0, distance, &mut admit, cancel)
}

/// The polygon form keeps the same borrowed operand and float frame while
/// exposing the common offset implementation's live-capacity admission.
pub(super) fn offset_polygon_admitted(
    polygon: &Polygon<f64>,
    distance: f64,
    mut admit: impl FnMut(
        usize,
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    offset_polygons(std::slice::from_ref(polygon), distance, &mut admit, cancel)
}

fn offset_polygons(
    polygons: &[Polygon<f64>],
    distance: f64,
    admit: &mut impl FnMut(
        usize,
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    check_polygons(polygons)?;
    if polygons.is_empty() || distance == 0.0 {
        let units = polygons.iter().map(polygon_points).sum();
        offset_admit(admit, units, cloned_polygons_bytes(polygons)?, None)?;
        let mut result = MultiPolygon(polygons.to_vec());
        offset_admit(
            admit,
            units,
            super::native::material_bytes(&mut result),
            None,
        )?;
        return Ok(result);
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
    offset_admit(admit, sides, coordinate_bytes(sides)?, None)?;
    let disk = (0..sides)
        .map(|index| {
            let angle = std::f64::consts::TAU * (index as f64 + 0.5) / sides as f64;
            Coord {
                x: circumradius * angle.cos(),
                y: circumradius * angle.sin(),
            }
        })
        .collect::<Vec<_>>();
    let disk_bytes = coordinate_bytes(disk.capacity())?;
    let mut result = if distance < 0.0 {
        // S eroded by a disk is S minus its boundary neighborhood. Preserve
        // exactly the existing per-island Boolean operands and float frames.
        let mut eroded_polygons = MultiPolygon(Vec::new());
        for polygon in polygons {
            check_cancel(cancel)?;
            let retained = sides + point_count(&eroded_polygons);
            let retained_bytes = offset_bytes(&[
                disk_bytes,
                super::native::material_bytes(&mut eroded_polygons),
            ])?;
            let mut tube = boundary_tube(polygon, &disk, retained, retained_bytes, admit, cancel)?;
            let tube_bytes = super::native::material_bytes(&mut tube);
            let source_points = polygon_points(polygon);
            let scratch = retained + point_count(&tube) + source_points;
            offset_admit(
                admit,
                scratch,
                offset_bytes(&[
                    retained_bytes,
                    tube_bytes,
                    cloned_polygons_bytes(std::slice::from_ref(polygon))?,
                ])?,
                None,
            )?;
            let mut source = MultiPolygon(vec![polygon.clone()]);
            let scratch_bytes = offset_bytes(&[
                retained_bytes,
                tube_bytes,
                super::native::material_bytes(&mut source),
            ])?;
            offset_admit(admit, scratch, scratch_bytes, Some((&source, &tube)))?;
            let mut eroded = source.difference(&tube);
            check_cancel(cancel)?;
            check_size(&eroded)?;
            let eroded_bytes = super::native::material_bytes(&mut eroded);
            offset_admit(
                admit,
                scratch + point_count(&eroded),
                offset_bytes(&[scratch_bytes, eroded_bytes])?,
                None,
            )?;
            drop(source);
            drop(tube);
            let units = retained + point_count(&eroded);
            reserve_offset_polygons(
                &mut eroded_polygons.0,
                eroded.0.len(),
                units,
                offset_bytes(&[retained_bytes, eroded_bytes])?,
                admit,
            )?;
            eroded_polygons.0.append(&mut eroded.0);
        }
        eroded_polygons
    } else {
        let points = polygons.iter().map(polygon_points).sum::<usize>();
        offset_admit(
            admit,
            sides + points,
            offset_bytes(&[disk_bytes, cloned_polygons_bytes(polygons)?])?,
            None,
        )?;
        let mut expanded = MultiPolygon(polygons.to_vec());
        for polygon in polygons {
            let retained = sides + point_count(&expanded);
            let retained_bytes =
                offset_bytes(&[disk_bytes, super::native::material_bytes(&mut expanded)])?;
            let mut tube = boundary_tube(polygon, &disk, retained, retained_bytes, admit, cancel)?;
            let scratch = retained + point_count(&tube);
            let scratch_bytes =
                offset_bytes(&[retained_bytes, super::native::material_bytes(&mut tube)])?;
            offset_admit(admit, scratch, scratch_bytes, Some((&expanded, &tube)))?;
            let mut replacement = expanded.union(&tube);
            offset_admit(
                admit,
                scratch + point_count(&replacement),
                offset_bytes(&[
                    scratch_bytes,
                    super::native::material_bytes(&mut replacement),
                ])?,
                None,
            )?;
            expanded = replacement;
            check_cancel(cancel)?;
            check_size(&expanded)?;
        }
        expanded
    };
    drop(disk);
    check_cancel(cancel)?;
    check_size(&result)?;
    // Stable slice sorting may own a temporary record array. Only one interior
    // or outer sort runs at once; reserve its complete record length.
    let sort_bytes = result
        .0
        .iter()
        .map(|p| {
            p.interiors()
                .len()
                .checked_mul(std::mem::size_of::<LineString<f64>>())
                .ok_or_else(|| resource("offset sort byte overflow"))
        })
        .try_fold(
            result
                .0
                .len()
                .checked_mul(std::mem::size_of::<Polygon<f64>>())
                .ok_or_else(|| resource("offset sort byte overflow"))?,
            |largest, bytes| Ok::<_, ManufacturingError>(largest.max(bytes?)),
        )?;
    offset_admit(
        admit,
        point_count(&result),
        offset_bytes(&[super::native::material_bytes(&mut result), sort_bytes])?,
        None,
    )?;
    canonicalize(&mut result);
    Ok(result)
}

fn polygon_points(polygon: &Polygon<f64>) -> usize {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(|ring| ring.0.len())
        .sum()
}

fn coordinate_bytes(capacity: usize) -> Result<usize, ManufacturingError> {
    capacity
        .checked_mul(std::mem::size_of::<Coord<f64>>())
        .ok_or_else(|| resource("offset coordinate byte overflow"))
}
fn offset_bytes(parts: &[usize]) -> Result<usize, ManufacturingError> {
    parts.iter().try_fold(0usize, |total, bytes| {
        total
            .checked_add(*bytes)
            .ok_or_else(|| resource("offset owned-byte overflow"))
    })
}
pub(super) fn cloned_polygons_bytes(
    polygons: &[Polygon<f64>],
) -> Result<usize, ManufacturingError> {
    polygons.iter().try_fold(
        polygons
            .len()
            .checked_mul(std::mem::size_of::<Polygon<f64>>())
            .ok_or_else(|| resource("offset cloned container byte overflow"))?,
        |bytes, polygon| {
            let rings = polygon
                .interiors()
                .len()
                .checked_mul(std::mem::size_of::<LineString<f64>>())
                .ok_or_else(|| resource("offset cloned interior byte overflow"))?;
            offset_bytes(&[bytes, rings, coordinate_bytes(polygon_points(polygon))?])
        },
    )
}
fn offset_admit(
    admit: &mut impl FnMut(
        usize,
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    units: usize,
    bytes: usize,
    overlay: Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
) -> Result<(), ManufacturingError> {
    admit(
        units.max(bytes.div_ceil(std::mem::size_of::<Coord<f64>>())),
        bytes,
        overlay,
    )
}
fn reserve_offset_polygons(
    polygons: &mut Vec<Polygon<f64>>,
    additional: usize,
    units: usize,
    live_bytes: usize,
    admit: &mut impl FnMut(
        usize,
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
) -> Result<(), ManufacturingError> {
    let needed = polygons
        .len()
        .checked_add(additional)
        .ok_or_else(|| resource("offset polygon count overflow"))?;
    if needed <= polygons.capacity() {
        return Ok(());
    }
    let capacity = polygons.capacity().saturating_mul(2).max(4).max(needed);
    let bytes = capacity
        .checked_mul(std::mem::size_of::<Polygon<f64>>())
        .ok_or_else(|| resource("offset polygon capacity byte overflow"))?;
    // The old container and every moved polygon payload remain owned while a
    // replacement container grows. Reserve geometrically, never one record/run.
    offset_admit(admit, units, offset_bytes(&[live_bytes, bytes])?, None)?;
    polygons
        .try_reserve_exact(capacity - polygons.len())
        .map_err(|_| resource("offset polygon allocation refused"))?;
    let actual = polygons
        .capacity()
        .checked_mul(std::mem::size_of::<Polygon<f64>>())
        .ok_or_else(|| resource("offset polygon capacity byte overflow"))?;
    offset_admit(admit, units, offset_bytes(&[live_bytes, actual])?, None)
}

fn boundary_tube(
    polygon: &Polygon<f64>,
    disk: &[Coord<f64>],
    external: usize,
    external_bytes: usize,
    admit: &mut impl FnMut(
        usize,
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut primitives = MultiPolygon(Vec::new());
    let mut retained_points = 0usize;
    let mut retained_capacity = 0usize;
    let mut payload_bytes = 0usize;
    let construction_points = disk
        .len()
        .checked_mul(2)
        .ok_or_else(|| resource("offset capsule construction point overflow"))?;
    // The pinned hull partitions input in place; its output fits every input
    // point plus closure, with a power-of-two capacity. No source ring is hulled.
    let hull_capacity = construction_points
        .checked_add(1)
        .and_then(usize::checked_next_power_of_two)
        .ok_or_else(|| resource("offset capsule hull capacity overflow"))?;
    offset_admit(
        admit,
        external + construction_points,
        offset_bytes(&[external_bytes, coordinate_bytes(construction_points)?])?,
        None,
    )?;
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
            // Exact monotone coalescing applies ONLY to construction capsules,
            // never to authoritative original material coordinates.
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
            let units = external + retained_capacity + construction.capacity() + hull_capacity;
            let live_bytes = offset_bytes(&[
                external_bytes,
                payload_bytes,
                primitives
                    .0
                    .capacity()
                    .checked_mul(std::mem::size_of::<Polygon<f64>>())
                    .ok_or_else(|| resource("offset primitive container byte overflow"))?,
                coordinate_bytes(construction.capacity())?,
                coordinate_bytes(hull_capacity)?,
            ])?;
            reserve_offset_polygons(&mut primitives.0, 1, units, live_bytes, admit)?;
            offset_admit(
                admit,
                units,
                offset_bytes(&[
                    external_bytes,
                    payload_bytes,
                    primitives
                        .0
                        .capacity()
                        .checked_mul(std::mem::size_of::<Polygon<f64>>())
                        .ok_or_else(|| resource("offset primitive container byte overflow"))?,
                    coordinate_bytes(construction.capacity())?,
                    coordinate_bytes(hull_capacity)?,
                ])?,
                None,
            )?;
            construction.clear();
            construction.extend(segment.iter().flat_map(|endpoint| {
                disk.iter().map(move |delta| Coord {
                    x: endpoint.x + delta.x,
                    y: endpoint.y + delta.y,
                })
            }));
            let mut capsule = Polygon::new(quick_hull(&mut construction), Vec::new());
            retained_points = retained_points
                .checked_add(capsule.exterior().0.len())
                .filter(|count| *count <= MAX_RETAINED_GEOMETRY_UNITS)
                .ok_or_else(|| resource("offset primitive point limit exceeded"))?;
            retained_capacity = retained_capacity
                .checked_add(capsule.exterior().0.capacity())
                .ok_or_else(|| resource("offset primitive capacity overflow"))?;
            payload_bytes = payload_bytes
                .checked_add(super::native::polygon_bytes(&mut capsule))
                .ok_or_else(|| resource("offset primitive payload byte overflow"))?;
            let count = primitives.0.len() + 1;
            if count
                .checked_mul(count)
                .is_none_or(|pairs| pairs > MAX_BOOLEAN_PAIRS)
            {
                return Err(resource("offset primitive pair admission limit exceeded"));
            }
            offset_admit(
                admit,
                external + construction.capacity() + retained_capacity,
                offset_bytes(&[
                    external_bytes,
                    coordinate_bytes(construction.capacity())?,
                    payload_bytes,
                    primitives
                        .0
                        .capacity()
                        .checked_mul(std::mem::size_of::<Polygon<f64>>())
                        .ok_or_else(|| resource("offset primitive container byte overflow"))?,
                ])?,
                None,
            )?;
            primitives.0.push(capsule);
        }
    }
    drop(construction);
    check_size(&primitives)?;
    let primitive_bytes = super::native::material_bytes(&mut primitives);
    offset_admit(
        admit,
        external + retained_capacity,
        offset_bytes(&[external_bytes, primitive_bytes])?,
        Some((&primitives, &primitives)),
    )?;
    check_cancel(cancel)?;
    // Keep the same ONE unary union and float frame; no incremental overlays.
    let mut tube = unary_union(&primitives.0);
    check_cancel(cancel)?;
    check_size(&tube)?;
    offset_admit(
        admit,
        external + retained_capacity + point_count(&tube),
        offset_bytes(&[
            external_bytes,
            primitive_bytes,
            super::native::material_bytes(&mut tube),
        ])?,
        None,
    )?;
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

pub(super) fn support_contour_paths(
    centers: &MultiPolygon<f64>,
    z: f64,
    lower_z: f64,
    bead_width_mm: f64,
    kind: DepositionKind,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if !matches!(
        kind,
        DepositionKind::Support | DepositionKind::SupportInterface
    ) {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "support contour requires explicit support construction",
        ));
    }
    for p in &centers.0 {
        for ring in std::iter::once(p.exterior()).chain(p.interiors()) {
            check_cancel(cancel)?;
            reserve_segments(segments, ring.0.len() - 1)?;
            let points_mm = ring.0.iter().map(|c| [c.x, c.y, z]).collect();
            push_path(paths, kind, points_mm, bead_width_mm, [lower_z, z])?;
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
    pub(super) fn for_policy(policy: RectilinearAxisPolicy, index: u32) -> Self {
        match policy {
            RectilinearAxisPolicy::X => Self::X,
            RectilinearAxisPolicy::Y => Self::Y,
            RectilinearAxisPolicy::AlternatingXY if index.is_multiple_of(2) => Self::X,
            RectilinearAxisPolicy::AlternatingYX if !index.is_multiple_of(2) => Self::X,
            RectilinearAxisPolicy::AlternatingXY | RectilinearAxisPolicy::AlternatingYX => Self::Y,
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
    pub lower_z: f64,
    pub height: f64,
    pub axis: HatchAxis,
    pub top_axis: HatchAxis,
    pub recipe: &'a PlanarPrintRecipe,
}

/// Quarter-pitch residues relative to each ordinary polygon's centered lattice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OrdinaryHatchPhase {
    Centered,
    Quarter,
    Half,
    ThreeQuarters,
}

impl OrdinaryHatchPhase {
    pub(super) const CANDIDATES: [Self; 4] = [
        Self::Centered,
        Self::Quarter,
        Self::Half,
        Self::ThreeQuarters,
    ];

    pub(super) fn ordinal(self) -> u8 {
        match self {
            Self::Centered => 0,
            Self::Quarter => 1,
            Self::Half => 2,
            Self::ThreeQuarters => 3,
        }
    }
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
    hatch_with_phase(
        fill,
        kind,
        fraction,
        layer,
        None,
        OrdinaryHatchPhase::Centered,
        paths,
        segments,
        cancel,
    )
}

pub(super) fn hatch_with_phase(
    fill: &MultiPolygon<f64>,
    kind: DepositionKind,
    fraction: f64,
    layer: &HatchLayer<'_>,
    common_phase: Option<[f64; 2]>,
    phase: OrdinaryHatchPhase,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for polygon in &fill.0 {
        let rows = if let Some(origin) = common_phase {
            HatchRows::common(polygon, fraction, layer, origin)?
        } else {
            HatchRows::centered_phase(polygon, fraction, layer, phase)?.ok_or_else(|| {
                unsupported("selected ordinary hatch phase no longer preserves polygon row count")
            })?
        };
        hatch_polygon(polygon, kind, &rows, layer, paths, segments, cancel)?;
    }
    Ok(())
}

/// Prepared lattice shared by retention admission and actual component hatch.
pub(super) struct HatchRows {
    count: usize,
    start: f64,
    spacing: f64,
    edges: usize,
}

impl HatchRows {
    pub(super) fn new(
        polygon: &Polygon<f64>,
        fraction: f64,
        layer: &HatchLayer<'_>,
    ) -> Result<Self, ManufacturingError> {
        Self::prepare_centered(polygon, fraction, layer).map(|(rows, _, _)| rows)
    }

    fn prepare_centered(
        polygon: &Polygon<f64>,
        fraction: f64,
        layer: &HatchLayer<'_>,
    ) -> Result<(Self, [f64; 2], f64), ManufacturingError> {
        let width = layer.recipe.bead_width_mm;
        let h = layer.height;
        let solid_spacing = ((width - h) * h + std::f64::consts::PI * (h * 0.5).powi(2)) / h;
        let rect = polygon
            .bounding_rect()
            .ok_or_else(|| unsupported("fill has no finite bounds"))?;
        let (min, max) = if layer.axis == HatchAxis::Y {
            (rect.min().x, rect.max().x)
        } else {
            (rect.min().y, rect.max().y)
        };
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return Err(unsupported("hatch fraction must be finite and in [0,1]"));
        }
        // Zero means intentional void, not a forced centered sparse row.
        // Keep the region for coverage/provenance while all lattice consumers
        // (including retention and alternate/common phases) see zero rows.
        let count = if fraction == 0.0 {
            0.0
        } else {
            (((max - min) / solid_spacing) * fraction).ceil().max(1.0)
        };
        if !count.is_finite() || count > MAX_DEPOSITION_SEGMENTS as f64 {
            return Err(resource("rectilinear hatch row limit exceeded"));
        }
        let spacing = if count <= 1.0 {
            0.0
        } else {
            solid_spacing / fraction
        };
        let edges = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .try_fold(0usize, |total, ring| {
                total.checked_add(ring.0.len().saturating_sub(1))
            })
            .ok_or_else(|| resource("component hatch boundary count overflow"))?;
        Ok((
            Self {
                count: count as usize,
                start: min + ((max - min) - (count - 1.0) * spacing) * 0.5,
                spacing,
                edges,
            },
            [min, max],
            solid_spacing,
        ))
    }

    /// Alternative residues retain the original polygon's exact lattice count.
    /// A single centered row still uses the nominal pitch for quarter shifts.
    pub(super) fn centered_phase(
        polygon: &Polygon<f64>,
        fraction: f64,
        layer: &HatchLayer<'_>,
        phase: OrdinaryHatchPhase,
    ) -> Result<Option<Self>, ManufacturingError> {
        let (mut rows, bounds, solid_spacing) = Self::prepare_centered(polygon, fraction, layer)?;
        if rows.count == 0 || phase == OrdinaryHatchPhase::Centered {
            return Ok(Some(rows));
        }
        let spacing = if rows.count == 1 {
            solid_spacing / fraction
        } else {
            rows.spacing
        };
        let origin = rows.start + f64::from(phase.ordinal()) * 0.25 * spacing;
        let (first, count) = Self::lattice_extent(bounds, spacing, origin);
        if count != rows.count as f64 {
            return Ok(None);
        }
        rows.start = origin + first * spacing;
        rows.spacing = spacing;
        Ok(Some(rows))
    }

    /// Adjoining zones share the transported physical definition origin,
    /// never their cropped bounds or a distant build-global phase.
    pub(super) fn common(
        polygon: &Polygon<f64>,
        fraction: f64,
        layer: &HatchLayer<'_>,
        origin: [f64; 2],
    ) -> Result<Self, ManufacturingError> {
        let (mut rows, bounds, solid_spacing) = Self::prepare_centered(polygon, fraction, layer)?;
        if rows.count == 0 {
            return Ok(rows);
        }
        let spacing = if rows.count == 1 {
            solid_spacing / fraction
        } else {
            rows.spacing
        };
        let origin = origin[usize::from(layer.axis == HatchAxis::X)];
        let (first, count) = Self::lattice_extent(bounds, spacing, origin);
        if !count.is_finite() || count > MAX_DEPOSITION_SEGMENTS as f64 {
            return Err(resource("common phase hatch row limit exceeded"));
        }
        rows.start = origin + first * spacing;
        rows.count = count as usize;
        rows.spacing = spacing;
        Ok(rows)
    }

    fn lattice_extent(bounds: [f64; 2], spacing: f64, origin: f64) -> (f64, f64) {
        let first = ((bounds[0] - origin) / spacing).ceil();
        let stop = ((bounds[1] - origin) / spacing).ceil();
        (first, (stop - first).max(0.))
    }

    pub(super) fn point_bound(&self) -> Result<usize, ManufacturingError> {
        if self.count == 0 {
            return Ok(0);
        }
        // Row intersections stay live beside all emitted endpoints.
        self.count
            .checked_add(1)
            .and_then(|count| self.edges.checked_mul(count))
            .filter(|points| *points <= MAX_RETAINED_GEOMETRY_UNITS)
            .ok_or_else(|| resource("component hatch point bound exceeds retained budget"))
    }
}

/// The borrowed component is the same classified owner for both axis proposals.
pub(super) fn hatch_polygon(
    polygon: &Polygon<f64>,
    kind: DepositionKind,
    rows: &HatchRows,
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if rows.count == 0 {
        return Ok(());
    }
    if rows.count > MAX_DEPOSITION_SEGMENTS as usize - *segments {
        return Err(resource("rectilinear hatch row limit exceeded"));
    }
    let mut hits = Vec::new();
    hits.try_reserve_exact(rows.edges)
        .map_err(|_| resource("component hatch intersection allocation refused"))?;
    for row in 0..rows.count {
        check_cancel(cancel)?;
        hits.clear();
        let coordinate = rows.start + row as f64 * rows.spacing;
        row_intersections(polygon, coordinate, layer.axis, &mut hits, cancel)?;
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
    Ok(())
}

/// Clip the positive perimeter-owned fill at actual classified hatch rows.
/// Classification islands and densities may have different centered lattices;
/// bridge ownership must follow emitted rows, not invent a second lattice.
pub(super) fn bridge_rows<'a>(
    fill: &MultiPolygon<f64>,
    exclusions: &[MultiPolygon<f64>],
    required: impl IntoIterator<Item = &'a DepositionPath>,
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
        if !matches!(
            path.kind,
            DepositionKind::Perimeter | DepositionKind::SeparatorWall
        ) && path.points_mm.len() == 2
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
                path.owner,
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
    let exclusion_edges = exclusions.iter().map(point_count).sum::<usize>();
    if exclusion_edges > MAX_DEPOSITION_SEGMENTS as usize - *segments - intervals.len() {
        return Err(resource("bridge exclusion row scratch limit exceeded"));
    }
    let mut excluded = Vec::new();
    excluded
        .try_reserve_exact(exclusion_edges)
        .map_err(|_| resource("bridge exclusion row allocation refused"))?;
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
            excluded.clear();
            charge_bridge_rows(&mut work, exclusion_edges)?;
            for polygon in exclusions.iter().flat_map(|region| &region.0) {
                row_intersections(polygon, coordinate, layer.axis, &mut excluded, cancel)?;
            }
            excluded.sort_unstable_by(f64::total_cmp);
            for full in hits.as_chunks::<2>().0 {
                let mut cursor = full[0];
                let end = full[1];
                let mut cuts = excluded.as_chunks::<2>().0.iter();
                for pair in std::iter::from_fn(|| {
                    while cursor < end {
                        if let Some(cut) = cuts.next() {
                            if cut[1] <= cursor {
                                continue;
                            }
                            let start = cursor;
                            cursor = cursor.max(cut[1]);
                            if start < end.min(cut[0]) {
                                return Some([start, end.min(cut[0])]);
                            }
                        } else {
                            let start = cursor;
                            cursor = end;
                            return Some([start, end]);
                        }
                    }
                    None
                }) {
                    check_cancel(cancel)?;
                    let lo = grid(pair[0]);
                    let hi = grid(pair[1]);
                    let begin = owners.partition_point(|piece| piece.0 < lo);
                    let end = owners.partition_point(|piece| piece.0 <= hi);
                    charge_bridge_rows(&mut work, end - begin + 1)?;
                    // Auxiliary bounds are relevant only when this exact positive
                    // interval owns a complete required piece. Unrequested islands,
                    // tangent points and tiny slivers are not deposition proposals.
                    let mut complete = owners[begin..end].iter().filter(|piece| piece.1 <= hi);
                    let Some(first_owner) = complete.next() else {
                        continue;
                    };
                    let core_owner = first_owner
                        .4
                        .filter(|owner| complete.all(|piece| piece.4 == Some(*owner)));
                    if intervals.len()
                        + local.len()
                        + hits.len()
                        + excluded.capacity()
                        + rows.len() * 2
                        + 2
                        > MAX_DEPOSITION_SEGMENTS as usize - *segments
                    {
                        return Err(resource("bridge row ownership scratch limit exceeded"));
                    }
                    let first_path = rows.len();
                    append_hatch_row(
                        &pair,
                        coordinate,
                        DepositionKind::Bridge,
                        layer,
                        false,
                        &mut rows,
                        segments,
                    )?;
                    for path in &mut rows[first_path..] {
                        path.owner = core_owner;
                    }
                }
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

pub(super) fn row_intersections(
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
    hits.sort_unstable_by(f64::total_cmp);
    if !hits.len().is_multiple_of(2) {
        return Err(unsupported(
            "rectilinear hatch clipping has odd boundary parity",
        ));
    }
    Ok(())
}

pub(super) fn append_hatch_row(
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
        push_path(
            paths,
            kind,
            vec![a, b],
            layer.recipe.bead_width_mm,
            [layer.lower_z, layer.z],
        )?;
    }
    Ok(())
}
fn push_path(
    paths: &mut Vec<DepositionPath>,
    kind: DepositionKind,
    points_mm: Vec<[f64; 3]>,
    bead_width_mm: f64,
    physical_z_mm: [f64; 2],
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
    paths.push(DepositionPath {
        material_domain: None,
        kind,
        width: WidthProfile::Constant {
            width_mm: bead_width_mm,
        },
        physical_z_mm,
        owner: match kind {
            DepositionKind::BottomSkin => Some(DepositionOwner::MandatorySkin {
                skin: ProcessSkinKind::BottomSkin,
            }),
            DepositionKind::TopSkin => Some(DepositionOwner::MandatorySkin {
                skin: ProcessSkinKind::TopSkin,
            }),
            _ => None,
        },
        points_mm,
    });
    Ok(())
}

/// Scalar-only producers (hatch rows/interfaces) must establish that ownership;
/// never flatten an adaptive wall to a recipe width or an average.
pub(crate) fn constant_width(path: &DepositionPath) -> Result<f64, ManufacturingError> {
    path.checked_edges()?;
    path.width.constant_width_mm().ok_or_else(|| {
        error(
            ManufacturingErrorCode::InvalidSpecification,
            "constant-width row operation received an adaptive width profile",
        )
    })
}

pub(crate) fn nominal_volume(
    path: &DepositionPath,
    index: u32,
    recipe: &PlanarPrintRecipe,
) -> Result<f64, ManufacturingError> {
    let height = path.height_mm();
    let mut factor = recipe.flow_multiplier;
    if index == 0 {
        factor *= recipe.process.first_layer_flow_factor;
    }
    if path.kind == DepositionKind::Bridge {
        factor *= recipe.process.bridge_flow_factor;
    }
    let mut volume = 0.0;
    for edge in path.checked_edges()? {
        let a = edge.points_mm[0];
        let b = edge.points_mm[1];
        let area =
            (edge.width_mm - height) * height + std::f64::consts::PI * (height * 0.5).powi(2);
        volume += (b[0] - a[0]).hypot(b[1] - a[1]) * area * factor;
    }
    Ok(volume)
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

#[cfg(test)]
#[test]
fn ordinary_quarter_hatch_preserves_each_polygon_cadence_and_authored_origins() {
    let recipe = super::tests::recipe(0.5);
    let cancel = AtomicBool::new(false);
    let layer = HatchLayer {
        z: recipe.layer_height_mm,
        lower_z: 0.0,
        height: recipe.layer_height_mm,
        axis: HatchAxis::X,
        top_axis: HatchAxis::Y,
        recipe: &recipe,
    };
    let h = layer.height;
    let pitch = ((recipe.bead_width_mm - h) * h + std::f64::consts::PI * (h * 0.5).powi(2))
        / h
        / recipe.infill_fraction;
    let ring = |x, y, width, height| {
        LineString::from(vec![
            (x, y),
            (x + width, y),
            (x + width, y + height),
            (x, y + height),
            (x, y),
        ])
    };
    let second_y = 19.125 * pitch;
    let fill = MultiPolygon(vec![
        Polygon::new(
            ring(0.0, 0.0, 10.0, 12.75 * pitch),
            vec![ring(3.0, 4.0 * pitch, 4.0, 4.0 * pitch)],
        ),
        Polygon::new(ring(20.0, second_y, 10.0, 8.75 * pitch), vec![]),
    ]);
    let emit = |kind, fraction, origin, phase| {
        let mut paths = Vec::new();
        let mut segments = 0;
        hatch_with_phase(
            &fill,
            kind,
            fraction,
            &layer,
            origin,
            phase,
            &mut paths,
            &mut segments,
            &cancel,
        )
        .unwrap();
        assert_eq!(segments, paths.len());
        paths
    };
    let centered = emit(
        DepositionKind::SparseFill,
        recipe.infill_fraction,
        None,
        OrdinaryHatchPhase::Centered,
    );
    let quarter = emit(
        DepositionKind::SparseFill,
        recipe.infill_fraction,
        None,
        OrdinaryHatchPhase::Quarter,
    );
    let rows = |paths: &[DepositionPath], second: bool| {
        let mut coordinates: Vec<_> = paths
            .iter()
            .filter(|path| (path.points_mm[0][0] > 15.0) == second)
            .map(|path| path.points_mm[0][1])
            .collect();
        coordinates.dedup();
        coordinates
    };
    for (second, expected_count) in [(false, 13), (true, 9)] {
        let original_rows = rows(&centered, second);
        let shifted_rows = rows(&quarter, second);
        assert_eq!(original_rows.len(), expected_count);
        assert_eq!(shifted_rows.len(), expected_count);
        for (index, (&original, &shifted)) in original_rows.iter().zip(&shifted_rows).enumerate() {
            assert!((shifted - original - 0.25 * pitch).abs() < 1e-10);
            if index > 0 {
                assert!((shifted - shifted_rows[index - 1] - pitch).abs() < 1e-10);
            }
            let mut nonempty = 0;
            for path in quarter.iter().filter(|path| {
                (path.points_mm[0][0] > 15.0) == second && path.points_mm[0][1] == shifted
            }) {
                assert_eq!(path.points_mm.len(), 2);
                let [a, b] = path.points_mm.as_slice() else {
                    unreachable!()
                };
                assert_eq!(a[1], b[1]);
                assert_eq!(a[2], layer.z);
                assert_eq!(b[2], layer.z);
                assert!((b[0] - a[0]).abs() > 0.000002);
                nonempty += 1;
            }
            assert!(nonempty > 0);
        }
    }
    let hole_paths: Vec<_> = quarter
        .iter()
        .filter(|path| {
            path.points_mm[0][0] < 15.0
                && path.points_mm[0][1] > 4.0 * pitch
                && path.points_mm[0][1] < 8.0 * pitch
        })
        .collect();
    assert_eq!(hole_paths.len(), 8);
    for path in hole_paths {
        let a = path.points_mm[0][0];
        let b = path.points_mm[1][0];
        assert!(a.max(b) <= 3.0 || a.min(b) >= 7.0);
    }

    let origin = Some([0.0, 0.123 * pitch]);
    assert_eq!(
        emit(
            DepositionKind::SparseFill,
            recipe.infill_fraction,
            origin,
            OrdinaryHatchPhase::Centered,
        ),
        emit(
            DepositionKind::SparseFill,
            recipe.infill_fraction,
            origin,
            OrdinaryHatchPhase::Quarter,
        ),
    );

    let mut rejected = Vec::new();
    let mut rejected_segments = 0;
    let failure = hatch_with_phase(
        &fill,
        DepositionKind::SparseFill,
        recipe.infill_fraction,
        &layer,
        None,
        OrdinaryHatchPhase::Half,
        &mut rejected,
        &mut rejected_segments,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(failure.code, ManufacturingErrorCode::UnsupportedGeometry);
    let half_origin = rows(&centered, false)[0] + 0.5 * pitch;
    let unrestricted_half = emit(
        DepositionKind::SparseFill,
        recipe.infill_fraction,
        Some([0.0, half_origin]),
        OrdinaryHatchPhase::Half,
    );
    assert_eq!(rows(&unrestricted_half, false).len(), 12);
}

#[cfg(test)]
#[test]
fn zero_fraction_has_no_rows_for_centered_alternate_or_transported_phases() {
    let recipe = super::tests::recipe(0.0);
    let cancel = AtomicBool::new(false);
    let polygon = Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]),
        vec![],
    );
    for axis in [HatchAxis::X, HatchAxis::Y] {
        let layer = HatchLayer {
            z: 0.2,
            lower_z: 0.0,
            height: 0.2,
            axis,
            top_axis: axis,
            recipe: &recipe,
        };
        for phase in OrdinaryHatchPhase::CANDIDATES {
            let rows = HatchRows::centered_phase(&polygon, 0.0, &layer, phase)
                .unwrap()
                .unwrap();
            assert_eq!(rows.point_bound().unwrap(), 0);
            assert!(rows.start.is_finite() && rows.spacing.is_finite());
            let mut paths = Vec::new();
            let mut segments = 0;
            hatch_polygon(
                &polygon,
                DepositionKind::SparseFill,
                &rows,
                &layer,
                &mut paths,
                &mut segments,
                &cancel,
            )
            .unwrap();
            assert!(paths.is_empty());
            assert_eq!(segments, 0);
        }
        let rows = HatchRows::common(&polygon, 0.0, &layer, [5.125, -10.75]).unwrap();
        assert_eq!(rows.point_bound().unwrap(), 0);
        let mut paths = Vec::new();
        let mut segments = 0;
        hatch_polygon(
            &polygon,
            DepositionKind::SparseFill,
            &rows,
            &layer,
            &mut paths,
            &mut segments,
            &cancel,
        )
        .unwrap();
        assert!(paths.is_empty());
        assert_eq!(segments, 0);
        assert!(
            hatch_polygon(
                &polygon,
                DepositionKind::SparseFill,
                &rows,
                &layer,
                &mut paths,
                &mut segments,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
