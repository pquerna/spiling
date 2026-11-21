use super::{cancelled, failed};
use spiling_contracts::manufacturing::{
    ManufacturingError, ManufacturingErrorCode, SUPPORT_GRID_CELL_XYZ6,
    program_coordinates::xyz6_coordinate_error_mm,
};
use spiling_geometry::PlanarCapsuleMm;
use std::cmp::Ordering;
use std::sync::atomic::AtomicBool;

mod ancestry;
mod clearance;
pub(super) use clearance::{Clearance, RoadInput};

const CELL_TICKS: i64 = SUPPORT_GRID_CELL_XYZ6 as i64;
const MAX_XYZ6_TICKS: i64 = 1 << 52;

type Point = [i64; 2];

fn validate_cell_size(recorded: u32) -> Result<(), ManufacturingError> {
    if recorded != SUPPORT_GRID_CELL_XYZ6 {
        return Err(failed(
            "plan support grid cell size is not the independently verified XYZ6 size",
        ));
    }
    Ok(())
}

pub(super) fn xyz6_ticks(text: &str) -> Result<i64, &'static str> {
    let unsigned = text
        .strip_prefix('+')
        .or_else(|| text.strip_prefix('-'))
        .unwrap_or(text);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let range_error = "command coordinate exceeds the exact XYZ6 grid range";
    let whole = whole.parse::<i64>().map_err(|_| range_error)?;
    let fraction_ticks = if fraction.is_empty() {
        0
    } else {
        let digits = u32::try_from(fraction.len()).map_err(|_| range_error)?;
        let padding = 6u32.checked_sub(digits).ok_or(range_error)?;
        fraction.parse::<i64>().map_err(|_| range_error)? * 10i64.pow(padding)
    };
    let ticks = whole
        .checked_mul(1_000_000)
        .and_then(|scaled| scaled.checked_add(fraction_ticks))
        .filter(|magnitude| (0..=MAX_XYZ6_TICKS).contains(magnitude))
        .ok_or(range_error)?;
    Ok(if text.starts_with('-') { -ticks } else { ticks })
}

pub(super) fn virtual_xyz6(point: [f64; 3]) -> [i64; 3] {
    point.map(|coordinate| (coordinate * 1_000_000.0).round() as i64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Cell(Point);

impl Cell {
    fn containing(point: Point) -> Self {
        Self(point.map(|value| value.div_euclid(CELL_TICKS)))
    }

    fn corners(self) -> [Point; 4] {
        let [left, bottom] = self.0.map(|index| index * CELL_TICKS);
        let right = left + CELL_TICKS;
        let top = bottom + CELL_TICKS;
        [[left, bottom], [left, top], [right, bottom], [right, top]]
    }
}

fn overflow() -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ResourceLimit,
        "support replay grid arithmetic overflow",
    )
}

fn squared(vector: [i128; 2]) -> i128 {
    dot(vector, vector)
}

fn dot(left: [i128; 2], right: [i128; 2]) -> i128 {
    left[0] * right[0] + left[1] * right[1]
}

fn cross(left: [i128; 2], right: [i128; 2]) -> i128 {
    left[0] * right[1] - left[1] * right[0]
}

fn delta(origin: Point, point: Point) -> [i128; 2] {
    std::array::from_fn(|axis| i128::from(point[axis]) - i128::from(origin[axis]))
}

#[derive(Clone, Copy)]
struct Chord {
    start: Point,
    end: Point,
    vector: [i128; 2],
    length_squared: i128,
}

#[derive(Clone, Copy, Debug)]
struct Parameter {
    numerator: u64,
    denominator: u64,
}

impl Parameter {
    const START: Self = Self {
        numerator: 0,
        denominator: 1,
    };
    const END: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    fn cmp(self, other: Self) -> Ordering {
        (u128::from(self.numerator) * u128::from(other.denominator))
            .cmp(&(u128::from(other.numerator) * u128::from(self.denominator)))
    }

    fn cmp_projection(self, projection: i128, length_squared: i128) -> Ordering {
        wide_product(u128::from(self.numerator), length_squared as u128).cmp(&wide_product(
            projection as u128,
            u128::from(self.denominator),
        ))
    }
}

