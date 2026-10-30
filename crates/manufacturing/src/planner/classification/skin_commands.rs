// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Minimum motion construction, not a reclassification of positive skin.
use super::*;
use spiling_contracts::manufacturing::program_coordinates::{
    MIN_DEPOSITION_COMMAND_MM, project_xyz6_coordinate_mm, project_xyz6_point_mm,
    xyz6_coordinate_error_mm,
};
use std::borrow::Cow;
use std::mem::size_of;

fn refused(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::UnsupportedGeometry, message)
}
fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}

struct Budget {
    bytes: usize,
}
impl Budget {
    fn admit(
        &self,
        extra: usize,
        work: usize,
        admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        let bytes = byte_sum(self.bytes, extra)?;
        admit(bytes.div_ceil(size_of::<geo::Coord<f64>>()), bytes, work)
    }
    fn charge(
        &self,
        extra: usize,
        amount: usize,
        work: &mut usize,
        admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        super::super::boolean::charge(work, amount)?;
        self.admit(extra, amount, admit)
    }
    fn retain(&mut self, bytes: usize) -> Result<(), ManufacturingError> {
        self.bytes = byte_sum(self.bytes, bytes)?;
        Ok(())
    }
}

#[derive(Default)]
struct Rows {
    hits: Vec<f64>,
    intervals: Vec<[f64; 2]>,
}
impl Rows {
    fn bytes(&self) -> usize {
        capacity_bytes(&self.hits) + capacity_bytes(&self.intervals)
    }
    fn read(
        &mut self,
        masks: &[DiaphragmPolygonMm],
        along: usize,
        coordinate: f64,
        projected: bool,
        budget: &Budget,
        extra: usize,
        work: &mut usize,
        admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        self.intervals.clear();
        for polygon in masks {
            check_cancel(cancel)?;
            let count = polygon.outer.len() + polygon.holes.iter().map(Vec::len).sum::<usize>();
            budget.charge(extra + self.bytes(), count, work, admit)?;
            if self.hits.capacity() < count {
                budget.admit(extra + self.bytes() + count * size_of::<f64>(), 0, admit)?;
                self.hits
                    .try_reserve_exact(count - self.hits.len())
                    .map_err(|_| resource("skin fitting row allocation refused"))?;
            }
            self.hits.clear();
            for ring in std::iter::once(&polygon.outer).chain(&polygon.holes) {
                for edge in ring.windows(2) {
                    let (a, b) = (edge[0], edge[1]);
                    let fixed = 1 - along;
                    if (a[fixed] <= coordinate && coordinate < b[fixed])
                        || (b[fixed] <= coordinate && coordinate < a[fixed])
                    {
                        let t = (coordinate - a[fixed]) / (b[fixed] - a[fixed]);
                        let value = a[along] + t * (b[along] - a[along]);
                        if !value.is_finite() {
                            return Err(refused("skin fitting row is unconditioned"));
                        }
                        self.hits.push(value);
                    }
                }
            }
            budget.charge(
                extra + self.bytes(),
                self.hits.len().saturating_mul(
                    (usize::BITS - self.hits.len().max(1).leading_zeros()) as usize + 1,
                ),
                work,
                admit,
            )?;
            self.hits.sort_unstable_by(f64::total_cmp);
            if !self.hits.len().is_multiple_of(2) {
                return Err(refused("skin fitting row has odd parity"));
            }
            let needed = self.intervals.len() + self.hits.len() / 2;
            if self.intervals.capacity() < needed {
                budget.admit(
                    extra + self.bytes() + needed * size_of::<[f64; 2]>(),
                    0,
                    admit,
                )?;
                self.intervals
                    .try_reserve_exact(needed - self.intervals.len())
                    .map_err(|_| resource("skin fitting interval allocation refused"))?;
            }
            for pair in self.hits.chunks_exact(2) {
                if pair[0] < pair[1] {
                    self.intervals.push([pair[0], pair[1]]);
                }
            }
        }
        budget.charge(
            extra + self.bytes(),
            self.intervals.len().saturating_mul(
                (usize::BITS - self.intervals.len().max(1).leading_zeros()) as usize + 1,
            ),
            work,
            admit,
        )?;
        self.intervals
            .sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        let mut used = 0usize;
        for index in 0..self.intervals.len() {
            let span = self.intervals[index];
            if used > 0 && span[0] <= self.intervals[used - 1][1] {
                self.intervals[used - 1][1] = self.intervals[used - 1][1].max(span[1]);
            } else {
                self.intervals[used] = span;
                used += 1;
            }
        }
        self.intervals.truncate(used);
        if projected {
            for span in &mut self.intervals {
                span[0] = project_xyz6_coordinate_mm(span[0])?;
                span[1] = project_xyz6_coordinate_mm(span[1])?;
            }
        }
        budget.admit(extra + self.bytes(), 0, admit)
    }
}

