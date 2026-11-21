use super::clearance::{Admission, Road, Rows, debit};
use super::{Chord, MAX_XYZ6_TICKS, Parameter, delta, failed, overflow};
use crate::planner::{OFFSET_TOLERANCE_MM, support::StrokeRuns};
use spiling_contracts::manufacturing::{
    DepositionKind, ManufacturingError, PlanarPrintRecipe, PrintLayer,
    program_coordinates::{enclosed_vertical_gap_mm, project_xyz6_bead_height_mm},
};
use std::{cell::Cell, sync::atomic::AtomicBool};

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
    last_query: Cell<usize>,
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
    height: [f64; 2],
    inventory: Option<Inventory>,
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
            && let Some(inventory) = self.previous.and_then(|band| band.inventory.as_ref())
            && visit(inventory, Bond::Direct)?
        {
            return Ok(true);
        }
        for band in self.separated {
            if let Some(inventory) = &band.inventory
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

    fn interval(self, allowance: f64) -> Option<Span> {
        let radius = (self.radius + allowance).next_down() - self.slack;
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
}

impl Replay<'_> {
    fn query(
        &mut self,
        window: &Window<'_>,
        receiver: Road,
        allowance_mm: f64,
    ) -> Result<[Bond; 2], ManufacturingError> {
        self.covered.clear();
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
        let query_id = *self.work;
        let radius = radius_ticks(receiver.inner_radius_mm, 0.0).max(0.0);
        let allowance = radius_ticks(allowance_mm, 0.0).max(0.0);
        receiver.chord.row_candidates(
            outside_ticks(receiver.inner_radius_mm.max(allowance_mm))?,
            self.cancel,
            |row, left, right, _| {
                debit(self.work, 1, self.cancel)?;
                window.for_each(receiver, |inventory, bond| {
                    for entry in inventory.rows.intersect(row, left, right) {
                        debit(self.work, 1, self.cancel)?;
                        let donor = &inventory.donors[entry.road];
                        if entry.right < left
                            || donor.last_query.get() == query_id
                            || !window.admits(receiver, donor.road, bond)?
                        {
                            continue;
                        }
                        donor.last_query.set(query_id);
                        if let Some(capsule) = LocalCapsule::for_pair(receiver, donor) {
                            if let Some(span) = capsule.interval(allowance) {
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
        self.admission.push(
            &mut inventory.donors,
            Donor {
                road,
                span,
                last_query: Cell::new(0),
            },
        )?;
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

    fn establish_model(
        &mut self,
        inventory: &mut Inventory,
        road: Road,
        window: &Window<'_>,
        allowance: f64,
    ) -> Result<(), ManufacturingError> {
        let mut published = road;
        published.inner_radius_mm = road.inner_radius_mm.min(allowance);
        self.query(
            window,
            road,
            (allowance - published.inner_radius_mm).next_down().max(0.0),
        )?;
        let mut pending: Option<Span> = None;
        for index in 0..self.covered.len() {
            debit(self.work, 1, self.cancel)?;
            let current = self.covered[index];
            pending = Some(match pending {
                Some(prior) if current.start <= prior.end => Span {
                    start: prior.start,
                    end: prior.end.max(current.end),
                },
                Some(prior) => {
                    self.insert(inventory, published, prior)?;
                    current
                }
                None => current,
            });
        }
        if let Some(span) = pending {
            self.insert(inventory, published, span)?;
        }
        Ok(())
    }

    fn stroke(
        &mut self,
        window: &Window<'_>,
        roads: impl Iterator<Item = Road>,
        width: f64,
        limit: f64,
        bridge: bool,
    ) -> Result<bool, ManufacturingError> {
        self.run.clear();
        let mut length = 0.0;
        let mut ends = [Bond::None; 2];
        for (index, road) in roads.enumerate() {
            let edge_ends = self.query(window, road, 0.0)?;
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
        if bridge
            && (ends[0] == Bond::None && self.run.first().is_none_or(|span| span.start > 0.0)
                || ends[1] == Bond::None && !self.run.iter().rev().any(|span| span.end >= length))
        {
            return Ok(false);
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
                return Ok(false);
            }
            covered = covered.max(coverage.end);
        }
        Ok(covered >= length
            || accepts(
                length - covered,
                covered > 0.0 || ends[0] != Bond::None,
                ends[1] != Bond::None,
            ))
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
    bands: &[Band],
    floor: f64,
    gap: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let (mut start, mut end) = (0, bands.len());
    while start < end {
        debit(work, 1, cancel)?;
        let middle = start + (end - start) / 2;
        if enclosed_vertical_gap_mm(floor, bands[middle].height[1])? >= gap {
            start = middle + 1;
        } else {
            end = middle;
        }
    }
    Ok(start)
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
    let mut replay = Replay {
        admission,
        work,
        cancel,
        covered: Vec::new(),
        run: Vec::new(),
    };
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
    let mut bands = Vec::<Band>::new();
    replay.admission.reserve(
        &mut bands,
        last_support.checked_add(1).ok_or_else(overflow)?,
    )?;
    let mut unpublished = roads;
    let mut expired = 0;
    let mut last_separated_end = 0;
    for (index, layer) in selected_layers.iter().enumerate() {
        debit(replay.work, 1, cancel)?;
        let height = project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm)?;
        let count = unpublished.partition_point(|road| road.layer as usize == index);
        let (current, later) = unpublished.split_at(count);
        unpublished = later;
        let end = separated_end(&bands, height[0], z_gap, replay.work, cancel)?;
        let support_start = last_separated_end.min(end);
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
            if let Some(inventory) = band.inventory.take() {
                inventory.release(&mut replay.admission);
            }
        }
        expired = oldest;
        let mut inventory = Inventory::empty();
        let previous = bands.last();
        let previous_nominal_top = index.checked_sub(1).map(|prior| layers[prior].z_mm);
        for path_roads in current.chunk_by(|left, right| left.path == right.path) {
            debit(replay.work, 1, cancel)?;
            let first_road = path_roads[0];
            let is_support = support(first_road.role);
            if index == last_support && !is_support {
                continue;
            }
            let window = Window {
                previous,
                previous_nominal_top,
                z_gap,
                separated: if is_support {
                    &bands[support_start..end]
                } else {
                    &bands[contact_index..end]
                },
            };
            if index != 0 && (is_support || first_road.role == DepositionKind::Bridge) {
                let path = &layer.paths[first_road.path];
                if path_roads.len() + 1 != path.points_mm.len() {
                    return Err(failed(
                        "decoded support stroke differs from its associated edge count",
                    ));
                }
                let mut rooted = true;
                for (first, last, width) in StrokeRuns::for_path(path)? {
                    let count = if last > first {
                        last - first
                    } else {
                        path_roads.len() - first + last
                    };
                    let run = (0..count).map(|edge| path_roads[(first + edge) % path_roads.len()]);
                    if !replay.stroke(
                        &window,
                        run,
                        width,
                        recipe.process.max_bridge_span_mm,
                        !is_support,
                    )? {
                        rooted = false;
                        break;
                    }
                }
                if !rooted {
                    if is_support {
                        return Err(failed(format!(
                            "actual support layer {index} (line {}) has no established bed, continuing support or earlier-part contact",
                            first_road.line
                        )));
                    }
                    continue;
                }
            }
            if index != last_support {
                for &road in path_roads {
                    if index == 0 || is_support || road.role == DepositionKind::Bridge {
                        replay.insert(&mut inventory, road, Span::FULL)?;
                    } else {
                        replay.establish_model(
                            &mut inventory,
                            road,
                            &window,
                            recipe.bead_width_mm * 0.5,
                        )?;
                    }
                }
            }
        }
        inventory.rows.finish(replay.work, cancel)?;
        bands.push(Band {
            height,
            inventory: Some(inventory),
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
    fn bridge_carries_chain_but_cannot_land_support() {
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
    fn local_tie_break_keeps_thin_overlap_and_has_no_internal_cap() {
        let donor = Donor {
            road: row(0, 0, DepositionKind::SolidFill, 0, 4_000_000),
            span: Span {
                start: 0.0,
                end: 0.25,
            },
            last_query: Cell::new(0),
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
                .interval(0.0)
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
