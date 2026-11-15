// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent check of road connectors against the actual walls of their
//! layer, after the decoded program/path bijection. Shares no code with the
//! planner's connector construction.
use super::{cancelled, failed, project_xyz6_point_mm};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

/// Slack on the wall band beyond half wall plus half road: the recorded
/// offset tolerance on each side, plus output rounding.
const BAND_SLACK_MM: f64 = 0.010_002;
const CELL_MM: f64 = 2.0;

struct Walls {
    origin: [f64; 2],
    columns: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
    /// (a, b, half width)
    segments: Vec<([f64; 2], [f64; 2], f64)>,
}
impl Walls {
    fn new(layer: &PrintLayer) -> Result<Self, ManufacturingError> {
        let mut segments = Vec::new();
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for path in layer
            .paths
            .iter()
            .filter(|p| p.owner == Some(DepositionOwner::Wall))
        {
            for (edge, pair) in path.points_mm.windows(2).enumerate() {
                let half = 0.5
                    * match &path.width {
                        WidthProfile::Constant { width_mm } => *width_mm,
                        WidthProfile::PerEdge { widths_mm } => widths_mm[edge],
                    };
                let a = project_xyz6_point_mm(pair[0])?;
                let b = project_xyz6_point_mm(pair[1])?;
                let (a, b) = ([a[0], a[1]], [b[0], b[1]]);
                for p in [a, b] {
                    for axis in 0..2 {
                        lo[axis] = lo[axis].min(p[axis]);
                        hi[axis] = hi[axis].max(p[axis]);
                    }
                }
                segments.push((a, b, half));
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
        let mut walls = Self {
            origin: lo,
            columns,
            rows,
            cells: vec![Vec::new(); columns * rows],
            segments,
        };
        for k in 0..walls.segments.len() {
            let (a, b, _) = walls.segments[k];
            let (c0, r0) = walls.cell([a[0].min(b[0]), a[1].min(b[1])]);
            let (c1, r1) = walls.cell([a[0].max(b[0]), a[1].max(b[1])]);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    walls.cells[r * columns + c].push(k as u32);
                }
            }
        }
        Ok(walls)
    }
    fn cell(&self, p: [f64; 2]) -> (usize, usize) {
        let c = ((p[0] - self.origin[0]) / CELL_MM).floor().max(0.0) as usize;
        let r = ((p[1] - self.origin[1]) / CELL_MM).floor().max(0.0) as usize;
        (c.min(self.columns - 1), r.min(self.rows - 1))
    }
    fn candidates(&self, lo: [f64; 2], hi: [f64; 2], out: &mut Vec<u32>) {
        out.clear();
        if self.columns == 0 {
            return;
        }
        let (c0, r0) = self.cell(lo);
        let (c1, r1) = self.cell(hi);
        for r in r0..=r1 {
            for c in c0..=c1 {
                out.extend(&self.cells[r * self.columns + c]);
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

fn distance_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

fn crosses(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    orient(a, b, c) * orient(a, b, d) < 0.0 && orient(c, d, a) * orient(c, d, b) < 0.0
}

/// Every connector edge stays within the bead band inside an actual wall of
/// its layer along its whole length and crosses no wall; connectors appear
/// only on single-tool skin, solid and rectilinear/Gyroid sparse roads.
pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut scratch = Vec::new();
    for layer in &plan.layers {
        cancelled(cancel)?;
        if layer.paths.iter().all(|p| p.connectors.is_empty()) {
            continue;
        }
        let walls = Walls::new(layer)?;
        for path in layer.paths.iter().filter(|p| !p.connectors.is_empty()) {
            if !matches!(
                (path.kind, path.owner),
                (
                    DepositionKind::BottomSkin | DepositionKind::TopSkin,
                    Some(DepositionOwner::MandatorySkin { .. })
                ) | (
                    DepositionKind::SolidFill | DepositionKind::SparseFill,
                    Some(DepositionOwner::Regional { .. })
                )
            ) {
                return Err(failed("connector on a road role that cannot connect"));
            }
            let WidthProfile::Constant { width_mm } = path.width else {
                return Err(failed("connected roads require one bead width"));
            };
            for run in &path.connectors {
                for edge in run.first as usize..run.last as usize {
                    cancelled(cancel)?;
                    let a = project_xyz6_point_mm(path.points_mm[edge])?;
                    let b = project_xyz6_point_mm(path.points_mm[edge + 1])?;
                    let (a, b) = ([a[0], a[1]], [b[0], b[1]]);
                    let reach = 4.0 * width_mm;
                    walls.candidates(
                        [a[0].min(b[0]) - reach, a[1].min(b[1]) - reach],
                        [a[0].max(b[0]) + reach, a[1].max(b[1]) + reach],
                        &mut scratch,
                    );
                    *work = work
                        .checked_add(scratch.len().max(1))
                        .ok_or_else(|| failed("connector verification work overflow"))?;
                    if scratch.iter().any(|k| {
                        let (c, d, _) = walls.segments[*k as usize];
                        crosses(a, b, c, d)
                    }) {
                        return Err(failed("connector crosses an actual wall"));
                    }
                    // Sample at a quarter bead, ends included: every sample
                    // lies within some wall's band.
                    let length = (b[0] - a[0]).hypot(b[1] - a[1]);
                    let samples = (length / (0.25 * width_mm)).ceil().max(1.0) as usize;
                    for k in 0..=samples {
                        let t = k as f64 / samples as f64;
                        let p = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                        let inside = scratch.iter().any(|s| {
                            let (c, d, half) = walls.segments[*s as usize];
                            distance_to_segment(p, c, d) <= half + 0.5 * width_mm + BAND_SLACK_MM
                        });
                        if !inside {
                            return Err(failed(
                                "connector leaves the bead band inside the layer's walls",
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
