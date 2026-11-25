use super::clearance::{Admission, Road, Rows, debit};
use super::{Chord, MAX_XYZ6_TICKS, Parameter, delta, failed, overflow};
use crate::planner::{OFFSET_TOLERANCE_MM, support::StrokeRuns};
use rayon::prelude::*;
use spiling_contracts::manufacturing::{
    DepositionKind, ManufacturingError, PlanarPrintRecipe, PrintLayer,
    program_coordinates::{enclosed_vertical_gap_mm, project_xyz6_bead_height_mm},
};
use std::sync::atomic::AtomicBool;

const TICKS_PER_MM: f64 = 1_000_000.0;
const FRACTION_TICKS: u64 = 1 << 53;

#[derive(Clone, Copy)]
struct Span {
    start: f64,
    end: f64,
}

impl Span {
    const FULL: Self = Self {
        start: 0.0,
        end: 1.0,
    };

    fn inner_parameters(self) -> [Parameter; 2] {
        [
            (self.start * FRACTION_TICKS as f64).ceil() as u64,
            (self.end * FRACTION_TICKS as f64).floor() as u64,
        ]
        .map(|numerator| Parameter {
            numerator,
            denominator: FRACTION_TICKS,
        })
    }

    fn indexed_chord(self, chord: Chord) -> Result<(Chord, i64), ManufacturingError> {
        if self.start == 0.0 && self.end == 1.0 {
            return Ok((chord, 0));
        }
        let scale = i128::from(FRACTION_TICKS);
        let point = |fraction: f64| {
            let numerator = (fraction * FRACTION_TICKS as f64).floor() as i128;
            std::array::from_fn(|axis| {
                let offset = (chord.vector[axis] * numerator).div_euclid(scale) as i64;
                chord.start[axis] + offset
            })
        };
        Ok((Chord::new(point(self.start), point(self.end))?, 3))
    }
}

fn support(role: DepositionKind) -> bool {
    matches!(
        role,
        DepositionKind::Support | DepositionKind::SupportInterface
    )
}

struct Donor {
    road: Road,
    span: Span,
}

struct Inventory {
    rows: Rows,
    donors: Vec<Donor>,
}

impl Inventory {
    fn empty() -> Self {
        Self {
            rows: Rows::default(),
            donors: Vec::new(),
        }
    }

    fn release(self, admission: &mut Admission) {
        self.rows.release(admission);
        admission.release(self.donors);
    }
}

struct Band {
    deposited: Option<Inventory>,
    landing: Option<Inventory>,
}

