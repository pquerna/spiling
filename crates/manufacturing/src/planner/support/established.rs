// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::grid::{Bits, Raster};
use super::*;

const SLACK_MM: f64 = 0.000_004;
const PROPOSAL_JOIN_MM: f64 = 0.001;
const CAPSULE_STEPS: usize = 8;

/// An eligible lower centreline subspan: its endpoints, its donor
/// radius, and whether each end is a real road end that keeps its cap.
#[derive(Clone, Copy)]
pub(super) struct Donor {
    ends: [[f64; 3]; 2],
    radius: f64,
    caps: [bool; 2],
}

impl Donor {
    fn bounds(&self, by: f64) -> [[f64; 2]; 2] {
        let [a, b] = self.ends;
        [
            [a[0].min(b[0]) - by, a[1].min(b[1]) - by],
            [a[0].max(b[0]) + by, a[1].max(b[1]) + by],
        ]
    }

    /// The donor frame: origin, unit axis and length.
    fn frame(&self) -> ([f64; 2], [f64; 2], f64) {
        let [a, b] = self.ends;
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = dx.hypot(dy);
        let axis = if length > 0.0 {
            [dx / length, dy / length]
        } else {
            [1.0, 0.0]
        };
        ([a[0], a[1]], axis, length)
    }

    /// The open parameter interval of segment `pq` inside the donor region
    /// bounded by the strip across the subspan and the round caps
    /// at real ends.
    fn interval(&self, p: [f64; 3], q: [f64; 3]) -> Option<(f64, f64)> {
        let radius = self.radius - SLACK_MM;
        if radius <= 0.0 {
            return None;
        }
        let (origin, axis, length) = self.frame();
        let local = |point: [f64; 3]| {
            let (x, y) = (point[0] - origin[0], point[1] - origin[1]);
            [axis[0] * x + axis[1] * y, axis[0] * y - axis[1] * x]
        };
        let (o, e) = (local(p), local(q));
        let c = [e[0] - o[0], e[1] - o[1]];
        let mut result: Option<(f64, f64)> = None;
        let mut include = |span: (f64, f64)| {
            if span.0 < span.1 {
                result = Some(result.map_or(span, |r| (r.0.min(span.0), r.1.max(span.1))));
            }
        };
        // Strip: 0 ≤ u ≤ length, |v| < radius (shrunk by the slack).
        let mut strip = (0.0_f64, 1.0_f64);
        let mut clip = |origin: f64, change: f64, lo: f64, hi: f64| -> bool {
            if change == 0.0 {
                return lo < origin && origin < hi;
            }
            let (t0, t1) = ((lo - origin) / change, (hi - origin) / change);
            strip.0 = strip.0.max(t0.min(t1).next_up());
            strip.1 = strip.1.min(t0.max(t1).next_down());
            strip.0 < strip.1
        };
        let along = [
            if self.caps[0] { 0.0 } else { SLACK_MM },
            if self.caps[1] {
                length
            } else {
                length - SLACK_MM
            },
        ];
        if clip(o[0], c[0], along[0], along[1]) && clip(o[1], c[1], -radius, radius) {
            include(strip);
        }
        let speed = c[0].hypot(c[1]);
        if speed > 0.0 {
            for (end, centre) in [(0, 0.0), (1, length)] {
                if !self.caps[end] {
                    continue;
                }
                // |o + t c − (centre, 0)| < radius.
                let rel = [o[0] - centre, o[1]];
                let b = (rel[0] * c[0] + rel[1] * c[1]) / (speed * speed);
                let k = (rel[0] * rel[0] + rel[1] * rel[1] - radius * radius) / (speed * speed);
                let disc = b * b - k;
                if disc > 0.0 {
                    let root = disc.sqrt();
                    include(((-b - root).max(0.0), (-b + root).min(1.0)));
                }
            }
        }
        result
    }

