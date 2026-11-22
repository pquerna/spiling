// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Continuous roads: join the ends of clipped infill and skin roads along the
//! inside of the layer's walls, so each region prints as few uninterrupted
//! extrusions as its geometry allows. One rule serves every eligible pattern
//! (skin and solid rows; rectilinear and Grid sparse rows; Gyroid curves;
//! Cathedral rooms are their own closed construction): a road end
//! lies in the bead band inside a wall; two ends that are neighbours along
//! that wall, with nothing between them, may be joined by a connector that
//! follows the wall at the roads' offset. Every road keeps its exact
//! geometry; a connector is ordinary deposition of the joined roads' role,
//! recorded as a `ConnectorRun` and checked by independent replay against the
//! actual walls. Ends that cannot be joined stay ends.
use super::*;

/// Slack on the wall band, beyond half the wall plus half the road: the
/// recorded offset tolerance on each side.
const BAND_SLACK_MM: f64 = 2.0 * crate::planner::OFFSET_TOLERANCE_MM;
/// Uniform grid cell for wall and road lookups.
const CELL_MM: f64 = 2.0;

fn distance2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Nearest point of segment `a..b` to `p`: (distance, parameter, point).
fn project(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64, [f64; 2]) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + t * d[0], a[1] + t * d[1]];
    (distance2(p, q), t, q)
}

/// Proper intersection of two segments (touching does not count).
fn crosses(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (o1, o2) = (orient(a, b, c), orient(a, b, d));
    let (o3, o4) = (orient(c, d, a), orient(c, d, b));
    o1 * o2 < 0.0 && o3 * o4 < 0.0
}

/// One indexed edge: (path, edge, start, end, half width).
type Segment = (u32, u32, [f64; 2], [f64; 2], f64);

/// Segments of a set of paths, bucketed on a uniform grid.
pub(crate) struct SegmentGrid {
    origin: [f64; 2],
    columns: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
    segments: Vec<Segment>,
}
impl SegmentGrid {
    pub(crate) fn new<'a>(
        paths: impl Iterator<Item = (usize, &'a DepositionPath)>,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let mut segments = Vec::new();
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for (index, path) in paths {
            check_cancel(cancel)?;
            for (edge, pair) in path.points_mm.windows(2).enumerate() {
                let half = 0.5
                    * match &path.width {
                        WidthProfile::Constant { width_mm } => *width_mm,
                        WidthProfile::PerEdge { widths_mm } => widths_mm[edge],
                    };
                let (a, b) = ([pair[0][0], pair[0][1]], [pair[1][0], pair[1][1]]);
                for p in [a, b] {
                    for axis in 0..2 {
                        lo[axis] = lo[axis].min(p[axis]);
                        hi[axis] = hi[axis].max(p[axis]);
                    }
                }
                segments.push((index as u32, edge as u32, a, b, half));
            }
        }
        if segments.is_empty() {
            return Ok(Self {
                origin: [0.0; 2],
                columns: 0,
                rows: 0,
                cells: Vec::new(),
                segments,
            });
        }
        let columns = (((hi[0] - lo[0]) / CELL_MM).floor() as usize + 1).min(4096);
        let rows = (((hi[1] - lo[1]) / CELL_MM).floor() as usize + 1).min(4096);
        let mut grid = Self {
            origin: lo,
            columns,
            rows,
            cells: vec![Vec::new(); columns * rows],
            segments,
        };
        for (k, s) in grid.segments.iter().enumerate() {
            let (c0, r0) = grid.cell([s.2[0].min(s.3[0]), s.2[1].min(s.3[1])]);
            let (c1, r1) = grid.cell([s.2[0].max(s.3[0]), s.2[1].max(s.3[1])]);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    grid.cells[r * columns + c].push(k as u32);
                }
            }
        }
        Ok(grid)
    }
    fn cell(&self, p: [f64; 2]) -> (usize, usize) {
        let c = ((p[0] - self.origin[0]) / CELL_MM).floor().max(0.0) as usize;
        let r = ((p[1] - self.origin[1]) / CELL_MM).floor().max(0.0) as usize;
        (c.min(self.columns - 1), r.min(self.rows - 1))
    }
    /// Segment indexes whose cells meet the square of `radius` around `p`.
    fn near(&self, p: [f64; 2], radius: f64, out: &mut Vec<u32>) {
        out.clear();
        if self.columns == 0 {
            return;
        }
        let (c0, r0) = self.cell([p[0] - radius, p[1] - radius]);
        let (c1, r1) = self.cell([p[0] + radius, p[1] + radius]);
        for r in r0..=r1 {
            for c in c0..=c1 {
                out.extend(&self.cells[r * self.columns + c]);
            }
        }
        out.sort_unstable();
        out.dedup();
    }
    /// Nearest segment to `p` within `radius`: (segment, distance, t, point).
    fn nearest(
        &self,
        p: [f64; 2],
        radius: f64,
        scratch: &mut Vec<u32>,
    ) -> Option<(usize, f64, f64, [f64; 2])> {
        self.near(p, radius, scratch);
        scratch
            .iter()
            .map(|k| {
                let s = &self.segments[*k as usize];
                let (d, t, q) = project(p, s.2, s.3);
                (*k as usize, d, t, q)
            })
            .filter(|n| n.1 <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
    }
    /// Distance from `p` to the nearest segment within `radius`, if any.
    pub(crate) fn distance_within(
        &self,
        p: [f64; 2],
        radius: f64,
        scratch: &mut Vec<u32>,
    ) -> Option<(f64, f64)> {
        self.nearest(p, radius, scratch)
            .map(|(k, d, _, _)| (d, self.segments[k].4))
    }
    /// Whether segment `a..b` properly crosses any segment.
    pub(crate) fn crossed(&self, a: [f64; 2], b: [f64; 2], scratch: &mut Vec<u32>) -> bool {
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        self.near(mid, distance2(a, b) * 0.5 + CELL_MM, scratch);
        scratch.iter().any(|k| {
            let s = &self.segments[*k as usize];
            crosses(a, b, s.2, s.3)
        })
    }
}

