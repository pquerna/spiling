use super::{Chord, MAX_XYZ6_TICKS, cancelled, failed, overflow, validate_cell_size};
use spiling_contracts::geometry::GeometryErrorCode;
use spiling_contracts::manufacturing::{
    DepositionKind, ManufacturingError, ManufacturingErrorCode, NormalizedPrintPlan,
    PlanarPrintRecipe, SupportPolicy,
    program_coordinates::{enclosed_vertical_gap_mm, xyz6_coordinate_error_mm},
};
use std::{mem::size_of, ops::Range, sync::atomic::AtomicBool};

pub(in crate::verifier) struct RoadInput {
    pub layer: u32,
    pub line: usize,
    pub path: usize,
    pub role: DepositionKind,
    pub from: [i64; 3],
    pub to: [i64; 3],
    pub physical_z: [f64; 2],
    pub maximum_width_mm: f64,
    pub minimum_width_mm: f64,
    pub length_upper_mm: f64,
    pub arc_deviation_mm: f64,
}

#[derive(Clone, Copy)]
pub(super) struct Road {
    pub(super) chord: Chord,
    pub(super) height: [f64; 2],
    radius_mm: f64,
    pub(super) inner_radius_mm: f64,
    pub(super) length_upper_mm: f64,
    pub(super) arc_deviation_mm: f64,
    pub(super) floor: f64,
    pub(super) layer: u32,
    pub(super) line: usize,
    pub(super) path: usize,
    pub(super) role: DepositionKind,
    support: bool,
}

impl Road {
    pub(super) fn new(input: RoadInput) -> Result<Self, ManufacturingError> {
        let RoadInput {
            layer,
            line,
            path,
            role,
            from,
            to,
            physical_z,
            maximum_width_mm,
            minimum_width_mm,
            length_upper_mm,
            arc_deviation_mm,
        } = input;
        let chord = Chord::new([from[0], from[1]], [to[0], to[1]])?;
        let bead_height = -enclosed_vertical_gap_mm(physical_z[0], physical_z[1])?;
        let [mut lower, mut upper] = [f64::INFINITY, f64::NEG_INFINITY];
        for tick in [from[2], to[2]] {
            let top = tick as f64 / 1_000_000.0;
            let error = xyz6_coordinate_error_mm(top)?;
            lower = lower.min(enclosed_vertical_gap_mm(
                enclosed_vertical_gap_mm(top, bead_height)?,
                error,
            )?);
            upper = upper.max(-enclosed_vertical_gap_mm(-top, error)?);
        }
        Ok(Self {
            chord,
            height: [lower, upper],
            radius_mm: maximum_width_mm * 0.5,
            inner_radius_mm: minimum_width_mm * 0.5,
            length_upper_mm,
            arc_deviation_mm,
            floor: physical_z[0],
            layer,
            line,
            path,
            role,
            support: !role.is_model(),
        })
    }

    fn outer_radius(self, clearance_mm: f64) -> Result<i64, ManufacturingError> {
        let with_arc = self.radius_mm + self.arc_deviation_mm;
        let total = (with_arc.next_up() + clearance_mm).next_up();
        let ticks = (total * 1_000_000.0).next_up().ceil();
        if !ticks.is_finite() || !(0.0..=MAX_XYZ6_TICKS as f64).contains(&ticks) {
            return Err(overflow());
        }
        Ok(ticks as i64)
    }
}

pub(super) fn exhausted() -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ResourceLimit,
        "decoded support replay exceeds live byte allowance",
    )
}

#[derive(Default)]
pub(super) struct Admission(pub(super) usize);

impl Admission {
    pub(super) fn reserve<T>(
        &mut self,
        values: &mut Vec<T>,
        required: usize,
    ) -> Result<(), ManufacturingError> {
        let old = values.capacity();
        if required <= old {
            return Ok(());
        }
        let requested = required.max(old.checked_mul(2).ok_or_else(exhausted)?);
        let permitted = crate::working_set::bytes();
        self.0
            .checked_add(
                requested
                    .checked_mul(size_of::<T>())
                    .ok_or_else(exhausted)?,
            )
            .filter(|peak| *peak <= permitted)
            .ok_or_else(exhausted)?;
        values
            .try_reserve_exact(requested - values.len())
            .map_err(|_| exhausted())?;
        self.0 = self
            .0
            .checked_add(
                (values.capacity() - old)
                    .checked_mul(size_of::<T>())
                    .ok_or_else(exhausted)?,
            )
            .filter(|live| *live <= permitted)
            .ok_or_else(exhausted)?;
        Ok(())
    }

