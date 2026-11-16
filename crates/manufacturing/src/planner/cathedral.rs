// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Cathedral: rooms first, roofs second (RFC 12).
//!
//! **Rooms.** One part-level frame anchors two families of lattice lines that
//! stay fixed through Z. Every room (lattice cell clipped to the classified
//! sparse domain) prints as closed loops offset inward by half the stadium
//! bead spacing `s`, so each shared wall is two bonded beads and walls stack
//! exactly. Corners at interior crossings are pinned inward until their bead
//! covers the crossing: an explicit 2x2 pier. A strip too narrow for a loop
//! carries one open spine bead. Neighbouring loops are spliced across their
//! shared wall into one loop per connected region, and top skin that covers
//! part of a cell takes the whole cell on that layer, so roofs always end on
//! walls.
//!
//! **Roofs.** Whatever the next physical layer deposits over a room (skin,
//! solid, bridges) must be supported. Tooth columns perpendicular to those
//! roads, at most the roof spacing apart, grow from the room's own loop into
//! the room: two legs `s` apart and a tip, inserted into the loop so the room
//! stays one closed road. Demand is propagated top-down over the actual
//! schedule; teeth then grow bottom-up by at most one bead step per physical
//! interface, so every tooth is rooted in its loop and stacks on itself.

use super::region::offset_admitted;
use crate::{check_cancel, error};
use geo::{
    Area, BooleanOps, BoundingRect, Coord, LineString, MinimumRotatedRect, MultiPolygon, Polygon,
    Rect,
};
use spiling_contracts::manufacturing::program_coordinates::MIN_DEPOSITION_COMMAND_MM;
use spiling_contracts::manufacturing::*;
use std::collections::BTreeMap;
use std::sync::{Arc, atomic::AtomicBool};

/// Largest lateral tooth advance per physical interface, in bead widths:
/// below replay's half-bead one-sided anchor rule.
pub(crate) const ROOF_STEP_WIDTHS: f64 = 0.45;
/// Pinned junction corners sit this fraction of a bead radius from the
/// crossing, so the corner bead covers it.
const JUNCTION_PIN: f64 = 0.9;

/// Stadium spacing of neighbouring bead centerlines at height `h`.
pub(crate) fn spacing(width: f64, height: f64) -> f64 {
    width - height * (1.0 - std::f64::consts::PI / 4.0)
}

/// Material-equivalent cell pitch: two beads per lattice line.
pub(crate) fn cell_pitch(width: f64, nominal_height: f64, fraction: f64) -> f64 {
    let area = (width - nominal_height) * nominal_height
        + std::f64::consts::PI * (nominal_height * 0.5).powi(2);
    4.0 * area / (nominal_height * fraction)
}

/// Lattice line coordinates per axis, centered in the frame.
pub(crate) fn lattice(frame: [[f64; 2]; 2], pitch: f64) -> [Vec<f64>; 2] {
    std::array::from_fn(|axis| {
        let length = frame[1][axis] - frame[0][axis];
        let count = (length / pitch).ceil().max(1.0) as usize;
        let first = frame[0][axis] + (length - (count - 1) as f64 * pitch) * 0.5;
        (0..count).map(|i| first + i as f64 * pitch).collect()
    })
}

/// A Cathedral road: regional sparse fill under the Cathedral policy.
pub(super) fn is_road(path: &DepositionPath) -> bool {
    path.kind == DepositionKind::SparseFill
        && matches!(path.owner, Some(DepositionOwner::Regional { .. }))
}

pub(super) struct Field {
    frame_mm: [[f64; 2]; 2],
}

/// The part-level frame: material bounds inside the walls.
pub(super) fn field(
    materials: &[&Arc<MultiPolygon<f64>>],
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<Field, ManufacturingError> {
    let refuse = |message: &str| error(ManufacturingErrorCode::UnsupportedGeometry, message);
    let mut frame = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    let mut previous: Option<&Arc<MultiPolygon<f64>>> = None;
    for &material in materials {
        check_cancel(cancel)?;
        if previous.is_some_and(|p| Arc::ptr_eq(p, material)) {
            continue;
        }
        previous = Some(material);
        if let Some(bounds) = material.bounding_rect() {
            frame[0] = [
                frame[0][0].min(bounds.min().x),
                frame[0][1].min(bounds.min().y),
            ];
            frame[1] = [
                frame[1][0].max(bounds.max().x),
                frame[1][1].max(bounds.max().y),
            ];
        }
    }
    let inset = f64::from(recipe.perimeter_count) * recipe.bead_width_mm;
    let [minimum, maximum] = &mut frame;
    for (lower, upper) in minimum.iter_mut().zip(maximum.iter_mut()) {
        if *upper - *lower > 2.0 * inset + recipe.bead_width_mm {
            *lower += inset;
            *upper -= inset;
        }
    }
    if frame.iter().flatten().any(|v| !v.is_finite())
        || (0..2).any(|axis| frame[0][axis] >= frame[1][axis])
    {
        return Err(refuse("Cathedral requires a part with positive XY extent"));
    }
    Ok(Field { frame_mm: frame })
}

impl Field {
    pub(super) fn roof(&self) -> super::region::Roof {
        super::region::Roof {
            frame_mm: self.frame_mm,
        }
    }

    /// Record the frame for independent replay.
    pub(super) fn stamp(&self, domains: &mut [WebRoofDomain]) {
        for domain in domains {
            domain.frame_mm = self.frame_mm;
        }
    }

    /// Regenerate every Cathedral road of the whole part, now that every
    /// layer's next layer is known: rooms plus roof teeth.
    pub(super) fn stabilize(
        &self,
        layers: &mut [PrintLayer],
        recipe: &PlanarPrintRecipe,
        segments: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let rooms = layers
            .iter()
            .map(|layer| layer_rooms(layer, self.frame_mm, recipe, cancel))
            .collect::<Result<Vec<_>, _>>()?;
        let teeth = grow_teeth(layers, &rooms, recipe, cancel)?;
        let mut anchors = super::support::AnchorWorkspace::default();
        let mut work = 0usize;
        for index in 0..layers.len() {
            check_cancel(cancel)?;
            let (before, rest) = layers.split_at_mut(index);
            let layer = &mut rest[0];
            let lower = before.last().filter(|lower| lower.z_mm == layer.lower_z_mm);
            let cathedral = is_road;
            let removed: usize = layer
                .paths
                .iter()
                .filter(|path| cathedral(path))
                .map(|path| path.points_mm.len().saturating_sub(1))
                .sum();
            *segments = segments.checked_sub(removed).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "Cathedral segment accounting underflow",
                )
            })?;
            let mut replacement = Vec::new();
            for domain in &rooms[index] {
                let mut roads = Vec::new();
                for room in &domain.rooms {
                    room_roads(room, &teeth[index], domain, &mut roads, cancel)?;
                }
                let roads = splice(roads, domain, index, cancel)?;
                // Spines print only where the actual layer below carries them.
                let prepared = match lower {
                    Some(lower) if roads.iter().any(|r| r.points.first() != r.points.last()) => {
                        Some(super::support::PreparedAnchorPaths::new(
                            lower.paths.iter().filter(|p| {
                                p.kind.is_model() && p.physical_z_mm[1] == layer.lower_z_mm
                            }),
                            cancel,
                        )?)
                    }
                    _ => None,
                };
                for road in roads {
                    let open = road.points.first() != road.points.last();
                    if open {
                        let Some(prepared) = prepared.as_ref() else {
                            continue;
                        };
                        if !rooted(
                            &road.points,
                            layer.z_mm,
                            prepared,
                            recipe,
                            &mut anchors,
                            &mut work,
                            cancel,
                        )? {
                            continue;
                        }
                    }
                    emit(
                        &road.points,
                        domain.region_index,
                        [layer.lower_z_mm, layer.z_mm],
                        recipe.bead_width_mm,
                        &mut replacement,
                        segments,
                    )?;
                }
            }
            layer.paths.retain(|path| !cathedral(path));
            layer.paths.append(&mut replacement);
            let retained = super::bonds::path_bytes(&layer.paths);
            super::classification::reserve_skin_fits(layer, segments, retained, cancel)?;
        }
        Ok(())
    }
}