struct LayerWindow {
    top: f64,
    separated_end: usize,
    /// First layer close enough below for a support root to stand on.
    landing_start: usize,
    road_end: usize,
    reads_landing: bool,
    deposited_read: DepositedRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DepositedRead {
    Unused,
    SeparatedModel,
    ContinuingSupport,
    DirectModel,
}

impl DepositedRead {
    fn reads(self, role: DepositionKind) -> bool {
        match self {
            Self::Unused => false,
            Self::SeparatedModel => role == DepositionKind::SupportInterface,
            Self::ContinuingSupport => support(role),
            Self::DirectModel => true,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Bond {
    #[default]
    None,
    Separated,
    Direct,
}

struct Window<'a> {
    previous: Option<&'a Band>,
    separated: &'a [Band],
    z_gap: f64,
    previous_nominal_top: Option<f64>,
}

impl Window<'_> {
    fn for_each(
        &self,
        receiver: Road,
        mut visit: impl FnMut(&Inventory, Bond) -> Result<bool, ManufacturingError>,
    ) -> Result<bool, ManufacturingError> {
        if self.previous_nominal_top == Some(receiver.floor)
            && let Some(inventory) = self.previous.and_then(|band| band.deposited.as_ref())
            && visit(inventory, Bond::Direct)?
        {
            return Ok(true);
        }
        for band in self.separated {
            let inventory = if support(receiver.role) {
                &band.landing
            } else {
                &band.deposited
            };
            if let Some(inventory) = inventory
                && visit(inventory, Bond::Separated)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn admits(&self, receiver: Road, donor: Road, bond: Bond) -> Result<bool, ManufacturingError> {
        if bond == Bond::Direct {
            return Ok(!support(receiver.role) || support(donor.role));
        }
        let eligible = if support(receiver.role) {
            receiver.role == DepositionKind::SupportInterface
                && donor.role.is_model()
                && donor.role != DepositionKind::Bridge
        } else {
            donor.role == DepositionKind::SupportInterface
        };
        Ok(
            eligible
                && enclosed_vertical_gap_mm(receiver.height[0], donor.height[1])? >= self.z_gap,
        )
    }
}

fn radius_ticks(radius_mm: f64, erosion_mm: f64) -> f64 {
    ((radius_mm - erosion_mm).next_down() * TICKS_PER_MM).next_down()
}

fn outside_ticks(radius_mm: f64) -> Result<i64, ManufacturingError> {
    let ticks = (radius_mm.next_up() * TICKS_PER_MM).next_up().ceil();
    if !(0.0..=MAX_XYZ6_TICKS as f64).contains(&ticks) {
        return Err(overflow());
    }
    Ok(ticks as i64)
}

#[derive(Clone, Copy)]
struct LocalCapsule {
    radius: f64,
    bounds: [f64; 2],
    caps: [bool; 2],
    origin: [f64; 2],
    change: [f64; 2],
    slack: f64,
}

impl LocalCapsule {
    fn for_pair(receiver: Road, donor: &Donor) -> Option<Self> {
        let chord = donor.road.chord;
        let [run, rise] = chord.vector.map(|coordinate| coordinate as f64);
        let length = run.hypot(rise);
        if length == 0.0 {
            return None;
        }
        let axes = [run / length, rise / length];
        let transform = |vector: [i128; 2]| {
            let [x, y] = vector.map(|coordinate| coordinate as f64);
            [axes[0] * x + axes[1] * y, axes[0] * y - axes[1] * x]
        };
        let origin = transform(delta(chord.start, receiver.chord.start));
        let change = transform(receiver.chord.vector);
        let radius = radius_ticks(
            donor.road.inner_radius_mm,
            (donor.road.arc_deviation_mm + receiver.arc_deviation_mm).next_up(),
        );
        let slack = 128.0
            * f64::EPSILON
            * origin[0]
                .abs()
                .max(origin[1].abs())
                .max(change[0].abs())
                .max(change[1].abs())
                .max(length)
                .max(radius)
                .max(1.0);
        let caps = [donor.span.start == 0.0, donor.span.end == 1.0];
        let bounds = [
            if caps[0] {
                0.0
            } else {
                donor.span.start * length + slack
            },
            if caps[1] {
                length
            } else {
                donor.span.end * length - slack
            },
        ];
        (radius > slack && bounds[0] < bounds[1]).then_some(Self {
            radius,
            bounds,
            caps,
            origin,
            change,
            slack,
        })
    }

    fn interval(self) -> Option<Span> {
        let radius = self.radius.next_down() - self.slack;
        let mut result = None;
        let mut strip = Span::FULL;
        if restrict(&mut strip, self.origin[0], self.change[0], self.bounds)
            && restrict(
                &mut strip,
                self.origin[1],
                self.change[1],
                [-radius, radius],
            )
        {
            include(&mut result, strip);
        }
        let length = self.change[0].hypot(self.change[1]);
        let direction = self.change.map(|coordinate| coordinate / length);
        for (end, cap) in self.caps.into_iter().enumerate() {
            if !cap {
                continue;
            }
            let center = self.bounds[end];
            let rel = [center - self.origin[0], -self.origin[1]];
            let projection = direction[0] * rel[0] + direction[1] * rel[1];
            let perpendicular = (direction[0] * rel[1] - direction[1] * rel[0]).abs();
            if perpendicular >= radius {
                continue;
            }
            let half = ((radius + perpendicular) * (radius - perpendicular)).sqrt();
            let half = (half - self.slack).max(0.0);
            let mut cap_span = Span {
                start: (((projection - half + self.slack) / length).next_up()).max(0.0),
                end: (((projection + half - self.slack) / length).next_down()).min(1.0),
            };
            let region = [
                if self.caps[0] {
                    f64::NEG_INFINITY
                } else {
                    self.bounds[0]
                },
                if self.caps[1] {
                    f64::INFINITY
                } else {
                    self.bounds[1]
                },
            ];
            if restrict(&mut cap_span, self.origin[0], self.change[0], region) {
                include(&mut result, cap_span);
            }
        }
        result
    }

    fn endpoint(self, end: usize, radius: f64) -> bool {
        let point = if end == 0 {
            self.origin
        } else {
            [
                self.origin[0] + self.change[0],
                self.origin[1] + self.change[1],
            ]
        };
        let across = point[1].abs();
        let reach = self.radius - self.slack;
        let side = (across - reach).max(0.0);
        let along = point[0] - point[0].clamp(self.bounds[0], self.bounds[1]);
        let mut distance = along.hypot(side);
        for (cap, outside) in self
            .caps
            .into_iter()
            .zip([self.bounds[0] - point[0], point[0] - self.bounds[1]])
        {
            if cap && outside >= 0.0 {
                distance = distance.min((outside.hypot(across) - reach).max(0.0));
            }
        }
        distance + self.slack < radius
    }
}

fn restrict(span: &mut Span, origin: f64, change: f64, bounds: [f64; 2]) -> bool {
    if change == 0.0 {
        return bounds[0] <= origin && origin <= bounds[1] && span.start < span.end;
    }
    let first = (bounds[0] - origin) / change;
    let last = (bounds[1] - origin) / change;
    span.start = span.start.max(first.min(last).next_up());
    span.end = span.end.min(first.max(last).next_down());
    span.start < span.end
}

fn include(result: &mut Option<Span>, span: Span) {
    if span.start < span.end {
        *result = Some(result.map_or(span, |prior| Span {
            start: prior.start.min(span.start),
            end: prior.end.max(span.end),
        }));
    }
}

struct Replay<'a> {
    admission: Admission,
    work: &'a mut usize,
    cancel: &'a AtomicBool,
    covered: Vec<Span>,
    run: Vec<Span>,
    unsupported: Vec<Span>,
    /// Donors already measured by the current query, by inventory and ordinal.
    seen: Vec<(usize, usize)>,
}

/// Deposited roads rasterize in chunks of this many, one buffer per chunk.
const RASTER_CHUNK: usize = 256;

/// One chunk's row incidences: road `k` of the chunk owns
/// `rows[ends[k - 1]..ends[k]]`; `refusal` is the first road that failed.
#[derive(Default)]
struct Rasterized {
    rows: Vec<[i64; 3]>,
    ends: Vec<usize>,
    refusal: Option<ManufacturingError>,
}

impl Rasterized {
    fn new(
        roads: impl Iterator<Item = Road>,
        mut admission: Admission,
        cancel: &AtomicBool,
    ) -> Self {
        let mut chunk = Self::default();
        for road in roads {
            let rows = &mut chunk.rows;
            let result = outside_ticks(road.inner_radius_mm)
                .and_then(|radius| {
                    road.chord
                        .outer_rows(radius, &mut 0, cancel, |row, left, right| {
                            admission.push(rows, [row, left, right])
                        })
                })
                .and_then(|()| admission.push(&mut chunk.ends, chunk.rows.len()));
            if let Err(error) = result {
                chunk.refusal = Some(error);
                break;
            }
        }
        chunk
    }

    fn take(&mut self, local: usize) -> Result<&[[i64; 3]], ManufacturingError> {
        match self.ends.get(local) {
            Some(&end) => {
                let start = local.checked_sub(1).map_or(0, |prior| self.ends[prior]);
                Ok(&self.rows[start..end])
            }
            None => Err(self.refusal.take().expect("rasterized road or its refusal")),
        }
    }
}

/// A worker thread's reusable query buffers and their admitted bytes.
#[derive(Default)]
struct Scratch {
    admission: usize,
    covered: Vec<Span>,
    run: Vec<Span>,
    unsupported: Vec<Span>,
    seen: Vec<(usize, usize)>,
}

/// One support or landing stroke run: `count` edges of the path whose roads
/// start at `start` in the layer, beginning at edge `first`.
#[derive(Clone, Copy)]
struct Measure {
    start: usize,
    len: usize,
    first: usize,
    count: usize,
    width: f64,
}

impl Measure {
    fn roads(self, current: &[Road]) -> impl Iterator<Item = Road> + Clone + '_ {
        let roads = &current[self.start..self.start + self.len];
        (0..self.count).map(move |edge| roads[(self.first + edge) % roads.len()])
    }
}

impl<'a> Replay<'a> {
    fn new(admission: Admission, work: &'a mut usize, cancel: &'a AtomicBool) -> Self {
        Self {
            admission,
            work,
            cancel,
            covered: Vec::new(),
            run: Vec::new(),
            unsupported: Vec::new(),
            seen: Vec::new(),
        }
    }

