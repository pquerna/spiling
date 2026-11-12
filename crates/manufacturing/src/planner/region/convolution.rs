// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Polygon offsets by the convolution cycle of each boundary ring with the
//! circumscribed convex disk polygon.
//!
//! For a ring traversed with its material on the left and a convex disk `D`,
//! the convolution cycle translates every edge by the disk vertex extreme in
//! the edge's outward (right) normal and, at every ring vertex, walks the disk
//! vertices between the two edge supports: counterclockwise at a left turn and
//! clockwise (backwards) at a right turn; see [`Join::Through`] for the
//! equivalent reflex join used here. The material dilated by `D` is
//! exactly the set of points with positive winding number about the cycles of
//! all its rings (Guibas-Ramshaw-Stolfi convolution; Wein, ESA 2006). This is
//! the same set as the material united with the boundary tube of per-edge
//! `segment ⊕ D` capsules, built from the same disk vertices, so the offset
//! contract (circumscribed disk, radial error within half the offset budget)
//! is unchanged while one overlay of `O(edges + turned disk vertices)`
//! segments replaces a union of heavily overlapping capsules.
//!
//! Erosion dilates the complement: the reversed rings carry the complement on
//! their left and an implicit `+1` at infinity, so the eroded material is the
//! negative-winding region of their cycles.

use super::*;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::overlay::FloatOverlay;
use i_overlay::i_shape::source::resource::ShapeResource;
use std::mem::size_of;