impl Chord {
    fn new(start: Point, end: Point) -> Result<Self, ManufacturingError> {
        if start
            .into_iter()
            .chain(end)
            .any(|coordinate| coordinate.unsigned_abs() > MAX_XYZ6_TICKS as u64)
        {
            return Err(failed("invalid actual chord for support replay grid"));
        }
        let vector = delta(start, end);
        Ok(Self {
            start,
            end,
            vector,
            length_squared: squared(vector),
        })
    }

    fn projection(self, point: Point) -> i128 {
        dot(self.vector, delta(self.start, point))
    }

    fn clearance_capsule(
        self,
        radius_mm: f64,
        arc_deviation_mm: f64,
    ) -> Result<PlanarCapsuleMm, ManufacturingError> {
        if !arc_deviation_mm.is_finite() || arc_deviation_mm < 0.0 {
            return Err(failed(
                "invalid decoded chord deviation for support replay grid",
            ));
        }
        let segment_xy_mm =
            [self.start, self.end].map(|point| point.map(|tick| tick as f64 / 1_000_000.0));
        let mut endpoint_error = 0.0_f64;
        for [x_mm, y_mm] in segment_xy_mm {
            endpoint_error = endpoint_error
                .max(xyz6_coordinate_error_mm(x_mm)?.hypot(xyz6_coordinate_error_mm(y_mm)?));
        }
        let endpoint_error = if endpoint_error > 0.0 {
            endpoint_error.next_up()
        } else {
            0.0
        };
        let endpoint_error_mm = if arc_deviation_mm > 0.0 {
            (endpoint_error + arc_deviation_mm).next_up()
        } else {
            endpoint_error
        };
        Ok(PlanarCapsuleMm {
            segment_xy_mm,
            radius_mm,
            endpoint_error_mm,
        })
    }

    fn point_within(self, point: Point, radius_squared: i128) -> bool {
        let projection = self.projection(point);
        if projection <= 0 {
            return squared(delta(self.start, point)) <= radius_squared;
        }
        if projection >= self.length_squared {
            return squared(delta(self.end, point)) <= radius_squared;
        }
        let area = cross(self.vector, delta(self.start, point)).unsigned_abs();
        wide_product(area, area)
            <= wide_product(radius_squared as u128, self.length_squared as u128)
    }

    #[cfg(test)]
    fn cell_within(self, cell: Cell, radius_squared: i128) -> bool {
        cell.corners()
            .into_iter()
            .all(|corner| self.point_within(corner, radius_squared))
    }

    fn cell_within_span(
        self,
        cell: Cell,
        radius_squared: i128,
        [start, end]: [Parameter; 2],
    ) -> bool {
        cell.corners().into_iter().all(|corner| {
            let projection = self.projection(corner).clamp(0, self.length_squared);
            !start
                .cmp_projection(projection, self.length_squared)
                .is_gt()
                && !end.cmp_projection(projection, self.length_squared).is_lt()
                && self.point_within(corner, radius_squared)
        })
    }

    fn walk_while(
        self,
        work: &mut usize,
        cancel: &AtomicBool,
        mut visit: impl FnMut(Cell, [Parameter; 2], &mut usize) -> Result<bool, ManufacturingError>,
    ) -> Result<bool, ManufacturingError> {
        let step = self.vector.map(|component| component.signum() as i64);
        let mut cell = Cell::containing(self.start);
        for (axis, direction) in step.into_iter().enumerate() {
            if direction < 0 && self.start[axis].rem_euclid(CELL_TICKS) == 0 {
                cell.0[axis] -= 1;
            }
        }
        let crossing = |cell: Cell, axis: usize| {
            if step[axis] == 0 {
                return Parameter::END;
            }
            let boundary = (cell.0[axis] + i64::from(step[axis] > 0)) * CELL_TICKS;
            Parameter {
                numerator: boundary.abs_diff(self.start[axis]),
                denominator: self.start[axis].abs_diff(self.end[axis]),
            }
        };
        let mut start = Parameter::START;
        loop {
            cancelled(cancel)?;
            *work = work.checked_add(1).ok_or_else(overflow)?;
            let crossings = [crossing(cell, 0), crossing(cell, 1)];
            let end = crossings.into_iter().fold(Parameter::END, |end, next| {
                if next.cmp(end).is_lt() { next } else { end }
            });
            if !visit(cell, [start, end], work)? {
                return Ok(false);
            }
            if end.cmp(Parameter::END).is_eq() {
                return Ok(true);
            }
            for (axis, next) in crossings.into_iter().enumerate() {
                if next.cmp(end).is_eq() {
                    cell.0[axis] += step[axis];
                }
            }
            start = end;
        }
    }

