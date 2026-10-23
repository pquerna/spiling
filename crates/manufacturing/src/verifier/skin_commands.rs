// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Minimum-command witnesses are checked only after the complete actual-byte /
//! nominal-path bijection. Captured masks retain their original classification;
//! neither a fit nor its extra extrusion enlarges the mandatory-skin area.
use super::{arithmetic_slack, cancelled, constant_width, failed, project_xyz6_point_mm};
use crate::planner::support::{
    AnchorCandidate, AnchorWorkspace, GapKind, gap_kind, projected_edges,
};
use spiling_contracts::manufacturing::*;
use std::{mem::size_of, sync::atomic::AtomicBool};

fn resource() -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ResourceLimit,
        "minimum skin command replay work or retained-byte limit exceeded",
    )
}
fn charge(work: &mut usize, count: usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(count)
        .filter(|n| *n <= 128_000_000)
        .ok_or_else(resource)?;
    Ok(())
}
fn add_bytes(total: &mut usize, bytes: usize) -> Result<(), ManufacturingError> {
    *total = total
        .checked_add(bytes)
        .filter(|n| *n as u64 <= MAX_MANUFACTURING_RETAINED_BYTES)
        .ok_or_else(resource)?;
    Ok(())
}
fn extent_bytes(domain: &[DiaphragmPolygonMm]) -> usize {
    domain
        .iter()
        .map(|p| {
            p.outer.capacity() * size_of::<[f64; 2]>()
                + p.holes.capacity() * size_of::<Vec<[f64; 2]>>()
                + p.holes
                    .iter()
                    .map(|r| r.capacity() * size_of::<[f64; 2]>())
                    .sum::<usize>()
        })
        .sum()
}

/// Account captured operands and witnesses together with their borrowed path
/// owners, not just the largest single domain. Row scratch never copies a mask.
fn retained_bytes(plan: &NormalizedPrintPlan) -> Result<usize, ManufacturingError> {
    let mut bytes = plan.skin_command_fit_bytes()?;
    add_bytes(&mut bytes, plan.layers.capacity() * size_of::<PrintLayer>())?;
    for layer in &plan.layers {
        add_bytes(
            &mut bytes,
            layer.paths.capacity() * size_of::<DepositionPath>(),
        )?;
        for path in &layer.paths {
            add_bytes(
                &mut bytes,
                path.points_mm.capacity() * size_of::<[f64; 3]>(),
            )?;
            if let WidthProfile::PerEdge { widths_mm } = &path.width {
                add_bytes(&mut bytes, widths_mm.capacity() * size_of::<f64>())?;
            }
        }
        let coverage = &layer.region_coverage;
        add_bytes(
            &mut bytes,
            coverage.skin_bridge_domains.capacity() * size_of::<SkinBridgeDomain>(),
        )?;
        for domain in &coverage.skin_bridge_domains {
            add_bytes(&mut bytes, domain.allocated_bytes())?;
        }
        add_bytes(
            &mut bytes,
            coverage.process_diaphragms.capacity() * size_of::<ProcessDiaphragmCoverage>(),
        )?;
        for plate in &coverage.process_diaphragms {
            add_bytes(
                &mut bytes,
                plate.extent.capacity() * size_of::<DiaphragmPolygonMm>(),
            )?;
            add_bytes(&mut bytes, extent_bytes(&plate.extent))?;
        }
    }
    Ok(bytes)
}

