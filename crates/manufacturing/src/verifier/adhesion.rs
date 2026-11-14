// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Independent skirt and brim checks on the first layer, after the decoded
//! program has been matched path for path to the plan. Skirt and brim are
//! intentional non-model deposition: no skirt or brim bead may cover a part
//! or support bead's centreline, the skirt stays the declared distance clear
//! of every other first-layer bead, and the brim touches the part's first
//! layer within its declared separation. The planner's construction (offset
//! outlines, convex outline) is not consulted.
use super::failed;
use crate::check_cancel;
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

/// Allowance for XYZ6 projection and arithmetic, far below any bead.
const TOLERANCE_MM: f64 = 0.000_1;

#[derive(Clone, Copy)]
struct Segment {
    a: [f64; 2],
    b: [f64; 2],
    half: f64,
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let Some(first) = plan.layers.first() else {
        return Ok(());
    };
    let (mut skirt, mut brim, mut model, mut other) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for path in &first.paths {
        check_cancel(cancel)?;
        let target = match path.kind {
            DepositionKind::Skirt => &mut skirt,
            DepositionKind::Brim => &mut brim,
            kind if kind.is_model() => &mut model,
            _ => &mut other,
        };
        for edge in path.checked_edges()? {
            let (a, b) = (edge.points_mm[0], edge.points_mm[1]);
            target.push(Segment {
                a: [a[0], a[1]],
                b: [b[0], b[1]],
                half: 0.5 * edge.width_mm,
            });
        }
    }
    if skirt.is_empty() && brim.is_empty() {
        return Ok(());
    }
    let widest = first.paths.iter().try_fold(0.0f64, |w, path| {
        path.checked_edges()
            .map(|edges| edges.fold(w, |w, edge| w.max(edge.width_mm)))
    })?;
    if let Some(policy) = &recipe.brim {
        let reach = widest + policy.separation_mm + TOLERANCE_MM;
        let grid = Grid::new(&model, reach)?;
        let mut attached = false;
        for segment in &brim {
            check_cancel(cancel)?;
            grid.near(&model, segment, reach, |candidate, distance| {
                if distance < segment.half.max(candidate.half) - TOLERANCE_MM {
                    return Err(failed("brim bead covers part deposition"));
                }
                attached |=
                    distance <= segment.half + candidate.half + policy.separation_mm + TOLERANCE_MM;
                Ok(())
            })?;
        }
        if !brim.is_empty() && !attached {
            return Err(failed("brim does not attach to the part's first layer"));
        }
        // A brim bead never runs over support either.
        let others = Grid::new(&other, reach)?;
        for segment in &brim {
            check_cancel(cancel)?;
            others.near(&other, segment, reach, |candidate, distance| {
                if distance < segment.half.max(candidate.half) - TOLERANCE_MM {
                    return Err(failed("brim bead covers support deposition"));
                }
                Ok(())
            })?;
        }
    } else if !brim.is_empty() {
        return Err(failed("brim deposition without a brim policy"));
    }
    if let Some(policy) = &recipe.skirt {
        let mut rest = model;
        rest.extend(other);
        rest.extend(brim);
        let reach = policy.distance_mm + widest + TOLERANCE_MM;
        let grid = Grid::new(&rest, reach)?;
        for segment in &skirt {
            check_cancel(cancel)?;
            grid.near(&rest, segment, reach, |candidate, distance| {
                if distance < policy.distance_mm + segment.half + candidate.half - TOLERANCE_MM {
                    return Err(failed(
                        "skirt is closer to first-layer deposition than its declared distance",
                    ));
                }
                Ok(())
            })?;
        }
    } else if !skirt.is_empty() {
        return Err(failed("skirt deposition without a skirt policy"));
    }
    Ok(())
}

/// Segments bucketed by bounding box on a uniform grid.
struct Grid {
    min: [f64; 2],
    cell: f64,
    columns: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(segments: &[Segment], reach: f64) -> Result<Self, ManufacturingError> {
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for s in segments {
            for axis in 0..2 {
                min[axis] = min[axis].min(s.a[axis]).min(s.b[axis]);
                max[axis] = max[axis].max(s.a[axis]).max(s.b[axis]);
            }
        }
        if segments.is_empty() {
            return Ok(Self {
                min: [0.0; 2],
                cell: 1.0,
                columns: 0,
                rows: 0,
                cells: Vec::new(),
            });
        }
        let extent = (max[0] - min[0]).max(max[1] - min[1]);
        let cell = (2.0 * reach).max(extent / 512.0).max(1e-6);
        let columns = ((max[0] - min[0]) / cell) as usize + 1;
        let rows = ((max[1] - min[1]) / cell) as usize + 1;
        let bytes = columns * rows * std::mem::size_of::<Vec<u32>>()
            + segments.len() * 4 * std::mem::size_of::<u32>();
        if bytes > crate::working_set::bytes() {
            return Err(failed("skirt/brim check index exceeds the working set"));
        }
        let mut grid = Self {
            min,
            cell,
            columns,
            rows,
            cells: vec![Vec::new(); columns * rows],
        };
        for (index, s) in segments.iter().enumerate() {
            let ([c0, r0], [c1, r1]) = grid.range(s, 0.0);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    grid.cells[r * columns + c].push(index as u32);
                }
            }
        }
        Ok(grid)
    }

    fn range(&self, s: &Segment, pad: f64) -> ([usize; 2], [usize; 2]) {
        let index = |v: f64, axis: usize, count: usize| {
            (((v - self.min[axis]) / self.cell).floor().max(0.0) as usize).min(count - 1)
        };
        (
            [
                index(s.a[0].min(s.b[0]) - pad, 0, self.columns),
                index(s.a[1].min(s.b[1]) - pad, 1, self.rows),
            ],
            [
                index(s.a[0].max(s.b[0]) + pad, 0, self.columns),
                index(s.a[1].max(s.b[1]) + pad, 1, self.rows),
            ],
        )
    }

    /// Visit every indexed segment whose centreline lies within `reach` of
    /// `query`'s centreline (possibly more than once), with that distance.
    fn near(
        &self,
        segments: &[Segment],
        query: &Segment,
        reach: f64,
        mut visit: impl FnMut(&Segment, f64) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        if self.cells.is_empty() {
            return Ok(());
        }
        let ([c0, r0], [c1, r1]) = self.range(query, reach);
        for r in r0..=r1 {
            for c in c0..=c1 {
                for &index in &self.cells[r * self.columns + c] {
                    let candidate = &segments[index as usize];
                    let distance = segment_distance(query.a, query.b, candidate.a, candidate.b);
                    if distance <= reach {
                        visit(candidate, distance)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn point_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    let t = if length > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

fn segment_distance(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    if o1 * o2 < 0.0 && o3 * o4 < 0.0 {
        return 0.0;
    }
    point_segment(a, c, d)
        .min(point_segment(b, c, d))
        .min(point_segment(c, a, b))
        .min(point_segment(d, a, b))
}