    pub(super) fn push<T>(
        &mut self,
        values: &mut Vec<T>,
        value: T,
    ) -> Result<(), ManufacturingError> {
        self.reserve(values, values.len().checked_add(1).ok_or_else(exhausted)?)?;
        values.push(value);
        Ok(())
    }

    pub(super) fn release<T>(&mut self, values: Vec<T>) {
        self.0 -= values.capacity() * size_of::<T>();
        drop(values);
    }
}

pub(super) fn debit(
    counter: &mut usize,
    count: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    cancelled(cancel)?;
    *counter = counter.checked_add(count).ok_or_else(overflow)?;
    Ok(())
}

fn separated(first: [f64; 2], second: [f64; 2], gap: f64) -> Result<bool, ManufacturingError> {
    Ok(enclosed_vertical_gap_mm(first[0], second[1])? >= gap
        || enclosed_vertical_gap_mm(second[0], first[1])? >= gap)
}

#[derive(Default)]
pub(in crate::verifier) struct ClearanceReceipt {
    pub shared_edge_cells: usize,
    pub checked_pairs: usize,
}

pub(in crate::verifier) struct Clearance {
    roads: Vec<Road>,
    admission: Admission,
    z_gap: f64,
    xy_gap: f64,
}

impl Clearance {
    pub(in crate::verifier) fn new(
        cell_size: u32,
        policy: &SupportPolicy,
    ) -> Result<Self, ManufacturingError> {
        validate_cell_size(cell_size)?;
        let SupportPolicy::Breakaway {
            z_gap_mm,
            xy_clearance_mm,
            ..
        } = *policy
        else {
            return Err(failed("actual support requires breakaway policy"));
        };
        Ok(Self {
            roads: Vec::new(),
            admission: Admission::default(),
            z_gap: z_gap_mm,
            xy_gap: xy_clearance_mm,
        })
    }

    pub(in crate::verifier) fn record(
        &mut self,
        input: RoadInput,
    ) -> Result<(), ManufacturingError> {
        if input.role.is_model()
            || matches!(
                input.role,
                DepositionKind::Support | DepositionKind::SupportInterface
            )
        {
            self.admission.push(&mut self.roads, Road::new(input)?)?;
        }
        Ok(())
    }

    pub(in crate::verifier) fn verify_ancestry(
        &self,
        plan: &NormalizedPrintPlan,
        recipe: &PlanarPrintRecipe,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        super::ancestry::verify(
            &self.roads,
            &plan.layers,
            recipe,
            self.z_gap,
            Admission(self.admission.0),
            work,
            cancel,
        )
    }