#[derive(Default)]
struct Rows {
    hits: Vec<f64>,
    intervals: Vec<[f64; 2]>,
}
impl Rows {
    fn admit(
        &self,
        retained: usize,
        hits: usize,
        intervals: usize,
    ) -> Result<(), ManufacturingError> {
        let mut bytes = retained;
        add_bytes(
            &mut bytes,
            hits.max(self.hits.capacity()) * size_of::<f64>(),
        )?;
        add_bytes(
            &mut bytes,
            intervals.max(self.intervals.capacity()) * size_of::<[f64; 2]>(),
        )
    }
    /// Same half-open ring sweep and sole XYZ6 endpoint projection as the mixed
    /// skin-bridge proof. Sorting/union permits adjacent components but never
    /// fills a hole or an intervening positive gap.
    fn read(
        &mut self,
        domain: &[DiaphragmPolygonMm],
        along: usize,
        row: f64,
        projected: bool,
        retained: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        self.intervals.clear();
        let fixed = 1 - along;
        for polygon in domain {
            cancelled(cancel)?;
            let edges = polygon.outer.len() + polygon.holes.iter().map(Vec::len).sum::<usize>();
            charge(work, edges)?;
            self.hits.clear();
            self.admit(retained, edges, self.intervals.len() + edges / 2)?;
            if self.hits.capacity() < edges {
                self.hits.try_reserve_exact(edges).map_err(|_| resource())?;
            }
            for ring in std::iter::once(&polygon.outer).chain(&polygon.holes) {
                for edge in ring.windows(2) {
                    cancelled(cancel)?;
                    let (a, b) = (edge[0], edge[1]);
                    if (a[fixed] <= row && row < b[fixed]) || (b[fixed] <= row && row < a[fixed]) {
                        let t = (row - a[fixed]) / (b[fixed] - a[fixed]);
                        let value = a[along] + t * (b[along] - a[along]);
                        if !value.is_finite() {
                            return Err(failed("skin fit has an unconditioned domain row"));
                        }
                        self.hits.push(value);
                    }
                }
            }
            charge(
                work,
                self.hits.len().saturating_mul(
                    (usize::BITS - self.hits.len().max(1).leading_zeros()) as usize + 1,
                ),
            )?;
            self.hits.sort_unstable_by(f64::total_cmp);
            if !self.hits.len().is_multiple_of(2) {
                return Err(failed("skin fit captured domain row is ambiguous"));
            }
            let count = self.intervals.len() + self.hits.len() / 2;
            self.admit(retained, self.hits.capacity(), count)?;
            if self.intervals.capacity() < count {
                self.intervals
                    .try_reserve_exact(count - self.intervals.len())
                    .map_err(|_| resource())?;
            }
            for pair in self.hits.chunks_exact(2) {
                let interval = [pair[0], pair[1]];
                if interval[0] < interval[1] {
                    self.intervals.push(interval);
                }
            }
            self.admit(retained, self.hits.capacity(), self.intervals.capacity())?;
        }
        charge(
            work,
            self.intervals.len().saturating_mul(
                (usize::BITS - self.intervals.len().max(1).leading_zeros()) as usize + 1,
            ),
        )?;
        self.intervals
            .sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        let mut merged = 0usize;
        for index in 0..self.intervals.len() {
            let interval = self.intervals[index];
            if merged > 0 && interval[0] <= self.intervals[merged - 1][1] {
                self.intervals[merged - 1][1] = self.intervals[merged - 1][1].max(interval[1]);
            } else {
                self.intervals[merged] = interval;
                merged += 1;
            }
        }
        self.intervals.truncate(merged);
        if projected {
            // Never merge a positive raw hole just because its two boundaries
            // project to the same lattice coordinate.
            for span in &mut self.intervals {
                span[0] = program_coordinates::project_xyz6_coordinate_mm(span[0])?;
                span[1] = program_coordinates::project_xyz6_coordinate_mm(span[1])?;
            }
        }
        Ok(())
    }
    fn contains(&self, interval: [f64; 2], projected: bool) -> bool {
        if interval[0] == interval[1] {
            return true;
        }
        self.intervals.iter().any(|span| {
            let slack = if projected {
                0.0
            } else {
                arithmetic_slack(span[0].abs().max(span[1].abs()))
            };
            span[0] <= interval[0] + slack && interval[1] <= span[1] + slack
        })
    }
    fn complete(&self, interval: [f64; 2]) -> bool {
        self.intervals.iter().any(|span| {
            span.into_iter()
                .zip(interval)
                .all(|(a, b)| (*a - b).abs() <= arithmetic_slack(a.abs().max(b.abs())))
        })
    }
    fn intersects(&self, interval: [f64; 2]) -> bool {
        self.intervals
            .iter()
            .any(|span| span[0].max(interval[0]) < span[1].min(interval[1]))
    }
}
fn interval(ends: [[f64; 3]; 2], along: usize) -> [f64; 2] {
    [
        ends[0][along].min(ends[1][along]),
        ends[0][along].max(ends[1][along]),
    ]
}
fn fixed_axis(policy: RectilinearAxisPolicy, layer: u32) -> usize {
    match policy {
        RectilinearAxisPolicy::X => 1,
        RectilinearAxisPolicy::Y => 0,
        RectilinearAxisPolicy::AlternatingXY => usize::from(layer.is_multiple_of(2)),
        RectilinearAxisPolicy::AlternatingYX => usize::from(!layer.is_multiple_of(2)),
    }
}
fn same_ends(a: [[f64; 3]; 2], b: [[f64; 3]; 2]) -> bool {
    a == b || a == [b[1], b[0]]
}
fn contains_interval(outer: [f64; 2], inner: [f64; 2]) -> bool {
    outer[0] <= inner[0] && inner[1] <= outer[1]
}