fn encode(material: &MultiPolygon<f64>) -> Vec<DiaphragmPolygonMm> {
    material
        .0
        .iter()
        .map(|polygon| DiaphragmPolygonMm {
            outer: polygon.exterior().0.iter().map(|p| [p.x, p.y]).collect(),
            holes: polygon
                .interiors()
                .iter()
                .map(|ring| ring.0.iter().map(|p| [p.x, p.y]).collect())
                .collect(),
        })
        .collect()
}
fn source_matches(
    source: &super::super::ProcessDomain,
    target: &ProcessRegionCoverage,
    value: spiling_contracts::process::ResolvedProcessValue,
) -> bool {
    !source.coverage.contributions.is_empty()
        && value.fraction == target.fraction
        && source.coverage.contributions.iter().all(|candidate| {
            target.contributions.iter().any(|owner| {
                candidate.occurrence_id == owner.occurrence_id
                    && candidate.definition_id == owner.definition_id
                    && value.provenance == owner.provenance
            })
        })
}

/// Captured source masks are clipped to the actual effective spatial winner,
/// rather than using a source occurrence's unsplit base coverage as permission.
fn source_owner<'a>(
    target: &ProcessRegionCoverage,
    sources: &'a [super::super::ProcessDomain],
    budget: &Budget,
    extra: usize,
    work: &mut usize,
    admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<Cow<'a, MultiPolygon<f64>>, ManufacturingError> {
    if let [source] = sources
        && source.zone.is_none()
    {
        budget.charge(
            extra,
            source
                .coverage
                .contributions
                .len()
                .saturating_mul(target.contributions.len()),
            work,
            admit,
        )?;
        let value = spiling_contracts::process::ResolvedProcessValue {
            fraction: source.coverage.fraction,
            provenance: source
                .coverage
                .contributions
                .first()
                .and_then(|c| c.provenance),
        };
        return Ok(if source_matches(source, target, value) {
            Cow::Borrowed(source.material.as_ref())
        } else {
            Cow::Owned(MultiPolygon(Vec::new()))
        });
    }
    let mut result = MultiPolygon(Vec::new());
    let mut result_bytes = 0usize;
    for source in sources {
        check_cancel(cancel)?;
        for side in 0..if source.zone.is_some() { 2 } else { 1 } {
            let value = source.zone.map_or_else(
                || spiling_contracts::process::ResolvedProcessValue {
                    fraction: source.coverage.fraction,
                    provenance: source
                        .coverage
                        .contributions
                        .first()
                        .and_then(|c| c.provenance),
                },
                |zone| zone.values[side],
            );
            budget.charge(
                extra + result_bytes,
                source
                    .coverage
                    .contributions
                    .len()
                    .saturating_mul(target.contributions.len()),
                work,
                admit,
            )?;
            if !source_matches(source, target, value) {
                continue;
            }
            let mut clipping = None;
            if let Some(zone) = source.zone {
                if zone.axis == 2 {
                    if usize::from(source.physical_z_mm[0] >= zone.coordinate) != side {
                        continue;
                    }
                } else {
                    let Some(bounds) = source.material.bounding_rect() else {
                        continue;
                    };
                    let mut low = [bounds.min().x, bounds.min().y];
                    let mut high = [bounds.max().x, bounds.max().y];
                    if side == 0 {
                        high[zone.axis] = high[zone.axis].min(zone.coordinate);
                    } else {
                        low[zone.axis] = low[zone.axis].max(zone.coordinate);
                    }
                    if low[0] >= high[0] || low[1] >= high[1] {
                        continue;
                    }
                    budget.admit(
                        extra
                            + result_bytes
                            + 5 * size_of::<geo::Coord<f64>>()
                            + size_of::<Polygon<f64>>(),
                        0,
                        admit,
                    )?;
                    clipping = Some(MultiPolygon(vec![
                        geo::Rect::new(
                            geo::Coord {
                                x: low[0],
                                y: low[1],
                            },
                            geo::Coord {
                                x: high[0],
                                y: high[1],
                            },
                        )
                        .to_polygon(),
                    ]));
                }
            }
            let mut piece = if let Some(mut clipping) = clipping {
                let clipping_bytes = super::super::native::material_bytes(&mut clipping);
                let scratch =
                    (region::point_count(&clipping) + region::point_count(&source.material)) * 48;
                budget.admit(extra + result_bytes + clipping_bytes + scratch, 0, admit)?;
                let before = *work;
                super::super::boolean::budget_accumulated(
                    &source.material,
                    &clipping,
                    work,
                    cancel,
                )?;
                budget.admit(
                    extra + result_bytes + clipping_bytes + scratch,
                    *work - before,
                    admit,
                )?;
                let mut clipped = source.material.intersection(&clipping);
                check_cancel(cancel)?;
                region::check_size(&clipped)?;
                region::canonicalize(&mut clipped);
                let clipped_bytes = super::super::native::material_bytes(&mut clipped);
                budget.admit(
                    extra + result_bytes + clipping_bytes + scratch + clipped_bytes,
                    0,
                    admit,
                )?;
                clipped
            } else {
                budget.charge(
                    extra + result_bytes,
                    region::point_count(&source.material),
                    work,
                    admit,
                )?;
                budget.admit(
                    extra + result_bytes + region::cloned_polygons_bytes(&source.material.0)?,
                    0,
                    admit,
                )?;
                (*source.material).clone()
            };
            let piece_bytes = super::super::native::material_bytes(&mut piece);
            budget.admit(extra + result_bytes + piece_bytes, 0, admit)?;
            if piece.0.is_empty() {
                continue;
            }
            if result.0.is_empty() {
                result = piece;
                result_bytes = piece_bytes;
                continue;
            }
            let scratch = (region::point_count(&result) + region::point_count(&piece)) * 48;
            budget.admit(extra + result_bytes + piece_bytes + scratch, 0, admit)?;
            let before = *work;
            super::super::boolean::budget_accumulated(&result, &piece, work, cancel)?;
            budget.admit(
                extra + result_bytes + piece_bytes + scratch,
                *work - before,
                admit,
            )?;
            let mut joined = result.union(&piece);
            check_cancel(cancel)?;
            region::check_size(&joined)?;
            region::canonicalize(&mut joined);
            let joined_bytes = super::super::native::material_bytes(&mut joined);
            budget.admit(
                extra + result_bytes + piece_bytes + joined_bytes + scratch,
                0,
                admit,
            )?;
            result = joined;
            result_bytes = joined_bytes;
        }
    }
    Ok(Cow::Owned(result))
}

