// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use geo::algorithm::coordinate_position::{CoordPos, coord_pos_relative_to_ring};
use geo::{BoundingRect, LineString, Rect};
use spiling_contracts::manufacturing::program_coordinates::MIN_DEPOSITION_COMMAND_MM;

#[cfg(test)]
mod tests;

pub(super) struct Inward {
    pub(super) address: Ear,
    pub(super) point: Coord<f64>,
}

const OPTIONAL_RING_BOUNDS: usize = 96;
const SKIPPED_EDGES_PER_CHECK: usize = 1024;

#[cfg_attr(test, derive(Clone))]
struct RingBounds {
    entries: Vec<Option<Rect<f64>>>,
    current_len: usize,
}

enum RingBook {
    Unprepared,
    Declined,
    Ready(RingBounds),
}

impl RingBounds {
    fn prepare(
        current: &MultiPolygon<f64>,
        original: &MultiPolygon<f64>,
        retained: usize,
        owned_bytes: usize,
        cancel: &AtomicBool,
    ) -> Option<Self> {
        let owned_bytes = owned_bytes.checked_add(std::mem::size_of::<RingBook>())?;
        super::super::super::super::remaining_geometry_units(retained, owned_bytes).ok()?;
        let mut bounds = Self {
            entries: Vec::new(),
            current_len: 0,
        };
        super::super::super::reserve_owned(&mut bounds.entries, OPTIONAL_RING_BOUNDS, owned_bytes)
            .ok()?;
        super::super::super::super::remaining_geometry_units(
            retained,
            owned_bytes.checked_add(super::super::super::vec_bytes(&bounds.entries))?,
        )
        .ok()?;
        bounds.append(current, cancel)?;
        bounds.current_len = bounds.entries.len();
        if !std::ptr::eq(current, original) {
            bounds.append(original, cancel)?;
        }
        Some(bounds)
    }

    fn append(&mut self, domain: &MultiPolygon<f64>, cancel: &AtomicBool) -> Option<()> {
        for ring in rings(domain) {
            if self.entries.len() == OPTIONAL_RING_BOUNDS {
                return None;
            }
            let entry = finite_ring_bounds(ring, cancel);
            check_cancel(cancel).ok()?;
            self.entries.push(entry);
        }
        Some(())
    }
}

fn rings(domain: &MultiPolygon<f64>) -> impl Iterator<Item = &LineString<f64>> {
    domain
        .0
        .iter()
        .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
}

fn finite_ring_bounds(ring: &LineString<f64>, cancel: &AtomicBool) -> Option<Rect<f64>> {
    let mut min = *ring.0.first()?;
    let mut max = min;
    for chunk in ring.0.chunks(64) {
        check_cancel(cancel).ok()?;
        for &point in chunk {
            if !point.x.is_finite() || !point.y.is_finite() {
                return None;
            }
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
        }
    }
    Some(Rect::new(min, max))
}

struct Search<'a> {
    current: &'a MultiPolygon<f64>,
    original: &'a MultiPolygon<f64>,
    interface: Option<&'a MultiPolygon<f64>>,
    retained: usize,
    owned_bytes: usize,
    work: &'a mut usize,
    foreign_work: usize,
    cancel: &'a AtomicBool,
    cuts: Vec<(f64, Coord<f64>)>,
    pocket: geo::LineString<f64>,
    ring_book: RingBook,
}