    /// Whether the disk of `radius` about `point` overlaps the donor region.
    fn touches(&self, point: [f64; 3], radius: f64) -> bool {
        let (origin, axis, length) = self.frame();
        let (x, y) = (point[0] - origin[0], point[1] - origin[1]);
        let (u, v) = (axis[0] * x + axis[1] * y, (axis[0] * y - axis[1] * x).abs());
        let reach = self.radius - SLACK_MM;
        let start = if self.caps[0] { 0.0 } else { SLACK_MM };
        let end = if self.caps[1] {
            length
        } else {
            length - SLACK_MM
        };
        if start >= end || reach <= 0.0 {
            return false;
        }
        let mut distance = (u - u.clamp(start, end)).hypot((v - reach).max(0.0));
        for (cap, centre) in self.caps.into_iter().zip([0.0, length]) {
            if cap {
                distance = distance.min(((u - centre).hypot(v) - reach).max(0.0));
            }
        }
        distance + SLACK_MM < radius
    }
}

pub(super) fn proposal_geometry<'p>(
    raster: &Raster<'_>,
    paths: impl Iterator<Item = &'p DepositionPath>,
    max_radius: f64,
    live_bytes: usize,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let bounds = raster.frame.bounds();
    let mut capsules: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut extent = bounds;
    for path in paths {
        check_cancel(raster.cancel)?;
        let Ok(edges) = path.checked_edges() else {
            continue;
        };
        for edge in edges {
            let &[p, q] = edge.points_mm else {
                continue;
            };
            let half = (edge.width_mm * 0.5).min(max_radius) + PROPOSAL_JOIN_MM;
            let (a, b) = ([p[0], p[1]], [q[0], q[1]]);
            let reach = [
                [a[0].min(b[0]) - half, a[1].min(b[1]) - half],
                [a[0].max(b[0]) + half, a[1].max(b[1]) + half],
            ];
            raster.charge(1);
            if !rects_meet(reach, bounds) || half.is_nan() || half <= 0.0 {
                continue;
            }
            extent = union_frame(Some(extent), reach);
            (raster.admit)(
                live_bytes
                    .saturating_add(
                        (capsules.len() + 1)
                            .saturating_mul((2 * CAPSULE_STEPS + 2) * size_of::<[f64; 2]>()),
                    )
                    .saturating_add(2 * vec_bytes(&capsules)),
            )?;
            raster.reserve(&mut capsules, 1)?;
            capsules.push(inscribed_capsule(a, b, half));
        }
    }
    if capsules.is_empty() {
        return Ok(MultiPolygon::new(Vec::new()));
    }
    use i_overlay::core::overlay::ShapeType;
    use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
    use i_overlay::float::overlay::FloatOverlay;
    use i_overlay::i_float::{adapter::FloatPointAdapter, float::rect::FloatRect};
    let points: usize = capsules.iter().map(Vec::len).sum();
    (raster.admit)(live_bytes.saturating_add(points.saturating_mul(64)))?;
    let adapter = FloatPointAdapter::new(FloatRect {
        min_x: extent[0][0],
        min_y: extent[0][1],
        max_x: extent[1][0],
        max_y: extent[1][1],
    });
    let shapes = FloatOverlay::with_adapter(adapter, points)
        .unsafe_add_source(&capsules, ShapeType::Subject)
        .overlay(OverlayRule::Subject, FillRule::NonZero);
    drop(capsules);
    raster.charge(points);
    let union = shapes
        .into_iter()
        .map(|shape| {
            let mut rings = shape.into_iter().map(|ring| {
                let mut line = geo::LineString(ring.into_iter().map(geo::Coord::from).collect());
                line.close();
                line
            });
            Polygon::new(
                rings.next().unwrap_or_else(geo::LineString::empty),
                rings.collect(),
            )
        })
        .collect();
    Ok(MultiPolygon::new(union))
}