fn span(ends: [[f64; 3]; 2], along: usize) -> [f64; 2] {
    [
        ends[0][along].min(ends[1][along]),
        ends[0][along].max(ends[1][along]),
    ]
}
fn grid(value: f64) -> Result<i64, ManufacturingError> {
    Ok((project_xyz6_coordinate_mm(value)? * 1_000_000.0).round() as i64)
}
fn raw(integer: i64) -> f64 {
    integer as f64 / 1_000_000.0
}
fn slack(value: f64) -> f64 {
    128.0 * f64::EPSILON * value.abs().max(1.0)
}

fn consider_start(first: i64, last: i64, preferred: i64, selected: &mut Option<i64>) {
    if first <= last {
        let candidate = preferred.clamp(first, last);
        if selected.is_none_or(|old| (candidate - preferred).abs() < (old - preferred).abs()) {
            *selected = Some(candidate);
        }
    }
}

/// Integer start constraints exhaust the finite XYZ6 lattice without sampling
/// three arbitrary placements. Excluding a short remainder never deletes it.
fn choose(
    domain: &SkinCommandFitDomain,
    required: [[f64; 3]; 2],
    along: usize,
    path_index: usize,
    paths: &[DepositionPath],
    rows: &mut Rows,
    forbidden: &mut Vec<[i64; 2]>,
    budget: &Budget,
    extra: usize,
    work: &mut usize,
    admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<Option<[[f64; 3]; 2]>, ManufacturingError> {
    let required_span = span(required, along);
    let fixed = 1 - along;
    let mut bounds = [[f64::NEG_INFINITY, f64::INFINITY]; 2];
    for projected in [false, true] {
        let available = &mut bounds[usize::from(projected)];
        let ends = if projected {
            [
                project_xyz6_point_mm(required[0])?,
                project_xyz6_point_mm(required[1])?,
            ]
        } else {
            required
        };
        let needed = span(ends, along);
        let row = required[0][fixed];
        rows.read(
            &domain.mandatory,
            along,
            row,
            projected,
            budget,
            extra + capacity_bytes(forbidden),
            work,
            admit,
            cancel,
        )?;
        if !rows.intervals.iter().any(|s| {
            if projected {
                s[0] <= needed[0] && needed[1] <= s[1]
            } else {
                (s[0] - needed[0]).abs() <= slack(s[0]) && (s[1] - needed[1]).abs() <= slack(s[1])
            }
        }) {
            return Ok(None);
        }
        rows.read(
            &domain.source_owner,
            along,
            ends[0][fixed],
            projected,
            budget,
            extra + capacity_bytes(forbidden),
            work,
            admit,
            cancel,
        )?;
        let Some(source) = rows
            .intervals
            .iter()
            .find(|s| s[0] <= needed[0] + slack(s[0]) && needed[1] <= s[1] + slack(s[1]))
        else {
            return Ok(None);
        };
        available[0] = available[0].max(source[0]);
        available[1] = available[1].min(source[1]);
        rows.read(
            &domain.regional,
            along,
            row,
            projected,
            budget,
            extra + capacity_bytes(forbidden),
            work,
            admit,
            cancel,
        )?;
        let left = rows
            .intervals
            .iter()
            .filter(|s| s[0] < needed[0] && needed[0] <= s[1] + slack(s[1]))
            .map(|s| s[0])
            .fold(needed[0], f64::min);
        let right = rows
            .intervals
            .iter()
            .filter(|s| s[0] <= needed[1] + slack(s[0]) && needed[1] < s[1])
            .map(|s| s[1])
            .fold(needed[1], f64::max);
        available[0] = available[0].max(left);
        available[1] = available[1].min(right);
    }
    let target = MIN_DEPOSITION_COMMAND_MM + 0.000003;
    let raw_low = bounds[0][0].max(required_span[1] - target);
    let raw_high = bounds[0][1].min(required_span[0] + target);
    if raw_low > required_span[0] || raw_high < required_span[1] {
        return Ok(None);
    }
    let low = grid(raw_low)?.max(grid(bounds[1][0])?);
    let high = grid(raw_high)?.min(grid(bounds[1][1])?);
    let minimum = (MIN_DEPOSITION_COMMAND_MM * 1_000_000.0).round() as i64;
    let maximum = (minimum + 3).min(high - low);
    if maximum < minimum {
        return Ok(None);
    }
    // At most four actual command lengths; exhaust all, not just the longest.
    for length in (minimum..=maximum).rev() {
        let max_start = grid(required_span[0])?.min(high - length);
        let min_start = (grid(required_span[1])? - length).max(low);
        if min_start > max_start {
            continue;
        }
        forbidden.clear();
        let actual_row = project_xyz6_coordinate_mm(required[0][fixed])?;
        let actual_z = project_xyz6_coordinate_mm(required[0][2])?;
        for (index, path) in paths.iter().enumerate() {
            check_cancel(cancel)?;
            budget.charge(
                extra + rows.bytes() + capacity_bytes(forbidden),
                path.points_mm.len(),
                work,
                admit,
            )?;
            if index == path_index {
                continue;
            }
            let regional = path.points_mm.len() == 2
                && path.owner
                    == Some(DepositionOwner::Regional {
                        region_index: domain.region_index,
                    })
                && matches!(
                    path.kind,
                    DepositionKind::SolidFill | DepositionKind::SparseFill
                )
                && path.width.constant_width_mm() == paths[path_index].width.constant_width_mm()
                && path.physical_z_mm == paths[path_index].physical_z_mm;
            for edge in path.points_mm.windows(2) {
                let ends = [
                    project_xyz6_point_mm(edge[0])?,
                    project_xyz6_point_mm(edge[1])?,
                ];
                if ends
                    .iter()
                    .any(|p| p[fixed] != actual_row || p[2] != actual_z)
                {
                    continue;
                }
                let other = [
                    grid(ends[0][along])?.min(grid(ends[1][along])?),
                    grid(ends[0][along])?.max(grid(ends[1][along])?),
                ];
                if other[0] == other[1] {
                    continue;
                }
                let cuts = if regional {
                    // The existing emitter compares actual decoded f64 lengths.
                    // A nominal 20,000-quantum remainder can round just below MIN.
                    let left_bad = i64::from(
                        raw(other[0] + minimum) - raw(other[0]) < MIN_DEPOSITION_COMMAND_MM,
                    );
                    let right_bad = i64::from(
                        raw(other[1]) - raw(other[1] - minimum) < MIN_DEPOSITION_COMMAND_MM,
                    );
                    [
                        [other[0] + 1, other[0] + minimum - 1 + left_bad],
                        [
                            other[1] - minimum + 1 - length - right_bad,
                            other[1] - 1 - length,
                        ],
                    ]
                } else {
                    [[other[0] - length + 1, other[1] - 1], [1, 0]]
                };
                for cut in cuts {
                    if cut[0] <= cut[1] && cut[1] >= min_start && cut[0] <= max_start {
                        if forbidden.len() == forbidden.capacity() {
                            let target = record_target(forbidden, 1);
                            budget.admit(
                                extra
                                    + rows.bytes()
                                    + capacity_bytes(forbidden)
                                    + target * size_of::<[i64; 2]>(),
                                0,
                                admit,
                            )?;
                            forbidden
                                .try_reserve_exact(target - forbidden.len())
                                .map_err(|_| resource("skin fit reservation allocation refused"))?;
                        }
                        forbidden.push(cut);
                    }
                }
            }
        }
        budget.charge(
            extra + rows.bytes() + capacity_bytes(forbidden),
            forbidden.len().saturating_mul(
                (usize::BITS - forbidden.len().max(1).leading_zeros()) as usize + 1,
            ),
            work,
            admit,
        )?;
        forbidden.sort_unstable();
        let preferred = grid((required_span[0] + required_span[1] - raw(length)) * 0.5)?
            .clamp(min_start, max_start);
        let mut first = min_start;
        let mut selected = None::<i64>;
        // Each forbidden interval is inclusive on the finite command lattice.
        for cut in forbidden.iter() {
            consider_start(first, (cut[0] - 1).min(max_start), preferred, &mut selected);
            first = first.max(cut[1] + 1);
            if first > max_start {
                break;
            }
        }
        consider_start(first, max_start, preferred, &mut selected);
        let Some(start) = selected else {
            continue;
        };
        // Keep unextended original endpoints verbatim. Forcing them onto the
        // command grid would either lose raw skin or add unowned material.
        let mut raw_start = raw(start).clamp(raw_low, required_span[0]);
        let mut raw_end = raw(start + length).clamp(required_span[1], raw_high);
        if raw_end - raw_start > target {
            let shorter_end = (raw_start + target).max(required_span[1]).min(raw_end);
            if grid(shorter_end)? == start + length {
                raw_end = shorter_end;
            } else {
                let later_start = (raw_end - target).min(required_span[0]).max(raw_start);
                if grid(later_start)? == start {
                    raw_start = later_start;
                } else {
                    let enclosed_end = shorter_end.next_up();
                    if enclosed_end <= raw_high && grid(enclosed_end)? == start + length {
                        raw_end = enclosed_end;
                    }
                }
            }
        }
        if grid(raw_start)? != start
            || grid(raw_end)? != start + length
            || raw_end - raw_start > target + slack(raw_start.abs().max(raw_end.abs()))
        {
            continue;
        }
        let mut fitted = required;
        let ascending = required[0][along] < required[1][along];
        fitted[usize::from(!ascending)][along] = raw_start;
        fitted[usize::from(ascending)][along] = raw_end;
        let actual = [
            project_xyz6_point_mm(fitted[0])?,
            project_xyz6_point_mm(fitted[1])?,
        ];
        let error = xyz6_coordinate_error_mm(actual[0][along])?
            + xyz6_coordinate_error_mm(actual[1][along])?
            + slack(actual[0][along]);
        if (actual[1][along] - actual[0][along]).abs() - error < MIN_DEPOSITION_COMMAND_MM {
            continue;
        }
        return Ok(Some(fitted));
    }
    Ok(None)
}

/// Reserve actual Regional extrusion, not an unprintable raw-coordinate tail.
fn reserve_regional_material(
    paths: &mut Vec<DepositionPath>,
    index: usize,
    region_index: u16,
    fitted: [[f64; 3]; 2],
    along: usize,
    segments: &mut usize,
    budget: &mut Budget,
    scratch_bytes: usize,
    work: &mut usize,
    admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let fitted_span = span(fitted, along);
    let actual_fitted_span = span(
        [
            project_xyz6_point_mm(fitted[0])?,
            project_xyz6_point_mm(fitted[1])?,
        ],
        along,
    );
    let fixed = 1 - along;
    let actual_row = project_xyz6_coordinate_mm(fitted[0][fixed])?;
    let actual_z = project_xyz6_coordinate_mm(fitted[0][2])?;
    let before_clip = paths.len();
    for other in 0..before_clip {
        check_cancel(cancel)?;
        budget.charge(scratch_bytes, paths[other].points_mm.len(), work, admit)?;
        if other == index || paths[other].points_mm.len() != 2 {
            continue;
        }
        let p = &paths[other];
        if p.owner != Some(DepositionOwner::Regional { region_index })
            || !matches!(
                p.kind,
                DepositionKind::SolidFill | DepositionKind::SparseFill
            )
            || p.width.constant_width_mm() != paths[index].width.constant_width_mm()
            || p.physical_z_mm != paths[index].physical_z_mm
        {
            continue;
        }
        let ends = [p.points_mm[0], p.points_mm[1]];
        let actual = [
            project_xyz6_point_mm(ends[0])?,
            project_xyz6_point_mm(ends[1])?,
        ];
        if actual
            .iter()
            .any(|v| v[fixed] != actual_row || v[2] != actual_z)
        {
            continue;
        }
        let old = span(actual, along);
        if old[0].max(actual_fitted_span[0]) >= old[1].min(actual_fitted_span[1]) {
            continue;
        }
        let left = old[0] < actual_fitted_span[0];
        let right = actual_fitted_span[1] < old[1];
        if !left && !right {
            budget.bytes -= paths[other].points_mm.capacity() * size_of::<[f64; 3]>();
            paths[other].points_mm = Vec::new();
            *segments -= 1;
            continue;
        }
        let ascending = ends[0][along] < ends[1][along];
        let low_index = usize::from(!ascending);
        let high_index = usize::from(ascending);
        if left && right {
            let old_capacity = paths.capacity();
            let target = record_target(paths, 1);
            let additional = if target > old_capacity {
                target * size_of::<DepositionPath>()
            } else {
                0
            };
            budget.admit(
                scratch_bytes + additional + 2 * size_of::<[f64; 3]>(),
                1,
                admit,
            )?;
            paths
                .try_reserve_exact(1)
                .map_err(|_| resource("skin fit regional split allocation refused"))?;
            budget.retain(
                (paths.capacity() - old_capacity) * size_of::<DepositionPath>()
                    + 2 * size_of::<[f64; 3]>(),
            )?;
            let p = &paths[other];
            let mut remainder = DepositionPath {
                material_domain: None,
                kind: p.kind,
                owner: p.owner,
                width: WidthProfile::Constant {
                    width_mm: region::constant_width(p)?,
                },
                physical_z_mm: p.physical_z_mm,
                points_mm: vec![ends[0], ends[1]],
            };
            remainder.points_mm[low_index][along] = fitted_span[1];
            paths.push(remainder);
            *segments = segments
                .checked_add(1)
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| resource("skin fit regional split command limit exceeded"))?;
        }
        if left {
            paths[other].points_mm[high_index][along] = fitted_span[0];
        } else {
            paths[other].points_mm[low_index][along] = fitted_span[1];
        }
    }
    Ok(())
}