impl Search<'_> {
    fn charge(&mut self, count: usize) -> Result<(), ManufacturingError> {
        charge(self.work, count, self.foreign_work, self.cancel)
    }

    fn short(
        &mut self,
        edge: [Coord<f64>; 2],
        cached: &mut Option<bool>,
    ) -> Result<bool, ManufacturingError> {
        self.charge(1)?;
        if !commands::command_length(edge[0], edge[1])? {
            return Ok(true);
        }
        match self.interface {
            Some(interface) => short_role_piece(
                edge,
                interface,
                cached,
                &mut self.cuts,
                self.retained,
                self.owned_bytes,
                self.work,
                self.foreign_work,
                self.cancel,
                || {
                    if matches!(self.ring_book, RingBook::Ready(_)) {
                        self.ring_book = RingBook::Unprepared;
                    }
                },
            ),
            None => Ok(false),
        }
    }

    fn segment_inside(
        &mut self,
        domain: &MultiPolygon<f64>,
        [first, last]: [Coord<f64>; 2],
    ) -> Result<bool, ManufacturingError> {
        self.charge(region::point_count(domain))?;
        if !domain.contains(&Point::new(
            first.x * 0.5 + last.x * 0.5,
            first.y * 0.5 + last.y * 0.5,
        )) {
            return Ok(false);
        }
        if matches!(self.ring_book, RingBook::Unprepared) {
            let live = self
                .owned_bytes
                .saturating_add(super::super::super::vec_bytes(&self.cuts));
            self.ring_book = RingBounds::prepare(
                self.current,
                self.original,
                self.retained,
                live,
                self.cancel,
            )
            .map_or(RingBook::Declined, RingBook::Ready);
        }
        let bounds = match &self.ring_book {
            RingBook::Ready(bounds) if std::ptr::eq(domain, self.current) => {
                &bounds.entries[..bounds.current_len]
            }
            RingBook::Ready(bounds) if std::ptr::eq(domain, self.original) => {
                &bounds.entries[bounds.current_len..]
            }
            _ => &[],
        };
        let segment = Line::new(first, last);
        let query = (!bounds.is_empty()
            && [first.x, first.y, last.x, last.y]
                .into_iter()
                .all(f64::is_finite))
        .then(|| segment.bounding_rect());
        for (ordinal, ring) in rings(domain).enumerate() {
            let lanes = ring.0.len().saturating_sub(1);
            if lanes != 0
                && query
                    .zip(bounds.get(ordinal).copied().flatten())
                    .is_some_and(|(query, ring)| !query.intersects(&ring))
                && *self.work >= self.foreign_work
                && self.work.checked_add(lanes).is_some()
            {
                let mut remaining = lanes;
                while remaining != 0 {
                    let amount = remaining.min(SKIPPED_EDGES_PER_CHECK);
                    charge(self.work, amount, self.foreign_work, self.cancel)?;
                    remaining -= amount;
                }
                continue;
            }
            for edge in ring.0.windows(2) {
                charge(self.work, 1, self.foreign_work, self.cancel)?;
                match line_intersection(segment, Line::new(edge[0], edge[1])) {
                    None => {}
                    Some(LineIntersection::SinglePoint { intersection, .. })
                        if intersection == first || intersection == last => {}
                    _ => return Ok(false),
                }
            }
        }
        Ok(true)
    }

    fn admissible(
        &mut self,
        address: &Ear,
        [previous, at, next]: [Coord<f64>; 3],
        point: Coord<f64>,
    ) -> Result<bool, ManufacturingError> {
        self.charge(1)?;
        if !point.x.is_finite()
            || !point.y.is_finite()
            || RobustKernel::orient2d(previous, at, point) != Orientation::CounterClockwise
            || RobustKernel::orient2d(at, next, point) != Orientation::CounterClockwise
            || RobustKernel::orient2d(previous, point, next) == Orientation::Collinear
        {
            return Ok(false);
        }
        for edge in [[previous, point], [point, next]] {
            if self.short(edge, &mut None)?
                || !self.segment_inside(self.current, edge)?
                || (!std::ptr::eq(self.current, self.original)
                    && !self.segment_inside(self.original, edge)?)
            {
                return Ok(false);
            }
        }
        self.charge(5)?;
        self.pocket.0.clear();
        self.pocket
            .0
            .extend_from_slice(&[previous, at, next, point, previous]);
        let current = self.current;
        for (polygon_index, polygon) in current.0.iter().enumerate() {
            for (ring_index, ring) in std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .enumerate()
            {
                let count = ring.0.len() - 1;
                for (vertex, coordinate) in ring.0[..count].iter().enumerate() {
                    self.charge(5)?;
                    if polygon_index == address.polygon
                        && ring_index == address.ring
                        && [
                            (address.vertex + count - 1) % count,
                            address.vertex,
                            (address.vertex + 1) % count,
                        ]
                        .contains(&vertex)
                    {
                        continue;
                    }
                    if coord_pos_relative_to_ring(*coordinate, &self.pocket) != CoordPos::Outside {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Optional coordinate search retains original material, original role ownership and all live work/resource limits independently"
)]
pub(super) fn find(
    current: &MultiPolygon<f64>,
    original: &MultiPolygon<f64>,
    interface: Option<&MultiPolygon<f64>>,
    constraints: &mut [[Option<bool>; 2]],
    retained: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<Option<Inward>, ManufacturingError> {
    charge(work, 1, foreign_work, cancel)?;
    let mut pocket = geo::LineString::new(Vec::new());
    super::super::super::super::remaining_geometry_units(retained.saturating_add(5), owned_bytes)?;
    super::super::super::reserve_owned(&mut pocket.0, 5, owned_bytes)?;
    let retained = retained.saturating_add(pocket.0.capacity());
    let owned_bytes = owned_bytes.saturating_add(super::super::super::vec_bytes(&pocket.0));
    super::super::super::super::remaining_geometry_units(retained, owned_bytes)?;
    let mut search = Search {
        current,
        original,
        interface,
        retained,
        owned_bytes,
        work,
        foreign_work,
        cancel,
        cuts: Vec::new(),
        pocket,
        ring_book: RingBook::Unprepared,
    };
    for priority in [2, 1] {
        let mut ring_start = 0;
        for (polygon_index, polygon) in current.0.iter().enumerate() {
            for (ring_index, ring) in std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .enumerate()
            {
                search.charge(1)?;
                let count = ring.0.len() - 1;
                if count <= 3 {
                    ring_start += count;
                    continue;
                }
                for vertex in 0..count {
                    search.charge(1)?;
                    let previous = ring.0[(vertex + count - 1) % count];
                    let at = ring.0[vertex];
                    let next = ring.0[(vertex + 1) % count];
                    if RobustKernel::orient2d(previous, at, next) == Orientation::CounterClockwise
                        || !commands::command_length(previous, next)?
                    {
                        continue;
                    }
                    let address = Ear {
                        polygon: polygon_index,
                        ring: ring_index,
                        vertex,
                        constraint: ring_start + vertex,
                        previous_constraint: ring_start + (vertex + count - 1) % count,
                    };
                    let mut bad = 0;
                    for (edge, index) in [
                        ([previous, at], address.previous_constraint),
                        ([at, next], address.constraint),
                    ] {
                        let mut uncached = None;
                        let cached = if interface.is_some() {
                            &mut constraints[index][0]
                        } else {
                            &mut uncached
                        };
                        bad += usize::from(search.short(edge, cached)?);
                    }
                    if bad != priority {
                        continue;
                    }
                    let incoming = [at.x - previous.x, at.y - previous.y];
                    let outgoing = [next.x - at.x, next.y - at.y];
                    let incoming_length = incoming[0].hypot(incoming[1]);
                    let outgoing_length = outgoing[0].hypot(outgoing[1]);
                    let normal = [
                        -incoming[1] / incoming_length - outgoing[1] / outgoing_length,
                        incoming[0] / incoming_length + outgoing[0] / outgoing_length,
                    ];
                    let normal_length = normal[0].hypot(normal[1]);
                    if !normal_length.is_finite() || normal_length <= 0.0 {
                        continue;
                    }
                    for scale in [0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0] {
                        let distance = MIN_DEPOSITION_COMMAND_MM * scale;
                        let point = Coord {
                            x: at.x + normal[0] / normal_length * distance,
                            y: at.y + normal[1] / normal_length * distance,
                        };
                        if search.admissible(&address, [previous, at, next], point)? {
                            return Ok(Some(Inward { address, point }));
                        }
                    }
                }
                ring_start += count;
            }
        }
    }
    Ok(None)
}