    #[cfg(test)]
    fn walk(
        self,
        work: &mut usize,
        cancel: &AtomicBool,
        mut visit: impl FnMut(Cell, [Parameter; 2]) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        self.walk_while(work, cancel, |cell, range, _| {
            visit(cell, range)?;
            Ok(true)
        })?;
        Ok(())
    }

    #[cfg(test)]
    fn walk_coverage(
        self,
        work: &mut usize,
        cancel: &AtomicBool,
        mut covered: impl FnMut(Cell) -> bool,
        mut visit: impl FnMut([Parameter; 2], bool) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        let mut pending = None::<([Parameter; 2], bool)>;
        self.walk(work, cancel, |cell, [start, end]| {
            let direct = covered(cell);
            pending = Some(match pending {
                Some(([begin, _], previous)) if previous == direct => ([begin, end], direct),
                Some((span, previous)) => {
                    visit(span, previous)?;
                    ([start, end], direct)
                }
                None => ([start, end], direct),
            });
            Ok(())
        })?;
        cancelled(cancel)?;
        if let Some((span, direct)) = pending {
            visit(span, direct)?;
        }
        Ok(())
    }

    fn cell_intersects(self, cell: Cell, radius_squared: i128) -> bool {
        let corners = cell.corners();
        let lower = corners[0];
        let upper = corners[3];
        let bbox_overlaps = (0..2).all(|axis| {
            lower[axis] <= self.start[axis].max(self.end[axis])
                && upper[axis] >= self.start[axis].min(self.end[axis])
        });
        if bbox_overlaps {
            let sides = corners.map(|point| cross(self.vector, delta(self.start, point)));
            if sides.iter().any(|side| *side <= 0) && sides.iter().any(|side| *side >= 0) {
                return true;
            }
        }
        for point in [self.start, self.end] {
            let closest = std::array::from_fn(|axis| point[axis].clamp(lower[axis], upper[axis]));
            if squared(delta(point, closest)) <= radius_squared {
                return true;
            }
        }
        corners
            .into_iter()
            .any(|corner| self.point_within(corner, radius_squared))
    }

    #[cfg(test)]
    fn raster_cells(
        self,
        outer_radius: i64,
        inner_radius: Option<i64>,
        work: &mut usize,
        cancel: &AtomicBool,
        mut visit: impl FnMut(Cell, bool) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        if inner_radius.is_some_and(|inner| !(1..=outer_radius).contains(&inner)) {
            return Err(overflow());
        }
        let outer_squared = i128::from(outer_radius).pow(2);
        let inner_squared = inner_radius.map(|radius| i128::from(radius).pow(2));
        self.row_candidates(outer_radius, cancel, |row, first, last, _| {
            for column in first..=last {
                cancelled(cancel)?;
                *work = work.checked_add(1).ok_or_else(overflow)?;
                let cell = Cell([column, row]);
                if self.cell_intersects(cell, outer_squared) {
                    visit(
                        cell,
                        inner_squared.is_some_and(|square| self.cell_within(cell, square)),
                    )?;
                }
            }
            Ok(())
        })
    }

    fn row_candidates(
        self,
        outer_radius: i64,
        cancel: &AtomicBool,
        mut visit: impl FnMut(i64, i64, i64, i64) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        if !(0..=MAX_XYZ6_TICKS).contains(&outer_radius) {
            return Err(overflow());
        }
        let [start, end] = if self.start[1] <= self.end[1] {
            [self.start, self.end]
        } else {
            [self.end, self.start]
        };
        let bottom = (start[1] - outer_radius - 1).div_euclid(CELL_TICKS);
        let top = (end[1] + outer_radius).div_euclid(CELL_TICKS);
        let rise = i128::from(end[1]) - i128::from(start[1]);
        for row in bottom..=top {
            cancelled(cancel)?;
            let y_min = row * CELL_TICKS - outer_radius;
            let y_max = (row + 1) * CELL_TICKS + outer_radius;
            let [x_min, x_max, seed_x] = if rise == 0 {
                [start[0].min(end[0]), start[0].max(end[0]), start[0]]
            } else {
                let run = i128::from(end[0]) - i128::from(start[0]);
                let project = |height: i64| {
                    i128::from(start[0]) * rise
                        + i128::from(height.clamp(start[1], end[1]) - start[1]) * run
                };
                let left = project(y_min);
                let right = project(y_max);
                [
                    i64::try_from(left.min(right).div_euclid(rise)).map_err(|_| overflow())?,
                    i64::try_from(-(-left.max(right)).div_euclid(rise)).map_err(|_| overflow())?,
                    i64::try_from(project(row * CELL_TICKS + CELL_TICKS / 2).div_euclid(rise))
                        .map_err(|_| overflow())?,
                ]
            };
            let first = (x_min - outer_radius - 1).div_euclid(CELL_TICKS);
            let last = (x_max + outer_radius).div_euclid(CELL_TICKS);
            visit(row, first, last, seed_x.div_euclid(CELL_TICKS))?;
        }
        Ok(())
    }