/// An open spine prints only where the layer below carries it, judged by the
/// same capsule anchors replay uses: real supported length, and no
/// unsupported run near the sparse allowance.
fn rooted(
    points: &[Coord<f64>],
    z: f64,
    prepared: &super::support::PreparedAnchorPaths<'_>,
    recipe: &PlanarPrintRecipe,
    anchors: &mut super::support::AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    use super::support::{AnchorCandidate, GapKind, gap_kind};
    let w = recipe.bead_width_mm;
    let (mut run, mut supported) = (0.0_f64, 0.0_f64);
    for e in points.windows(2) {
        check_cancel(cancel)?;
        let length = (e[1].x - e[0].x).hypot(e[1].y - e[0].y);
        let gaps = anchors.gaps_prepared(
            AnchorCandidate {
                endpoints_mm: [[e[0].x, e[0].y, z], [e[1].x, e[1].y, z]],
                bead_width_mm: w,
            },
            prepared,
            work,
            cancel,
        )?;
        let (mut cursor, mut unsupported) = (0.0, 0.0);
        for gap in gaps {
            let span = (gap.end - gap.start) * length;
            if gap.start > cursor {
                run = 0.0;
            }
            match gap_kind(gap, span, w, recipe.process.max_bridge_span_mm) {
                GapKind::Supported => {}
                GapKind::Bridge | GapKind::Contact => run = 0.0,
                GapKind::Demand => {
                    run += span;
                    unsupported += span;
                    if run > 2.0 * w {
                        return Ok(false);
                    }
                }
            }
            cursor = gap.end;
        }
        if cursor < 1.0 {
            run = 0.0;
        }
        supported += (length - unsupported).max(0.0);
    }
    Ok(supported >= 2.0 * w)
}

/// Rooms of one layer's sparse domain, without roof teeth: the per-layer
/// proposal before the whole part is known.
pub(super) fn rooms(
    fill: &MultiPolygon<f64>,
    fraction: f64,
    layer: &super::region::HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let Some(roof) = layer.roof else {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "Cathedral requires its part-level lattice frame",
        ));
    };
    if fraction <= 0.0 || fill.0.is_empty() {
        return Ok(());
    }
    let s = spacing(layer.width_mm, layer.height);
    let pitch = cell_pitch(layer.width_mm, layer.recipe.layer_height_mm, fraction);
    let domain = DomainRooms {
        region_index: 0,
        s,
        lines: lattice(roof.frame_mm, pitch),
        rooms: cells(fill, roof.frame_mm, pitch, s, layer.width_mm, cancel)?,
    };
    // A proposal knows no lower layer, so it carries no spines; the
    // whole-part stabilization roots them.
    let empty = BTreeMap::new();
    let mut roads = Vec::new();
    for room in &domain.rooms {
        room_roads(room, &empty, &domain, &mut roads, cancel)?;
    }
    let parity = (layer.z / layer.height).round() as usize;
    for road in splice(roads, &domain, parity, cancel)? {
        if road.points.first() != road.points.last() {
            continue;
        }
        emit(
            &road.points,
            domain.region_index,
            [layer.lower_z, layer.z],
            layer.width_mm,
            paths,
            segments,
        )?;
    }
    Ok(())
}

/// One room component: its loops (exterior first) in the frame-clipped
/// lattice cell `bounds`.
pub(super) struct Room {
    bounds: [[f64; 2]; 2],
    rings: Vec<Vec<Coord<f64>>>,
    /// A spine: one open road in a strip too narrow for a loop.
    open: bool,
}

struct DomainRooms {
    region_index: u16,
    s: f64,
    /// Lattice line coordinates per axis.
    lines: [Vec<f64>; 2],
    rooms: Vec<Room>,
}

fn layer_rooms(
    layer: &PrintLayer,
    frame: [[f64; 2]; 2],
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<Vec<DomainRooms>, ManufacturingError> {
    let mut out = Vec::new();
    for domain in &layer.region_coverage.web_roof_domains {
        let fraction = layer
            .region_coverage
            .process_regions
            .get(usize::from(domain.region_index))
            .map_or(recipe.infill_fraction, |region| region.fraction);
        if fraction <= 0.0 || fraction >= 1.0 {
            continue;
        }
        let fill = MultiPolygon(domain.extent.iter().map(decode).collect());
        let s = spacing(recipe.bead_width_mm, layer.height_mm);
        let pitch = cell_pitch(recipe.bead_width_mm, recipe.layer_height_mm, fraction);
        out.push(DomainRooms {
            region_index: domain.region_index,
            s,
            lines: lattice(frame, pitch),
            rooms: cells(&fill, frame, pitch, s, recipe.bead_width_mm, cancel)?,
        });
    }
    Ok(out)
}

pub(in crate::planner) fn decode(polygon: &DiaphragmPolygonMm) -> Polygon<f64> {
    let ring = |points: &[[f64; 2]]| {
        LineString(points.iter().map(|p| Coord { x: p[0], y: p[1] }).collect())
    };
    Polygon::new(
        ring(&polygon.outer),
        polygon.holes.iter().map(|h| ring(h)).collect(),
    )
}

fn offset(
    region: &MultiPolygon<f64>,
    distance: f64,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    offset_admitted(
        region,
        distance,
        |_, bytes, overlay| {
            if let Some((a, b)) = overlay {
                super::boolean_budget(a, b, cancel)?;
            }
            if bytes > crate::working_set::bytes() {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "Cathedral room offset exceeds the working set",
                ));
            }
            Ok(())
        },
        cancel,
    )
}