fn eligible(path: &DepositionPath, policy: &SparseInfillPolicy, layer: &PrintLayer) -> bool {
    let kind_ok = match (path.kind, path.owner) {
        (
            DepositionKind::BottomSkin | DepositionKind::TopSkin,
            Some(DepositionOwner::MandatorySkin { .. }),
        ) => true,
        (DepositionKind::SolidFill, Some(DepositionOwner::Regional { .. })) => true,
        (DepositionKind::SparseFill, Some(DepositionOwner::Regional { .. })) => !matches!(
            policy,
            SparseInfillPolicy::CombineEligible { .. } | SparseInfillPolicy::WebRoof { .. }
        ),
        // Sparse infill spanning the rows beneath it connects like sparse.
        (DepositionKind::Bridge, owner) => {
            speeds::is_sparse_region_bridge(layer, path.kind, owner)
                && !matches!(policy, SparseInfillPolicy::CombineEligible { .. })
        }
        _ => false,
    };
    kind_ok
        && path.points_mm.len() >= 2
        && path.points_mm.first() != path.points_mm.last()
        && path.arcs.is_empty()
        && path.connectors.is_empty()
        && matches!(path.width, WidthProfile::Constant { .. })
        && path.physical_z_mm == [layer.lower_z_mm, layer.z_mm]
}

#[derive(Clone, Copy)]
struct End {
    path: usize,
    /// 0 = first point, 1 = last point.
    side: usize,
    wall: usize,
    /// Arc position along the wall path.
    s: f64,
    /// Signed distance from the wall centerline (sign = side of the wall).
    offset: f64,
}