    fn outer_rows(
        self,
        radius: i64,
        work: &mut usize,
        cancel: &AtomicBool,
        mut visit: impl FnMut(i64, i64, i64) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        if !(0..=MAX_XYZ6_TICKS).contains(&radius) {
            return Err(overflow());
        }
        let square = i128::from(radius).pow(2);
        let [min_x, max_x] = [
            self.start[0].min(self.end[0]),
            self.start[0].max(self.end[0]),
        ];
        let [min_y, max_y] = [
            self.start[1].min(self.end[1]),
            self.start[1].max(self.end[1]),
        ];
        if min_x == max_x || min_y == max_y {
            let bottom = (min_y - radius - 1).div_euclid(CELL_TICKS);
            let top = (max_y + radius).div_euclid(CELL_TICKS);
            for row in bottom..=top {
                clearance::debit(work, 1, cancel)?;
                let distance = (min_y - (row + 1) * CELL_TICKS)
                    .max(row * CELL_TICKS - max_y)
                    .max(0);
                let reach = if distance == 0 {
                    radius
                } else {
                    (square - i128::from(distance).pow(2)).isqrt() as i64
                };
                visit(
                    row,
                    (min_x - reach - 1).div_euclid(CELL_TICKS),
                    (max_x + reach).div_euclid(CELL_TICKS),
                )?;
            }
            return Ok(());
        }

        let hints = CapsuleRowHints::new(self, radius);
        self.row_candidates(radius, cancel, |row, left, right, seed| {
            clearance::debit(work, 1, cancel)?;
            let mut hit = |column| {
                clearance::debit(work, 1, cancel)?;
                Ok::<_, ManufacturingError>(self.cell_intersects(Cell([column, row]), square))
            };
            let [left_hint, right_hint] = hints.for_row(row).unwrap_or([seed, seed]);
            let first = first_intersecting_column(left, seed, left_hint, &mut hit)?;
            let last =
                -first_intersecting_column(-right, -seed, -right_hint, |column| hit(-column))?;
            visit(row, first, last)
        })
    }
}

struct CapsuleRowHints {
    start: [f64; 2],
    end: [f64; 2],
    offset: [f64; 2],
    slope: f64,
    radius: f64,
}

impl CapsuleRowHints {
    fn new(chord: Chord, radius: i64) -> Self {
        let [start, end] = if chord.start[1] < chord.end[1] {
            [chord.start, chord.end]
        } else {
            [chord.end, chord.start]
        };
        let [run, rise] = delta(start, end).map(|component| component as f64);
        let radius = radius as f64;
        let norm = radius / run.hypot(rise);
        Self {
            start: start.map(|value| value as f64),
            end: end.map(|value| value as f64),
            offset: [norm * rise, -norm * run],
            slope: run / rise,
            radius,
        }
    }

    fn for_row(&self, row: i64) -> Option<[i64; 2]> {
        let bottom = (row * CELL_TICKS) as f64;
        let top = ((row + 1) * CELL_TICKS) as f64;
        let (mut left, mut right) = (f64::INFINITY, f64::NEG_INFINITY);
        for [x, y] in [self.start, self.end] {
            let distance = (y - top).max(bottom - y).max(0.0);
            if distance <= self.radius {
                let reach = ((self.radius - distance) * (self.radius + distance)).sqrt();
                left = left.min(x - reach);
                right = right.max(x + reach);
            }
        }
        for side in [-1.0, 1.0] {
            let [shift_x, shift_y] = self.offset.map(|component| side * component);
            let y_start = self.start[1] + shift_y;
            let y_end = self.end[1] + shift_y;
            let lower = bottom.max(y_start);
            let upper = top.min(y_end);
            if lower <= upper {
                let x = |y| self.start[0] + shift_x + self.slope * (y - y_start);
                let [first, last] = [x(lower), x(upper)];
                left = left.min(first.min(last));
                right = right.max(first.max(last));
            }
        }
        (left <= right).then(|| {
            [
                ((left - 1.0) / CELL_TICKS as f64).floor() as i64,
                (right / CELL_TICKS as f64).floor() as i64,
            ]
        })
    }
}

