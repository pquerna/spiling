use super::{CELL_TICKS, Chord, MAX_XYZ6_TICKS, cancelled, failed, overflow, validate_cell_size};
use spiling_contracts::geometry::GeometryErrorCode;
use spiling_contracts::manufacturing::{
    DepositionKind, ManufacturingError, ManufacturingErrorCode, NormalizedPrintPlan,
    PlanarPrintRecipe, SupportPolicy,
    program_coordinates::{enclosed_vertical_gap_mm, xyz6_coordinate_error_mm},
};
use std::{mem::size_of, ops::Range, sync::atomic::AtomicBool};

const UNSEEN: usize = usize::MAX;
const DISJOINT: usize = usize::MAX - 1;

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

    /// Charge bytes another task admitted from this same base, so the
    /// combined live total stays within the working set.
    pub(super) fn absorb(&mut self, bytes: usize) -> Result<(), ManufacturingError> {
        self.0 = self
            .0
            .checked_add(bytes)
            .filter(|live| *live <= crate::working_set::bytes())
            .ok_or_else(exhausted)?;
        Ok(())
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

#[derive(Debug, Default, PartialEq, Eq)]
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
            z_gap: spiling_contracts::manufacturing::program_coordinates::honoured_z_gap_mm(
                z_gap_mm,
            ),
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
        self,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<ClearanceReceipt, ManufacturingError> {
        self.verify_screened(work, cancel, true)
    }

    fn verify_screened(
        mut self,
        work: &mut usize,
        cancel: &AtomicBool,
        screen: bool,
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
        marks.resize(self.roads.len(), UNSEEN);
        if screen {
            self.screen_disjoint(&bands, &mut marks, work, cancel)?;
        }
        let mut receipt = ClearanceReceipt::default();
        let mut discard_before = 0;
        let working_set = crate::working_set::current();
        const BAND_WAVE: usize = 32;
        use rayon::prelude::*;
        let mut wave_start = 0;
        while wave_start < bands.len() {
            let wave_end = (wave_start + BAND_WAVE).min(bands.len());
            // Collect all distinct model bands needed by any support band in this wave.
            let mut needed_rows = Vec::new();
            for ordinal in wave_start..wave_end {
                debit(work, 1, cancel)?;
                let [floor, top] = bands[ordinal].support;
                if !floor.is_finite() {
                    continue;
                }
                let window = model_window(&bands, [floor, top], self.z_gap);
                for b in window {
                    debit(work, 1, cancel)?;
                    if bands[b].rows.is_none()
                        && bands[b].model[0].is_finite()
                        && !separated([floor, top], bands[b].model, self.z_gap)?
                        && !needed_rows.contains(&b)
                    {
                        needed_rows.push(b);
                    }
                }
            }
            if !needed_rows.is_empty() {
                let base_admission = self.admission.0;
                let roads = &self.roads;
                let xy_gap = self.xy_gap;
                let marks_ref = &marks;
                let spans: Vec<(usize, Range<usize>)> = needed_rows
                    .iter()
                    .map(|&b| (b, bands[b].roads.clone()))
                    .collect();
                let built: Vec<Result<(usize, Rows, usize, usize), ManufacturingError>> = spans
                    .par_iter()
                    .map(|&(b, ref span)| {
                        crate::working_set::scope(working_set, || {
                            let mut local_admission = Admission(base_admission);
                            let mut local_work = 0usize;
                            let rows = Rows::build(
                                roads,
                                span.clone(),
                                xy_gap,
                                marks_ref,
                                &mut local_admission,
                                &mut local_work,
                                cancel,
                            )?;
                            Ok((b, rows, local_admission.0 - base_admission, local_work))
                        })
                    })
                    .collect();
                for result in built {
                    let (b, rows, added_bytes, local_work) = result?;
                    debit(work, local_work, cancel)?;
                    self.admission.absorb(added_bytes)?;
                    bands[b].rows = Some(rows);
                }
            }
            // Verify every support band in the wave in parallel, then reduce in band order.
            let base_admission = self.admission.0;
            let roads = &self.roads;
            let bands_ref = &bands;
            let marks_ref = &marks;
            let z_gap = self.z_gap;
            let xy_gap = self.xy_gap;
            let wave_results: Vec<Result<(usize, usize, usize), ManufacturingError>> = (wave_start
                ..wave_end)
                .into_par_iter()
                .map(|ordinal| {
                    crate::working_set::scope(working_set, || {
                        let [floor, top] = bands_ref[ordinal].support;
                        if !floor.is_finite() {
                            return Ok((0, 0, 0));
                        }
                        let window = model_window(bands_ref, [floor, top], z_gap);
                        let windows = &bands_ref[window];
                        let mut local_admission = Admission(base_admission);
                        let mut local_work = 0usize;
                        let mut shared_edge_cells = 0usize;
                        let mut checked_pairs = 0usize;
                        let mut selected = Vec::new();
                        let mut overlaps = Vec::new();
                        let mut native_work = spiling_geometry::GeometryWorkBudget::compile();
                        for support_index in bands_ref[ordinal]
                            .roads
                            .clone()
                            .filter(|index| roads[*index].support)
                        {
                            debit(&mut local_work, 1, cancel)?;
                            if marks_ref[support_index] == DISJOINT {
                                continue;
                            }
                            let support = roads[support_index];
                            selected.clear();
                            let mut raster_work = 0;
                            support.chord.outer_rows(
                                support.outer_radius(0.0)?,
                                &mut raster_work,
                                cancel,
                                |row, left, right| {
                                    overlaps.clear();
                                    for band in windows {
                                        debit(&mut local_work, 1, cancel)?;
                                        if let Some(rows) = &band.rows {
                                            for entry in rows.intersect(row, left, right) {
                                                debit(&mut local_work, 1, cancel)?;
                                                if entry.right < left
                                                    || separated(
                                                        support.height,
                                                        roads[entry.road].height,
                                                        z_gap,
                                                    )?
                                                {
                                                    continue;
                                                }
                                                local_admission.push(
                                                    &mut overlaps,
                                                    (left.max(entry.left), right.min(entry.right)),
                                                )?;
                                                if !selected.contains(&entry.road) {
                                                    local_admission
                                                        .push(&mut selected, entry.road)?;
                                                }
                                            }
                                        }
                                    }
                                    count_union(
                                        &mut overlaps,
                                        &mut shared_edge_cells,
                                        &mut local_work,
                                        cancel,
                                    )
                                },
                            )?;
                            debit(&mut local_work, raster_work, cancel)?;
                            sort_by_key(&mut selected, |index| *index, &mut local_work, cancel)?;
                            let capsule = support
                                .chord
                                .clearance_capsule(support.radius_mm, support.arc_deviation_mm)?;
                            for &index in &selected {
                                debit(&mut local_work, 1, cancel)?;
                                debit(&mut checked_pairs, 1, cancel)?;
                                let model = roads[index];
                                let model_capsule = model
                                    .chord
                                    .clearance_capsule(model.radius_mm, model.arc_deviation_mm)?;
                                spiling_geometry::certify_planar_capsule_pair_clearance(
                                    [capsule, model_capsule],
                                    xy_gap,
                                    &mut native_work,
                                    cancel,
                                )
                                .map_err(|error| {
                                    match error.code {
                                        GeometryErrorCode::Cancelled => {
                                            return ManufacturingError::new(
                                                ManufacturingErrorCode::Cancelled,
                                                error.message,
                                            );
                                        }
                                        GeometryErrorCode::ResourceLimit => {
                                            return ManufacturingError::new(
                                                ManufacturingErrorCode::ResourceLimit,
                                                error.message,
                                            );
                                        }
                                        _ => {}
                                    }
                                    failed(format!(
                                        "actual support layer {} (line {}) violates object XY/Z clearance at model layer {} (line {}): {error:?}; support {:?} r={} e={}, model {:?} r={} e={}, gap {}",
                                        support.layer,
                                        support.line,
                                        model.layer,
                                        model.line,
                                        capsule.segment_xy_mm,
                                        capsule.radius_mm,
                                        capsule.endpoint_error_mm,
                                        model_capsule.segment_xy_mm,
                                        model_capsule.radius_mm,
                                        model_capsule.endpoint_error_mm,
                                        xy_gap
                                    ))
                                })?;
                            }
                        }
                        Ok((shared_edge_cells, checked_pairs, local_work))
                    })
                })
                .collect();
            for result in wave_results {
                let (shared, pairs, local_work) = result?;
                debit(work, local_work, cancel)?;
                debit(&mut receipt.shared_edge_cells, shared, cancel)?;
                debit(&mut receipt.checked_pairs, pairs, cancel)?;
            }
            let future_low = bands[wave_end - 1].future_support_low;
            let next = bands
                .partition_point(|band| band.prefix_high < (future_low - self.z_gap).next_down());
            for band in &mut bands[discard_before..next] {
                debit(work, 1, cancel)?;
                if let Some(rows) = band.rows.take() {
                    rows.release(&mut self.admission);
                }
            }
            discard_before = next;
            wave_start = wave_end;
        }
        Ok(receipt)
    }

    fn screen_disjoint(
        &self,
        bands: &[Band],
        marks: &mut [usize],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        use rayon::prelude::*;
        let Some(forest) = ChordForest::build(self, bands, work, cancel)? else {
            return Ok(());
        };
        marks.fill(DISJOINT);
        let support_bands: Vec<usize> = bands
            .iter()
            .enumerate()
            .filter_map(|(i, band)| band.support[0].is_finite().then_some(i))
            .collect();
        type ScreenResult = Result<Option<(Vec<usize>, usize)>, ManufacturingError>;
        let results: Vec<ScreenResult> = support_bands
            .par_iter()
            .map(|&band_idx| {
                let band = &bands[band_idx];
                let mut local_work = 1usize;
                let mut touched = Vec::new();
                let roots = &forest.roots[model_window(bands, band.support, self.z_gap)];
                for index in band
                    .roads
                    .clone()
                    .filter(|index| self.roads[*index].support)
                {
                    debit(&mut local_work, 1, cancel)?;
                    let Ok(bounds) = CellBounds::for_road(self.roads[index], 0.0) else {
                        return Ok(None);
                    };
                    let mut hits = false;
                    for &root in roots.iter().flatten() {
                        hits |= forest.collect_hits(
                            root,
                            bounds,
                            &mut touched,
                            &mut local_work,
                            cancel,
                        )?;
                    }
                    if hits {
                        touched.push(index);
                    }
                }
                Ok(Some((touched, local_work)))
            })
            .collect();
        for result in results {
            let Some((touched, local_work)) = result? else {
                marks.fill(UNSEEN);
                return Ok(());
            };
            debit(work, local_work, cancel)?;
            for idx in touched {
                marks[idx] = UNSEEN;
            }
        }
        Ok(())
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

fn model_window(bands: &[Band], support: [f64; 2], gap: f64) -> Range<usize> {
    let low = (support[0] - gap).next_down();
    let high = (support[1] + gap).next_up();
    let first = bands.partition_point(|band| band.prefix_high < low);
    let end = bands.partition_point(|band| band.suffix_model_low <= high);
    first..end.max(first)
}

#[derive(Clone, Copy)]
struct CellBounds {
    low: [i64; 2],
    high: [i64; 2],
}

impl CellBounds {
    fn for_road(road: Road, gap: f64) -> Result<Self, ManufacturingError> {
        let radius = road.outer_radius(gap)?;
        let [start, end] = [road.chord.start, road.chord.end];
        Ok(Self {
            low: std::array::from_fn(|axis| {
                (start[axis].min(end[axis]) - radius - 1).div_euclid(CELL_TICKS)
            }),
            high: std::array::from_fn(|axis| {
                (start[axis].max(end[axis]) + radius).div_euclid(CELL_TICKS)
            }),
        })
    }

    fn union(self, other: Self) -> Self {
        Self {
            low: std::array::from_fn(|axis| self.low[axis].min(other.low[axis])),
            high: std::array::from_fn(|axis| self.high[axis].max(other.high[axis])),
        }
    }

    fn intersects(self, other: Self) -> bool {
        (0..2).all(|axis| self.low[axis] <= other.high[axis] && other.low[axis] <= self.high[axis])
    }
}

struct ChordNode {
    bounds: CellBounds,
    first: usize,
    second: usize,
}

#[derive(Default)]
struct ChordForest {
    nodes: Vec<ChordNode>,
    roots: Vec<Option<usize>>,
}

impl ChordForest {
    fn build(
        clearance: &Clearance,
        bands: &[Band],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let mut model_count = 0usize;
        for road in &clearance.roads {
            debit(work, 1, cancel)?;
            model_count += usize::from(!road.support);
        }
        let Some(nodes) = model_count.checked_mul(2) else {
            return Ok(None);
        };
        let mut forest = Self::default();
        let mut scratch = Admission(clearance.admission.0);
        if scratch.reserve(&mut forest.nodes, nodes).is_err()
            || scratch.reserve(&mut forest.roots, bands.len()).is_err()
        {
            return Ok(None);
        }
        use rayon::prelude::*;
        type BandForest =
            Result<Option<(Vec<ChordNode>, Option<usize>, usize)>, ManufacturingError>;
        let built: Vec<BandForest> = bands
            .par_iter()
            .map(|band| {
                let mut local = Self::default();
                let mut local_work = 1usize;
                for index in band.roads.clone() {
                    debit(&mut local_work, 1, cancel)?;
                    let road = clearance.roads[index];
                    if road.support {
                        continue;
                    }
                    let Ok(bounds) = CellBounds::for_road(road, clearance.xy_gap) else {
                        return Ok(None);
                    };
                    local.nodes.push(ChordNode {
                        bounds,
                        first: index,
                        second: UNSEEN,
                    });
                }
                let end = local.nodes.len();
                let root = if end == 0 {
                    None
                } else {
                    Some(local.group(0..end, &mut local_work, cancel)?)
                };
                Ok(Some((local.nodes, root, local_work)))
            })
            .collect();
        for result in built {
            let Some((local_nodes, local_root, local_work)) = result? else {
                return Ok(None);
            };
            debit(work, local_work, cancel)?;
            let base = forest.nodes.len();
            for mut node in local_nodes {
                if node.second != UNSEEN {
                    node.first += base;
                    node.second += base;
                }
                forest.nodes.push(node);
            }
            forest.roots.push(local_root.map(|r| r + base));
        }
        Ok(Some(forest))
    }

    fn group(
        &mut self,
        span: Range<usize>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        if span.len() == 1 {
            return Ok(span.start);
        }
        let mut bounds = self.nodes[span.start].bounds;
        for node in &self.nodes[span.start + 1..span.end] {
            debit(work, 1, cancel)?;
            bounds = bounds.union(node.bounds);
        }
        let [width, height] = std::array::from_fn(|axis| bounds.high[axis] - bounds.low[axis]);
        let axis = usize::from(height > width);
        let half = span.len() / 2;
        let mut comparisons = 0usize;
        self.nodes[span.clone()].select_nth_unstable_by(half, |left, right| {
            comparisons = comparisons.saturating_add(1);
            (left.bounds.low[axis] + left.bounds.high[axis], left.first).cmp(&(
                right.bounds.low[axis] + right.bounds.high[axis],
                right.first,
            ))
        });
        debit(work, comparisons, cancel)?;
        let middle = span.start + half;
        let first = self.group(span.start..middle, work, cancel)?;
        let second = self.group(middle..span.end, work, cancel)?;
        let root = self.nodes.len();
        self.nodes.push(ChordNode {
            bounds,
            first,
            second,
        });
        Ok(root)
    }

    fn collect_hits(
        &self,
        root: usize,
        bounds: CellBounds,
        touched: &mut Vec<usize>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        debit(work, 1, cancel)?;
        let node = &self.nodes[root];
        if !bounds.intersects(node.bounds) {
            return Ok(false);
        }
        if node.second == UNSEEN {
            touched.push(node.first);
            return Ok(true);
        }
        let first = self.collect_hits(node.first, bounds, touched, work, cancel)?;
        let second = self.collect_hits(node.second, bounds, touched, work, cancel)?;
        Ok(first || second)
    }
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

#[derive(Clone, Copy, Default)]
struct RowBucket {
    next: usize,
    end: usize,
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
        marks: &[usize],
        admission: &mut Admission,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let mut entries = Self::default();
        for index in span {
            debit(work, 1, cancel)?;
            let road = roads[index];
            if !road.support && marks[index] != DISJOINT {
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

    /// Appends one road's `[row, left, right]` incidences, as `insert` would.
    pub(super) fn extend(
        &mut self,
        road: usize,
        rows: &[[i64; 3]],
        admission: &mut Admission,
    ) -> Result<(), ManufacturingError> {
        let required = self
            .entries
            .len()
            .checked_add(rows.len())
            .ok_or_else(overflow)?;
        admission.reserve(&mut self.entries, required)?;
        self.entries
            .extend(rows.iter().map(|&[row, left, right]| Incidence {
                row,
                left,
                right,
                prefix_right: right,
                road,
            }));
        Ok(())
    }

    pub(super) fn finish_ancestry(
        &mut self,
        admission: &mut Admission,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        cancelled(cancel)?;
        let Some(head) = self.entries.first() else {
            return Ok(());
        };
        let mut first = head.row;
        let mut last = first;
        for entry in &self.entries {
            debit(work, 1, cancel)?;
            first = first.min(entry.row);
            last = last.max(entry.row);
        }
        let Some(len) = last
            .checked_sub(first)
            .and_then(|span| usize::try_from(span).ok())
            .and_then(|span| span.checked_add(1))
            .filter(|len| *len <= self.entries.len())
        else {
            return self.finish(work, cancel);
        };
        let mut buckets = Vec::new();
        if admission.reserve(&mut buckets, len).is_err() {
            drop(buckets);
            return self.finish(work, cancel);
        }
        buckets.resize(len, RowBucket::default());
        let result = self.finish_bucketed(first, &mut buckets, work, cancel);
        admission.release(buckets);
        result
    }

    fn finish_bucketed(
        &mut self,
        first: i64,
        buckets: &mut [RowBucket],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        for entry in &self.entries {
            debit(work, 1, cancel)?;
            buckets[(entry.row - first) as usize].end += 1;
        }
        let mut end = 0;
        for bucket in &mut *buckets {
            debit(work, 1, cancel)?;
            bucket.next = end;
            end += bucket.end;
            bucket.end = end;
        }
        for index in 0..buckets.len() {
            while buckets[index].next < buckets[index].end {
                debit(work, 1, cancel)?;
                let current = buckets[index].next;
                let target = &mut buckets[(self.entries[current].row - first) as usize];
                self.entries.swap(current, target.next);
                target.next += 1;
            }
        }
        // Rows are disjoint ranges, so each sorts on its own; keys are
        // unique per row, so the order is the same on any thread.
        let sorted = sort_rows(&mut self.entries, buckets, 0, cancel)?;
        debit(work, sorted, cancel)?;
        let mut start = 0;
        for (entry, bucket) in self.entries.iter_mut().zip(&*buckets) {
            debit(work, 1, cancel)?;
            entry.row = start as i64;
            start = bucket.end;
        }
        self.directory = Some(RowDirectory {
            first,
            len: buckets.len(),
        });
        Ok(())
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

/// Sorts each bucket's entries by `(left, road)` and sets their running
/// `prefix_right`; `offset` is the index of `entries[0]` in the whole array.
/// Returns the work done.
fn sort_rows(
    entries: &mut [Incidence],
    buckets: &[RowBucket],
    offset: usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    const SERIAL_ENTRIES: usize = 4096;
    if buckets.len() > 1 && entries.len() > SERIAL_ENTRIES {
        let middle = buckets.len() / 2;
        let split = buckets[middle - 1].end - offset;
        let (low, high) = entries.split_at_mut(split);
        let (low, high) = rayon::join(
            || sort_rows(low, &buckets[..middle], offset, cancel),
            || sort_rows(high, &buckets[middle..], offset + split, cancel),
        );
        return low?.checked_add(high?).ok_or_else(overflow);
    }
    let mut work = 0;
    let mut start = 0;
    for bucket in buckets {
        let row = &mut entries[start..bucket.end - offset];
        start = bucket.end - offset;
        debit(&mut work, 1 + row.len(), cancel)?;
        sort_by_key(row, |entry| (entry.left, entry.road), &mut work, cancel)?;
        let mut prefix = i64::MIN;
        for entry in row {
            prefix = prefix.max(entry.right);
            entry.prefix_right = prefix;
        }
    }
    Ok(work)
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

    /// The profile tolerance the Z gap is honoured within, in XYZ6 ticks.
    const HONOURED_TICKS: i64 = 10_000;

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
    fn virtual_arc_chord_uses_the_decoded_z_for_clearance() {
        let cancel = AtomicBool::new(false);
        // One tick either side of the honoured gap.
        for shortfall in [-1, 1] {
            let mut grid = clearance();
            let mut arc = road(0, DepositionKind::Perimeter, 0, 250_000);
            arc.to = super::super::virtual_arc_xyz6([1.000_123, 0.0, 0.250_486], 250_000);
            assert_eq!(arc.to, [1_000_123, 0, 250_000]);
            arc.arc_deviation_mm = 0.011;
            grid.record(arc).unwrap();
            // The gap is honoured within the profile tolerance (10 000 ticks).
            grid.record(road(
                3,
                DepositionKind::Support,
                0,
                1_000_000 - HONOURED_TICKS - shortfall,
            ))
            .unwrap();
            let result = grid.verify(&mut 0, &cancel);
            if shortfall < 0 {
                assert_eq!(result.unwrap().checked_pairs, 0);
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .message
                        .contains("violates object XY/Z clearance")
                );
            }
        }
    }

    #[test]
    fn shared_cells_resolve_locally_and_count_each_pair_once() {
        let cancel = AtomicBool::new(false);
        for lateral in [499_999, 500_000, 500_001, 600_000] {
            for screen in [false, true] {
                let mut grid = clearance();
                grid.record(road(0, DepositionKind::Perimeter, lateral, 250_000))
                    .unwrap();
                grid.record(road(0, DepositionKind::Support, 0, 250_000))
                    .unwrap();
                let result = grid.verify_screened(&mut 0, &cancel, screen);
                if lateral == 499_999 {
                    assert!(result.err().unwrap().message.contains("line 1"));
                } else {
                    let receipt = result.unwrap();
                    assert_eq!(receipt.checked_pairs, usize::from(lateral < 600_000));
                    assert_eq!(receipt.shared_edge_cells > 0, lateral < 600_000);
                }
            }
        }
    }

    #[test]
    fn disjoint_members_of_an_overlapping_group_do_not_expand_rows() {
        let cancel = AtomicBool::new(false);
        let run = |screen| {
            let mut grid = clearance();
            for (x, role) in [
                (-1_000_000, DepositionKind::Perimeter),
                (1_000_000, DepositionKind::Perimeter),
                (0, DepositionKind::Support),
            ] {
                let mut input = road(0, role, 0, 250_000);
                input.from = [x, -20_000_000, 250_000];
                input.to = [x, 20_000_000, 250_000];
                input.length_upper_mm = 40.0;
                grid.record(input).unwrap();
            }
            let mut work = 0;
            (
                grid.verify_screened(&mut work, &cancel, screen).unwrap(),
                work,
            )
        };
        let (control, full_work) = run(false);
        let (screened, screened_work) = run(true);
        assert_eq!(screened, control);
        assert_eq!(screened, ClearanceReceipt::default());
        assert!(screened_work * 20 < full_work);
    }

    #[test]
    fn chord_screen_preserves_the_first_clearance_refusal() {
        let cancel = AtomicBool::new(false);
        let failure = |screen| {
            let mut grid = clearance();
            for (line, lateral, role) in [
                (3, -1_000_000, DepositionKind::Perimeter),
                (5, 100_000, DepositionKind::Perimeter),
                (6, -100_000, DepositionKind::Perimeter),
                (17, 0, DepositionKind::Support),
            ] {
                let mut input = road(0, role, lateral, 250_000);
                input.line = line;
                grid.record(input).unwrap();
            }
            grid.verify_screened(&mut 0, &cancel, screen).unwrap_err()
        };
        let reference = failure(false);
        let screened = failure(true);
        assert_eq!(
            (screened.code, &screened.message),
            (reference.code, &reference.message)
        );
        assert!(screened.message.contains("line 17"));
        assert!(screened.message.contains("line 5"));
    }

    #[test]
    fn chord_screen_declines_unadmitted_scratch() {
        let cancel = AtomicBool::new(false);
        let mut grid = clearance();
        grid.record(road(0, DepositionKind::Perimeter, 0, 250_000))
            .unwrap();
        let allowance = crate::working_set::WorkingSet::new(grid.admission.0).unwrap();
        crate::working_set::scope(allowance, || {
            assert!(
                ChordForest::build(&grid, &[], &mut 0, &cancel)
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn z_window_reaches_both_nonadjacent_sides_and_keeps_closed_ties() {
        let cancel = AtomicBool::new(false);
        for above in [false, true] {
            for overlap in [false, true] {
                let mut grid = clearance();
                let model_top = if above {
                    2_250_000 - HONOURED_TICKS - if overlap { 1 } else { -1 }
                } else {
                    750_000 + HONOURED_TICKS + if overlap { 1 } else { -1 }
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

    fn ancestry_rows(last: i64) -> (Rows, Admission) {
        let mut rows = Rows::default();
        let mut admission = Admission::default();
        for (road, row, left, right) in [
            (5, last, 2, 5),
            (6, 1, 0, 4),
            (4, -4, 1, 1),
            (1, last, 2, 2),
            (7, 1, 1, 3),
            (0, -4, -3, 4),
            (2, last, -1, 0),
            (3, last, 1, 2),
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
        (rows, admission)
    }

    #[test]
    fn bucketed_ancestry_preserves_queries_with_signed_rows_and_pressure_fallback() {
        let cancel = AtomicBool::new(false);
        for (last, scratch, bucketed) in [(3, 8, true), (3, 0, false), (100, 8, false)] {
            let (mut control, _) = ancestry_rows(last);
            control.finish(&mut 0, &cancel).unwrap();
            let (mut rows, mut admission) = ancestry_rows(last);
            let live = admission.0;
            let allowance =
                crate::working_set::WorkingSet::new(live + scratch * size_of::<RowBucket>())
                    .unwrap();
            crate::working_set::scope(allowance, || {
                rows.finish_ancestry(&mut admission, &mut 0, &cancel)
            })
            .unwrap();
            assert_eq!(rows.directory.is_some(), bucketed);
            for row in -5..=last + 1 {
                for left in -4..=6 {
                    for right in left..=6 {
                        let signature = |index: &Rows| {
                            index
                                .intersect(row, left, right)
                                .iter()
                                .map(|entry| (entry.road, entry.left, entry.right))
                                .collect::<Vec<_>>()
                        };
                        assert_eq!(signature(&rows), signature(&control));
                    }
                }
            }
            assert_eq!(admission.0, live);
            rows.release(&mut admission);
            assert_eq!(admission.0, 0);
        }
    }

    #[test]
    fn ancestry_bucket_refusal_does_not_retain_scratch() {
        let (mut rows, mut admission) = ancestry_rows(3);
        let live = admission.0;
        let cancel = AtomicBool::new(true);
        let refusal = rows
            .finish_ancestry(&mut admission, &mut 0, &cancel)
            .unwrap_err();
        assert_eq!(refusal.code, ManufacturingErrorCode::Cancelled);
        assert_eq!(admission.0, live);

        let cancel = AtomicBool::new(false);
        let mut work = usize::MAX - rows.entries.len();
        let refusal = rows
            .finish_ancestry(&mut admission, &mut work, &cancel)
            .unwrap_err();
        assert_eq!(refusal, overflow());
        assert_eq!(admission.0, live);
        assert!(rows.directory.is_none());
    }
}