fn rects_meet(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

/// A polygon inside the round capsule of `ab` and `radius`: the straight
/// sides and caps whose vertices lie on the circles.
fn inscribed_capsule(a: [f64; 2], b: [f64; 2], radius: f64) -> Vec<[f64; 2]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx.hypot(dy);
    let angle = if length > 0.0 { dy.atan2(dx) } else { 0.0 };
    let mut points = Vec::with_capacity(2 * CAPSULE_STEPS + 2);
    // Around b from +normal through +direction to −normal, then around a.
    for (centre, start) in [
        (b, angle + std::f64::consts::FRAC_PI_2),
        (a, angle - std::f64::consts::FRAC_PI_2),
    ] {
        for i in 0..=CAPSULE_STEPS {
            let t = start - std::f64::consts::PI * i as f64 / CAPSULE_STEPS as f64;
            points.push([centre[0] + radius * t.cos(), centre[1] + radius * t.sin()]);
        }
    }
    points
}

pub(super) fn ordinary(path: &DepositionPath) -> bool {
    path.kind.is_model() && path.kind != DepositionKind::Bridge
}

pub(super) fn new_contact_band(
    prior: &[PrintLayer],
    receiver: &PrintLayer,
    target: usize,
    gap: f64,
) -> Result<bool, ManufacturingError> {
    use program_coordinates::{enclosed_vertical_gap_mm, project_xyz6_bead_height_mm};
    let height = |layer: &PrintLayer| project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm);
    let top = height(&prior[target])?[1];
    let floor = height(receiver)?[0];
    let before = height(prior.last().expect("a separated root has an earlier layer"))?[0];
    Ok(
        enclosed_vertical_gap_mm(floor, top)? >= gap
            && enclosed_vertical_gap_mm(before, top)? < gap,
    )
}

fn can_measure(path: &DepositionPath) -> bool {
    use program_coordinates::MIN_DEPOSITION_COMMAND_MM;
    path.arcs.is_empty()
        && (path.points_mm.len() != 2
            || matches!(
                path.kind,
                DepositionKind::Perimeter | DepositionKind::SeparatorWall
            )
            || (path.points_mm[1][0] - path.points_mm[0][0])
                .hypot(path.points_mm[1][1] - path.points_mm[0][1])
                >= MIN_DEPOSITION_COMMAND_MM + SLACK_MM)
}