/// Offset `polygons` by the (symmetric, convex, counterclockwise) `disk`.
/// `erode` selects `polygons ⊖ disk`; otherwise `polygons ⊕ disk`. Retained
/// `external` units/bytes belong to the caller and are added to every
/// admission, as are this function's live scratch, cycle, packed-path and
/// output buffers. The overlay operand is offered once before the overlay.
pub(super) fn offset(
    polygons: &[Polygon<f64>],
    disk: &[Coord<f64>],
    erode: bool,
    external: usize,
    external_bytes: usize,
    admit: &mut impl FnMut(
        usize,
        usize,
        Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let sides = disk.len();
    let largest_ring = polygons
        .iter()
        .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
        .map(|ring| ring.0.len())
        .max()
        .unwrap_or(0);
    let ring_count = polygons
        .iter()
        .map(|polygon| 1 + polygon.interiors().len())
        .sum::<usize>();
    // Per-ring scratch: distinct vertices and one disk support per edge.
    let scratch_bytes = offset_bytes(&[
        coordinate_bytes(largest_ring)?,
        largest_ring
            .checked_mul(size_of::<usize>())
            .ok_or_else(|| resource("offset convolution scratch byte overflow"))?,
        ring_count
            .checked_mul(size_of::<Polygon<f64>>())
            .ok_or_else(|| resource("offset convolution cycle container byte overflow"))?,
    ])?;
    offset_admit(
        admit,
        external + 2 * largest_ring,
        offset_bytes(&[external_bytes, scratch_bytes])?,
        None,
    )?;
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(largest_ring)
        .map_err(|_| resource("offset convolution vertex allocation refused"))?;
    let mut supports = Vec::new();
    supports
        .try_reserve_exact(largest_ring)
        .map_err(|_| resource("offset convolution support allocation refused"))?;
    let mut cycles = MultiPolygon(Vec::new());
    cycles
        .0
        .try_reserve_exact(ring_count)
        .map_err(|_| resource("offset convolution cycle allocation refused"))?;
    let mut cycle_points = 0usize;
    let mut cycle_bytes = 0usize;
    for polygon in polygons {
        for (index, ring) in std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .enumerate()
        {
            check_cancel(cancel)?;
            // Material on the left: exterior counterclockwise, holes
            // clockwise. Erosion reverses every ring to put the complement
            // on the left.
            let counterclockwise = index == 0;
            let reverse = (signed_area(&ring.0) < 0.0) == counterclockwise;
            distinct_vertices(&ring.0, reverse != erode, &mut vertices, cancel)?;
            if vertices.is_empty() {
                continue;
            }
            let points = plan_cycle(&vertices, sides, &mut supports, cancel)?;
            let capacity = points
                .checked_add(1)
                .ok_or_else(|| resource("offset convolution cycle point overflow"))?;
            cycle_points = cycle_points
                .checked_add(capacity)
                .filter(|count| *count <= MAX_RETAINED_GEOMETRY_UNITS)
                .ok_or_else(|| resource("offset convolution point limit exceeded"))?;
            cycle_bytes = offset_bytes(&[cycle_bytes, coordinate_bytes(capacity)?])?;
            offset_admit(
                admit,
                external + 2 * largest_ring + cycle_points,
                offset_bytes(&[external_bytes, scratch_bytes, cycle_bytes])?,
                None,
            )?;
            let mut cycle = Vec::new();
            cycle
                .try_reserve_exact(capacity)
                .map_err(|_| resource("offset convolution cycle allocation refused"))?;
            emit_cycle(&vertices, &supports, disk, &mut cycle, cancel)?;
            cycles.0.push(Polygon::new(LineString(cycle), Vec::new()));
        }
    }
    drop(vertices);
    drop(supports);
    if cycles.0.is_empty() {
        return Ok(MultiPolygon(Vec::new()));
    }
    let cycles_bytes = super::super::native::material_bytes(&mut cycles);
    // The overlay consumes a packed copy without ring closures.
    let packed_points = cycle_points - cycles.0.len();
    let packed_bytes = offset_bytes(&[
        packed_points
            .checked_mul(size_of::<[f64; 2]>())
            .ok_or_else(|| resource("offset convolution packed byte overflow"))?,
        cycles
            .0
            .len()
            .checked_mul(size_of::<usize>())
            .ok_or_else(|| resource("offset convolution packed byte overflow"))?,
    ])?;
    let staged = external + cycle_points + packed_points;
    let staged_bytes = offset_bytes(&[external_bytes, cycles_bytes, packed_bytes])?;
    offset_admit(admit, staged, staged_bytes, Some((&cycles, &cycles)))?;
    let mut packed = Paths::default();
    packed
        .coordinates
        .try_reserve_exact(packed_points)
        .map_err(|_| resource("offset convolution packed allocation refused"))?;
    packed
        .ends
        .try_reserve_exact(cycles.0.len())
        .map_err(|_| resource("offset convolution packed allocation refused"))?;
    for cycle in &cycles.0 {
        check_cancel(cancel)?;
        let ring = &cycle.exterior().0;
        packed
            .coordinates
            .extend(ring[..ring.len() - 1].iter().map(|c| [c.x, c.y]));
        packed.ends.push(packed.coordinates.len());
    }
    drop(cycles);
    check_cancel(cancel)?;
    let fill = if erode {
        FillRule::Negative
    } else {
        FillRule::Positive
    };
    let shapes = FloatOverlay::with_subj(&packed).overlay(OverlayRule::Subject, fill);
    drop(packed);
    check_cancel(cancel)?;
    let output_points = shapes
        .iter()
        .flatten()
        .map(|ring| ring.len() + 1)
        .sum::<usize>();
    let output_bytes = shapes.iter().try_fold(
        shapes
            .len()
            .checked_mul(size_of::<Vec<Vec<[f64; 2]>>>() + size_of::<Polygon<f64>>())
            .ok_or_else(|| resource("offset convolution output byte overflow"))?,
        |bytes, shape| {
            offset_bytes(&[
                bytes,
                shape
                    .len()
                    .checked_mul(size_of::<Vec<[f64; 2]>>() + size_of::<LineString<f64>>())
                    .ok_or_else(|| resource("offset convolution output byte overflow"))?,
                shape
                    .iter()
                    .map(|ring| 2 * ring.len() + 1)
                    .sum::<usize>()
                    .checked_mul(size_of::<Coord<f64>>())
                    .ok_or_else(|| resource("offset convolution output byte overflow"))?,
            ])
        },
    )?;
    offset_admit(
        admit,
        external + 2 * output_points,
        offset_bytes(&[external_bytes, output_bytes])?,
        None,
    )?;
    let mut result = MultiPolygon(
        shapes
            .into_iter()
            .map(|shape| {
                let mut rings = shape.into_iter().map(|ring| {
                    let mut line = LineString(ring.into_iter().map(Coord::from).collect());
                    line.close();
                    line
                });
                Polygon::new(rings.next().unwrap_or(LineString::empty()), rings.collect())
            })
            .collect(),
    );
    let result_bytes = super::super::native::material_bytes(&mut result);
    offset_admit(
        admit,
        external + point_count(&result),
        offset_bytes(&[external_bytes, result_bytes])?,
        None,
    )?;
    Ok(result)
}

fn signed_area(ring: &[Coord<f64>]) -> f64 {
    ring.windows(2)
        .map(|pair| pair[0].x * pair[1].y - pair[1].x * pair[0].y)
        .sum::<f64>()
}

/// Ring vertices in traversal order without the closure or consecutive
/// repeats. Exact equality only: distinct authoritative vertices stay.
fn distinct_vertices(
    ring: &[Coord<f64>],
    reverse: bool,
    vertices: &mut Vec<Coord<f64>>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    vertices.clear();
    let open = match ring.split_last() {
        Some((last, rest)) if rest.first() == Some(last) => rest,
        _ => ring,
    };
    let mut push = |point: Coord<f64>| {
        if vertices.last() != Some(&point) {
            vertices.push(point);
        }
    };
    if reverse {
        open.iter().rev().copied().for_each(&mut push);
    } else {
        open.iter().copied().for_each(&mut push);
    }
    while vertices.len() > 1 && vertices.first() == vertices.last() {
        vertices.pop();
    }
    Ok(())
}

/// Disk vertex `k` sits at angle `(k + 1/2)·2π/sides`; it is extreme for every
/// outward normal in the half-open cone `[k, k + 1)·2π/sides`.
fn support(from: Coord<f64>, to: Coord<f64>, sides: usize) -> usize {
    // Right normal (dy, -dx) of the directed edge.
    let normal = (from.x - to.x).atan2(to.y - from.y);
    let turns = normal.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
    ((turns * sides as f64).floor() as usize).min(sides - 1)
}

/// Record each edge's support and return the cycle's point count.
fn plan_cycle(
    vertices: &[Coord<f64>],
    sides: usize,
    supports: &mut Vec<usize>,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    supports.clear();
    let count = vertices.len();
    if count == 1 {
        return Ok(sides);
    }
    for index in 0..count {
        if index % 1024 == 0 {
            check_cancel(cancel)?;
        }
        supports.push(support(
            vertices[index],
            vertices[(index + 1) % count],
            sides,
        ));
    }
    let mut points = 0usize;
    for index in 0..count {
        let join = match turn(vertices, supports, index, sides) {
            Join::Arc(steps) => steps + 1,
            Join::Through => 3,
        };
        points = points
            .checked_add(join)
            .ok_or_else(|| resource("offset convolution cycle point overflow"))?;
    }
    Ok(points)
}

/// Cycle path at one ring vertex between the incoming edge's disk support and
/// the outgoing edge's.
#[derive(Clone, Copy)]
enum Join {
    /// Walk this many disk vertices counterclockwise (a convex vertex).
    Arc(usize),
    /// A reflex vertex: the convolution walks the disk backwards. Route
    /// through the ring vertex instead. Relative to the backward walk this
    /// adds one counterclockwise disk fan around the vertex, i.e. `+1` winding
    /// only on points of `vertex ⊕ disk`, which already belong to the sum, so
    /// the positive-winding set is unchanged. It also avoids disk chords that
    /// retrace the translated edges almost antiparallel, whose snapped
    /// crossings leave zero-winding slivers.
    Through,
}

fn turn(vertices: &[Coord<f64>], supports: &[usize], index: usize, sides: usize) -> Join {
    let count = vertices.len();
    let previous = (index + count - 1) % count;
    let from = supports[previous];
    let to = supports[index];
    let forward = (to + sides - from) % sides;
    let backward = (from + sides - to) % sides;
    let a = vertices[previous];
    let b = vertices[index];
    let c = vertices[(index + 1) % count];
    let counterclockwise = match RobustKernel::orient2d(a, b, c) {
        // A genuine turn below π rotates the normal by at most half the disk;
        // a larger count is floating support noise at an almost straight
        // vertex, where the short way is the same walk.
        Orientation::CounterClockwise => forward <= sides / 2,
        Orientation::Clockwise => backward > sides / 2,
        Orientation::Collinear => {
            let dot = (b.x - a.x) * (c.x - b.x) + (b.y - a.y) * (c.y - b.y);
            // A reversal is a zero-width spike or crack: its boundary tube
            // wraps the tip, so walk counterclockwise around it.
            dot < 0.0 || forward <= backward
        }
    };
    if counterclockwise || backward == 0 {
        Join::Arc(forward)
    } else {
        Join::Through
    }
}

fn emit_cycle(
    vertices: &[Coord<f64>],
    supports: &[usize],
    disk: &[Coord<f64>],
    cycle: &mut Vec<Coord<f64>>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let sides = disk.len();
    let translate = |vertex: Coord<f64>, k: usize| Coord {
        x: vertex.x + disk[k].x,
        y: vertex.y + disk[k].y,
    };
    if vertices.len() == 1 {
        cycle.extend(disk.iter().map(|delta| vertices[0] + *delta));
        cycle.push(cycle[0]);
        return Ok(());
    }
    let count = vertices.len();
    for index in 0..count {
        if index % 1024 == 0 {
            check_cancel(cancel)?;
        }
        let mut k = supports[(index + count - 1) % count];
        cycle.push(translate(vertices[index], k));
        match turn(vertices, supports, index, sides) {
            Join::Arc(steps) => {
                for _ in 0..steps {
                    k = (k + 1) % sides;
                    cycle.push(translate(vertices[index], k));
                }
            }
            Join::Through => {
                cycle.push(vertices[index]);
                cycle.push(translate(vertices[index], supports[index]));
            }
        }
    }
    cycle.push(cycle[0]);
    Ok(())
}

#[derive(Default)]
struct Paths {
    coordinates: Vec<[f64; 2]>,
    ends: Vec<usize>,
}

impl ShapeResource<[f64; 2], f64> for Paths {
    type ResourceIter<'a> = Rings<'a>;

    fn iter_paths(&self) -> Rings<'_> {
        Rings {
            coordinates: &self.coordinates,
            ends: self.ends.iter(),
            start: 0,
        }
    }
}