fn first_intersecting_column(
    mut lower: i64,
    mut inside: i64,
    hint: i64,
    mut hit: impl FnMut(i64) -> Result<bool, ManufacturingError>,
) -> Result<i64, ManufacturingError> {
    let hint = hint.clamp(lower, inside);
    if hint == inside || hit(hint)? {
        if hint == lower || !hit(hint - 1)? {
            return Ok(hint);
        }
        inside = hint - 1;
    } else {
        if hint + 1 == inside || hit(hint + 1)? {
            return Ok(hint + 1);
        }
        lower = hint + 2;
    }
    while lower < inside {
        let middle = lower + (inside - lower) / 2;
        if hit(middle)? {
            inside = middle;
        } else {
            lower = middle + 1;
        }
    }
    Ok(inside)
}

fn wide_product(left: u128, right: u128) -> [u128; 2] {
    if let Some(low) = left.checked_mul(right) {
        return [0, low];
    }
    let mask = u128::from(u64::MAX);
    let low_low = (left & mask) * (right & mask);
    let low_high = (left & mask) * (right >> 64);
    let high_low = (left >> 64) * (right & mask);
    let high_high = (left >> 64) * (right >> 64);
    let middle = (low_low >> 64) + (low_high & mask) + (high_low & mask);
    [
        high_high + (low_high >> 64) + (high_low >> 64) + (middle >> 64),
        (middle << 64) | (low_low & mask),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_cell_size_must_match_the_independent_reference() {
        validate_cell_size(50_000).unwrap();
        assert!(validate_cell_size(49_999).is_err());
        assert!(validate_cell_size(50_001).is_err());
    }

    fn decoded_ticks(line: &str) -> (super::super::Decoded, [Option<i64>; 3]) {
        let mut ticks = [None; 3];
        let command = super::super::decode_with_xyz6(line, false, |axis, word| {
            ticks[axis] = Some(xyz6_ticks(word)?);
            Ok(())
        })
        .unwrap()
        .unwrap();
        (command, ticks)
    }

    #[test]
    fn decoded_xyz6_retains_a_tick_float_round_trip_loses() {
        let line = "G1 X4503599627.270496 Y-0.050001 Z+1.2 E1 F1500";
        let (command, ticks) = decoded_ticks(line);
        assert_eq!(
            ticks,
            [Some(4_503_599_627_270_496), Some(-50_001), Some(1_200_000)]
        );
        assert_ne!(
            (command.fields[0].unwrap() * 1_000_000.0).round() as i64,
            ticks[0].unwrap()
        );
    }

    #[test]
    fn negative_grid_boundary_walk_is_directed_and_ordered() {
        let cancel = AtomicBool::new(false);
        let diagonal = Chord::new([50_000, 50_000], [-50_000, -50_000]).unwrap();
        let mut cells = Vec::new();
        let mut work = 0;
        diagonal
            .walk(&mut work, &cancel, |cell, [start, end]| {
                assert!(start.cmp(end).is_lt());
                cells.push(cell);
                Ok(())
            })
            .unwrap();
        assert_eq!(cells, [Cell([0, 0]), Cell([-1, -1])]);
        assert_eq!(work, cells.len());
    }

    #[test]
    fn oblique_end_encloses_outer_but_not_inner_cell() {
        let chord = Chord::new([0, 0], [100_000, 100_000]).unwrap();
        let tip = Cell([2, 2]);
        let radius_squared = i128::from(CELL_TICKS).pow(2);
        assert!(chord.cell_intersects(tip, radius_squared));
        assert!(!chord.cell_within(tip, radius_squared));
        let middle = Cell([1, 1]);
        assert!(chord.cell_within(middle, radius_squared));
        assert!(!chord.cell_within_span(
            middle,
            radius_squared,
            [
                Parameter::START,
                Parameter {
                    numerator: 1,
                    denominator: 2
                },
            ]
        ));
    }

    #[test]
    fn exact_integer_products_preserve_a_wide_threshold() {
        assert_eq!(wide_product(u128::MAX, u128::MAX), [u128::MAX - 1, 1]);
        assert_eq!(wide_product(1 << 100, 1 << 70), [1 << 42, 0]);
        assert!(wide_product(1 << 100, (1 << 70) - 1) < wide_product(1 << 100, 1 << 70));
    }

    #[test]
    fn outer_cells_keep_the_exact_closed_tick_threshold() {
        let boundary = Cell([0, 1]);
        let radius_squared = i128::from(25_000).pow(2);
        for (height, reaches) in [(24_999, false), (25_000, true), (25_001, true)] {
            let chord = Chord::new([-80_000, height], [130_000, height]).unwrap();
            assert_eq!(chord.cell_intersects(boundary, radius_squared), reaches);
            let mut found = false;
            chord
                .raster_cells(
                    25_000,
                    None,
                    &mut 0,
                    &AtomicBool::new(false),
                    |cell, inner| {
                        assert!(!inner);
                        found |= cell == boundary;
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(found, reaches);
        }
    }

    #[test]
    fn compact_outer_rows_equal_the_per_cell_reference() {
        let cancel = AtomicBool::new(false);
        for first in [[-100_000, 50_001], [25_000, -50_001], [-200_000, -150_000]] {
            for last in [
                [500_001, 50_001],
                [30_000, 130_000],
                [-250_000, -150_000],
                first,
            ] {
                for radius in [0, 1, 49_999, 50_000, 101_001] {
                    let chord = Chord::new(first, last).unwrap();
                    let mut reference = Vec::new();
                    let mut compact = Vec::new();
                    chord
                        .raster_cells(radius, None, &mut 0, &cancel, |cell, _| {
                            reference.push(cell);
                            Ok(())
                        })
                        .unwrap();
                    chord
                        .outer_rows(radius, &mut 0, &cancel, |row, left, right| {
                            compact.extend((left..=right).map(|column| Cell([column, row])));
                            Ok(())
                        })
                        .unwrap();
                    assert_eq!(compact, reference, "{first:?}, {last:?}, {radius}");
                }
            }
        }
    }

    #[test]
    fn exact_row_boundaries_match_cells_at_signed_and_large_offsets() {
        let cancel = AtomicBool::new(false);
        let local = [-50_001, -50_000, -49_999, -1, 0, 1, 49_999, 50_000, 50_001];
        let mut state = 17_u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            local[(state >> 32) as usize % local.len()]
        };
        for origin in [0, MAX_XYZ6_TICKS - 1_000_000, -MAX_XYZ6_TICKS + 1_000_000] {
            for _ in 0..384 {
                let start = [origin + next(), origin + next()];
                let end = [origin + next(), origin + next()];
                let radius = next().abs();
                let chord = Chord::new(start, end).unwrap();
                let mut reference = Vec::new();
                let mut compact = Vec::new();
                chord
                    .raster_cells(radius, None, &mut 0, &cancel, |cell, _| {
                        reference.push(cell);
                        Ok(())
                    })
                    .unwrap();
                chord
                    .outer_rows(radius, &mut 0, &cancel, |row, left, right| {
                        compact.extend((left..=right).map(|column| Cell([column, row])));
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(compact, reference, "{start:?}, {end:?}, r={radius}");
            }
        }
    }

    #[test]
    fn local_capsule_tie_break_accepts_the_exact_gap_but_not_a_tick_less() {
        let cancel = AtomicBool::new(false);
        let support = Chord::new([0, 0], [1_000_000, 0])
            .unwrap()
            .clearance_capsule(0.125, 0.0)
            .unwrap();
        for (y_tick, clears) in [(499_999, false), (500_000, true), (500_001, true)] {
            let model = Chord::new([0, y_tick], [1_000_000, y_tick])
                .unwrap()
                .clearance_capsule(0.125, 0.0)
                .unwrap();
            let result = spiling_geometry::certify_planar_capsule_pair_clearance(
                [support, model],
                0.25,
                &mut spiling_geometry::GeometryWorkBudget::compile(),
                &cancel,
            );
            assert_eq!(result.is_ok(), clears);
        }
        let deviated = Chord::new([0, 500_000], [1_000_000, 500_000])
            .unwrap()
            .clearance_capsule(0.125, 0.000_001)
            .unwrap();
        assert!(
            spiling_geometry::certify_planar_capsule_pair_clearance(
                [support, deviated],
                0.25,
                &mut spiling_geometry::GeometryWorkBudget::compile(),
                &cancel,
            )
            .is_err()
        );
    }

    #[test]
    fn unanchored_middle_preserves_both_flanks() {
        let chord = Chord::new([0, 25_000], [250_000, 25_000]).unwrap();
        let mut spans = Vec::new();
        chord
            .walk_coverage(
                &mut 0,
                &AtomicBool::new(false),
                |cell| cell != Cell([2, 0]),
                |bounds, direct| {
                    spans.push((bounds, direct));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            spans.iter().map(|(_, direct)| *direct).collect::<Vec<_>>(),
            [true, false, true]
        );
        assert!(
            spans[1].0[0]
                .cmp(Parameter {
                    numerator: 2,
                    denominator: 5
                })
                .is_eq()
        );
        assert!(
            spans[1].0[1]
                .cmp(Parameter {
                    numerator: 3,
                    denominator: 5
                })
                .is_eq()
        );
    }

    #[test]
    fn moving_a_decoded_donor_removes_covered_cells() {
        let cancel = AtomicBool::new(false);
        let point = |line| {
            let (_, [x_tick, y_tick, _]) = decoded_ticks(line);
            [x_tick.unwrap(), y_tick.unwrap()]
        };
        let check = |start, end| {
            let mut cells = std::collections::BTreeSet::new();
            let lower = Chord::new(point(start), point(end)).unwrap();
            lower
                .raster_cells(50_000, Some(50_000), &mut 0, &cancel, |cell, inner| {
                    if inner {
                        cells.insert(cell);
                    }
                    Ok(())
                })
                .unwrap();
            let upper = Chord::new([0, 25_000], [250_000, 25_000]).unwrap();
            let mut direct = Vec::new();
            upper
                .walk_coverage(
                    &mut 0,
                    &cancel,
                    |cell| cells.contains(&cell),
                    |_, covered| {
                        direct.push(covered);
                        Ok(())
                    },
                )
                .unwrap();
            direct
        };
        assert_eq!(
            check("G0 X0 Y0.025 Z0.2 F1500", "G1 X0.25 Y0.025 Z0.2 E1 F1500"),
            [true]
        );
        assert_eq!(
            check("G0 X0 Y0.125 Z0.2 F1500", "G1 X0.25 Y0.125 Z0.2 E1 F1500"),
            [false]
        );
    }

    #[test]
    fn partial_donation_cannot_cover_the_failed_middle_next_layer() {
        let partial = Chord::new([0, 25_000], [250_000, 25_000]).unwrap();
        let cancel = AtomicBool::new(false);
        let mut proved = Vec::new();
        partial
            .walk_coverage(
                &mut 0,
                &cancel,
                |cell| cell != Cell([2, 0]),
                |span, direct| {
                    if direct {
                        proved.push(span);
                    }
                    Ok(())
                },
            )
            .unwrap();
        let mut published = std::collections::BTreeSet::new();
        let radius = 50_000;
        partial
            .raster_cells(radius, Some(radius), &mut 0, &cancel, |cell, inner| {
                if inner
                    && proved.iter().any(|span| {
                        partial.cell_within_span(cell, i128::from(radius).pow(2), *span)
                    })
                {
                    published.insert(cell);
                }
                Ok(())
            })
            .unwrap();
        let mut upper = Vec::new();
        partial
            .walk_coverage(
                &mut 0,
                &cancel,
                |cell| published.contains(&cell),
                |_, direct| {
                    upper.push(direct);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(upper, [true, false, true]);
    }

    #[test]
    fn cancellation_stops_before_a_grid_cell_can_donate() {
        let chord = Chord::new([0, 0], [100_000, 100_000]).unwrap();
        let cancel = AtomicBool::new(true);
        assert_eq!(
            chord
                .walk(&mut 0, &cancel, |_, _| panic!("unadmitted cell"))
                .unwrap_err()
                .code,
            ManufacturingErrorCode::Cancelled
        );
        assert_eq!(
            chord
                .raster_cells(1, None, &mut 0, &cancel, |_, _| panic!("unadmitted cell"))
                .unwrap_err()
                .code,
            ManufacturingErrorCode::Cancelled
        );
    }
}