    pub(in crate::verifier) fn verify(
        mut self,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<ClearanceReceipt, ManufacturingError> {
        let mut bands = Vec::new();
        let mut start = 0;
        let mut prefix_high = f64::NEG_INFINITY;
        for roads in self.roads.chunk_by(|one, next| one.layer == next.layer) {
            let mut model = [f64::INFINITY, f64::NEG_INFINITY];
            let mut support = model;
            for road in roads {
                debit(work, 1, cancel)?;
                let bounds = if road.support {
                    &mut support
                } else {
                    &mut model
                };
                bounds[0] = bounds[0].min(road.height[0]);
                bounds[1] = bounds[1].max(road.height[1]);
            }
            prefix_high = prefix_high.max(model[1]);
            let end = start + roads.len();
            self.admission.push(
                &mut bands,
                Band {
                    roads: start..end,
                    model,
                    support,
                    prefix_high,
                    suffix_model_low: model[0],
                    future_support_low: f64::INFINITY,
                    rows: None,
                },
            )?;
            start = end;
        }
        let mut future_support_low = f64::INFINITY;
        let mut suffix_model_low = f64::INFINITY;
        for band in bands.iter_mut().rev() {
            debit(work, 1, cancel)?;
            band.future_support_low = future_support_low;
            future_support_low = future_support_low.min(band.support[0]);
            suffix_model_low = suffix_model_low.min(band.model[0]);
            band.suffix_model_low = suffix_model_low;
        }
        let mut marks = Vec::new();
        self.admission.reserve(&mut marks, self.roads.len())?;
        marks.resize(self.roads.len(), usize::MAX);
        let mut selected = Vec::new();
        let mut overlaps = Vec::new();
        let mut receipt = ClearanceReceipt::default();
        let mut native_work = spiling_geometry::GeometryWorkBudget::compile();
        let mut discard_before = 0;
        for ordinal in 0..bands.len() {
            debit(work, 1, cancel)?;
            let [floor, top] = bands[ordinal].support;
            if !floor.is_finite() {
                continue;
            }
            let low_bound = (floor - self.z_gap).next_down();
            let high_bound = (top + self.z_gap).next_up();
            let first = bands.partition_point(|band| band.prefix_high < low_bound);
            let end = bands.partition_point(|band| band.suffix_model_low <= high_bound);
            let windows = &mut bands[first..end.max(first)];
            for band in &mut *windows {
                debit(work, 1, cancel)?;
                if band.rows.is_none()
                    && band.model[0].is_finite()
                    && !separated([floor, top], band.model, self.z_gap)?
                {
                    band.rows = Some(Rows::build(
                        &self.roads,
                        band.roads.clone(),
                        self.xy_gap,
                        &mut self.admission,
                        work,
                        cancel,
                    )?);
                }
            }
            let roads = bands[ordinal].roads.clone();
            let windows = &bands[first..end.max(first)];
            for support_index in roads.filter(|index| self.roads[*index].support) {
                debit(work, 1, cancel)?;
                let support = self.roads[support_index];
                selected.clear();
                let mut raster_work = 0;
                support.chord.outer_rows(
                    support.outer_radius(0.0)?,
                    &mut raster_work,
                    cancel,
                    |row, left, right| {
                        overlaps.clear();
                        for band in windows {
                            debit(work, 1, cancel)?;
                            if let Some(rows) = &band.rows {
                                for entry in rows.intersect(row, left, right) {
                                    debit(work, 1, cancel)?;
                                    if entry.right < left
                                        || separated(
                                            support.height,
                                            self.roads[entry.road].height,
                                            self.z_gap,
                                        )?
                                    {
                                        continue;
                                    }
                                    self.admission.push(
                                        &mut overlaps,
                                        (left.max(entry.left), right.min(entry.right)),
                                    )?;
                                    if marks[entry.road] != support_index {
                                        marks[entry.road] = support_index;
                                        self.admission.push(&mut selected, entry.road)?;
                                    }
                                }
                            }
                        }
                        count_union(&mut overlaps, &mut receipt.shared_edge_cells, work, cancel)
                    },
                )?;
                debit(work, raster_work, cancel)?;
                sort_by_key(&mut selected, |index| *index, work, cancel)?;
                let capsule = support
                    .chord
                    .clearance_capsule(support.radius_mm, support.arc_deviation_mm)?;
                for &index in &selected {
                    debit(work, 1, cancel)?;
                    debit(&mut receipt.checked_pairs, 1, cancel)?;
                    let model = self.roads[index];
                    let model_capsule = model
                        .chord
                        .clearance_capsule(model.radius_mm, model.arc_deviation_mm)?;
                    spiling_geometry::certify_planar_capsule_pair_clearance([capsule, model_capsule], self.xy_gap, &mut native_work, cancel)
                        .map_err(|error| {
                            match error.code {
                                GeometryErrorCode::Cancelled => return ManufacturingError::new(ManufacturingErrorCode::Cancelled, error.message),
                                GeometryErrorCode::ResourceLimit => return ManufacturingError::new(ManufacturingErrorCode::ResourceLimit, error.message),
                                _ => {}
                            }
                            failed(format!("actual support layer {} (line {}) violates object XY/Z clearance at model layer {} (line {}): {error:?}", support.layer, support.line, model.layer, model.line))
                        })?;
                }
            }
            let future_low = bands[ordinal].future_support_low;
            let next = bands
                .partition_point(|band| band.prefix_high < (future_low - self.z_gap).next_down());
            for band in &mut bands[discard_before..next] {
                debit(work, 1, cancel)?;
                if let Some(rows) = band.rows.take() {
                    rows.release(&mut self.admission);
                }
            }
            discard_before = next;
        }
        Ok(receipt)
    }
}

struct Band {
    roads: Range<usize>,
    model: [f64; 2],
    support: [f64; 2],
    prefix_high: f64,
    suffix_model_low: f64,
    future_support_low: f64,
    rows: Option<Rows>,
}

pub(super) struct Incidence {
    row: i64,
    left: i64,
    pub(super) right: i64,
    prefix_right: i64,
    pub(super) road: usize,
}

struct RowDirectory {
    first: i64,
    len: usize,
}

#[derive(Default)]
pub(super) struct Rows {
    entries: Vec<Incidence>,
    directory: Option<RowDirectory>,
}

impl Rows {
    fn build(
        roads: &[Road],
        span: Range<usize>,
        clearance_mm: f64,
        admission: &mut Admission,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let mut entries = Self::default();
        for index in span {
            debit(work, 1, cancel)?;
            let road = roads[index];
            if !road.support {
                entries.insert(
                    index,
                    road.chord,
                    road.outer_radius(clearance_mm)?,
                    admission,
                    work,
                    cancel,
                )?;
            }
        }
        entries.finish(work, cancel)?;
        Ok(entries)
    }