/// Room loops: the domain eroded by `s/2`, intersected with each lattice cell
/// inset by `s/2` (erosion distributes over intersection, so lattice sides
/// stay exact). Components narrower than one spacing print nothing.
fn cells(
    fill: &MultiPolygon<f64>,
    frame: [[f64; 2]; 2],
    pitch: f64,
    s: f64,
    width: f64,
    cancel: &AtomicBool,
) -> Result<Vec<Room>, ManufacturingError> {
    let eroded = offset(fill, -0.5 * s, cancel)?;
    if eroded.0.is_empty() {
        return Ok(Vec::new());
    }
    // Junction pins need the crossing well inside the domain.
    let deep = offset(fill, -s, cancel)?;
    let lines = lattice(frame, pitch);
    let Some(bounds) = eroded.bounding_rect() else {
        return Ok(Vec::new());
    };
    let pad = 2.0 * pitch;
    let breaks = |axis: usize| -> Vec<(f64, f64, bool, bool)> {
        // (low, high, low is a lattice line, high is a lattice line)
        let l = &lines[axis];
        let mut out = Vec::with_capacity(l.len() + 1);
        out.push((frame[0][axis].min(l[0]) - pad, l[0], false, true));
        for pair in l.windows(2) {
            out.push((pair[0], pair[1], true, true));
        }
        out.push((
            l[l.len() - 1],
            frame[1][axis].max(l[l.len() - 1]) + pad,
            true,
            false,
        ));
        out
    };
    let (columns, rows) = (breaks(0), breaks(1));
    let mut rooms = Vec::new();
    for &(x0, x1, x0_line, x1_line) in &columns {
        if x1 < bounds.min().x || x0 > bounds.max().x {
            continue;
        }
        for &(y0, y1, y0_line, y1_line) in &rows {
            check_cancel(cancel)?;
            if y1 < bounds.min().y || y0 > bounds.max().y {
                continue;
            }
            let inset = |v: f64, line: bool, sign: f64| if line { v + sign * 0.5 * s } else { v };
            let rect = Rect::new(
                Coord {
                    x: inset(x0, x0_line, 1.0),
                    y: inset(y0, y0_line, 1.0),
                },
                Coord {
                    x: inset(x1, x1_line, -1.0),
                    y: inset(y1, y1_line, -1.0),
                },
            )
            .to_polygon();
            let rect = MultiPolygon(vec![rect]);
            super::boolean_budget(&eroded, &rect, cancel)?;
            let clipped = eroded.intersection(&rect);
            let cell_bounds = [
                [x0.max(frame[0][0]), y0.max(frame[0][1])],
                [x1.min(frame[1][0]), y1.min(frame[1][1])],
            ];
            for polygon in clipped.0 {
                let around: f64 = std::iter::once(polygon.exterior())
                    .chain(polygon.interiors())
                    .map(|ring| ring_length(&ring.0))
                    .sum();
                if around <= 0.0 {
                    continue;
                }
                if 2.0 * polygon.unsigned_area() / around < s {
                    // Too narrow for a loop: one spine bead, so whatever
                    // lands on the strip later has a wall under it.
                    for run in spine(&polygon, width) {
                        rooms.push(Room {
                            bounds: cell_bounds,
                            rings: vec![run],
                            open: true,
                        });
                    }
                    continue;
                }
                let mut rings = Vec::new();
                for (index, ring) in std::iter::once(polygon.exterior())
                    .chain(polygon.interiors())
                    .enumerate()
                {
                    let points = if index == 0 {
                        pin_junctions(&ring.0, &lines, &deep, s, width)
                    } else {
                        ring.0.clone()
                    };
                    let points = drop_short_edges(points);
                    if points.len() >= 4 {
                        rings.push(points);
                    }
                }
                if !rings.is_empty() {
                    rooms.push(Room {
                        bounds: cell_bounds,
                        rings,
                        open: false,
                    });
                }
            }
        }
    }
    Ok(rooms)
}

/// The midline of a thin piece's minimum rotated rectangle along its long
/// side, clipped to the piece: every clipped run of at least two bead
/// widths. A bent strip keeps only the runs its midline crosses; whatever
/// later lands on the rest is judged, and reported, like any other road.
fn spine(piece: &Polygon<f64>, width: f64) -> Vec<Vec<Coord<f64>>> {
    spine_runs(piece, width).unwrap_or_default()
}

fn spine_runs(piece: &Polygon<f64>, width: f64) -> Option<Vec<Vec<Coord<f64>>>> {
    let rect = piece.minimum_rotated_rect()?;
    let c = &rect.exterior().0;
    if c.len() < 4 {
        return None;
    }
    let length = |a: Coord<f64>, b: Coord<f64>| (b.x - a.x).hypot(b.y - a.y);
    let mid = |a: Coord<f64>, b: Coord<f64>| Coord {
        x: 0.5 * (a.x + b.x),
        y: 0.5 * (a.y + b.y),
    };
    // Join the midpoints of the two short sides.
    let (from, to) = if length(c[0], c[1]) >= length(c[1], c[2]) {
        (mid(c[1], c[2]), mid(c[3], c[0]))
    } else {
        (mid(c[0], c[1]), mid(c[2], c[3]))
    };
    let line = geo::MultiLineString(vec![LineString(vec![from, to])]);
    let clipped = piece.clip(&line, false);
    Some(
        clipped
            .0
            .into_iter()
            .map(|run| run.0)
            .filter(|run| run.len() >= 2 && ring_length(run) >= 2.0 * width)
            .collect(),
    )
}

fn ring_length(points: &[Coord<f64>]) -> f64 {
    points
        .windows(2)
        .map(|e| (e[1].x - e[0].x).hypot(e[1].y - e[0].y))
        .sum()
}

/// Pin each loop corner that sits `s/2` from an interior lattice crossing on
/// both axes: the corner moves toward the crossing until its bead covers it,
/// with new vertices half a spacing along both sides so the walls stay put.
fn pin_junctions(
    ring: &[Coord<f64>],
    lines: &[Vec<f64>; 2],
    deep: &MultiPolygon<f64>,
    s: f64,
    width: f64,
) -> Vec<Coord<f64>> {
    use geo::Contains;
    let near = |value: f64, axis: usize| -> Option<f64> {
        let l = &lines[axis];
        let i = l.partition_point(|line| *line < value);
        [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|i| l.get(i).copied())
            .find(|line| ((value - line).abs() - 0.5 * s).abs() <= 1e-6)
    };
    let edges = ring.len() - 1;
    let mut out = Vec::with_capacity(ring.len() + 8);
    for index in 0..edges {
        let v = ring[index];
        let pinned = match (near(v.x, 0), near(v.y, 1)) {
            (Some(x), Some(y)) if deep.contains(&geo::Point(Coord { x, y })) => {
                Some(Coord { x, y })
            }
            _ => None,
        };
        let Some(crossing) = pinned else {
            out.push(v);
            continue;
        };
        let prev = ring[(index + edges - 1) % edges];
        let next = ring[index + 1];
        let toward = |p: Coord<f64>| {
            let l = (p.x - v.x).hypot(p.y - v.y);
            (l > s).then(|| Coord {
                x: v.x + (p.x - v.x) * 0.5 * s / l,
                y: v.y + (p.y - v.y) * 0.5 * s / l,
            })
        };
        let (Some(a), Some(b)) = (toward(prev), toward(next)) else {
            out.push(v);
            continue;
        };
        let d = (v.x - crossing.x).hypot(v.y - crossing.y);
        let reach = JUNCTION_PIN * 0.5 * width;
        out.push(a);
        out.push(Coord {
            x: crossing.x + (v.x - crossing.x) * reach / d,
            y: crossing.y + (v.y - crossing.y) * reach / d,
        });
        out.push(b);
    }
    out.push(out[0]);
    out
}