pub(super) fn local_donors(
    layers: &[PrintLayer],
    target: usize,
    contact_top: Option<usize>,
    recipe: &PlanarPrintRecipe,
    raster: &Raster<'_>,
    owned_bytes: usize,
    donors: &mut Vec<Donor>,
) -> Result<(), ManufacturingError> {
    let previous = target
        .checked_sub(1)
        .into_iter()
        .flat_map(|index| &layers[index].paths);
    let contact = contact_top
        .filter(|&top| top + 1 < target)
        .into_iter()
        .flat_map(|index| &layers[index].paths)
        .filter(|path| path.kind == DepositionKind::SupportInterface);
    let mut lower = strokes::LowerStrokes::new(
        previous.filter(|path| can_measure(path)),
        contact.filter(|path| can_measure(path)),
    );
    let mut anchors = AnchorWorkspace::default();
    anchors.set_budget(workspace_budget(
        0,
        owned_bytes.saturating_add(vec_bytes(donors)),
    )?);
    let mut work = 0;
    let mut excluded = Vec::new();
    for path in layers[target]
        .paths
        .iter()
        .filter(|path| ordinary(path) && can_measure(path))
    {
        check_cancel(raster.cancel)?;
        let edges = path.checked_edges()?;
        for (first, last, width) in StrokeRuns::for_path(path)? {
            let start = path.points_mm[first];
            let end = path.points_mm[last];
            let length = (end[0] - start[0]).hypot(end[1] - start[1]);
            if length <= SLACK_MM {
                continue;
            }
            excluded.clear();
            if target != 0 {
                let gaps = lower.gaps_supplied_contacts(
                    AnchorCandidate {
                        endpoints_mm: [start, end],
                        bead_width_mm: width,
                    },
                    path.physical_z_mm[0],
                    true,
                    &mut anchors,
                    &mut work,
                    raster.cancel,
                )?;
                reserve_owned(
                    &mut excluded,
                    gaps.len(),
                    owned_bytes.saturating_add(vec_bytes(donors)),
                )?;
                for gap in gaps {
                    let span = (gap.end - gap.start) * length;
                    if matches!(
                        strokes::gap_kind(
                            gap,
                            span + 2.0 * SLACK_MM,
                            width,
                            recipe.process.max_bridge_span_mm
                        ),
                        GapKind::Demand
                    ) {
                        excluded.push((
                            (gap.start - SLACK_MM / length).max(0.0),
                            (gap.end + SLACK_MM / length).min(1.0),
                        ));
                    }
                }
            }
            let parameter = |point: [f64; 3]| {
                ((point[0] - start[0]) * ((end[0] - start[0]) / length)
                    + (point[1] - start[1]) * ((end[1] - start[1]) / length))
                    / length
            };
            let count = if last > first {
                last - first
            } else {
                edges.len() - first + last
            };
            let mut gap = 0;
            for offset in 0..count {
                let edge = edges
                    .edge((first + offset) % edges.len())
                    .expect("edge of the measured stroke");
                let &[from, to] = edge.points_mm else {
                    continue;
                };
                let begin = if offset == 0 { 0.0 } else { parameter(from) };
                let finish = if offset + 1 == count {
                    1.0
                } else {
                    parameter(to)
                };
                let step = finish - begin;
                if !step.is_finite() || step <= 0.0 {
                    continue;
                }
                let from = program_coordinates::project_xyz6_point_mm(from)?;
                let to = program_coordinates::project_xyz6_point_mm(to)?;
                let radius = width * 0.5 - SLACK_MM;
                let mut emit = |left: f64, right: f64| -> Result<(), ManufacturingError> {
                    if left < right {
                        let at = |part: f64| {
                            let fraction = ((part - begin) / step).clamp(0.0, 1.0);
                            std::array::from_fn(|axis| {
                                from[axis] + (to[axis] - from[axis]) * fraction
                            })
                        };
                        reserve_owned(
                            donors,
                            1,
                            owned_bytes
                                .saturating_add(anchors.retained_bytes())
                                .saturating_add(vec_bytes(&excluded)),
                        )?;
                        donors.push(Donor {
                            ends: [at(left), at(right)],
                            radius,
                            caps: [left == begin, right == finish],
                        });
                    }
                    Ok(())
                };
                let mut cursor = begin;
                while let Some(&(left, right)) = excluded.get(gap).filter(|span| span.0 < finish) {
                    emit(cursor, left)?;
                    if right >= finish {
                        cursor = finish;
                        break;
                    }
                    cursor = cursor.max(right);
                    gap += 1;
                }
                emit(cursor, finish)?;
            }
        }
    }
    raster.charge(work);
    Ok(())
}

pub(super) fn inward_donors(
    raster: &Raster<'_>,
    donors: &[Donor],
) -> Result<Bits, ManufacturingError> {
    (raster.admit)(
        donors
            .len()
            .saturating_mul(64 * std::mem::size_of::<[f64; 2]>()),
    )?;
    let mut polygons = Vec::new();
    for donor in donors {
        check_cancel(raster.cancel)?;
        let (start, axis, length) = donor.frame();
        let radius = donor.radius - 2.0 * SLACK_MM;
        let low = if donor.caps[0] { 0.0 } else { 2.0 * SLACK_MM };
        let high = if donor.caps[1] {
            length
        } else {
            length - 2.0 * SLACK_MM
        };
        if high <= low || radius <= 0.0 {
            continue;
        }
        let point = |along: f64, across: f64| {
            [
                start[0] + axis[0] * along - axis[1] * across,
                start[1] + axis[1] * along + axis[0] * across,
            ]
        };
        let mut polygon = |ring: Vec<[f64; 2]>| {
            let mut line = geo::LineString(ring.into_iter().map(geo::Coord::from).collect());
            line.close();
            polygons.push(Polygon::new(line, Vec::new()));
        };
        if donor.caps == [true; 2] {
            polygon(inscribed_capsule(point(low, 0.0), point(high, 0.0), radius));
        } else {
            polygon(vec![
                point(low, -radius),
                point(high, -radius),
                point(high, radius),
                point(low, radius),
            ]);
            for (cap, edge) in donor.caps.into_iter().zip([low, high]) {
                if cap {
                    let center = point(edge, 0.0);
                    polygon(inscribed_capsule(center, center, radius));
                }
            }
        }
    }
    raster.inward(&polygons)
}