    fn query(
        &mut self,
        window: &Window<'_>,
        receiver: Road,
    ) -> Result<[Bond; 2], ManufacturingError> {
        self.covered.clear();
        self.seen.clear();
        let mut ends = [Bond::None; 2];
        if !window.for_each(receiver, |inventory, _| Ok(!inventory.donors.is_empty()))? {
            debit(self.work, 1, self.cancel)?;
            return Ok(ends);
        }
        let full = receiver
            .chord
            .walk_while(self.work, self.cancel, |cell, _, work| {
                let [column, row] = cell.0;
                window.for_each(receiver, |inventory, bond| {
                    if bond != Bond::Direct {
                        return Ok(false);
                    }
                    for entry in inventory.rows.intersect(row, column, column) {
                        debit(work, 1, self.cancel)?;
                        let donor = &inventory.donors[entry.road];
                        if entry.right < column || !window.admits(receiver, donor.road, bond)? {
                            continue;
                        }
                        let radius = radius_ticks(
                            donor.road.inner_radius_mm,
                            (donor.road.arc_deviation_mm + receiver.arc_deviation_mm).next_up(),
                        )
                        .floor();
                        if (1.0..=MAX_XYZ6_TICKS as f64).contains(&radius)
                            && donor.road.chord.cell_within_span(
                                cell,
                                i128::from(radius as i64).pow(2),
                                donor.span.inner_parameters(),
                            )
                        {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                })
            })?;
        if full {
            self.admission.push(&mut self.covered, Span::FULL)?;
            return Ok([Bond::Direct; 2]);
        }

        debit(self.work, 1, self.cancel)?;
        let radius = radius_ticks(receiver.inner_radius_mm, 0.0).max(0.0);
        receiver.chord.row_candidates(
            outside_ticks(receiver.inner_radius_mm)?,
            self.cancel,
            |row, left, right, _| {
                debit(self.work, 1, self.cancel)?;
                window.for_each(receiver, |inventory, bond| {
                    for entry in inventory.rows.intersect(row, left, right) {
                        debit(self.work, 1, self.cancel)?;
                        let donor = &inventory.donors[entry.road];
                        if entry.right < left || !window.admits(receiver, donor.road, bond)? {
                            continue;
                        }
                        let key = (std::ptr::from_ref(inventory).addr(), entry.road);
                        if self.seen.contains(&key) {
                            continue;
                        }
                        self.admission.push(&mut self.seen, key)?;
                        if let Some(capsule) = LocalCapsule::for_pair(receiver, donor) {
                            if let Some(span) = capsule.interval() {
                                self.admission.push(&mut self.covered, span)?;
                            }
                            for (end, present) in ends.iter_mut().enumerate() {
                                if *present < bond && capsule.endpoint(end, radius) {
                                    *present = bond;
                                }
                            }
                        }
                    }
                    Ok(false)
                })?;
                Ok(())
            },
        )?;
        sort_coverage(&mut self.covered, self.work, self.cancel)?;
        Ok(ends)
    }

    fn insert(
        &mut self,
        inventory: &mut Inventory,
        road: Road,
        span: Span,
    ) -> Result<(), ManufacturingError> {
        debit(self.work, 1, self.cancel)?;
        let ordinal = inventory.donors.len();
        let (indexed, rounding) = span.indexed_chord(road.chord)?;
        self.admission
            .push(&mut inventory.donors, Donor { road, span })?;
        inventory.rows.insert(
            ordinal,
            indexed,
            outside_ticks(road.inner_radius_mm)?
                .checked_add(rounding)
                .ok_or_else(overflow)?,
            &mut self.admission,
            self.work,
            self.cancel,
        )
    }

    /// `insert` of a full road whose incidences `rasterize` built.
    fn insert_rows(
        &mut self,
        inventory: &mut Inventory,
        road: Road,
        rows: Result<&[[i64; 3]], ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        debit(self.work, 1, self.cancel)?;
        let ordinal = inventory.donors.len();
        self.admission.push(
            &mut inventory.donors,
            Donor {
                road,
                span: Span::FULL,
            },
        )?;
        // The task that built `rows` admitted them; this inventory admits its copy.
        inventory.rows.extend(ordinal, rows?, &mut self.admission)
    }

    fn measure_stroke(
        &mut self,
        window: &Window<'_>,
        roads: impl Iterator<Item = Road>,
        width: f64,
        limit: f64,
    ) -> Result<(), ManufacturingError> {
        self.run.clear();
        self.unsupported.clear();
        let mut length = 0.0;
        let mut ends = [Bond::None; 2];
        for (index, road) in roads.enumerate() {
            let edge_ends = self.query(window, road)?;
            if index == 0 {
                ends[0] = edge_ends[0];
            }
            ends[1] = edge_ends[1];
            for coverage in &self.covered {
                debit(self.work, 1, self.cancel)?;
                self.admission.push(
                    &mut self.run,
                    Span {
                        start: length + coverage.start * road.length_upper_mm,
                        end: length + coverage.end * road.length_upper_mm,
                    },
                )?;
            }
            length += road.length_upper_mm;
            if !length.is_finite() {
                return Err(overflow());
            }
        }
        let accepts = |span: f64, left: bool, right: bool| {
            (left || right)
                && (span <= 2.0 * OFFSET_TOLERANCE_MM
                    || (!(left && right) && span <= width * 0.5 + 2.0 * OFFSET_TOLERANCE_MM))
                || left && right && span <= limit
        };
        let mut covered = 0.0_f64;
        for coverage in &self.run {
            debit(self.work, 1, self.cancel)?;
            if coverage.start > covered
                && !accepts(
                    coverage.start - covered,
                    covered > 0.0 || ends[0] != Bond::None,
                    true,
                )
            {
                self.admission.push(
                    &mut self.unsupported,
                    Span {
                        start: covered,
                        end: coverage.start,
                    },
                )?;
            }
            covered = covered.max(coverage.end);
        }
        if covered < length
            && !accepts(
                length - covered,
                covered > 0.0 || ends[0] != Bond::None,
                ends[1] != Bond::None,
            )
        {
            self.admission.push(
                &mut self.unsupported,
                Span {
                    start: covered,
                    end: length,
                },
            )?;
        }
        Ok(())
    }

    fn publish_landing(
        &mut self,
        inventory: &mut Inventory,
        roads: impl Iterator<Item = Road>,
        unsupported: &[Span],
    ) -> Result<(), ManufacturingError> {
        let mut offset = 0.0;
        let mut gap = 0;
        for road in roads {
            debit(self.work, 1, self.cancel)?;
            let length = road.length_upper_mm;
            let end = offset + length;
            let mut start = 0.0;
            while let Some(&excluded) = unsupported.get(gap).filter(|span| span.start < end) {
                debit(self.work, 1, self.cancel)?;
                let stop = ((excluded.start - offset) / length).next_down();
                if start < stop {
                    self.insert(inventory, road, Span { start, end: stop })?;
                }
                if excluded.end >= end {
                    start = 1.0;
                    break;
                }
                start = ((excluded.end - offset) / length).next_up().max(start);
                gap += 1;
            }
            if start < 1.0 {
                self.insert(inventory, road, Span { start, end: 1.0 })?;
            }
            offset = end;
        }
        Ok(())
    }
}

fn sort_coverage(
    coverage: &mut [Span],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut comparisons = 0usize;
    coverage.sort_unstable_by(|first, second| {
        comparisons = comparisons.saturating_add(1);
        first
            .start
            .total_cmp(&second.start)
            .then(first.end.total_cmp(&second.end))
    });
    debit(work, comparisons, cancel)
}

fn separated_end(
    windows: &[LayerWindow],
    floor: f64,
    gap: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let (mut start, mut end) = (0, windows.len());
    while start < end {
        debit(work, 1, cancel)?;
        let middle = start + (end - start) / 2;
        if enclosed_vertical_gap_mm(floor, windows[middle].top)? >= gap {
            start = middle + 1;
        } else {
            end = middle;
        }
    }
    Ok(start)
}

fn layer_windows(
    roads: &[Road],
    layers: &[PrintLayer],
    z_gap: f64,
    max_separation: f64,
    admission: &mut Admission,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<LayerWindow>, ManufacturingError> {
    let mut windows = Vec::new();
    admission.reserve(&mut windows, layers.len())?;
    let mut road_start = 0;
    let mut previous_end = 0;
    for (index, layer) in layers.iter().enumerate() {
        debit(work, 1, cancel)?;
        let [floor, top] = project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm)?;
        let end = separated_end(&windows, floor, z_gap, work, cancel)?;
        let road_end =
            road_start + roads[road_start..].partition_point(|road| road.layer as usize == index);
        // A support root may stand on any part top separated by the Z gap up
        // to the planner's bead-width band, not only the newly separated one.
        let landing_start =
            separated_end(&windows, floor, max_separation.next_up(), work, cancel)?.min(end);
        let start = previous_end.min(landing_start);
        let mut reads_continuation = false;
        for road in &roads[road_start..road_end] {
            debit(work, 1, cancel)?;
            if !support(road.role) {
                continue;
            }
            reads_continuation = true;
            if start == end || road.role == DepositionKind::SupportInterface {
                for donor in &mut windows[start..end] {
                    debit(work, 1, cancel)?;
                    donor.reads_landing = true;
                }
                break;
            }
        }
        if reads_continuation && let Some(previous) = windows.last_mut() {
            previous.deposited_read = DepositedRead::ContinuingSupport;
        }
        windows.push(LayerWindow {
            top,
            separated_end: end,
            landing_start,
            road_end,
            reads_landing: false,
            deposited_read: DepositedRead::Unused,
        });
        road_start = road_end;
        previous_end = end;
    }
    for index in 1..windows.len() {
        debit(work, 1, cancel)?;
        if windows[index].reads_landing {
            let contact = windows[index].separated_end.checked_sub(1);
            windows[index - 1].deposited_read = DepositedRead::DirectModel;
            if let Some(contact) = contact {
                let read = &mut windows[contact].deposited_read;
                *read = (*read).max(DepositedRead::SeparatedModel);
            }
        }
    }
    Ok(windows)
}

pub(super) fn verify(
    roads: &[Road],
    layers: &[PrintLayer],
    recipe: &PlanarPrintRecipe,
    z_gap: f64,
    admission: Admission,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut replay = Replay::new(admission, work, cancel);
    let working_set = crate::working_set::current();
    let limit = recipe.process.max_bridge_span_mm;
    let mut measures = Vec::<Measure>::new();
    let mut measured = Vec::<Result<Vec<Span>, ManufacturingError>>::new();
    let mut deposits = Vec::<usize>::new();
    let mut rasterized = Vec::<Rasterized>::new();
    let mut last_support = None;
    for road in roads.iter().rev() {
        debit(replay.work, 1, cancel)?;
        if support(road.role) {
            last_support = Some(road.layer as usize);
            break;
        }
    }
    let Some(last_support) = last_support else {
        return Ok(());
    };
    let selected_layers = layers
        .get(..=last_support)
        .ok_or_else(|| failed("decoded support layer is outside the verified schedule"))?;
    let windows = layer_windows(
        roads,
        selected_layers,
        z_gap,
        crate::planner::support::part_root_max_separation(z_gap, recipe),
        &mut replay.admission,
        replay.work,
        cancel,
    )?;
    let mut bands = Vec::<Band>::new();
    replay
        .admission
        .reserve(&mut bands, selected_layers.len())?;
    let mut road_start = 0;
    let mut expired = 0;
    let mut last_separated_end = 0;
    for (index, (layer, use_window)) in selected_layers.iter().zip(&windows).enumerate() {
        debit(replay.work, 1, cancel)?;
        let current = &roads[road_start..use_window.road_end];
        road_start = use_window.road_end;
        let end = use_window.separated_end;
        let support_start = last_separated_end.min(use_window.landing_start);
        let contact_index = end.saturating_sub(1);
        let oldest = support_start
            .min(contact_index)
            .min(index.saturating_sub(1));
        if oldest < expired {
            return Err(failed(
                "support ancestry schedule reverses its contact window",
            ));
        }
        for band in &mut bands[expired..oldest] {
            debit(replay.work, 1, cancel)?;
            if let Some(inventory) = band.deposited.take() {
                inventory.release(&mut replay.admission);
            }
            if let Some(inventory) = band.landing.take() {
                inventory.release(&mut replay.admission);
            }
        }
        expired = oldest;
        let mut deposited =
            (use_window.deposited_read != DepositedRead::Unused).then(Inventory::empty);
        let mut landing = Inventory::empty();
        let previous = bands.last();
        let previous_nominal_top = index.checked_sub(1).map(|prior| layers[prior].z_mm);
        let window = |role: DepositionKind| Window {
            previous,
            previous_nominal_top,
            z_gap,
            separated: match role {
                DepositionKind::SupportInterface => &bands[support_start..end],
                DepositionKind::Support => &[],
                _ => &bands[contact_index..end],
            },
        };
        let can_land = |role: DepositionKind| {
            role.is_model() && role != DepositionKind::Bridge && use_window.reads_landing
        };
        // A stroke reads only earlier layers' bands, so a layer's strokes
        // measure in parallel. Path validation, landing and deposited inserts
        // stay serial in path order, and the first error in that order is the
        // one reported.
        measures.clear();
        let mut refusal = None;
        let mut start = 0;
        for path_roads in current.chunk_by(|left, right| left.path == right.path) {
            debit(replay.work, 1, cancel)?;
            let path_start = start;
            start += path_roads.len();
            let first_road = path_roads[0];
            let is_support = support(first_road.role);
            let ordinary_model =
                first_road.role.is_model() && first_road.role != DepositionKind::Bridge;
            if index == 0 || index == last_support && !is_support || !(is_support || ordinary_model)
            {
                continue;
            }
            let land = can_land(first_road.role);
            let path = &layer.paths[first_road.path];
            let runs = if path_roads.len() + 1 != path.points_mm.len() {
                Err(failed(
                    "decoded support stroke differs from its associated edge count",
                ))
            } else if !is_support && !land {
                path.checked_edges().map(|_| None)
            } else {
                StrokeRuns::for_path(path).map(Some)
            };
            let runs = match runs {
                Ok(runs) => runs,
                Err(error) => {
                    refusal = Some((path_start, error));
                    break;
                }
            };
            for (first, last, width) in runs.into_iter().flatten() {
                let count = if last > first {
                    last - first
                } else {
                    path_roads.len() - first + last
                };
                replay.admission.push(
                    &mut measures,
                    Measure {
                        start: path_start,
                        len: path_roads.len(),
                        first,
                        count,
                        width,
                    },
                )?;
            }
        }
        deposits.clear();
        if deposited.is_some() && index != last_support {
            for (position, road) in current.iter().enumerate() {
                if use_window.deposited_read.reads(road.role) {
                    replay.admission.push(&mut deposits, position)?;
                }
            }
        }
        replay.admission.reserve(&mut measured, measures.len())?;
        replay
            .admission
            .reserve(&mut rasterized, deposits.len().div_ceil(RASTER_CHUNK))?;
        let base = Admission(replay.admission.0);
        let rasterize = || {
            deposits
                .par_chunks(RASTER_CHUNK)
                .map(|chunk| {
                    crate::working_set::scope(working_set, || {
                        let roads = chunk.iter().map(|&position| current[position]);
                        Rasterized::new(roads, Admission(base.0), cancel)
                    })
                })
                .collect_into_vec(&mut rasterized)
        };
        let measure = || {
            measures
                .par_iter()
                .map_init(
                    || Scratch {
                        admission: base.0,
                        ..Scratch::default()
                    },
                    |scratch, &measure| {
                        crate::working_set::scope(working_set, || {
                            let mut work = 0;
                            let mut task = Replay {
                                admission: Admission(scratch.admission),
                                work: &mut work,
                                cancel,
                                covered: std::mem::take(&mut scratch.covered),
                                run: std::mem::take(&mut scratch.run),
                                unsupported: std::mem::take(&mut scratch.unsupported),
                                seen: std::mem::take(&mut scratch.seen),
                            };
                            let road = current[measure.start];
                            let result = task
                                .measure_stroke(
                                    &window(road.role),
                                    measure.roads(current),
                                    measure.width,
                                    limit,
                                )
                                .and_then(|()| {
                                    let mut unsupported = Vec::new();
                                    if can_land(road.role) {
                                        task.admission
                                            .reserve(&mut unsupported, task.unsupported.len())?;
                                        unsupported.extend_from_slice(&task.unsupported);
                                    } else if !task.unsupported.is_empty() {
                                        // A support stroke refuses on any gap.
                                        task.admission
                                            .push(&mut unsupported, task.unsupported[0])?;
                                    }
                                    Ok(unsupported)
                                });
                            scratch.admission = task.admission.0;
                            scratch.covered = task.covered;
                            scratch.run = task.run;
                            scratch.unsupported = task.unsupported;
                            scratch.seen = task.seen;
                            result
                        })
                    },
                )
                .collect_into_vec(&mut measured)
        };
        rayon::join(measure, rasterize);
        let mut next = 0;
        let mut next_deposit = 0;
        let mut start = 0;
        for path_roads in current.chunk_by(|left, right| left.path == right.path) {
            let path_start = start;
            start += path_roads.len();
            if refusal.as_ref().is_some_and(|(at, _)| *at == path_start) {
                return Err(refusal.take().expect("checked refusal").1);
            }
            let first_road = path_roads[0];
            let is_support = support(first_road.role);
            if index == last_support && !is_support {
                continue;
            }
            let land = can_land(first_road.role);
            while let Some(&measure) = measures.get(next).filter(|m| m.start == path_start) {
                let unsupported = std::mem::replace(&mut measured[next], Ok(Vec::new()))?;
                next += 1;
                if is_support && !unsupported.is_empty() {
                    let mm = |point: [i64; 2]| point.map(|tick| tick as f64 * 1e-6);
                    let mut stroke = measure.roads(current);
                    let from = stroke.next().map(|road| mm(road.chord.start));
                    let to = measure.roads(current).last().map(|road| mm(road.chord.end));
                    let gap = unsupported[0];
                    return Err(failed(format!(
                        "actual support layer {index} (line {}) has no established bed, continuing support or earlier-part contact: {:?} at Z {:.3} from {from:?} to {to:?}, {} unsupported intervals (first {:.3}..{:.3} mm along), separated bands {}..{}",
                        first_road.line,
                        first_road.role,
                        layer.z_mm,
                        unsupported.len(),
                        gap.start,
                        gap.end,
                        support_start,
                        end
                    )));
                }
                if land {
                    replay.publish_landing(&mut landing, measure.roads(current), &unsupported)?;
                }
            }
            if index != last_support {
                for &road in path_roads {
                    if use_window.deposited_read.reads(road.role)
                        && let Some(inventory) = &mut deposited
                    {
                        let rows = rasterized[next_deposit / RASTER_CHUNK]
                            .take(next_deposit % RASTER_CHUNK);
                        next_deposit += 1;
                        replay.insert_rows(inventory, road, rows)?;
                    }
                    if index == 0 && land {
                        replay.insert(&mut landing, road, Span::FULL)?;
                    }
                }
            }
        }
        measured.clear();
        rasterized.clear();
        if let Some(inventory) = &mut deposited {
            inventory
                .rows
                .finish_ancestry(&mut replay.admission, replay.work, cancel)?;
        }
        if use_window.reads_landing {
            landing
                .rows
                .finish_ancestry(&mut replay.admission, replay.work, cancel)?;
        }
        bands.push(Band {
            deposited,
            landing: use_window.reads_landing.then_some(landing),
        });
        last_separated_end = end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verifier::grid::RoadInput;
    use spiling_contracts::manufacturing::{DepositionPath, ManufacturingErrorCode, WidthProfile};

    const Z_STEP: f64 = 0.25;
    const Z_GAP: f64 = 0.125;

    fn road(layer: u32, path: usize, role: DepositionKind, from: [i64; 2], to: [i64; 2]) -> Road {
        let top = (i64::from(layer) + 1) * 250_000;
        Road::new(RoadInput {
            layer,
            path,
            line: path + 1,
            role,
            from: [from[0], from[1], top],
            to: [to[0], to[1], top],
            physical_z: [f64::from(layer) * Z_STEP, (f64::from(layer) + 1.0) * Z_STEP],
            maximum_width_mm: 0.4,
            minimum_width_mm: 0.4,
            length_upper_mm: ((to[0] - from[0]) as f64).hypot((to[1] - from[1]) as f64)
                / TICKS_PER_MM,
            arc_deviation_mm: 0.0,
        })
        .unwrap()
    }

    fn row(layer: u32, path: usize, role: DepositionKind, start: i64, end: i64) -> Road {
        road(layer, path, role, [start, 0], [end, 0])
    }

    fn fixture(roads: &[Road]) -> (PlanarPrintRecipe, Vec<PrintLayer>) {
        let (intent, plan) = crate::verifier::tests::fixture();
        let mut layers = Vec::new();
        for index in 0..=roads.last().unwrap().layer {
            let mut layer = plan.layers[0].clone();
            layer.index = index;
            layer.lower_z_mm = f64::from(index) * Z_STEP;
            layer.z_mm = layer.lower_z_mm + Z_STEP;
            layer.height_mm = Z_STEP;
            layer.paths.clear();
            for road in roads.iter().filter(|road| road.layer == index) {
                assert_eq!(layer.paths.len(), road.path);
                layer.paths.push(DepositionPath {
                    kind: road.role,
                    width: WidthProfile::Constant {
                        width_mm: road.inner_radius_mm * 2.0,
                    },
                    physical_z_mm: [layer.lower_z_mm, layer.z_mm],
                    points_mm: [road.chord.start, road.chord.end]
                        .map(|point| {
                            [
                                point[0] as f64 / TICKS_PER_MM,
                                point[1] as f64 / TICKS_PER_MM,
                                layer.z_mm,
                            ]
                        })
                        .into(),
                    material_domain: None,
                    owner: None,
                    exposed: false,
                    arcs: Vec::new(),
                    connectors: Vec::new(),
                });
            }
            layers.push(layer);
        }
        (intent.recipe, layers)
    }

    #[test]
    fn only_separated_interface_readers_request_landing_bands() {
        let roads = [
            row(0, 0, DepositionKind::SolidFill, 0, 1_000_000),
            row(1, 0, DepositionKind::SolidFill, 0, 1_000_000),
            row(2, 0, DepositionKind::Support, 0, 1_000_000),
            row(3, 0, DepositionKind::SupportInterface, 0, 1_000_000),
            row(4, 0, DepositionKind::Support, 0, 1_000_000),
        ];
        let (_, layers) = fixture(&roads);
        let windows = layer_windows(
            &roads,
            &layers,
            Z_STEP,
            Z_STEP,
            &mut Admission::default(),
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            windows
                .iter()
                .map(|window| window.reads_landing)
                .collect::<Vec<_>>(),
            [false, true, false, false, false]
        );
        assert_eq!(windows[3].separated_end, 2);
    }

    #[test]
    fn raw_donor_bands_keep_only_roles_reachable_by_their_readers() {
        let roads = [
            row(0, 0, DepositionKind::SolidFill, 0, 1_000_000),
            row(1, 0, DepositionKind::SupportInterface, 0, 1_000_000),
            row(2, 0, DepositionKind::Bridge, 0, 1_000_000),
            row(3, 0, DepositionKind::SolidFill, 0, 1_000_000),
            row(4, 0, DepositionKind::SolidFill, 0, 1_000_000),
            row(5, 0, DepositionKind::SupportInterface, 0, 1_000_000),
            row(6, 0, DepositionKind::Support, 0, 1_000_000),
        ];
        let (_, layers) = fixture(&roads);
        let windows = layer_windows(
            &roads,
            &layers,
            Z_STEP,
            Z_STEP,
            &mut Admission::default(),
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        use DepositedRead::{ContinuingSupport, DirectModel, SeparatedModel, Unused};
        assert_eq!(
            windows
                .iter()
                .map(|window| window.deposited_read)
                .collect::<Vec<_>>(),
            [
                ContinuingSupport,
                SeparatedModel,
                DirectModel,
                Unused,
                ContinuingSupport,
                ContinuingSupport,
                Unused,
            ]
        );
        assert!(DirectModel.reads(DepositionKind::Bridge));
        assert!(!ContinuingSupport.reads(DepositionKind::SolidFill));
        assert!(ContinuingSupport.reads(DepositionKind::SupportInterface));
        assert!(!SeparatedModel.reads(DepositionKind::Support));
        assert!(SeparatedModel.reads(DepositionKind::SupportInterface));
    }

    #[test]
    fn ordinary_road_donates_both_flanks_but_not_its_failed_middle() {
        let mut roads = [
            row(0, 0, DepositionKind::SolidFill, -2_000_000, -1_000_000),
            row(0, 1, DepositionKind::SolidFill, 1_000_000, 2_000_000),
            row(1, 0, DepositionKind::SolidFill, -2_000_000, 2_000_000),
            row(
                3,
                0,
                DepositionKind::SupportInterface,
                -1_900_000,
                -1_100_000,
            ),
            row(3, 1, DepositionKind::SupportInterface, 1_100_000, 1_900_000),
        ];
        let cancel = AtomicBool::new(false);
        let (mut recipe, layers) = fixture(&roads);
        recipe.process.max_bridge_span_mm = 1.0;
        verify(
            &roads,
            &layers,
            &recipe,
            Z_GAP,
            Admission::default(),
            &mut 0,
            &cancel,
        )
        .unwrap();
        roads[4] = row(3, 1, DepositionKind::SupportInterface, -150_000, 150_000);
        let failure = verify(
            &roads,
            &layers,
            &recipe,
            Z_GAP,
            Admission::default(),
            &mut 0,
            &cancel,
        )
        .unwrap_err();
        assert!(
            failure
                .message
                .starts_with("actual support layer 3 (line 2)")
        );
        roads[4] = row(3, 1, DepositionKind::SupportInterface, 1_100_000, 1_900_000);
        roads[1] = road(
            0,
            1,
            DepositionKind::SolidFill,
            [1_000_000, 1_000_000],
            [2_000_000, 1_000_000],
        );
        let failure = verify(
            &roads,
            &layers,
            &recipe,
            Z_GAP,
            Admission::default(),
            &mut 0,
            &cancel,
        )
        .unwrap_err();
        assert!(
            failure
                .message
                .starts_with("actual support layer 3 (line 2)")
        );
    }

    #[test]
    fn bridge_counts_as_deposition_but_cannot_land_support() {
        let mut roads = vec![
            row(0, 0, DepositionKind::SolidFill, 0, 500_000),
            row(0, 1, DepositionKind::SolidFill, 2_500_000, 3_000_000),
            row(1, 0, DepositionKind::Bridge, 0, 3_000_000),
            row(2, 0, DepositionKind::SolidFill, 1_300_000, 1_700_000),
            row(4, 0, DepositionKind::SupportInterface, 1_300_000, 1_700_000),
        ];
        let cancel = AtomicBool::new(false);
        let (recipe, layers) = fixture(&roads);
        verify(
            &roads,
            &layers,
            &recipe,
            Z_GAP,
            Admission::default(),
            &mut 0,
            &cancel,
        )
        .unwrap();
        roads.truncate(3);
        roads.push(row(
            3,
            0,
            DepositionKind::SupportInterface,
            1_300_000,
            1_700_000,
        ));
        let (recipe, layers) = fixture(&roads);
        let failure = verify(
            &roads,
            &layers,
            &recipe,
            Z_GAP,
            Admission::default(),
            &mut 0,
            &cancel,
        )
        .unwrap_err();
        assert!(failure.message.starts_with("actual support layer 3 "));
    }

    #[test]
    fn an_unsupported_road_is_still_deposition_for_the_model_above() {
        let mut roads = [
            row(1, 0, DepositionKind::SolidFill, 0, 1_000_000),
            row(2, 0, DepositionKind::SolidFill, 0, 1_000_000),
            row(4, 0, DepositionKind::SupportInterface, 0, 1_000_000),
        ];
        let cancel = AtomicBool::new(false);
        let (recipe, layers) = fixture(&roads);
        verify(
            &roads,
            &layers,
            &recipe,
            Z_GAP,
            Admission::default(),
            &mut 0,
            &cancel,
        )
        .unwrap();
        roads[1] = row(2, 0, DepositionKind::SolidFill, 2_000_000, 3_000_000);
        assert!(
            verify(
                &roads,
                &layers,
                &recipe,
                Z_GAP,
                Admission::default(),
                &mut 0,
                &cancel,
            )
            .is_err()
        );
    }

    #[test]
    fn local_tie_break_keeps_thin_overlap_and_has_no_internal_cap() {
        let donor = Donor {
            road: row(0, 0, DepositionKind::SolidFill, 0, 4_000_000),
            span: Span {
                start: 0.0,
                end: 0.25,
            },
        };
        let crossing = road(
            1,
            0,
            DepositionKind::SolidFill,
            [1_020_000, -300_000],
            [1_020_000, 300_000],
        );
        assert!(
            LocalCapsule::for_pair(crossing, &donor)
                .unwrap()
                .interval()
                .is_none()
        );
        let mut roads = [
            donor.road,
            road(
                2,
                0,
                DepositionKind::SupportInterface,
                [100_000, 399_999],
                [900_000, 399_999],
            ),
        ];
        let (recipe, layers) = fixture(&roads);
        let cancel = AtomicBool::new(false);
        verify(
            &roads,
            &layers,
            &recipe,
            Z_GAP,
            Admission::default(),
            &mut 0,
            &cancel,
        )
        .unwrap();
        roads[1] = road(
            2,
            0,
            DepositionKind::SupportInterface,
            [100_000, 400_000],
            [900_000, 400_000],
        );
        assert!(
            verify(
                &roads,
                &layers,
                &recipe,
                Z_GAP,
                Admission::default(),
                &mut 0,
                &cancel
            )
            .is_err()
        );
    }

    #[test]
    fn refuses_cancellation_and_unadmitted_ancestry() {
        let roads = [row(0, 0, DepositionKind::Support, 0, 500_000)];
        let (recipe, layers) = fixture(&roads);
        let cancel = AtomicBool::new(true);
        let refusal = verify(
            &roads,
            &layers,
            &recipe,
            Z_GAP,
            Admission::default(),
            &mut 0,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(refusal.code, ManufacturingErrorCode::Cancelled);
        let allowance = crate::working_set::WorkingSet::new(1).unwrap();
        let refusal = crate::working_set::scope(allowance, || {
            verify(
                &roads,
                &layers,
                &recipe,
                Z_GAP,
                Admission::default(),
                &mut 0,
                &AtomicBool::new(false),
            )
            .unwrap_err()
        });
        assert_eq!(refusal.code, ManufacturingErrorCode::ResourceLimit);
    }
}