impl ClassifiedRegions {
    pub(super) fn fit_skin_commands(
        &self,
        paths: &mut Vec<DepositionPath>,
        sources: &[super::super::ProcessDomain],
        segments: &mut usize,
        retained_bytes: usize,
        admit: &mut impl FnMut(usize, usize, usize) -> Result<(), ManufacturingError>,
        cancel: &AtomicBool,
    ) -> Result<Vec<SkinCommandFitDomain>, ManufacturingError> {
        let mut domains = Vec::<SkinCommandFitDomain>::new();
        let mut budget = Budget {
            bytes: byte_sum(retained_bytes, super::super::bonds::path_bytes(paths))?,
        };
        let mut rows = Rows::default();
        let mut forbidden = Vec::new();
        let mut work = 0usize;
        let original_count = paths.len();
        for index in 0..original_count {
            check_cancel(cancel)?;
            let path = &paths[index];
            let skin = match path.owner {
                Some(DepositionOwner::MandatorySkin { skin })
                    if matches!(
                        path.kind,
                        DepositionKind::BottomSkin | DepositionKind::TopSkin
                    ) =>
                {
                    skin
                }
                _ => continue,
            };
            if path.points_mm.len() != 2 {
                continue;
            }
            let required = [path.points_mm[0], path.points_mm[1]];
            let along = if required[0][0] == required[1][0] {
                1
            } else if required[0][1] == required[1][1] {
                0
            } else {
                continue;
            };
            let projected = [
                project_xyz6_point_mm(required[0])?,
                project_xyz6_point_mm(required[1])?,
            ];
            if (projected[1][along] - projected[0][along]).abs() >= MIN_DEPOSITION_COMMAND_MM {
                continue;
            }
            let mut chosen = None;
            for region_index in 0..self.interiors.len() {
                let interior = &self.interiors[region_index];
                if interior.coverage.fraction <= 0.0
                    || self
                        .coverage
                        .process_diaphragms
                        .iter()
                        .any(|p| usize::from(p.region_index) == region_index)
                {
                    continue;
                }
                let existing = domains
                    .iter()
                    .position(|d| usize::from(d.region_index) == region_index && d.skin == skin);
                let mut pending = None;
                if existing.is_none() {
                    let mut source = source_owner(
                        &interior.coverage,
                        sources,
                        &budget,
                        rows.bytes() + capacity_bytes(&forbidden),
                        &mut work,
                        admit,
                        cancel,
                    )?;
                    let source_bytes = match &mut source {
                        Cow::Borrowed(_) => 0,
                        Cow::Owned(material) => super::super::native::material_bytes(material),
                    };
                    if source.0.is_empty() {
                        continue;
                    }
                    let mandatory = if skin == ProcessSkinKind::TopSkin {
                        &self.top
                    } else {
                        &self.bottom
                    };
                    let expected = encoded_domain_bytes(&interior.domain)
                        + encoded_domain_bytes(mandatory)
                        + encoded_domain_bytes(&source)
                        + contribution_clone_bytes(&interior.coverage.contributions);
                    budget.admit(
                        rows.bytes() + capacity_bytes(&forbidden) + expected + source_bytes,
                        0,
                        admit,
                    )?;
                    pending = Some(SkinCommandFitDomain {
                        region_index: region_index as u16,
                        region: interior.coverage.clone(),
                        skin,
                        regional: encode(&interior.domain),
                        mandatory: encode(mandatory),
                        source_owner: encode(&source),
                        fits: Vec::new(),
                    });
                }
                let domain = existing
                    .map(|i| &domains[i])
                    .or(pending.as_ref())
                    .expect("existing or pending owner");
                let extra = pending
                    .as_ref()
                    .map_or(0, SkinCommandFitDomain::allocated_bytes);
                if let Some(fitted) = choose(
                    domain,
                    required,
                    along,
                    index,
                    paths,
                    &mut rows,
                    &mut forbidden,
                    &budget,
                    extra,
                    &mut work,
                    admit,
                    cancel,
                )? {
                    chosen = Some((existing, pending, fitted));
                    break;
                }
            }
            let Some((existing, pending, fitted)) = chosen else {
                // Nonproof is not a new refusal before the existing demanded
                // Bridge constructor has had its independent opportunity.
                // Realization still refuses every unresolved short command.

                continue;
            };
            let domain_index = if let Some(index) = existing {
                index
            } else {
                let domain = pending.expect("new captured owner");
                let old = domains.capacity();
                if domains.len() == old {
                    budget.admit(
                        rows.bytes()
                            + capacity_bytes(&forbidden)
                            + domain.allocated_bytes()
                            + (domains.len() + 1) * size_of::<SkinCommandFitDomain>(),
                        0,
                        admit,
                    )?;
                    domains
                        .try_reserve_exact(1)
                        .map_err(|_| resource("skin fit domain allocation refused"))?;
                    budget
                        .retain((domains.capacity() - old) * size_of::<SkinCommandFitDomain>())?;
                }
                budget.retain(domain.allocated_bytes())?;
                domains.push(domain);
                domains.len() - 1
            };
            let domain = &mut domains[domain_index];
            if domain.fits.len() == domain.fits.capacity() {
                let old = domain.fits.capacity();
                let target = record_target(&domain.fits, 1);
                budget.admit(
                    rows.bytes()
                        + capacity_bytes(&forbidden)
                        + target * size_of::<SkinCommandFit>(),
                    0,
                    admit,
                )?;
                domain
                    .fits
                    .try_reserve_exact(target - domain.fits.len())
                    .map_err(|_| resource("skin fit witness allocation refused"))?;
                budget.retain((domain.fits.capacity() - old) * size_of::<SkinCommandFit>())?;
            }
            domain.fits.push(SkinCommandFit {
                required_mm: required,
                fitted_mm: fitted,
            });
            let region_index = domain.region_index;
            reserve_regional_material(
                paths,
                index,
                region_index,
                fitted,
                along,
                segments,
                &mut budget,
                rows.bytes() + capacity_bytes(&forbidden),
                &mut work,
                admit,
                cancel,
            )?;
            paths[index].points_mm.copy_from_slice(&fitted);
            budget.admit(rows.bytes() + capacity_bytes(&forbidden), 0, admit)?;
        }
        paths.retain(|path| !path.points_mm.is_empty());
        for domain in &domains {
            domain.validate()?;
        }
        Ok(domains)
    }
}

#[cfg(test)]
mod tests;