struct Rings<'a> {
    coordinates: &'a [[f64; 2]],
    ends: std::slice::Iter<'a, usize>,
    start: usize,
}

impl<'a> Iterator for Rings<'a> {
    type Item = &'a [[f64; 2]];

    fn next(&mut self) -> Option<Self::Item> {
        let end = *self.ends.next()?;
        let ring = &self.coordinates[self.start..end];
        self.start = end;
        Some(ring)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::algorithm::convex_hull::quick_hull;
    use geo::{BooleanOps, unary_union};

    fn free(
        _: usize,
        _: usize,
        _: Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>,
    ) -> Result<(), ManufacturingError> {
        Ok(())
    }

    /// The former construction: material united with (or minus) the union of
    /// per-edge `segment ⊕ disk` capsules, per island for erosion.
    fn capsule_reference(
        polygons: &[Polygon<f64>],
        disk: &[Coord<f64>],
        erode: bool,
    ) -> MultiPolygon<f64> {
        let tube = |polygon: &Polygon<f64>| {
            let mut capsules = Vec::new();
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for pair in ring.0.windows(2) {
                    let mut points = pair
                        .iter()
                        .flat_map(|end| disk.iter().map(move |d| *end + *d))
                        .collect::<Vec<_>>();
                    capsules.push(Polygon::new(quick_hull(&mut points), Vec::new()));
                }
            }
            unary_union(&capsules)
        };
        if erode {
            let mut out = MultiPolygon(Vec::new());
            for polygon in polygons {
                let source = MultiPolygon(vec![polygon.clone()]);
                out.0.extend(source.difference(&tube(polygon)).0);
            }
            out
        } else {
            let mut out = MultiPolygon(polygons.to_vec());
            for polygon in polygons {
                out = out.union(&tube(polygon));
            }
            out
        }
    }