/// Point at arc position `s` along a wall polyline, offset to `offset` on the
/// signed side, with a mitred vertex normal.
fn along(points: &[[f64; 3]], cumulative: &[f64], s: f64, offset: f64) -> [f64; 2] {
    let n = points.len();
    let i = cumulative
        .partition_point(|c| *c <= s)
        .saturating_sub(1)
        .min(n - 2);
    let (a, b) = (points[i], points[i + 1]);
    let length = cumulative[i + 1] - cumulative[i];
    let t = if length > 0.0 {
        ((s - cumulative[i]) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let normal = |p: [f64; 3], q: [f64; 3]| {
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        let l = dx.hypot(dy);
        [-dy / l, dx / l]
    };
    let mut nrm = normal(a, b);
    // At a vertex, use the mitre of the two adjacent edges.
    let at_vertex = |k: usize, nrm: [f64; 2]| -> [f64; 2] {
        if k == 0 || k + 1 >= n {
            return nrm;
        }
        let (n0, n1) = (
            normal(points[k - 1], points[k]),
            normal(points[k], points[k + 1]),
        );
        let m = [n0[0] + n1[0], n0[1] + n1[1]];
        let l = m[0].hypot(m[1]);
        let cos = (n0[0] * n1[0] + n0[1] * n1[1]).max(-0.9);
        let scale = 1.0 / ((1.0 + cos) * 0.5).sqrt();
        if l > 1e-12 {
            [m[0] / l * scale, m[1] / l * scale]
        } else {
            nrm
        }
    };
    if t == 0.0 {
        nrm = at_vertex(i, nrm);
    } else if t == 1.0 {
        nrm = at_vertex(i + 1, nrm);
    }
    [
        a[0] + t * (b[0] - a[0]) + nrm[0] * offset,
        a[1] + t * (b[1] - a[1]) + nrm[1] * offset,
    ]
}

/// A sparse region's Bridge row spans the rows beneath it. Connectors on the
/// layer below lie along the walls and may now support such a row
/// throughout; an authored-axis row is then ordinary sparse fill and keeps no
/// Bridge label (or bridge flow) it no longer earns.
fn relabel_supported_sparse_bridges(
    (previous, layer): (&PrintLayer, &mut PrintLayer),
    course: u32,
    intent: &ManufacturingIntent,
    anchors: &mut crate::planner::support::AnchorWorkspace,
    cache_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    use crate::planner::support::{AnchorCandidate, GapKind, gap_kind, projected_edges};
    use spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;
    if previous.paths.iter().all(|p| p.connectors.is_empty()) || previous.z_mm != layer.lower_z_mm {
        return Ok(false);
    }
    let mut projected = None;
    let mut changed = false;
    for path_index in 0..layer.paths.len() {
        check_cancel(cancel)?;
        let path = &layer.paths[path_index];
        if !speeds::is_sparse_region_bridge(layer, path.kind, path.owner)
            || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
            || !on_sparse_axis(path, &intent.recipe, course)
        {
            continue;
        }
        let WidthProfile::Constant { width_mm } = path.width else {
            continue;
        };
        let mut spans_gap = false;
        for (first, last) in path.road_ranges() {
            for edge in first..last {
                let ends = [
                    project_xyz6_point_mm(path.points_mm[edge])?,
                    project_xyz6_point_mm(path.points_mm[edge + 1])?,
                ];
                let length = (ends[1][0] - ends[0][0]).hypot(ends[1][1] - ends[0][1]);
                if projected.is_none() {
                    projected = Some(cache_lower_edges(previous, cache_bytes, cancel)?);
                }
                let candidate = AnchorCandidate {
                    endpoints_mm: ends,
                    bead_width_mm: width_mm,
                };
                let gaps = match &projected {
                    Some(Some(edges)) => anchors.gaps_edges(
                        candidate,
                        edges
                            .iter()
                            .map(|&(points, width)| (Ok(points), true, width)),
                        work,
                        cancel,
                    ),
                    _ => anchors.gaps_edges(
                        candidate,
                        projected_edges(previous.paths.iter(), true),
                        work,
                        cancel,
                    ),
                }?;
                for gap in gaps {
                    let span = (gap.end - gap.start) * length;
                    spans_gap |= !matches!(
                        gap_kind(
                            gap,
                            span,
                            width_mm,
                            intent.recipe.process.max_bridge_span_mm
                        ),
                        GapKind::Supported
                    );
                }
            }
        }
        if !spans_gap {
            layer.paths[path_index].kind = DepositionKind::SparseFill;
            changed = true;
        }
    }
    Ok(changed)
}

type LowerEdge = ([[f64; 3]; 2], f64);

/// Optional layer-local cache. A failed projection stays on the original
/// checked query path, preserving its refusal and operand order.
fn cache_lower_edges(
    previous: &PrintLayer,
    cache_bytes: usize,
    cancel: &AtomicBool,
) -> Result<Option<Vec<LowerEdge>>, ManufacturingError> {
    let count = previous.paths.iter().try_fold(0usize, |count, path| {
        count.checked_add(path.points_mm.len().saturating_sub(1))
    });
    let Some(count) = count.filter(|&count| count <= cache_bytes / size_of::<LowerEdge>()) else {
        return Ok(None);
    };
    let mut edges = Vec::new();
    if edges.try_reserve_exact(count).is_err()
        || edges.capacity() > cache_bytes / size_of::<LowerEdge>()
    {
        return Ok(None);
    }
    for (points, _, width) in crate::planner::support::projected_edges(previous.paths.iter(), true)
    {
        check_cancel(cancel)?;
        let Ok(points) = points else { return Ok(None) };
        edges.push((points, width));
    }
    Ok(Some(edges))
}

fn on_sparse_axis(path: &DepositionPath, recipe: &PlanarPrintRecipe, course: u32) -> bool {
    if matches!(
        recipe.sparse_infill,
        SparseInfillPolicy::Grid | SparseInfillPolicy::Gyroid | SparseInfillPolicy::WebRoof { .. }
    ) {
        return true;
    }
    let fixed = match recipe.rectilinear_axis {
        RectilinearAxisPolicy::X => 1,
        RectilinearAxisPolicy::Y => 0,
        RectilinearAxisPolicy::AlternatingXY => usize::from(course.is_multiple_of(2)),
        RectilinearAxisPolicy::AlternatingYX => usize::from(!course.is_multiple_of(2)),
    };
    path.road_ranges().all(|(first, last)| {
        project_xyz6_coordinate_mm(path.points_mm[first][fixed]).is_ok_and(|coordinate| {
            path.points_mm[first + 1..=last].iter().all(|point| {
                project_xyz6_coordinate_mm(point[fixed]).is_ok_and(|value| value == coordinate)
            })
        })
    })
}

/// Join eligible roads on every single-tool layer whose rows no other
/// construction names individually.
pub(super) fn connect_roads(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    routed: bool,
    alternation: &[u32],
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    if routed || !plan.sparse_bands.is_empty() {
        return Ok(0);
    }
    let mut anchors = crate::planner::support::AnchorWorkspace::default();
    let mut work = 0usize;
    let mut payload = crate::planner::layer_payload_bytes(&plan.layers).ok();
    for index in 0..plan.layers.len() {
        check_cancel(cancel)?;
        let before = layer_bytes(&plan.layers[index]);
        // Leave room for the anchor workspace even when all lower edges hit.
        let cache_bytes = payload.map_or(0, |bytes| {
            crate::working_set::bytes()
                .saturating_sub(bytes)
                .saturating_sub(anchors.retained_bytes())
                / 4
        });
        let relabelled = if index > 0 {
            let (lower, upper) = plan.layers.split_at_mut(index);
            relabel_supported_sparse_bridges(
                (&lower[index - 1], &mut upper[0]),
                alternation[index],
                intent,
                &mut anchors,
                cache_bytes,
                &mut work,
                cancel,
            )?
        } else {
            false
        };
        // Joining groups by role, so relabelling must precede connector
        // discovery. Relabelling itself reads the connected layer below.
        let connected = connect_layer(plan, index, alternation, intent, cancel)?;
        if connected || relabelled {
            let layer = &mut plan.layers[index];
            crate::planner::support::object_volumes(std::slice::from_mut(layer), intent)?;
            payload = payload
                .and_then(|bytes| bytes.checked_sub(before)?.checked_add(layer_bytes(layer)));
        }
    }
    Ok(work)
}

fn layer_bytes(layer: &PrintLayer) -> usize {
    crate::planner::layer_payload_bytes(std::slice::from_ref(layer)).unwrap_or(usize::MAX)
}

fn connect_layer(
    plan: &mut NormalizedPrintPlan,
    index: usize,
    alternation: &[u32],
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let policy = intent.recipe.sparse_infill;
    let layer = &plan.layers[index];
    // Separators, diaphragms, process interfaces, skin-bridge domains and
    // minimum-command fits name individual rows of this layer.
    if layer
        .paths
        .iter()
        .any(|p| p.kind == DepositionKind::SeparatorWall)
        || !layer.region_coverage.process_diaphragms.is_empty()
        || !layer.region_coverage.process_interfaces.is_empty()
        || !layer.region_coverage.skin_bridge_domains.is_empty()
        || !layer.skin_command_fits.is_empty()
    {
        return Ok(false);
    }
    let candidates: Vec<usize> = (0..layer.paths.len())
        .filter(|i| eligible(&layer.paths[*i], &policy, layer))
        .collect();
    if candidates.len() < 2 {
        return Ok(false);
    }
    // A bridge on the next layer was decided from the roads below it; no
    // connector within two bead widths of its footprint.
    let bridges: Vec<[f64; 4]> = plan
        .layers
        .get(index + 1)
        .into_iter()
        .flat_map(|above| &above.paths)
        .filter(|p| {
            p.kind == DepositionKind::Bridge
                && (!speeds::is_sparse_region_bridge(&plan.layers[index + 1], p.kind, p.owner)
                    || !on_sparse_axis(p, &intent.recipe, alternation[index + 1]))
                || matches!(p.owner, Some(DepositionOwner::RegionalSkinBridge { .. }))
        })
        .map(|p| {
            let width = match &p.width {
                WidthProfile::Constant { width_mm } => *width_mm,
                WidthProfile::PerEdge { widths_mm } => {
                    widths_mm.iter().copied().fold(0.0, f64::max)
                }
            };
            let margin = 2.0 * width;
            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for q in &p.points_mm {
                for axis in 0..2 {
                    lo[axis] = lo[axis].min(q[axis]);
                    hi[axis] = hi[axis].max(q[axis]);
                }
            }
            [
                lo[0] - margin,
                lo[1] - margin,
                hi[0] + margin,
                hi[1] + margin,
            ]
        })
        .collect();
    // Support above may rest on these roads: replay decides each lower
    // path's rooting as a whole, so a road under support within the
    // support gap keeps its own identity.
    let gap = match intent.recipe.supports {
        SupportPolicy::Breakaway { z_gap_mm, .. } => z_gap_mm,
        SupportPolicy::Disabled => 0.0,
    };
    let z = plan.layers[index].z_mm;
    let supported: Vec<[f64; 4]> = plan.layers[index + 1..]
        .iter()
        .take_while(|above| above.lower_z_mm <= z + gap + 1e-9)
        .flat_map(|above| &above.paths)
        .filter(|p| {
            matches!(
                p.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            )
        })
        .map(|p| {
            let margin = match &p.width {
                WidthProfile::Constant { width_mm } => *width_mm,
                WidthProfile::PerEdge { widths_mm } => {
                    widths_mm.iter().copied().fold(0.0, f64::max)
                }
            };
            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for q in &p.points_mm {
                for axis in 0..2 {
                    lo[axis] = lo[axis].min(q[axis]);
                    hi[axis] = hi[axis].max(q[axis]);
                }
            }
            [
                lo[0] - margin,
                lo[1] - margin,
                hi[0] + margin,
                hi[1] + margin,
            ]
        })
        .collect();
    let under_support = |p: &DepositionPath| {
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for q in &p.points_mm {
            for axis in 0..2 {
                lo[axis] = lo[axis].min(q[axis]);
                hi[axis] = hi[axis].max(q[axis]);
            }
        }
        supported
            .iter()
            .any(|r| lo[0] <= r[2] && hi[0] >= r[0] && lo[1] <= r[3] && hi[1] >= r[1])
    };
    let layer = &plan.layers[index];
    let candidates: Vec<usize> = candidates
        .into_iter()
        .filter(|i| !under_support(&layer.paths[*i]))
        .collect();
    if candidates.len() < 2 {
        return Ok(false);
    }
    let walls = SegmentGrid::new(
        layer
            .paths
            .iter()
            .enumerate()
            .filter(|(_, p)| p.owner == Some(DepositionOwner::Wall)),
        cancel,
    )?;
    let roads = SegmentGrid::new(
        layer
            .paths
            .iter()
            .enumerate()
            .filter(|(_, p)| p.owner != Some(DepositionOwner::Wall)),
        cancel,
    )?;
    let mut scratch = Vec::new();
    // Cumulative arc length of every wall path.
    let cumulative: Vec<Vec<f64>> = layer
        .paths
        .iter()
        .map(|p| {
            if p.owner != Some(DepositionOwner::Wall) {
                return Vec::new();
            }
            let mut c = vec![0.0];
            for e in p.points_mm.windows(2) {
                c.push(c.last().unwrap() + (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]));
            }
            c
        })
        .collect();
    let mut ends: Vec<End> = Vec::new();
    for &path in &candidates {
        check_cancel(cancel)?;
        let p = &layer.paths[path];
        let WidthProfile::Constant { width_mm } = p.width else {
            unreachable!()
        };
        for side in 0..2 {
            let point = p.points_mm[if side == 0 { 0 } else { p.points_mm.len() - 1 }];
            let q = [point[0], point[1]];
            // Widest admitted wall plus this road, searched generously.
            let Some((k, distance, t, _)) = walls.nearest(q, 4.0 * width_mm, &mut scratch) else {
                continue;
            };
            let (wall, edge, a, b, half) = walls.segments[k];
            if distance > half + 0.5 * width_mm + BAND_SLACK_MM {
                continue;
            }
            let sign = ((b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])).signum();
            let c = &cumulative[wall as usize];
            let s = c[edge as usize] + t * (c[edge as usize + 1] - c[edge as usize]);
            ends.push(End {
                path,
                side,
                wall: wall as usize,
                s,
                offset: sign * distance,
            });
        }
    }
    // Neighbouring ends along each wall: only these may be joined, so a
    // connector never passes another road's end.
    ends.sort_by(|a, b| a.wall.cmp(&b.wall).then(a.s.total_cmp(&b.s)));
    let key = |p: &DepositionPath| {
        (
            p.kind,
            p.owner,
            p.material_domain,
            p.physical_z_mm.map(f64::to_bits),
            p.exposed,
            match p.width {
                WidthProfile::Constant { width_mm } => width_mm.to_bits(),
                _ => 0,
            },
        )
    };
    let mut pairs: Vec<(f64, usize, usize, bool)> = Vec::new();
    let mut start = 0;
    while start < ends.len() {
        let wall = ends[start].wall;
        let mut stop = start;
        while stop < ends.len() && ends[stop].wall == wall {
            stop += 1;
        }
        let points = &layer.paths[wall].points_mm;
        let closed = points.first() == points.last();
        let total = *cumulative[wall].last().unwrap();
        let count = stop - start;
        let links = if closed && count > 2 {
            count
        } else {
            count - 1
        };
        for k in 0..links {
            let (i, j) = (start + k, start + (k + 1) % count);
            let (a, b) = (ends[i], ends[j]);
            let forward = if j > i { b.s - a.s } else { b.s + total - a.s };
            if a.path == b.path
                || a.offset.signum() != b.offset.signum()
                || key(&layer.paths[a.path]) != key(&layer.paths[b.path])
                || forward > MAX_CONNECTOR_LENGTH_MM
            {
                continue;
            }
            pairs.push((forward, i, j, j < i));
        }
        start = stop;
    }
    // Shortest first, but treat lengths within a tenth of a millimetre as
    // equal and take those in order along each wall: equal rows then join
    // as one zigzag instead of colliding on float noise.
    pairs.sort_by(|a, b| {
        let bucket = |l: f64| (l / 0.1).floor();
        bucket(a.0).total_cmp(&bucket(b.0)).then(a.1.cmp(&b.1))
    });
    // Union-find over roads; each end joins at most once; no cycles.
    let mut parent: Vec<usize> = (0..layer.paths.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut joined =
        std::collections::HashMap::<(usize, usize), (usize, usize, Vec<[f64; 3]>)>::new();
    for (_, i, j, wraps) in pairs {
        check_cancel(cancel)?;
        let (a, b) = (ends[i], ends[j]);
        if joined.contains_key(&(a.path, a.side)) || joined.contains_key(&(b.path, b.side)) {
            continue;
        }
        let (ra, rb) = (find(&mut parent, a.path), find(&mut parent, b.path));
        if ra == rb {
            continue;
        }
        let wall = &layer.paths[a.wall].points_mm;
        let c = &cumulative[a.wall];
        let total = *c.last().unwrap();
        let z = layer.z_mm;
        let pa = layer.paths[a.path].points_mm[if a.side == 0 {
            0
        } else {
            layer.paths[a.path].points_mm.len() - 1
        }];
        let pb = layer.paths[b.path].points_mm[if b.side == 0 {
            0
        } else {
            layer.paths[b.path].points_mm.len() - 1
        }];
        // Wall vertices strictly between the two arc positions, in order;
        // a closed wall also offers its seam vertex.
        let span = if wraps { b.s + total - a.s } else { b.s - a.s };
        let closed = wall.first() == wall.last();
        let vertices = if closed {
            0..wall.len() - 1
        } else {
            1..wall.len() - 1
        };
        let mut between: Vec<(f64, [f64; 3])> = Vec::new();
        for v in vertices {
            let sv = c[v];
            let rel = if sv < a.s { sv + total - a.s } else { sv - a.s };
            if rel <= 1e-6 || rel >= span - 1e-6 {
                continue;
            }
            let offset = a.offset + (b.offset - a.offset) * rel / span;
            let q = along(wall, c, sv, offset);
            between.push((rel, [q[0], q[1], z]));
        }
        between.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut connector = vec![pa];
        connector.extend(between.into_iter().map(|(_, q)| q));
        connector.push(pb);
        connector.dedup_by(|q, p| {
            distance2([q[0], q[1]], [p[0], p[1]]) < program_coordinates::MIN_DEPOSITION_COMMAND_MM
        });
        // Keep the exact far end; ends closer than one command never join.
        *connector.last_mut().unwrap() = pb;
        if connector.len() < 2 {
            continue;
        }
        let width = match layer.paths[a.path].width {
            WidthProfile::Constant { width_mm } => width_mm,
            _ => unreachable!(),
        };
        // Check the connector before accepting it: inside the wall band,
        // crossing no wall, near no other road, clear of next-layer
        // bridges, within its length.
        let length: f64 = connector
            .windows(2)
            .map(|e| distance2([e[0][0], e[0][1]], [e[1][0], e[1][1]]))
            .sum();
        let mut ok = length <= MAX_CONNECTOR_LENGTH_MM
            && connector.windows(2).all(|e| {
                distance2([e[0][0], e[0][1]], [e[1][0], e[1][1]])
                    >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
            });
        for e in connector.windows(2) {
            if !ok {
                break;
            }
            let (p, q) = ([e[0][0], e[0][1]], [e[1][0], e[1][1]]);
            if walls.crossed(p, q, &mut scratch)
                || bridges.iter().any(|r| {
                    p[0].min(q[0]) <= r[2]
                        && p[0].max(q[0]) >= r[0]
                        && p[1].min(q[1]) <= r[3]
                        && p[1].max(q[1]) >= r[1]
                })
            {
                ok = false;
                break;
            }
            let samples = (distance2(p, q) / (0.25 * width)).ceil().max(1.0) as usize;
            for k in 0..=samples {
                let t = k as f64 / samples as f64;
                let x = [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])];
                match walls.distance_within(x, 4.0 * width, &mut scratch) {
                    Some((d, half)) if d <= half + 0.5 * width + BAND_SLACK_MM => {}
                    _ => {
                        ok = false;
                        break;
                    }
                }
                roads.near(x, 0.5 * width, &mut scratch);
                if scratch.iter().any(|s| {
                    let seg = &roads.segments[*s as usize];
                    let other = seg.0 as usize;
                    // Adjacent solid rows overlap by design; a connector
                    // may overlap another road no more than half a bead.
                    other != a.path && other != b.path && project(x, seg.2, seg.3).0 < 0.5 * width
                }) {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        parent[ra] = rb;
        joined.insert((a.path, a.side), (b.path, b.side, connector.clone()));
        connector.reverse();
        joined.insert((b.path, b.side), (a.path, a.side, connector));
    }
    if joined.is_empty() {
        return Ok(false);
    }
    // Walk each chain from a road end that is not joined.
    let layer = &mut plan.layers[index];
    let mut taken = vec![false; layer.paths.len()];
    let mut chains = Vec::new();
    for &start in &candidates {
        if taken[start] {
            continue;
        }
        let open = (0..2).find(|side| !joined.contains_key(&(start, *side)));
        let Some(open) = open else { continue };
        if !joined.contains_key(&(start, 1 - open)) {
            continue;
        }
        let mut path = layer.paths[start].clone();
        if open == 1 {
            path.reverse();
        }
        let mut points = path.points_mm.clone();
        let mut runs = Vec::new();
        let mut current = (start, 1 - open);
        taken[start] = true;
        while let Some((next, side, connector)) = joined.get(&current) {
            let first = points.len() - 1;
            points.extend(connector[1..connector.len() - 1].iter().copied());
            let mut road = layer.paths[*next].points_mm.clone();
            if *side == 1 {
                road.reverse();
            }
            points.extend(road);
            runs.push(ConnectorRun {
                first: first as u32,
                last: (first + connector.len() - 1) as u32,
            });
            taken[*next] = true;
            current = (*next, 1 - *side);
        }
        path.points_mm = points;
        path.connectors = runs;
        chains.push(path);
    }
    let mut kept = Vec::with_capacity(layer.paths.len());
    for (i, path) in std::mem::take(&mut layer.paths).into_iter().enumerate() {
        if !taken[i] {
            kept.push(path);
        }
    }
    kept.extend(chains);
    layer.paths = kept;
    Ok(true)
}