/// Remove vertices that would make an edge shorter than one minimum command,
/// keeping the ring closed.
fn drop_short_edges(mut points: Vec<Coord<f64>>) -> Vec<Coord<f64>> {
    if points.len() < 4 {
        return points;
    }
    points.pop();
    let mut changed = true;
    while changed && points.len() >= 3 {
        changed = false;
        let mut i = 0;
        while i < points.len() && points.len() >= 3 {
            let next = (i + 1) % points.len();
            let a = points[i];
            let b = points[next];
            if (b.x - a.x).hypot(b.y - a.y) < MIN_DEPOSITION_COMMAND_MM {
                points.remove(next);
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    if points.len() < 3 {
        return Vec::new();
    }
    points.push(points[0]);
    points
}

/// A tooth column: family 0 teeth run along Y at `x = c` (they carry roads
/// that run along X); family 1 teeth run along X at `y = c`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Column {
    region: u16,
    family: u8,
    coordinate: u64,
}

/// Tooth tips per column and side (0 grows from the low end of a chord, 1
/// from the high end).
type Tips = BTreeMap<(Column, u8), Vec<f64>>;

fn columns(room: &Room, family: usize, s: f64, spacing_mm: f64) -> Vec<f64> {
    if room.open {
        return Vec::new();
    }
    let axis = family; // family 0 positions lie along X
    let low = room.bounds[0][axis];
    let high = room.bounds[1][axis];
    let span = high - low - s;
    if span <= spacing_mm {
        return Vec::new();
    }
    let count = (span / spacing_mm).ceil() as usize - 1;
    (1..=count)
        .map(|k| low + 0.5 * s + k as f64 * span / (count + 1) as f64)
        .collect()
}

/// Chords of a room along the line `axis = c`: sorted `(low, high)` pairs of
/// the coordinate along the other axis.
fn chords(room: &Room, family: usize, c: f64) -> Vec<(f64, f64)> {
    let (fixed, along) = if family == 0 { (0, 1) } else { (1, 0) };
    let get = |p: &Coord<f64>, axis: usize| if axis == 0 { p.x } else { p.y };
    let mut hits = Vec::new();
    for ring in &room.rings {
        for edge in ring.windows(2) {
            let (a, b) = (get(&edge[0], fixed), get(&edge[1], fixed));
            if (a <= c && c < b) || (b <= c && c < a) {
                let t = (c - a) / (b - a);
                hits.push(get(&edge[0], along) + t * (get(&edge[1], along) - get(&edge[0], along)));
            }
        }
    }
    hits.sort_by(f64::total_cmp);
    hits.as_chunks::<2>()
        .0
        .iter()
        .map(|p| (p[0], p[1]))
        .collect()
}

/// Top-down demand, then bottom-up growth: the actual tooth tips per layer.
fn grow_teeth(
    layers: &[PrintLayer],
    rooms: &[Vec<DomainRooms>],
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<Vec<Tips>, ManufacturingError> {
    let w = recipe.bead_width_mm;
    let spacing_mm = recipe.web_roof_spacing_mm();
    let n = layers.len();
    let adjacent =
        |lower: usize| lower + 1 < n && layers[lower + 1].lower_z_mm == layers[lower].z_mm;
    let step = |upper: usize| (ROOF_STEP_WIDTHS * w).min(layers[upper].height_mm);
    // Desired tips, top-down.
    let mut desired: Vec<Tips> = vec![BTreeMap::new(); n];
    let mut carried: Tips = BTreeMap::new();
    for index in (0..n).rev() {
        check_cancel(cancel)?;
        if adjacent(index) {
            let advance = step(index + 1);
            for ((_, side), tips) in carried.iter_mut() {
                for tip in tips.iter_mut() {
                    *tip += if *side == 0 { -advance } else { advance };
                }
            }
        } else {
            carried.clear();
        }
        // New demand: roads of the next physical layer over these rooms.
        let roads = if adjacent(index) {
            roof_roads(&layers[index + 1])
        } else {
            [Vec::new(), Vec::new()]
        };
        let mut next: Tips = BTreeMap::new();
        for domain in &rooms[index] {
            let s = domain.s;
            for room in &domain.rooms {
                for (family, family_roads) in roads.iter().enumerate() {
                    for c in columns(room, family, s, spacing_mm) {
                        let column = Column {
                            region: domain.region_index,
                            family: family as u8,
                            coordinate: c.to_bits(),
                        };
                        for (low, high) in chords(room, family, c) {
                            if high - low < 3.0 * s {
                                continue;
                            }
                            let middle = 0.5 * (low + high - s);
                            let mut tips = [None::<f64>, None::<f64>];
                            // Carried requirement falling in this chord.
                            for side in 0..2u8 {
                                for &tip in carried.get(&(column, side)).into_iter().flatten() {
                                    if tip > low && tip < high {
                                        let t = &mut tips[side as usize];
                                        *t = Some(match (*t, side) {
                                            (Some(v), 0) => v.max(tip),
                                            (Some(v), _) => v.min(tip),
                                            (None, _) => tip,
                                        });
                                    }
                                }
                            }
                            // This roof's own requirement.
                            let (mut need_low, mut need_high) = (f64::INFINITY, f64::NEG_INFINITY);
                            for &(position, from, to) in family_roads {
                                if from < c && c < to && position > low && position < high {
                                    need_low = need_low.min(position);
                                    need_high = need_high.max(position);
                                }
                            }
                            if need_low <= need_high {
                                if need_low < middle {
                                    let tip = need_high.min(middle);
                                    tips[0] = Some(tips[0].map_or(tip, |v| v.max(tip)));
                                }
                                if need_high > middle + s {
                                    let tip = need_low.max(middle + s);
                                    tips[1] = Some(tips[1].map_or(tip, |v| v.min(tip)));
                                }
                            }
                            // Keep inside the chord and clear of each other.
                            if let Some(t) = tips[0].as_mut() {
                                *t = t.min(high - s);
                            }
                            if let Some(t) = tips[1].as_mut() {
                                *t = t.max(low + s);
                            }
                            if let (Some(a), Some(b)) = (tips[0], tips[1])
                                && a + s > b
                            {
                                let m = 0.5 * (a + b - s);
                                tips = [Some(m), Some(m + s)];
                            }
                            if tips[0].is_some_and(|t| t - low >= s) {
                                next.entry((column, 0)).or_default().push(tips[0].unwrap());
                            }
                            if tips[1].is_some_and(|t| high - t >= s) {
                                next.entry((column, 1)).or_default().push(tips[1].unwrap());
                            }
                        }
                    }
                }
            }
        }
        desired[index] = next.clone();
        carried = next;
    }
    // Actual tips, bottom-up: a tooth starts one spacing long and then
    // advances at most one step per physical interface.
    let mut actual: Vec<Tips> = vec![BTreeMap::new(); n];
    for index in 0..n {
        check_cancel(cancel)?;
        let below = (index > 0 && adjacent(index - 1)).then(|| &actual[index - 1]);
        let advance = step(index);
        let mut grown: Tips = BTreeMap::new();
        for domain in &rooms[index] {
            let s = domain.s;
            for room in &domain.rooms {
                for family in 0..2 {
                    for c in columns(room, family, s, spacing_mm) {
                        let column = Column {
                            region: domain.region_index,
                            family: family as u8,
                            coordinate: c.to_bits(),
                        };
                        for (low, high) in chords(room, family, c) {
                            for side in 0..2u8 {
                                let Some(want) = desired[index]
                                    .get(&(column, side))
                                    .into_iter()
                                    .flatten()
                                    .copied()
                                    .find(|t| *t > low && *t < high)
                                else {
                                    continue;
                                };
                                let root = if side == 0 { low } else { high };
                                let previous = below
                                    .and_then(|b| b.get(&(column, side)))
                                    .into_iter()
                                    .flatten()
                                    .copied()
                                    .find(|t| *t > low && *t < high);
                                let tip = match (previous, side) {
                                    (Some(p), 0) => want.min(p + advance),
                                    (Some(p), _) => want.max(p - advance),
                                    (None, 0) => want.min(root + s),
                                    (None, _) => want.max(root - s),
                                };
                                if (tip - root).abs() >= s - 1e-12 {
                                    grown.entry((column, side)).or_default().push(tip);
                                }
                            }
                        }
                    }
                }
            }
        }
        actual[index] = grown;
    }
    // Teeth of the two families must not cross: family 1 stops one spacing
    // short of any family 0 tooth in its room.
    for (index, tips) in actual.iter_mut().enumerate() {
        for domain in &rooms[index] {
            let s = domain.s;
            for room in &domain.rooms {
                let vertical: Vec<(f64, f64, f64)> = columns(room, 0, s, spacing_mm)
                    .into_iter()
                    .flat_map(|c| {
                        let column = Column {
                            region: domain.region_index,
                            family: 0,
                            coordinate: c.to_bits(),
                        };
                        let mut strips = Vec::new();
                        for (low, high) in chords(room, 0, c) {
                            for side in 0..2u8 {
                                for &t in tips.get(&(column, side)).into_iter().flatten() {
                                    if t > low && t < high {
                                        let root = if side == 0 { low } else { high };
                                        strips.push((c, root.min(t), root.max(t)));
                                    }
                                }
                            }
                        }
                        strips
                    })
                    .collect();
                if vertical.is_empty() {
                    continue;
                }
                for d in columns(room, 1, s, spacing_mm) {
                    let column = Column {
                        region: domain.region_index,
                        family: 1,
                        coordinate: d.to_bits(),
                    };
                    for (low, high) in chords(room, 1, d) {
                        for side in 0..2u8 {
                            let Some(list) = tips.get_mut(&(column, side)) else {
                                continue;
                            };
                            for t in list.iter_mut().filter(|t| **t > low && **t < high) {
                                let root = if side == 0 { low } else { high };
                                for &(c, y0, y1) in &vertical {
                                    if d < y0 - s || d > y1 + s {
                                        continue;
                                    }
                                    if side == 0 && c > root && *t > c - 1.5 * s {
                                        *t = c - 1.5 * s;
                                    }
                                    if side == 1 && c < root && *t < c + 1.5 * s {
                                        *t = c + 1.5 * s;
                                    }
                                }
                            }
                            list.retain(|t| {
                                if !(*t > low && *t < high) {
                                    return true;
                                }
                                let root = if side == 0 { low } else { high };
                                (t - root).abs() >= s - 1e-12
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(actual)
}

/// Roads of a layer that a roof must carry, per family: family 0 holds roads
/// running mostly along X as `(y at a given x, x from, x to)` segments, and
/// family 1 roads along Y as `(x, y from, y to)`.
fn roof_roads(layer: &PrintLayer) -> [Vec<(f64, f64, f64)>; 2] {
    let mut out = [Vec::new(), Vec::new()];
    for path in layer.paths.iter().filter(|p| {
        matches!(
            p.kind,
            DepositionKind::TopSkin
                | DepositionKind::BottomSkin
                | DepositionKind::SolidFill
                | DepositionKind::Bridge
        )
    }) {
        for edge in path.points_mm.windows(2) {
            let (dx, dy) = (edge[1][0] - edge[0][0], edge[1][1] - edge[0][1]);
            if dx.abs() >= dy.abs() {
                let (a, b) = if dx >= 0.0 {
                    (edge[0], edge[1])
                } else {
                    (edge[1], edge[0])
                };
                out[0].push((0.5 * (a[1] + b[1]), a[0], b[0]));
            } else {
                let (a, b) = if dy >= 0.0 {
                    (edge[0], edge[1])
                } else {
                    (edge[1], edge[0])
                };
                out[1].push((0.5 * (a[0] + b[0]), a[1], b[1]));
            }
        }
    }
    out
}

/// One Cathedral road before emission: closed iff its ends coincide.
struct Road {
    points: Vec<Coord<f64>>,
    /// A room's outer loop, which may be spliced to its neighbours.
    exterior: bool,
}

/// One room's roads: its loops with their teeth, or its spine.
fn room_roads(
    room: &Room,
    tips: &Tips,
    domain: &DomainRooms,
    out: &mut Vec<Road>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let s = domain.s;
    let mut teeth: Vec<Tooth> = Vec::new();
    for family in 0..2 {
        let positions: Vec<f64> = tips
            .keys()
            .filter(|(c, _)| c.region == domain.region_index && c.family == family as u8)
            .map(|(c, _)| f64::from_bits(c.coordinate))
            .collect();
        for c in positions {
            let column = Column {
                region: domain.region_index,
                family: family as u8,
                coordinate: c.to_bits(),
            };
            for (low, high) in chords(room, family, c) {
                for side in 0..2u8 {
                    for &tip in tips.get(&(column, side)).into_iter().flatten() {
                        if tip > low && tip < high {
                            teeth.push(Tooth {
                                family,
                                c,
                                root: if side == 0 { low } else { high },
                                tip,
                            });
                        }
                    }
                }
            }
        }
    }
    teeth.sort_by(|a, b| {
        (a.family, a.c, a.root)
            .partial_cmp(&(b.family, b.c, b.root))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    teeth.dedup_by(|a, b| a.family == b.family && a.c == b.c && a.root == b.root);
    for (index, ring) in room.rings.iter().enumerate() {
        check_cancel(cancel)?;
        if room.open {
            out.push(Road {
                points: ring.clone(),
                exterior: false,
            });
            continue;
        }
        let points = drop_short_edges(insert_teeth(ring, &teeth, s));
        if points.len() >= 4 {
            out.push(Road {
                points,
                exterior: index == 0,
            });
        }
    }
    Ok(())
}

/// Coordinate tolerance for recognising a shared wall's two parallel edges.
const WALL_MATCH_MM: f64 = 1e-3;

/// An axis-aligned straight edge of an outer loop.
struct WallEdge {
    road: usize,
    /// 0: the edge runs along Y at a fixed X; 1: along X at a fixed Y.
    axis: usize,
    fixed: f64,
    low: f64,
    high: f64,
}

/// A splice between two loops across their shared wall: the window
/// `[along, along + s]` on the edges at `fixed` (first loop) and `fixed + s`.
struct Splice {
    roads: [usize; 2],
    axis: usize,
    fixed: f64,
    along: f64,
    room: f64,
}

/// Merge neighbouring room loops into one loop per connected set (RFC 12
/// section 5). Neighbouring loops run parallel `s` apart along their shared
/// wall in opposite directions; a splice removes a window of length `s` from
/// both edges and joins them by two crossings of the wall. The crossings are
/// `s` apart, closer than a bead width, so the wall stays closed, and both
/// rest on the wall below. A spanning tree chooses the splices, longest
/// shared walls first; windows alternate along the wall between layers, so
/// splices on walls long enough to separate them do not stack.
fn splice(
    roads: Vec<Road>,
    domain: &DomainRooms,
    layer: usize,
    cancel: &AtomicBool,
) -> Result<Vec<Road>, ManufacturingError> {
    let s = domain.s;
    // Only an edge s/2 below a recorded lattice line faces a shared wall.
    let on_wall = |axis: usize, fixed: f64| {
        domain.lines[axis]
            .iter()
            .any(|c| (fixed + 0.5 * s - c).abs() <= WALL_MATCH_MM)
    };
    let mut loops: Vec<Option<Vec<Coord<f64>>>> = Vec::new();
    let mut rest = Vec::new();
    for road in roads {
        if road.exterior && road.points.first() == road.points.last() {
            let mut points = road.points;
            if signed_area(&points) < 0.0 {
                points.reverse();
            }
            loops.push(Some(points));
        } else {
            rest.push(road);
        }
    }
    if loops.len() > 1 {
        let mut edges = Vec::new();
        for (road, points) in loops.iter().enumerate() {
            let points = points.as_ref().expect("unspliced loop");
            for e in points.windows(2) {
                if (e[0].x - e[1].x).abs() <= WALL_MATCH_MM {
                    edges.push(WallEdge {
                        road,
                        axis: 0,
                        fixed: 0.5 * (e[0].x + e[1].x),
                        low: e[0].y.min(e[1].y),
                        high: e[0].y.max(e[1].y),
                    });
                } else if (e[0].y - e[1].y).abs() <= WALL_MATCH_MM {
                    edges.push(WallEdge {
                        road,
                        axis: 1,
                        fixed: 0.5 * (e[0].y + e[1].y),
                        low: e[0].x.min(e[1].x),
                        high: e[0].x.max(e[1].x),
                    });
                }
            }
        }
        edges.sort_by(|a, b| {
            (a.axis, a.fixed)
                .partial_cmp(&(b.axis, b.fixed))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let t = if layer.is_multiple_of(2) { 0.25 } else { 0.75 };
        let mut candidates = Vec::new();
        for (i, a) in edges.iter().enumerate() {
            check_cancel(cancel)?;
            let target = a.fixed + s;
            for b in &edges[i + 1..] {
                if b.axis != a.axis || b.fixed > target + WALL_MATCH_MM {
                    break;
                }
                if b.road == a.road || b.fixed < target - WALL_MATCH_MM || !on_wall(a.axis, a.fixed)
                {
                    continue;
                }
                // Keep one spacing clear of corners, pins and tooth legs.
                let low = a.low.max(b.low) + s;
                let high = a.high.min(b.high) - s;
                if high - low < s {
                    continue;
                }
                candidates.push(Splice {
                    roads: [a.road, b.road],
                    axis: a.axis,
                    fixed: a.fixed,
                    along: low + t * (high - low - s),
                    room: high - low,
                });
            }
        }
        candidates.sort_by(|a, b| {
            b.room
                .total_cmp(&a.room)
                .then(a.axis.cmp(&b.axis))
                .then(a.fixed.total_cmp(&b.fixed))
                .then(a.along.total_cmp(&b.along))
        });
        let mut parent: Vec<usize> = (0..loops.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for candidate in &candidates {
            check_cancel(cancel)?;
            let [ra, rb] = candidate.roads.map(|r| find(&mut parent, r));
            if ra == rb {
                continue;
            }
            let (a, b) = (
                loops[ra].as_ref().expect("live loop"),
                loops[rb].as_ref().expect("live loop"),
            );
            if let Some(merged) = join(a, b, candidate, s) {
                loops[rb] = None;
                loops[ra] = Some(merged);
                parent[rb] = ra;
            }
        }
    }
    Ok(loops
        .into_iter()
        .flatten()
        .map(|points| Road {
            points,
            exterior: true,
        })
        .chain(rest)
        .collect())
}

fn signed_area(points: &[Coord<f64>]) -> f64 {
    points
        .windows(2)
        .map(|e| e[0].x * e[1].y - e[1].x * e[0].y)
        .sum::<f64>()
        * 0.5
}

/// Splice loop `b` (beyond the wall) into loop `a` at the candidate window.
fn join(a: &[Coord<f64>], b: &[Coord<f64>], splice: &Splice, s: f64) -> Option<Vec<Coord<f64>>> {
    let axis = splice.axis;
    let fixed_of = |p: &Coord<f64>| if axis == 0 { p.x } else { p.y };
    let along_of = |p: &Coord<f64>| if axis == 0 { p.y } else { p.x };
    let at = |along: f64, fixed: f64| {
        if splice.axis == 0 {
            Coord { x: fixed, y: along }
        } else {
            Coord { x: along, y: fixed }
        }
    };
    let (v0, v1) = (splice.along, splice.along + s);
    // The edge of `ring` on `fixed` that holds the whole window, and its
    // direction along the wall.
    let window = |ring: &[Coord<f64>], fixed: f64| {
        ring.windows(2).enumerate().find_map(|(k, e)| {
            let on = |p: &Coord<f64>| (fixed_of(p) - fixed).abs() <= WALL_MATCH_MM;
            let (p, q) = (along_of(&e[0]), along_of(&e[1]));
            (on(&e[0]) && on(&e[1]) && p.min(q) < v0 && p.max(q) > v1).then_some((k, q > p))
        })
    };
    let (fa, fb) = (splice.fixed, splice.fixed + s);
    let (ka, up_a) = window(a, fa)?;
    let (kb, up_b) = window(b, fb)?;
    if up_a == up_b {
        return None;
    }
    let (first_a, second_a) = if up_a { (v0, v1) } else { (v1, v0) };
    let first_b = if up_b { v0 } else { v1 };
    let (n, m) = (a.len() - 1, b.len() - 1);
    let mut out = Vec::with_capacity(n + m + 5);
    out.extend((0..n).map(|t| a[(ka + 1 + t) % n]));
    out.push(at(first_a, fa));
    out.push(at(first_a, fb));
    out.extend((0..m).map(|t| b[(kb + 1 + t) % m]));
    out.push(at(first_b, fb));
    out.push(at(second_a, fa));
    out.push(out[0]);
    Some(out)
}

fn emit(
    points: &[Coord<f64>],
    region_index: u16,
    physical_z_mm: [f64; 2],
    width: f64,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
) -> Result<(), ManufacturingError> {
    let road: Vec<[f64; 3]> = points
        .iter()
        .map(|p| [p.x, p.y, physical_z_mm[1]])
        .collect();
    let additional = road.len() - 1;
    if additional > crate::working_set::units().saturating_sub(*segments) {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "retained deposition segment limit exceeded",
        ));
    }
    *segments += additional;
    paths.push(DepositionPath {
        material_domain: None,
        kind: DepositionKind::SparseFill,
        width: WidthProfile::Constant { width_mm: width },
        physical_z_mm,
        points_mm: road,
        arcs: Vec::new(),
        connectors: Vec::new(),
        owner: Some(DepositionOwner::Regional { region_index }),
        exposed: false,
    });
    Ok(())
}

struct Tooth {
    family: usize,
    c: f64,
    root: f64,
    tip: f64,
}

/// A point on a ring: edge index plus parameter, and its position.
#[derive(Clone, Copy)]
struct OnRing {
    at: f64,
    point: Coord<f64>,
}

fn crossings(ring: &[Coord<f64>], family: usize, c: f64) -> Vec<OnRing> {
    let (fixed, along) = if family == 0 { (0, 1) } else { (1, 0) };
    let get = |p: &Coord<f64>, axis: usize| if axis == 0 { p.x } else { p.y };
    let mut out = Vec::new();
    for (i, edge) in ring.windows(2).enumerate() {
        let (a, b) = (get(&edge[0], fixed), get(&edge[1], fixed));
        if (a <= c && c < b) || (b <= c && c < a) {
            let t = (c - a) / (b - a);
            let v = get(&edge[0], along) + t * (get(&edge[1], along) - get(&edge[0], along));
            let point = if family == 0 {
                Coord { x: c, y: v }
            } else {
                Coord { x: v, y: c }
            };
            out.push(OnRing {
                at: i as f64 + t,
                point,
            });
        }
    }
    out
}

/// Replace, for each tooth rooted on this ring, the short ring arc between
/// its two leg lines by the tooth: leg, tip, leg.
fn insert_teeth(ring: &[Coord<f64>], teeth: &[Tooth], s: f64) -> Vec<Coord<f64>> {
    let edges = ring.len() - 1;
    let along = |p: &Coord<f64>, family: usize| if family == 0 { p.y } else { p.x };
    // (start, end, replacement) with start < end in ring parameter.
    let mut cuts: Vec<(f64, f64, Vec<Coord<f64>>)> = Vec::new();
    for tooth in teeth {
        let root = crossings(ring, tooth.family, tooth.c)
            .into_iter()
            .find(|hit| {
                (along(&hit.point, tooth.family) - tooth.root).abs()
                    <= 1e-9 * (1.0 + tooth.root.abs())
            });
        let Some(root) = root else {
            continue;
        };
        let nearest = |line: f64| {
            crossings(ring, tooth.family, line)
                .into_iter()
                .min_by(|a, b| {
                    ring_distance(a.at, root.at, edges)
                        .total_cmp(&ring_distance(b.at, root.at, edges))
                })
        };
        let (Some(a), Some(b)) = (nearest(tooth.c - 0.5 * s), nearest(tooth.c + 0.5 * s)) else {
            continue;
        };
        if ring_distance(a.at, root.at, edges) > 2.0 || ring_distance(b.at, root.at, edges) > 2.0 {
            continue;
        }
        // Both legs must start near the root, on the side the tooth grows from.
        let grows_up = tooth.tip > tooth.root;
        let ok = |p: &OnRing| {
            let v = along(&p.point, tooth.family);
            (v - tooth.root).abs() <= s
                && if grows_up {
                    tooth.tip - v >= 0.5 * s
                } else {
                    v - tooth.tip >= 0.5 * s
                }
        };
        if !ok(&a) || !ok(&b) {
            continue;
        }
        // Order the cut forward along the ring from the first leg met.
        let (first, second) = if forward(a.at, root.at, edges) <= forward(a.at, b.at, edges)
            && forward(a.at, b.at, edges) < edges as f64 * 0.5
        {
            (a, b)
        } else {
            (b, a)
        };
        if forward(first.at, second.at, edges) >= edges as f64 * 0.5 {
            continue;
        }
        let tip_point = |p: &OnRing| {
            if tooth.family == 0 {
                Coord {
                    x: p.point.x,
                    y: tooth.tip,
                }
            } else {
                Coord {
                    x: tooth.tip,
                    y: p.point.y,
                }
            }
        };
        let replacement = vec![
            first.point,
            tip_point(&first),
            tip_point(&second),
            second.point,
        ];
        let (start, mut end) = (first.at, second.at);
        if end < start {
            end += edges as f64;
        }
        cuts.push((start, end, replacement));
    }
    if cuts.is_empty() {
        return ring.to_vec();
    }
    // Rotate so no cut wraps: start the walk just after the largest gap.
    cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut kept: Vec<(f64, f64, Vec<Coord<f64>>)> = Vec::new();
    for cut in cuts {
        if kept.last().is_some_and(|last| cut.0 <= last.1) {
            continue;
        }
        kept.push(cut);
    }
    if kept.len() > 1 && kept[kept.len() - 1].1 >= kept[0].0 + edges as f64 {
        kept.pop();
    }
    // Walk once around the ring, starting where the last cut ends.
    let total = edges as f64;
    let start = kept[kept.len() - 1].1.rem_euclid(total);
    let point_at = |at: f64| {
        let at = at.rem_euclid(total);
        let i = (at.floor() as usize).min(edges - 1);
        let t = at - i as f64;
        Coord {
            x: ring[i].x + t * (ring[i + 1].x - ring[i].x),
            y: ring[i].y + t * (ring[i + 1].y - ring[i].y),
        }
    };
    let mut events: Vec<(f64, f64, &Vec<Coord<f64>>)> = kept
        .iter()
        .map(|(a, b, r)| {
            let (mut a, mut b) = (*a, *b);
            while a < start {
                a += total;
                b += total;
            }
            (a, b, r)
        })
        .collect();
    events.sort_by(|x, y| x.0.total_cmp(&y.0));
    let finish = start + total;
    let mut out = vec![point_at(start)];
    let mut cursor = start;
    for (a, b, replacement) in events {
        if b > finish + 1e-9 {
            continue;
        }
        let mut vertex = cursor.floor() + 1.0;
        while vertex < a {
            out.push(point_at(vertex));
            vertex += 1.0;
        }
        out.extend(replacement.iter().copied());
        cursor = b;
    }
    let mut vertex = cursor.floor() + 1.0;
    while vertex < finish {
        out.push(point_at(vertex));
        vertex += 1.0;
    }
    out.push(out[0]);
    out
}

fn forward(from: f64, to: f64, edges: usize) -> f64 {
    (to - from).rem_euclid(edges as f64)
}

fn ring_distance(a: f64, b: f64, edges: usize) -> f64 {
    let d = forward(a, b, edges);
    d.min(edges as f64 - d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f64) -> MultiPolygon<f64> {
        MultiPolygon(vec![Rect::new([0.0, 0.0], [size, size]).to_polygon()])
    }

    #[test]
    fn rooms_are_closed_loops_half_a_spacing_inside_each_cell() {
        let cancel = AtomicBool::new(false);
        let (w, h) = (0.45, 0.2);
        let s = spacing(w, h);
        let pitch = cell_pitch(w, h, 0.15);
        assert!((pitch - 10.86).abs() < 0.01, "{pitch}");
        let frame = [[0.0, 0.0], [40.0, 40.0]];
        let rooms = cells(&square(40.0), frame, pitch, s, w, &cancel).unwrap();
        let lines = lattice(frame, pitch);
        assert_eq!(rooms.len(), (lines[0].len() + 1) * (lines[1].len() + 1));
        for room in &rooms {
            for ring in &room.rings {
                assert_eq!(ring.first(), ring.last());
                for p in ring {
                    // Every vertex is s/2 from a lattice line or pinned at a
                    // junction, or s/2 inside the domain boundary.
                    let wall = lines[0]
                        .iter()
                        .map(|x| ((p.x - x).abs() - 0.5 * s).abs())
                        .chain(lines[1].iter().map(|y| ((p.y - y).abs() - 0.5 * s).abs()))
                        .fold(f64::INFINITY, f64::min);
                    let boundary = [p.x, p.y, 40.0 - p.x, 40.0 - p.y]
                        .into_iter()
                        .map(|d| (d - 0.5 * s).abs())
                        .fold(f64::INFINITY, f64::min);
                    let pinned = lines[0].iter().any(|x| {
                        lines[1].iter().any(|y| {
                            ((p.x - x).hypot(p.y - y) - JUNCTION_PIN * 0.5 * w).abs() < 1e-9
                        })
                    });
                    assert!(wall < 1e-6 || boundary < 1e-3 || pinned, "{p:?}");
                }
            }
        }
    }

    #[test]
    fn a_tooth_replaces_the_root_arc_with_two_legs_and_a_tip() {
        let s = 0.4;
        let ring = vec![
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 10.0, y: 0.0 },
            Coord { x: 10.0, y: 10.0 },
            Coord { x: 0.0, y: 10.0 },
            Coord { x: 0.0, y: 0.0 },
        ];
        let teeth = [
            Tooth {
                family: 0,
                c: 5.0,
                root: 0.0,
                tip: 3.0,
            },
            Tooth {
                family: 0,
                c: 5.0,
                root: 10.0,
                tip: 6.0,
            },
        ];
        let out = drop_short_edges(insert_teeth(&ring, &teeth, s));
        assert_eq!(out.first(), out.last());
        let has = |x: f64, y: f64| {
            out.iter()
                .any(|p| (p.x - x).abs() < 1e-12 && (p.y - y).abs() < 1e-12)
        };
        assert!(has(4.8, 0.0) && has(4.8, 3.0) && has(5.2, 3.0) && has(5.2, 0.0));
        assert!(has(5.2, 10.0) && has(5.2, 6.0) && has(4.8, 6.0) && has(4.8, 10.0));
        // Still a simple closed loop of the original corners plus 8 tooth points.
        assert_eq!(out.len(), 5 + 8);
    }

    #[test]
    fn splices_join_every_room_of_a_region_into_one_loop() {
        let cancel = AtomicBool::new(false);
        let (w, h) = (0.45, 0.2);
        let s = spacing(w, h);
        let pitch = cell_pitch(w, h, 0.15);
        let frame = [[0.0, 0.0], [40.0, 40.0]];
        let rooms = cells(&square(40.0), frame, pitch, s, w, &cancel).unwrap();
        let domain = DomainRooms {
            region_index: 0,
            s,
            lines: lattice(frame, pitch),
            rooms,
        };
        let total: f64 = domain
            .rooms
            .iter()
            .map(|room| ring_length(&room.rings[0]))
            .sum();
        let mut windows = Vec::new();
        for layer in 0..2 {
            let mut roads = Vec::new();
            for room in &domain.rooms {
                room_roads(room, &BTreeMap::new(), &domain, &mut roads, &cancel).unwrap();
            }
            let joined = splice(roads, &domain, layer, &cancel).unwrap();
            assert_eq!(joined.len(), 1);
            let ring = &joined[0].points;
            assert_eq!(ring.first(), ring.last());
            // Each splice removes 2s of wall and adds two crossings of s.
            assert!((ring_length(ring) - total).abs() < 1e-6);
            // Crossings span exactly one wall: s long, across a lattice line.
            let lines = lattice(frame, pitch);
            let crossings: Vec<[f64; 2]> = ring
                .windows(2)
                .filter_map(|e| {
                    let (dx, dy) = (e[1].x - e[0].x, e[1].y - e[0].y);
                    let mid = [0.5 * (e[0].x + e[1].x), 0.5 * (e[0].y + e[1].y)];
                    let across = |d: f64, at: f64, axis: usize| {
                        (d.abs() - s).abs() < 1e-5
                            && lines[axis].iter().any(|c| (at - c).abs() < 1e-5)
                    };
                    (across(dx, mid[0], 0) && dy.abs() < 1e-12
                        || across(dy, mid[1], 1) && dx.abs() < 1e-12)
                        .then_some(mid)
                })
                .collect();
            assert_eq!(crossings.len(), 2 * (domain.rooms.len() - 1));
            windows.push(crossings);
        }
        // Splice windows move between layers so they never stack.
        assert!(windows[0].iter().all(|a| !windows[1].contains(a)));
    }

    #[test]
    fn a_strip_too_narrow_for_a_loop_carries_spines() {
        let cancel = AtomicBool::new(false);
        let (w, h) = (0.45, 0.2);
        let s = spacing(w, h);
        let pitch = cell_pitch(w, h, 0.15);
        // 0.6 wide: room for one bead centerline, not for a loop. The frame
        // puts one lattice line across the strip at x = 4 and none along it.
        let strip = MultiPolygon(vec![Rect::new([0.0, 0.0], [8.0, 0.6]).to_polygon()]);
        let frame = [[0.0, -10.0], [8.0, 30.0]];
        assert_eq!(lattice(frame, pitch)[0], vec![4.0]);
        let rooms = cells(&strip, frame, pitch, s, w, &cancel).unwrap();
        assert_eq!(rooms.len(), 2);
        for room in &rooms {
            assert!(room.open);
            let spine = &room.rings[0];
            assert_eq!(spine.len(), 2);
            for p in spine {
                assert!((p.y - 0.3).abs() < 1e-6, "{p:?}");
                // Inside the strip and s/2 clear of the wall line at x = 4.
                assert!(
                    p.x >= 0.5 * s - 1e-6 && p.x <= 8.0 - 0.5 * s + 1e-6,
                    "{p:?}"
                );
                assert!((p.x - 4.0).abs() >= 0.5 * s - 1e-6, "{p:?}");
            }
            assert!(columns(room, 0, s, 1.0).is_empty());
        }
    }
}
