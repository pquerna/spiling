// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent Cathedral replay (RFC 12, section 9), after the decoded
//! program/path bijection. It never calls the planner's offsets, columns or
//! tooth growth; it checks what the construction must achieve:
//!
//! 1. **Containment.** Every Cathedral road lies inside its layer's recorded
//!    domain, at least half a bead spacing from its boundary.
//! 2. **Rooms.** Every lattice line of the recorded frame and recipe pitch,
//!    away from the domain boundary, carries a road centerline half a spacing
//!    away on each side: a two-bead wall, with only short interruptions.
//! 3. **Ancestry.** Unsupported length along each road's physical run, over
//!    the adjacent earlier layer's actual model deposition, stays within the
//!    sparse allowance; every road has real supported length.
//! 4. **Roof coverage.** Every other model road on the next physical layer is
//!    supported over the Cathedral interior: by contact, by a span within the
//!    roof spacing, by a labelled Bridge within the bridge limit, or by a
//!    reported overhang.

use super::{cancelled, constant_width, failed, project_xyz6_point_mm};
use crate::planner::support::{
    AnchorCandidate, AnchorWorkspace, GapKind, PreparedAnchorPaths, gap_kind,
};
use spiling_contracts::manufacturing::*;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

/// Tolerance on recorded positions: the profile tolerance.
const TOLERANCE_MM: f64 = PROFILE_TOLERANCE_MM;

fn charge(work: &mut usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(1)
        .ok_or_else(|| failed("Cathedral replay work overflow"))?;
    Ok(())
}

fn cathedral_road(path: &DepositionPath) -> bool {
    path.kind == DepositionKind::SparseFill
        && matches!(path.owner, Some(DepositionOwner::Regional { .. }))
}

/// Uniform grid of segments for nearest-distance queries.
struct Segments {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
    segments: Vec<[[f64; 2]; 2]>,
}

impl Segments {
    fn new(segments: Vec<[[f64; 2]; 2]>, cell: f64) -> Self {
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (index, [a, b]) in segments.iter().enumerate() {
            let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
            let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
            for i in (x0 / cell).floor() as i64..=(x1 / cell).floor() as i64 {
                for j in (y0 / cell).floor() as i64..=(y1 / cell).floor() as i64 {
                    cells.entry((i, j)).or_default().push(index);
                }
            }
        }
        Self {
            cell,
            cells,
            segments,
        }
    }

    /// Visit segments that may lie within `radius` of `p`.
    fn near(&self, p: [f64; 2], radius: f64, mut visit: impl FnMut([[f64; 2]; 2])) {
        let r = (radius / self.cell).ceil() as i64;
        let (ci, cj) = (
            (p[0] / self.cell).floor() as i64,
            (p[1] / self.cell).floor() as i64,
        );
        let mut seen = Vec::new();
        for i in ci - r..=ci + r {
            for j in cj - r..=cj + r {
                for &index in self.cells.get(&(i, j)).into_iter().flatten() {
                    if !seen.contains(&index) {
                        seen.push(index);
                        visit(self.segments[index]);
                    }
                }
            }
        }
    }

    fn distance(&self, p: [f64; 2], radius: f64) -> f64 {
        let mut best = f64::INFINITY;
        self.near(p, radius, |[a, b]| {
            best = best.min(point_segment(p, a, b).0)
        });
        best
    }
}

/// Distance and nearest point.
fn point_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, [f64; 2]) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + t * d[0], a[1] + t * d[1]];
    ((p[0] - q[0]).hypot(p[1] - q[1]), q)
}

/// Even-odd containment in the domain (outer rings and holes).
fn inside(extent: &[DiaphragmPolygonMm], p: [f64; 2]) -> bool {
    let mut crossings = 0;
    for polygon in extent {
        for ring in std::iter::once(&polygon.outer).chain(&polygon.holes) {
            for edge in ring.windows(2) {
                let (a, b) = (edge[0], edge[1]);
                if (a[1] > p[1]) != (b[1] > p[1]) {
                    let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                    if x > p[0] {
                        crossings += 1;
                    }
                }
            }
        }
    }
    crossings % 2 == 1
}