fn check_masks(
    domain: &SkinCommandFitDomain,
    fit: &SkinCommandFit,
    path: &DepositionPath,
    along: usize,
    height: f64,
    rows: &mut Rows,
    retained: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let fixed = 1 - along;
    let construction = [path.points_mm[0], path.points_mm[1]];
    let width = constant_width(path)?;
    let solid_spacing =
        ((width - height) * height + std::f64::consts::PI * (height * 0.5).powi(2)) / height;
    if !solid_spacing.is_finite() || solid_spacing <= 0.0 {
        return Err(failed(
            "skin fit authored row spacing is not finite and positive",
        ));
    }
    let row = fit.required_mm[0][fixed];
    let mut authored_complete = false;
    // Ordinary mandatory skins use a centered, fraction-one lattice for each
    // ORIGINAL polygon. Reconstruct that lattice independently of fitting.
    // A row from another component cannot authorize this required interval.
    for polygon in &domain.mandatory {
        cancelled(cancel)?;
        charge(work, polygon.outer.len())?;
        let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
        for point in &polygon.outer {
            cancelled(cancel)?;
            min = min.min(point[fixed]);
            max = max.max(point[fixed]);
        }
        let count = ((max - min) / solid_spacing).ceil().max(1.0);
        if !count.is_finite() || count > MAX_DEPOSITION_SEGMENTS as f64 {
            return Err(resource());
        }
        let spacing = if count == 1.0 { 0.0 } else { solid_spacing };
        let start = min + ((max - min) - (count - 1.0) * spacing) * 0.5;
        let ordinal = if count == 1.0 {
            0.0
        } else {
            ((row - start) / spacing).round()
        };
        if !start.is_finite() || !ordinal.is_finite() {
            return Err(failed("skin fit authored row construction is not finite"));
        }
        if ordinal < 0.0 || ordinal >= count {
            continue;
        }
        let expected_row = start + ordinal * spacing;
        if !expected_row.is_finite() {
            return Err(failed("skin fit authored row coordinate is not finite"));
        }
        let scale = min
            .abs()
            .max(max.abs())
            .max(row.abs())
            .max(expected_row.abs());
        if (row - expected_row).abs() > arithmetic_slack(scale) {
            continue;
        }
        rows.read(
            std::slice::from_ref(polygon),
            along,
            row,
            false,
            retained,
            work,
            cancel,
        )?;
        if rows.complete(interval(fit.required_mm, along)) {
            authored_complete = true;
            break;
        }
    }
    if !authored_complete {
        return Err(failed(
            "skin fit omits its complete original interval or changes its authored mandatory row phase",
        ));
    }
    for projected in [false, true] {
        let required = if projected {
            [
                project_xyz6_point_mm(fit.required_mm[0])?,
                project_xyz6_point_mm(fit.required_mm[1])?,
            ]
        } else {
            fit.required_mm
        };
        let fitted = if projected {
            [
                project_xyz6_point_mm(construction[0])?,
                project_xyz6_point_mm(construction[1])?,
            ]
        } else {
            construction
        };
        // Classification is made on the authored row. XYZ6 represents the
        // image of those intervals; it does not authorize a second hatch at
        // the rounded fixed coordinate.
        let row = fit.required_mm[0][fixed];
        let required_interval = interval(required, along);
        let fitted_interval = interval(fitted, along);
        rows.read(
            &domain.mandatory,
            along,
            row,
            projected,
            retained,
            work,
            cancel,
        )?;
        if !rows.contains(required_interval, projected) {
            return Err(failed(
                "skin fit original interval escapes its mandatory mask at XYZ6",
            ));
        }
        rows.read(
            &domain.regional,
            along,
            row,
            projected,
            retained,
            work,
            cancel,
        )?;
        if !rows.contains([fitted_interval[0], required_interval[0]], projected)
            || !rows.contains([required_interval[1], fitted_interval[1]], projected)
        {
            return Err(failed(
                "skin fit added extrusion escapes its captured ordinary winner or enters a hole",
            ));
        }
        // Physical source ownership is independent: unlike classification,
        // the actual commanded row must remain inside this captured owner.
        rows.read(
            &domain.source_owner,
            along,
            fitted[0][fixed],
            projected,
            retained,
            work,
            cancel,
        )?;
        if !rows.contains(fitted_interval, projected) {
            return Err(failed(
                "skin fit escapes its complete captured source owner",
            ));
        }
    }
    Ok(())
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    cancelled(cancel)?;
    if plan
        .layers
        .iter()
        .all(|layer| layer.skin_command_fits.is_empty())
    {
        return Ok(());
    }
    let retained = retained_bytes(plan)?;
    let mut rows = Rows::default();
    let mut claimed = Vec::<bool>::new();
    for (index, layer) in plan.layers.iter().enumerate() {
        if layer.skin_command_fits.is_empty() {
            continue;
        }
        cancelled(cancel)?;
        let mut live = retained;
        add_bytes(&mut live, layer.paths.len().max(claimed.capacity()))?;
        add_bytes(&mut live, anchors.retained_bytes())?;
        rows.admit(live, 0, 0)?;
        claimed.clear();
        if claimed.capacity() < layer.paths.len() {
            claimed
                .try_reserve_exact(layer.paths.len())
                .map_err(|_| resource())?;
        }
        claimed.resize(layer.paths.len(), false);
        live = retained;
        add_bytes(&mut live, claimed.capacity())?;
        add_bytes(&mut live, anchors.retained_bytes())?;
        for domain in &layer.skin_command_fits {
            let region = layer
                .region_coverage
                .process_regions
                .get(usize::from(domain.region_index))
                .ok_or_else(|| {
                    failed("skin fit lost its ordinary regional source/process binding")
                })?;
            if *region != domain.region
                || region.contributions.is_empty()
                || layer
                    .region_coverage
                    .process_diaphragms
                    .iter()
                    .any(|plate| plate.region_index == domain.region_index)
            {
                return Err(failed(
                    "skin fit changes its captured ordinary source/process winner",
                ));
            }
            let policy = match domain.skin {
                ProcessSkinKind::BottomSkin => recipe.rectilinear_axis,
                ProcessSkinKind::TopSkin => recipe.top_skin_axis,
            };
            let fixed = fixed_axis(policy, layer.index);
            let along = 1 - fixed;
            for fit in &domain.fits {
                cancelled(cancel)?;
                charge(work, 1)?;
                live = retained;
                add_bytes(&mut live, claimed.capacity())?;
                add_bytes(&mut live, anchors.retained_bytes())?;
                rows.admit(live, 0, 0)?;
                if fit.required_mm[0][fixed] != fit.required_mm[1][fixed]
                    || fit.required_mm[0][2] != layer.z_mm
                {
                    return Err(failed(
                        "skin fit changes its authored row axis or physical interval",
                    ));
                }
                let required = [
                    project_xyz6_point_mm(fit.required_mm[0])?,
                    project_xyz6_point_mm(fit.required_mm[1])?,
                ];
                let fitted = [
                    project_xyz6_point_mm(fit.fitted_mm[0])?,
                    project_xyz6_point_mm(fit.fitted_mm[1])?,
                ];
                let required_length = (required[1][along] - required[0][along]).abs();
                let fitted_length = (fitted[1][along] - fitted[0][along]).abs();
                let raw_length = (fit.fitted_mm[1][along] - fit.fitted_mm[0][along]).abs();
                let scale = fit
                    .fitted_mm
                    .iter()
                    .map(|p| p[along].abs())
                    .fold(1.0, f64::max);
                let target = program_coordinates::MIN_DEPOSITION_COMMAND_MM + 0.000_003;
                let endpoint_error = |ends: [[f64; 3]; 2]| -> Result<f64, ManufacturingError> {
                    Ok(
                        program_coordinates::xyz6_coordinate_error_mm(ends[0][along])?
                            + program_coordinates::xyz6_coordinate_error_mm(ends[1][along])?
                            + arithmetic_slack(scale),
                    )
                };
                // Necessity follows the emitter's exact decoded-f64 guard;
                // adding an enclosure here would strand its sub-minimum ULP
                // band. The fitted construction still proves the closed lower
                // length enclosure, without weakening that command minimum.
                if required_length >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
                    || fitted_length - endpoint_error(fitted)?
                        < program_coordinates::MIN_DEPOSITION_COMMAND_MM
                    || raw_length > target + arithmetic_slack(scale)
                {
                    return Err(failed(
                        "skin fit is unnecessary, under minimum, or exceeds minimal construction/grid reserve",
                    ));
                }
                let mut construction = None;
                for (path_index, path) in layer.paths.iter().enumerate() {
                    cancelled(cancel)?;
                    charge(work, path.points_mm.len())?;
                    if path.owner != Some(DepositionOwner::MandatorySkin { skin: domain.skin })
                        || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
                        || path.width.constant_width_mm() != Some(recipe.bead_width_mm)
                    {
                        continue;
                    }
                    let ends = [
                        path.points_mm[0],
                        *path.points_mm.last().expect("validated path"),
                    ];
                    let normal_kind = match domain.skin {
                        ProcessSkinKind::BottomSkin => DepositionKind::BottomSkin,
                        ProcessSkinKind::TopSkin => DepositionKind::TopSkin,
                    };
                    let matches = if path.kind == normal_kind {
                        path.points_mm.len() == 2 && same_ends(ends, fit.fitted_mm)
                    } else if path.kind == DepositionKind::Bridge {
                        path.points_mm.len() == 2
                            && path
                                .points_mm
                                .iter()
                                .all(|p| p[fixed] == fit.fitted_mm[0][fixed] && p[2] == layer.z_mm)
                            && contains_interval(
                                interval(ends, along),
                                interval(fit.fitted_mm, along),
                            )
                    } else {
                        false
                    };
                    if matches {
                        if construction.replace((path_index, path, ends)).is_some() {
                            return Err(failed("skin fit has duplicate actual constructions"));
                        }
                    }
                }
                let (path_index, path, ends) = construction
                    .ok_or_else(|| failed("skin fit has no complete actual final construction"))?;
                if claimed[path_index] {
                    return Err(failed(
                        "skin fit repeats an already claimed actual construction",
                    ));
                }
                claimed[path_index] = true;
                let actual = [
                    project_xyz6_point_mm(ends[0])?,
                    project_xyz6_point_mm(ends[1])?,
                ];
                if !contains_interval(interval(actual, along), interval(fitted, along)) {
                    return Err(failed(
                        "skin fit final XYZ6 construction loses required material",
                    ));
                }
                check_masks(
                    domain,
                    fit,
                    path,
                    along,
                    layer.z_mm - layer.lower_z_mm,
                    &mut rows,
                    live,
                    work,
                    cancel,
                )?;
                for plate in &layer.region_coverage.process_diaphragms {
                    rows.read(
                        &plate.extent,
                        along,
                        actual[0][fixed],
                        true,
                        live,
                        work,
                        cancel,
                    )?;
                    if rows.intersects(interval(actual, along)) {
                        return Err(failed("skin fit duplicates permanent plate ownership"));
                    }
                }
                for interface in &layer.region_coverage.process_interfaces {
                    for line in &interface.centerlines_mm {
                        charge(work, 1)?;
                        let wall = [
                            project_xyz6_point_mm([line[0][0], line[0][1], layer.z_mm])?,
                            project_xyz6_point_mm([line[1][0], line[1][1], layer.z_mm])?,
                        ];
                        if super::interfaces::enters_finite_band(
                            actual,
                            wall,
                            usize::from(interface.axis),
                            interface.thickness_mm * 0.5,
                        ) {
                            return Err(failed("skin fit enters reserved separator material"));
                        }
                    }
                }
                if path.kind == DepositionKind::Bridge {
                    super::interfaces::verify_bridge_lower(
                        plan,
                        index,
                        actual,
                        constant_width(path)?,
                        recipe,
                        true,
                        work,
                        anchors,
                        cancel,
                    )?;
                } else if index > 0 {
                    let previous = &plan.layers[index - 1];
                    if previous.z_mm != path.physical_z_mm[0] {
                        return Err(failed(
                            "ordinary fitted skin crosses a physical lower-deposition gap",
                        ));
                    }
                    let earlier = projected_edges(
                        previous.paths.iter().filter(|lower| {
                            !matches!(
                                lower.kind,
                                DepositionKind::Support | DepositionKind::SupportInterface
                            ) && lower.physical_z_mm[1] == path.physical_z_mm[0]
                        }),
                        true,
                    );
                    let length = (actual[1][along] - actual[0][along]).abs();
                    for gap in anchors.gaps_edges(
                        AnchorCandidate {
                            endpoints_mm: actual,
                            bead_width_mm: constant_width(path)?,
                        },
                        earlier,
                        work,
                        cancel,
                    )? {
                        if !matches!(
                            gap_kind(
                                gap,
                                (gap.end - gap.start) * length,
                                constant_width(path)?,
                                recipe.process.max_bridge_span_mm
                            ),
                            GapKind::Supported
                        ) {
                            return Err(failed(
                                "ordinary fitted skin lacks physically earlier actual support",
                            ));
                        }
                    }
                }
                live = retained;
                add_bytes(&mut live, claimed.capacity())?;
                add_bytes(&mut live, anchors.retained_bytes())?;
                rows.admit(live, 0, 0)?;
                for (other_index, other) in layer.paths.iter().enumerate() {
                    if other_index == path_index {
                        continue;
                    }
                    charge(work, other.points_mm.len())?;
                    for edge in other.points_mm.windows(2) {
                        cancelled(cancel)?;
                        let a = project_xyz6_point_mm(edge[0])?;
                        let b = project_xyz6_point_mm(edge[1])?;
                        if a[fixed] == actual[0][fixed]
                            && b[fixed] == actual[0][fixed]
                            && a[2] == actual[0][2]
                            && b[2] == actual[0][2]
                        {
                            let other_interval = interval([a, b], along);
                            let actual_interval = interval(actual, along);
                            if actual_interval[0].max(other_interval[0])
                                < actual_interval[1].min(other_interval[1])
                            {
                                return Err(failed(
                                    "skin fit duplicates positive collinear commanded extrusion",
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