    pub(super) fn insert(
        &mut self,
        road: usize,
        chord: Chord,
        radius: i64,
        admission: &mut Admission,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        chord.outer_rows(radius, work, cancel, |row, left, right| {
            admission.push(
                &mut self.entries,
                Incidence {
                    row,
                    left,
                    right,
                    prefix_right: right,
                    road,
                },
            )
        })
    }

    pub(super) fn finish(
        &mut self,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        sort_by_key(
            &mut self.entries,
            |entry| (entry.row, entry.left, entry.road),
            work,
            cancel,
        )?;
        let Some(first) = self.entries.first().map(|entry| entry.row) else {
            return Ok(());
        };
        let mut previous = i64::MIN;
        let mut prefix = i64::MIN;
        let mut dense = true;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            debit(work, 1, cancel)?;
            if entry.row != previous {
                previous = entry.row;
                prefix = i64::MIN;
                dense &= usize::try_from(entry.row - first).is_ok_and(|row| row <= index);
            }
            prefix = prefix.max(entry.right);
            entry.prefix_right = prefix;
        }
        if dense {
            let mut next_offset = 0;
            let mut start = 0;
            while start < self.entries.len() {
                let row = self.entries[start].row;
                let end = start
                    + 1
                    + self.entries[start + 1..]
                        .iter()
                        .take_while(|entry| entry.row == row)
                        .count();
                let offset = (row - first) as usize;
                debit(work, end - start, cancel)?;
                for entry in &mut self.entries[next_offset..=offset] {
                    entry.row = start as i64;
                }
                next_offset = offset + 1;
                start = end;
            }
            self.directory = Some(RowDirectory {
                first,
                len: next_offset,
            });
        }
        Ok(())
    }

    pub(super) fn release(self, admission: &mut Admission) {
        admission.release(self.entries);
    }

    pub(super) fn intersect(&self, row: i64, first: i64, last: i64) -> &[Incidence] {
        let Some(directory) = &self.directory else {
            let start = self
                .entries
                .partition_point(|entry| (entry.row, entry.prefix_right) < (row, first));
            let end = self
                .entries
                .partition_point(|entry| (entry.row, entry.left) <= (row, last));
            return &self.entries[start..end.max(start)];
        };
        let Some(offset) = row
            .checked_sub(directory.first)
            .and_then(|offset| usize::try_from(offset).ok())
            .filter(|offset| *offset < directory.len)
        else {
            return &[];
        };
        let start = self.entries[offset].row as usize;
        let end = if offset + 1 == directory.len {
            self.entries.len()
        } else {
            self.entries[offset + 1].row as usize
        };
        let entries = &self.entries[start..end];
        let start = entries.partition_point(|entry| entry.prefix_right < first);
        let end = entries[start..].partition_point(|entry| entry.left <= last);
        &entries[start..start + end]
    }
}

fn sort_by_key<T, K: Ord>(
    values: &mut [T],
    mut key: impl FnMut(&T) -> K,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut comparisons = 0usize;
    values.sort_unstable_by(|first, second| {
        comparisons = comparisons.saturating_add(1);
        key(first).cmp(&key(second))
    });
    debit(work, comparisons, cancel)
}

