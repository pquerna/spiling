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
//! covers the crossing: an explicit 2x2 pier.
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
use geo::{Area, BooleanOps, BoundingRect, Coord, LineString, MultiPolygon, Polygon, Rect};
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
        for (index, layer) in layers.iter_mut().enumerate() {
            check_cancel(cancel)?;
            let cathedral = |path: &DepositionPath| {
                path.kind == DepositionKind::SparseFill
                    && matches!(path.owner, Some(DepositionOwner::Regional { .. }))
            };
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
                for room in &domain.rooms {
                    room_paths(
                        room,
                        &teeth[index],
                        domain,
                        [layer.lower_z_mm, layer.z_mm],
                        recipe.bead_width_mm,
                        &mut replacement,
                        segments,
                        cancel,
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
        rooms: cells(fill, roof.frame_mm, pitch, s, layer.width_mm, cancel)?,
    };
    let empty = BTreeMap::new();
    for room in &domain.rooms {
        room_paths(
            room,
            &empty,
            &domain,
            [layer.lower_z, layer.z],
            layer.width_mm,
            paths,
            segments,
            cancel,
        )?;
    }
    Ok(())
}

/// One room component: its loops (exterior first) in the frame-clipped
/// lattice cell `bounds`.
pub(super) struct Room {
    bounds: [[f64; 2]; 2],
    rings: Vec<Vec<Coord<f64>>>,
}

struct DomainRooms {
    region_index: u16,
    s: f64,
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
                if around <= 0.0 || 2.0 * polygon.unsigned_area() / around < s {
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
                    });
                }
            }
        }
    }
    Ok(rooms)
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

/// Emit one room's loops with their teeth.
#[allow(clippy::too_many_arguments)]
fn room_paths(
    room: &Room,
    tips: &Tips,
    domain: &DomainRooms,
    physical_z_mm: [f64; 2],
    width: f64,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
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
    for ring in &room.rings {
        check_cancel(cancel)?;
        let points = insert_teeth(ring, &teeth, s);
        let points = drop_short_edges(points);
        if points.len() < 4 {
            continue;
        }
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
            owner: Some(DepositionOwner::Regional {
                region_index: domain.region_index,
            }),
            exposed: false,
        });
    }
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
}