/// Support anchoring on eligible part cells: refines the gaps a stroke
/// run left on deposited support by the run's centreline pieces inside
/// `cells`. A gap end bounded by eligible cells is a contact end.
pub(super) struct CellContacts<'r, 'a> {
    pub raster: &'r Raster<'a>,
    pub cells: &'r Bits,
    pub donors: &'r [Donor],
}

impl CellContacts<'_, '_> {
    pub(super) fn stroke(
        &self,
        endpoints: [[f64; 3]; 2],
        radius: f64,
        pieces: &mut Vec<(f64, bool)>,
        intervals: &mut Vec<(f64, f64)>,
    ) -> Result<[bool; 2], ManufacturingError> {
        let [start, end] = [
            program_coordinates::project_xyz6_point_mm(endpoints[0])?,
            program_coordinates::project_xyz6_point_mm(endpoints[1])?,
        ];
        self.raster.along(
            self.cells,
            self.cells,
            [start[0], start[1]],
            [end[0], end[1]],
            pieces,
        )?;
        if pieces.as_slice() == [(1.0, true)] || self.donors.is_empty() {
            return Ok([false; 2]);
        }
        intervals.clear();
        let mut ends = [false; 2];
        let reach = [
            [start[0].min(end[0]) - radius, start[1].min(end[1]) - radius],
            [start[0].max(end[0]) + radius, start[1].max(end[1]) + radius],
        ];
        for donor in self.donors {
            check_cancel(self.raster.cancel)?;
            self.raster.charge(1);
            if rects_meet(donor.bounds(donor.radius), reach) {
                if let Some(interval) = donor.interval(start, end) {
                    self.raster.reserve(intervals, 1)?;
                    intervals.push(interval);
                }
                ends[0] |= donor.touches(start, radius);
                ends[1] |= donor.touches(end, radius);
            }
        }
        intervals.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
        pieces.clear();
        self.raster
            .reserve(pieces, intervals.len().saturating_mul(2).saturating_add(1))?;
        let mut previous = 0.0_f64;
        for &(start, end) in intervals.iter() {
            if end <= previous {
                continue;
            }
            if start > previous {
                pieces.push((start, false));
                pieces.push((end, true));
            } else if let Some(piece) = pieces.last_mut() {
                piece.0 = end;
            } else {
                pieces.push((end, true));
            }
            previous = end;
        }
        if previous < 1.0 {
            pieces.push((1.0, false));
        }
        Ok(ends)
    }
    /// The sub-gaps of `gap` (parameters along `ab`) outside the cells, as
    /// `(start, end, left, right, direct_ends)`, given the run's pieces.
    pub(super) fn refine(
        pieces: &[(f64, bool)],
        gap: &super::anchors::AnchorGap,
        ends: [bool; 2],
        mut visit: impl FnMut(f64, f64, bool, bool, bool) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        let mut start = 0.0_f64;
        for &(end, inside) in pieces {
            let (lo, hi) = (start.max(gap.start), end.min(gap.end));
            start = end;
            // A degenerate gap is judged as it was, once.
            if inside || lo > hi || (lo == hi && (gap.start < gap.end || end < gap.end)) {
                continue;
            }
            let left_original = lo <= gap.start;
            let right_original = hi >= gap.end;
            visit(
                lo,
                hi,
                !left_original || gap.left || (lo == 0.0 && ends[0]),
                !right_original || gap.right || (hi == 1.0 && ends[1]),
                left_original && right_original && gap.direct_ends,
            )?;
        }
        Ok(())
    }
}