fn count_union(
    ranges: &mut [(i64, i64)],
    count: &mut usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    sort_by_key(ranges, |range| *range, work, cancel)?;
    let mut previous_end = i64::MIN;
    for &(start, end) in ranges.iter() {
        let first = start.max(previous_end.saturating_add(1));
        if first <= end {
            debit(
                count,
                usize::try_from(end - first + 1).map_err(|_| overflow())?,
                cancel,
            )?;
        }
        previous_end = previous_end.max(end);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clearance() -> Clearance {
        Clearance::new(
            50_000,
            &SupportPolicy::Breakaway {
                density: 0.2,
                xy_clearance_mm: 0.25,
                z_gap_mm: 0.5,
                interface_thickness_mm: 0.25,
            },
        )
        .unwrap()
    }

    fn road(layer: u32, role: DepositionKind, y: i64, z: i64) -> RoadInput {
        RoadInput {
            layer,
            line: layer as usize + 1,
            path: 0,
            role,
            from: [0, y, z],
            to: [1_000_000, y, z],
            physical_z: [0.0, 0.25],
            maximum_width_mm: 0.25,
            minimum_width_mm: 0.25,
            length_upper_mm: 1.0,
            arc_deviation_mm: 0.0,
        }
    }

    #[test]
    fn shared_cells_resolve_locally_and_count_each_pair_once() {
        let cancel = AtomicBool::new(false);
        for lateral in [499_999, 500_000, 500_001, 600_000] {
            let mut grid = clearance();
            grid.record(road(0, DepositionKind::Perimeter, lateral, 250_000))
                .unwrap();
            grid.record(road(0, DepositionKind::Support, 0, 250_000))
                .unwrap();
            let result = grid.verify(&mut 0, &cancel);
            if lateral == 499_999 {
                assert!(result.err().unwrap().message.contains("line 1"));
            } else {
                let receipt = result.unwrap();
                assert_eq!(receipt.checked_pairs, usize::from(lateral < 600_000));
                assert_eq!(receipt.shared_edge_cells > 0, lateral < 600_000);
            }
        }
    }

    #[test]
    fn z_window_reaches_both_nonadjacent_sides_and_keeps_closed_ties() {
        let cancel = AtomicBool::new(false);
        for above in [false, true] {
            for overlap in [false, true] {
                let mut grid = clearance();
                let model_top = if above {
                    2_250_000 - i64::from(overlap)
                } else {
                    750_000 + i64::from(overlap)
                };
                let model = road(
                    if above { 4 } else { 0 },
                    DepositionKind::Perimeter,
                    0,
                    model_top,
                );
                if !above {
                    grid.record(model).unwrap();
                }
                grid.record(road(1, DepositionKind::Perimeter, 2_000_000, 875_000))
                    .unwrap();
                grid.record(road(2, DepositionKind::Support, 0, 1_500_000))
                    .unwrap();
                grid.record(road(3, DepositionKind::Perimeter, 2_000_000, 1_625_000))
                    .unwrap();
                if above {
                    grid.record(road(4, DepositionKind::Perimeter, 0, model_top))
                        .unwrap();
                }
                let result = grid.verify(&mut 0, &cancel);
                assert_eq!(result.is_err(), overlap, "above={above} overlap={overlap}");
            }
        }
    }

    #[test]
    fn capture_admits_the_live_edge_capacity_before_allocating() {
        let allowance = crate::working_set::WorkingSet::new(size_of::<Road>() - 1).unwrap();
        let refusal = crate::working_set::scope(allowance, || {
            clearance()
                .record(road(0, DepositionKind::Support, 0, 250_000))
                .unwrap_err()
        });
        assert_eq!(refusal.code, ManufacturingErrorCode::ResourceLimit);
    }

    #[test]
    fn signed_row_directory_preserves_order_empty_rows_and_closed_columns() {
        let cancel = AtomicBool::new(false);
        for (next_row, dense) in [(0, true), (9_000_000_000, false)] {
            let mut rows = Rows::default();
            let mut admission = Admission::default();
            for (road, row, left, right) in [
                (2, -2, 1, 2),
                (3, next_row, 1, 2),
                (0, -2, -4, 3),
                (1, -2, 1, 1),
            ] {
                admission
                    .push(
                        &mut rows.entries,
                        Incidence {
                            row,
                            left,
                            right,
                            prefix_right: right,
                            road,
                        },
                    )
                    .unwrap();
            }
            let allowance = crate::working_set::WorkingSet::new(admission.0).unwrap();
            crate::working_set::scope(allowance, || rows.finish(&mut 0, &cancel)).unwrap();
            assert_eq!(rows.directory.is_some(), dense);
            let roads = |row, left, right| {
                rows.intersect(row, left, right)
                    .iter()
                    .map(|entry| entry.road)
                    .collect::<Vec<_>>()
            };
            assert_eq!(roads(-2, 1, 1), [0, 1, 2]);
            assert_eq!(roads(-2, -4, -4), [0]);
            assert_eq!(roads(next_row, 2, 2), [3]);
            for row in [i64::MIN, -3, -1, next_row + 1, i64::MAX] {
                assert!(roads(row, i64::MIN, i64::MAX).is_empty());
            }
            assert!(roads(-2, 4, 4).is_empty());
            rows.release(&mut admission);
            assert_eq!(admission.0, 0);
        }
    }
}