    fn segment_distance(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
        let d = b - a;
        let length = d.x * d.x + d.y * d.y;
        let t = if length == 0.0 {
            0.0
        } else {
            (((p.x - a.x) * d.x + (p.y - a.y) * d.y) / length).clamp(0.0, 1.0)
        };
        let q = a + d * t;
        (p.x - q.x).hypot(p.y - q.y)
    }

    fn boundary_distance(p: Coord<f64>, polygons: &[Polygon<f64>]) -> f64 {
        polygons
            .iter()
            .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
            .flat_map(|ring| ring.0.windows(2))
            .map(|pair| segment_distance(p, pair[0], pair[1]))
            .fold(f64::INFINITY, f64::min)
    }

    fn perimeter(polygons: &[Polygon<f64>]) -> f64 {
        polygons
            .iter()
            .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
            .flat_map(|ring| ring.0.windows(2))
            .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y))
            .sum()
    }

    /// Compare against the capsule union: symmetric difference within Boolean
    /// snapping, and every output vertex inside the circumscribed-disk band.
    /// Returns (symmetric difference area, new vertices, old vertices).
    fn agree(polygons: &[Polygon<f64>], distance: f64, check_band: bool) -> (f64, usize, usize) {
        let disk = offset_disk(distance.abs()).unwrap();
        let erode = distance < 0.0;
        let cancel = AtomicBool::new(false);
        let mut fresh = if erode {
            let mut out = MultiPolygon(Vec::new());
            for polygon in polygons {
                out.0.extend(
                    offset(
                        std::slice::from_ref(polygon),
                        &disk,
                        true,
                        0,
                        0,
                        &mut free,
                        &cancel,
                    )
                    .unwrap()
                    .0,
                );
            }
            out
        } else {
            offset(polygons, &disk, false, 0, 0, &mut free, &cancel).unwrap()
        };
        canonicalize(&mut fresh);
        let mut old = capsule_reference(polygons, &disk, erode);
        canonicalize(&mut old);
        let xor = fresh.xor(&old).unsigned_area();
        let scale = perimeter(polygons).max(1.0);
        assert!(
            xor <= scale * 2e-7,
            "offset {distance}: symmetric difference {xor} over perimeter {scale}"
        );
        if check_band {
            let radius = distance.abs();
            let circumradius = disk[0].x.hypot(disk[0].y);
            let snap = 1e-6;
            for ring in fresh
                .0
                .iter()
                .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
            {
                for point in &ring.0 {
                    let d = boundary_distance(*point, polygons);
                    assert!(
                        d >= radius - snap && d <= circumradius + snap,
                        "offset {distance}: vertex {point:?} at {d} outside [{radius}, {circumradius}]"
                    );
                }
            }
        }
        (xor, point_count(&fresh), point_count(&old))
    }

    fn ring(points: &[(f64, f64)]) -> LineString<f64> {
        let mut ring = LineString(points.iter().map(|&(x, y)| Coord { x, y }).collect());
        ring.close();
        ring
    }

    fn arc_polygon(cx: f64, cy: f64, radius: f64, count: usize, wobble: f64) -> LineString<f64> {
        ring(
            &(0..count)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / count as f64;
                    let r =
                        radius * (1.0 + wobble * (5.0 * a).sin() + 0.3 * wobble * (13.0 * a).cos());
                    (cx + r * a.cos(), cy + r * a.sin())
                })
                .collect::<Vec<_>>(),
        )
    }

    fn shapes() -> Vec<Vec<Polygon<f64>>> {
        let square = Polygon::new(
            ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]),
            Vec::new(),
        );
        // Clockwise input orientation must not matter.
        let clockwise_l = Polygon::new(
            ring(&[
                (0.0, 0.0),
                (0.0, 6.0),
                (2.0, 6.0),
                (2.0, 2.0),
                (6.0, 2.0),
                (6.0, 0.0),
            ]),
            Vec::new(),
        );
        // A comb whose gaps (0.3 mm) close under larger dilations and whose
        // teeth (0.5 mm) vanish under larger erosions.
        let mut comb = vec![(0.0, 0.0), (8.0, 0.0), (8.0, 5.0)];
        for tooth in (0..10).rev() {
            let x = tooth as f64 * 0.8;
            comb.extend([(x + 0.5, 5.0), (x + 0.5, 1.0), (x, 1.0), (x, 5.0)]);
        }
        comb.pop();
        comb.push((0.0, 5.0));
        let comb = Polygon::new(ring(&comb), Vec::new());
        // Holes separated by thin webs, one clockwise and one counterclockwise.
        let holes = Polygon::new(
            ring(&[(0.0, 0.0), (12.0, 0.0), (12.0, 6.0), (0.0, 6.0)]),
            vec![
                ring(&[(1.0, 1.0), (1.0, 5.0), (5.0, 5.0), (5.0, 1.0)]),
                ring(&[(5.4, 1.0), (9.0, 1.0), (9.0, 5.0), (5.4, 5.0)]),
                arc_polygon(10.5, 3.0, 0.6, 40, 0.0),
            ],
        );
        let acute = Polygon::new(ring(&[(0.0, 0.0), (20.0, 0.3), (0.0, 0.8)]), Vec::new());
        let acute_hole = Polygon::new(
            ring(&[(-2.0, -2.0), (22.0, -2.0), (22.0, 3.0), (-2.0, 3.0)]),
            vec![ring(&[(0.0, 0.0), (0.0, 0.8), (20.0, 0.3)])],
        );
        let wobbly = Polygon::new(
            arc_polygon(0.0, 0.0, 8.0, 720, 0.15),
            vec![
                arc_polygon(0.0, 0.0, 3.0, 360, 0.2),
                arc_polygon(5.0, 0.0, 1.0, 90, 0.1),
            ],
        );
        // Collinear runs and a repeated vertex.
        let collinear = Polygon::new(
            ring(&[
                (0.0, 0.0),
                (1.0, 0.0),
                (2.0, 0.0),
                (2.0, 0.0),
                (3.0, 0.0),
                (3.0, 2.0),
                (1.5, 2.0),
                (0.0, 2.0),
            ]),
            Vec::new(),
        );
        // Islands close enough for their dilations to merge.
        let islands = vec![
            Polygon::new(
                ring(&[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]),
                Vec::new(),
            ),
            Polygon::new(
                ring(&[(2.3, 0.0), (4.0, 0.0), (4.0, 2.0), (2.3, 2.0)]),
                Vec::new(),
            ),
            Polygon::new(arc_polygon(3.0, 4.0, 1.0, 64, 0.0), Vec::new()),
        ];
        vec![
            vec![square],
            vec![clockwise_l],
            vec![comb],
            vec![holes],
            vec![acute],
            vec![acute_hole],
            vec![wobbly],
            vec![collinear],
            islands,
        ]
    }

    #[test]
    fn convolution_offset_matches_capsule_union_within_the_disk_band() {
        for (index, shape) in shapes().iter().enumerate() {
            for distance in [0.0025, -0.0025, 0.2, -0.2, 0.45, -0.45, 1.3, -1.3] {
                let (xor, _, _) = agree(shape, distance, true);
                assert!(xor.is_finite(), "shape {index}");
            }
        }
    }

    #[test]
    fn convolution_offset_of_degenerate_rings_keeps_their_tube() {
        // A zero-area sliver ring and a one-point ring still have a tube.
        let disk = offset_disk(0.2).unwrap();
        let cancel = AtomicBool::new(false);
        let sliver = Polygon::new(ring(&[(0.0, 0.0), (4.0, 0.0)]), Vec::new());
        let grown = offset(&[sliver], &disk, false, 0, 0, &mut free, &cancel).unwrap();
        let area = grown.unsigned_area();
        let expected = 4.0 * 0.4 + std::f64::consts::PI * 0.04;
        assert!((area - expected).abs() < 0.01, "{area} vs {expected}");
    }

    /// Corpus comparison on real tolerant sections:
    /// `SPILING_WALL_CORPUS=.cache/realistic-corpus/step cargo test -p spiling-manufacturing
    /// --profile profiling convolution_corpus -- --ignored --nocapture`
    #[test]
    #[ignore = "needs SPILING_WALL_CORPUS with STEP files"]
    fn convolution_corpus() {
        let dir = std::env::var("SPILING_WALL_CORPUS").expect("SPILING_WALL_CORPUS");
        let only = std::env::var("SPILING_WALL_PARTS").ok();
        let cancel = AtomicBool::new(false);
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        let (mut layers, mut worst) = (0usize, 0.0f64);
        for path in files {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if only
                .as_ref()
                .is_some_and(|o| !o.split(',').any(|p| name.starts_with(p)))
            {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let Ok(bodies) = spiling_geometry::source_bodies(&bytes, &cancel) else {
                continue;
            };
            let mut models = Vec::new();
            for body in &bodies {
                let Ok(definition) =
                    spiling_geometry::from_step_solid_with_cancel(&bytes, &body.solid, &cancel)
                else {
                    continue;
                };
                let Ok(model) = spiling_geometry::tolerant::TolerantModel::build(
                    &definition,
                    Default::default(),
                    &cancel,
                ) else {
                    continue;
                };
                for pose in &body.placements {
                    models.push(model.placed(pose));
                }
            }
            let Some((min, max)) = models
                .iter()
                .map(|m| m.bounds())
                .reduce(|(lo, hi), (a, b)| {
                    (
                        std::array::from_fn(|i| lo[i].min(a[i])),
                        std::array::from_fn(|i| hi[i].max(b[i])),
                    )
                })
            else {
                continue;
            };
            let started = std::time::Instant::now();
            let step = ((max[2] - min[2]) / 12.0).max(0.2);
            let mut z = min[2] + 0.1;
            let mut part_worst = 0.0f64;
            while z < max[2] {
                let mut material = MultiPolygon(Vec::new());
                for model in &models {
                    if let Ok(section) = model.section(z) {
                        let piece = geo::Simplify::simplify(
                            &crate::planner::tolerant::polygons(&section),
                            0.005,
                        );
                        material = if material.0.is_empty() {
                            piece
                        } else {
                            material.union(&piece)
                        };
                    }
                }
                canonicalize(&mut material);
                if !material.0.is_empty() {
                    layers += 1;
                    for distance in [0.0025, -0.0025, 0.225, -0.225, 0.45, -0.45, 1.0, -1.0] {
                        let (xor, _, _) = agree(&material.0, distance, false);
                        let relative = xor / perimeter(&material.0).max(1.0);
                        part_worst = part_worst.max(relative);
                    }
                }
                z += step;
            }
            println!(
                "PART {name}: worst xor/perimeter {part_worst:.3e} in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            worst = worst.max(part_worst);
        }
        println!("ALL: {layers} layers, worst xor/perimeter {worst:.3e}");
    }
}