fn boundary(extent: &[DiaphragmPolygonMm]) -> Vec<[[f64; 2]; 2]> {
    extent
        .iter()
        .flat_map(|polygon| std::iter::once(&polygon.outer).chain(&polygon.holes))
        .flat_map(|ring| ring.windows(2).map(|e| [e[0], e[1]]))
        .collect()
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if !matches!(recipe.sparse_infill, SparseInfillPolicy::WebRoof { .. }) {
        return Ok(());
    }
    let w = recipe.bead_width_mm;
    let nominal = recipe.layer_height_mm;
    let roof_spacing = recipe.web_roof_spacing_mm();
    for (index, layer) in plan.layers.iter().enumerate() {
        cancelled(cancel)?;
        let coverage = &layer.region_coverage;
        let roads: Vec<&DepositionPath> =
            layer.paths.iter().filter(|p| cathedral_road(p)).collect();
        if roads.is_empty() && coverage.web_roof_domains.is_empty() {
            continue;
        }
        let height = layer.z_mm - layer.lower_z_mm;
        let s = w - height * (1.0 - std::f64::consts::PI / 4.0);
        // Road segments of this layer, for rooms and coverage.
        let mut road_segments = Vec::new();
        for path in &roads {
            if path.points_mm.first() != path.points_mm.last()
                || !path.arcs.is_empty()
                || !path.connectors.is_empty()
                || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
                || constant_width(path)? != w
            {
                return Err(failed(
                    "Cathedral roads are closed fine-interval loops of one bead width",
                ));
            }
            let Some(DepositionOwner::Regional { region_index }) = path.owner else {
                unreachable!()
            };
            let domain = coverage
                .web_roof_domains
                .iter()
                .find(|d| d.region_index == region_index)
                .ok_or_else(|| failed("Cathedral road lacks its classified domain"))?;
            // 1. Containment.
            let edges = Segments::new(boundary(&domain.extent), 2.0);
            for edge in path.points_mm.windows(2) {
                charge(work)?;
                let a = project_xyz6_point_mm(edge[0])?;
                let b = project_xyz6_point_mm(edge[1])?;
                for t in [0.0, 0.5] {
                    let p = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                    if !inside(&domain.extent, p)
                        || edges.distance(p, 0.5 * s) < 0.5 * s - TOLERANCE_MM
                    {
                        return Err(failed(format!(
                            "Cathedral road leaves its room domain: layer {} at {p:?}",
                            layer.index
                        )));
                    }
                }
                road_segments.push([[a[0], a[1]], [b[0], b[1]]]);
            }
        }
        let road_grid = Segments::new(road_segments, 2.0);
        // 2. Rooms: two-bead walls along every interior lattice line.
        for domain in &coverage.web_roof_domains {
            let Some(region) = coverage
                .process_regions
                .get(usize::from(domain.region_index))
            else {
                return Err(failed(
                    "Cathedral domain owner is outside classified regions",
                ));
            };
            if region.fraction <= 0.0 || region.fraction >= 1.0 {
                continue;
            }
            let area = (w - nominal) * nominal + std::f64::consts::PI * (nominal * 0.5).powi(2);
            let pitch = 4.0 * area / (nominal * region.fraction);
            if !pitch.is_finite() || pitch <= 0.0 {
                return Err(failed("Cathedral has no positive room pitch"));
            }
            let frame = domain.frame_mm;
            let edges = Segments::new(boundary(&domain.extent), 2.0);
            for axis in 0..2 {
                let length = frame[1][axis] - frame[0][axis];
                let count = (length / pitch).ceil().max(1.0);
                if !count.is_finite() || count > crate::working_set::units() as f64 {
                    return Err(failed("Cathedral lattice exceeds the command budget"));
                }
                let first = frame[0][axis] + (length - (count - 1.0) * pitch) * 0.5;
                let other = 1 - axis;
                let step = 0.5 * s;
                let samples = ((frame[1][other] - frame[0][other]) / step).ceil();
                if !samples.is_finite() || samples > crate::working_set::units() as f64 {
                    return Err(failed(
                        "Cathedral lattice sampling exceeds the command budget",
                    ));
                }
                for line in 0..count as usize {
                    let c = first + line as f64 * pitch;
                    // Interruption length per side along this line.
                    let mut gap = [0.0_f64; 2];
                    for k in 0..=samples as usize {
                        cancelled(cancel)?;
                        charge(work)?;
                        let v = frame[0][other] + k as f64 * step;
                        let mut p = [0.0; 2];
                        p[axis] = c;
                        p[other] = v;
                        if !inside(&domain.extent, p) || edges.distance(p, 3.0 * s) < 3.0 * s {
                            gap = [0.0; 2];
                            continue;
                        }
                        let mut nearest = [f64::INFINITY; 2];
                        road_grid.near(p, s, |[a, b]| {
                            let (d, q) = point_segment(p, a, b);
                            let side = usize::from(q[axis] > c);
                            nearest[side] = nearest[side].min(d);
                        });
                        for side in 0..2 {
                            if (nearest[side] - 0.5 * s).abs() <= TOLERANCE_MM {
                                gap[side] = 0.0;
                            } else {
                                gap[side] += step;
                                if gap[side] > 2.0 * s + TOLERANCE_MM {
                                    return Err(failed(format!(
                                        "Cathedral room wall lacks its bonded bead: layer {} line {axis}={c:.4} near {v:.4} side {side} nearest {nearest:?} s {s:.4}",
                                        layer.index
                                    )));
                                }
                            }
                        }
                    }
                }
            }
        }
        // 3. Ancestry along each road's physical run.
        let previous = index.checked_sub(1).map(|i| &plan.layers[i]);
        if let Some(lower) = previous
            && lower.z_mm != layer.lower_z_mm
            && !roads.is_empty()
        {
            return Err(failed(
                "Cathedral road crosses an unreported physical layer gap",
            ));
        }
        if previous.is_none() && layer.lower_z_mm != 0.0 && !roads.is_empty() {
            return Err(failed("Cathedral road lacks an earlier physical layer"));
        }
        if let Some(lower) = previous
            && !roads.is_empty()
        {
            let prepared = PreparedAnchorPaths::new(
                lower
                    .paths
                    .iter()
                    .filter(|p| p.kind.is_model() && p.physical_z_mm[1] == layer.lower_z_mm),
                cancel,
            )?;
            for path in &roads {
                let mut run = 0.0_f64;
                let mut leading: Option<f64> = None;
                let mut supported = 0.0_f64;
                let mut worst = 0.0_f64;
                for edge in path.points_mm.windows(2) {
                    cancelled(cancel)?;
                    charge(work)?;
                    let a = project_xyz6_point_mm(edge[0])?;
                    let b = project_xyz6_point_mm(edge[1])?;
                    let length = (b[0] - a[0]).hypot(b[1] - a[1]);
                    let gaps = anchors.gaps_prepared(
                        AnchorCandidate {
                            endpoints_mm: [a, b],
                            bead_width_mm: w,
                        },
                        &prepared,
                        work,
                        cancel,
                    )?;
                    let mut cursor = 0.0;
                    let mut unsupported = 0.0;
                    for gap in gaps {
                        let span = (gap.end - gap.start) * length;
                        let kind = gap_kind(gap, span, w, recipe.process.max_bridge_span_mm);
                        if gap.start > cursor {
                            // Real contact before this gap ends the run.
                            if leading.is_none() {
                                leading = Some(run);
                            }
                            run = 0.0;
                        }
                        match kind {
                            GapKind::Supported => {}
                            GapKind::Bridge | GapKind::Contact => {
                                // Anchored both ends inside one straight edge.
                                if leading.is_none() {
                                    leading = Some(run);
                                }
                                run = 0.0;
                            }
                            GapKind::Demand => {
                                run += span;
                                unsupported += span;
                                worst = worst.max(run);
                            }
                        }
                        cursor = gap.end;
                    }
                    if cursor < 1.0 {
                        if leading.is_none() {
                            leading = Some(run);
                        }
                        run = 0.0;
                    }
                    supported += (length - unsupported).max(0.0);
                }
                // A closed loop's last run continues into its first.
                if let Some(lead) = leading {
                    worst = worst.max(run + lead);
                } else {
                    worst = worst.max(run);
                }
                if supported < w {
                    return Err(failed(format!(
                        "Cathedral road has no rooted length on the layer below: layer {}",
                        layer.index
                    )));
                }
                if worst > MAX_SPARSE_UNSUPPORTED_RUN_WIDTHS * w
                    && coverage.unsupported_overhang_mm == 0.0
                {
                    return Err(failed(format!(
                        "Cathedral road has an unreported continuous unsupported run: layer {}, {worst:.4} mm",
                        layer.index
                    )));
                }
            }
        }
        // 4. Roof coverage: what the next physical layer deposits over this
        // layer's Cathedral interior.
        let Some(next) = plan.layers.get(index + 1) else {
            continue;
        };
        if next.lower_z_mm != layer.z_mm || coverage.web_roof_domains.is_empty() {
            continue;
        }
        let interior: Vec<(&WebRoofDomain, Segments)> = coverage
            .web_roof_domains
            .iter()
            .map(|d| (d, Segments::new(boundary(&d.extent), 2.0)))
            .collect();
        let over_interior = |p: [f64; 2]| {
            interior
                .iter()
                .any(|(d, edges)| inside(&d.extent, p) && edges.distance(p, w) >= w)
        };
        let prepared =
            PreparedAnchorPaths::new(layer.paths.iter().filter(|p| p.kind.is_model()), cancel)?;
        for path in next
            .paths
            .iter()
            .filter(|p| p.kind.is_model() && !cathedral_road(p))
        {
            let width = match &path.width {
                WidthProfile::Constant { width_mm } => *width_mm,
                WidthProfile::PerEdge { widths_mm } => {
                    widths_mm.iter().copied().fold(f64::INFINITY, f64::min)
                }
            };
            for edge in path.points_mm.windows(2) {
                cancelled(cancel)?;
                charge(work)?;
                let a = project_xyz6_point_mm(edge[0])?;
                let b = project_xyz6_point_mm(edge[1])?;
                let length = (b[0] - a[0]).hypot(b[1] - a[1]);
                if length == 0.0 {
                    continue;
                }
                for gap in anchors.gaps_prepared(
                    AnchorCandidate {
                        endpoints_mm: [a, b],
                        bead_width_mm: width,
                    },
                    &prepared,
                    work,
                    cancel,
                )? {
                    let t = 0.5 * (gap.start + gap.end);
                    let mid = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                    if !over_interior(mid) {
                        continue;
                    }
                    let span = (gap.end - gap.start) * length;
                    let kind = gap_kind(gap, span, width, recipe.process.max_bridge_span_mm);
                    let carried = match kind {
                        GapKind::Supported => true,
                        GapKind::Bridge | GapKind::Contact => {
                            span <= roof_spacing + TOLERANCE_MM
                                || path.kind == DepositionKind::Bridge
                        }
                        GapKind::Demand => false,
                    };
                    if !carried && next.region_coverage.unsupported_overhang_mm == 0.0 {
                        return Err(failed(format!(
                            "Cathedral roof is unsupported over its rooms: layer {} {:?} {:.4} mm",
                            next.index, path.kind, span
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}
