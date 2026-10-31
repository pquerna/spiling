// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete coefficient-qualified cover of a native face-plane zero set.
//! Partition cells never overlap in their interiors. Their sampling charts may
//! overlap, but that overlap supplies neither trim membership nor a new root
//! identity: every artificial endpoint belongs to the shared cut inventory.
use super::{Admission, cuts::CutInventory, indeterminate, roots::sign};
use crate::{
    GeometryError, GeometryWorkBudget, PlaneMm,
    parametric::{self as p, Interval, SurfaceFirstEnclosure},
};
use solidcore::surface::surface::Surface;
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

const MAX_DEPTH: u8 = 16;
const CUT_FRACTIONS: [f64; 5] = [0.5, 0.375, 0.625, 0.25, 0.75];
const MARGIN_FRACTIONS: [f64; 7] = [0.5, 0.25, 0.125, 0.0625, 0.03125, 0.015625, 0.0];

#[derive(Clone, Copy, Debug)]
pub(super) struct GraphCell {
    pub(super) cell: [[f64; 2]; 2],
    pub(super) cuts: [[usize; 2]; 2],
    /// Contains every possible cell root, with checked dependent margin. It
    /// need not contain excluded strips of the partition cell.
    pub(super) graph_domain: [[f64; 2]; 2],
    pub(super) axis: usize,
    pub(super) dependent_sign: i8,
}

#[derive(Clone, Copy)]
struct Candidate {
    axis: usize,
    dependent_sign: i8,
    necessary: Interval,
    retained: Interval,
}

fn dot(row: [Interval; 3], direction: [f64; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(row[axis].mul(Interval::point(direction[axis])))
    })
}

fn height(position: [Interval; 3], plane: PlaneMm) -> Interval {
    dot(
        std::array::from_fn(|axis| position[axis].sub(Interval::point(plane.origin_mm[axis]))),
        plane.normal,
    )
}

fn first_jet(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceFirstEnclosure, GeometryError> {
    // The native first-enclosure helper proves coefficient C0 continuity over
    // this entire box, not just the original face rectangle. In particular its
    // check also covers any sampling overlap beyond that rectangle.
    let mut jet = p::surface_piecewise_first_enclosure(surface, domain, work, cancel)?;
    for row in [&mut jet.position, &mut jet.du, &mut jet.dv] {
        for value in row {
            *value = value.mul(Interval::point(scale));
        }
    }
    Ok(jet)
}

fn partial(jet: &SurfaceFirstEnclosure, axis: usize, plane: PlaneMm) -> Interval {
    dot(if axis == 0 { jet.du } else { jet.dv }, plane.normal)
}

/// Exhaust original native knot rectangles, not a fitted or sampled chart.
/// Recursion retains only fixed depth-16 scratch within section's already
/// admitted 256 KiB stack allowance; the sole owned buffer grows via Admission.
#[allow(clippy::too_many_arguments)]
pub(super) fn cover(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    scale: f64,
    plane: PlaneMm,
    protected_cut: &impl Fn(
        usize,
        f64,
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<bool, GeometryError>,
    cuts: &mut CutInventory<'_>,
    admission: &mut Admission,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<GraphCell>, GeometryError> {
    work.charge(1, cancel)?;
    if !scale.is_finite() || scale <= 0.0 {
        return Err(indeterminate(
            "native graph atlas has an invalid spatial scale",
        ));
    }
    let active = surface.natural_uv_domain();
    let lo = active.min();
    let hi = active.max();
    let native_domain = [[lo.x, hi.x], [lo.y, hi.y]];
    for axis in 0..2 {
        let range = Interval::new(domain[axis][0], domain[axis][1])?;
        let native = native_domain[axis];
        // An unbounded native carrier still admits finite face rectangles and
        // finite checked overlap; its infinities are never evaluation operands.
        if native[0].is_nan()
            || native[1].is_nan()
            || native[0] >= native[1]
            || range.lo >= range.hi
            || range.lo < native[0]
            || range.hi > native[1]
        {
            return Err(indeterminate(
                "native graph atlas has no valid active face rectangle",
            ));
        }
    }
    if !p::surface_parameter_span_continuous(surface, domain, work, cancel)? {
        return Err(indeterminate(
            "native graph atlas lacks complete original C0 authority",
        ));
    }
    let mut output = Vec::new();
    for cell in p::surface_parameter_boxes(surface, domain, work, cancel)? {
        visit(
            surface,
            cell,
            native_domain,
            scale,
            plane,
            0,
            protected_cut,
            cuts,
            admission,
            &mut output,
            work,
            cancel,
        )?;
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn visit(
    surface: &Surface,
    cell: [[f64; 2]; 2],
    native_domain: [[f64; 2]; 2],
    scale: f64,
    plane: PlaneMm,
    depth: u8,
    protected_cut: &impl Fn(
        usize,
        f64,
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<bool, GeometryError>,
    cuts: &mut CutInventory<'_>,
    admission: &mut Admission,
    output: &mut Vec<GraphCell>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let jet = match first_jet(surface, cell, scale, work, cancel) {
        Ok(jet) => Some(jet),
        Err(error) if error.code == GeometryErrorCode::AnalysisIndeterminate => None,
        Err(error) => return Err(error),
    };
    let mut candidates = [None; 2];
    if let Some(jet) = jet {
        if sign(height(jet.position, plane)) != 0 {
            return Ok(());
        }
        for (index, axis) in [1, 0].into_iter().enumerate() {
            let derivative = partial(&jet, axis, plane);
            let dependent_sign = sign(derivative);
            if dependent_sign == 0 {
                continue;
            }
            // Mean-value contraction encloses ANY root for every independent
            // coordinate in the entire cell. It is necessity, not existence.
            let midpoint = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
            let mut middle = cell;
            middle[axis] = [midpoint; 2];
            let position = p::surface_position_enclosure(surface, middle, work, cancel)?
                .map(|value| value.mul(Interval::point(scale)));
            let necessary = Interval::point(midpoint).sub(height(position, plane).div(derivative)?);
            let low = necessary.lo.max(cell[axis][0]);
            let high = necessary.hi.min(cell[axis][1]);
            if low > high {
                return Ok(());
            }
            let necessary = Interval::new(low, high)?;
            // Retain half of proved root-free strips, rounded outwards. This
            // leaves strict sampling room rather than using a root bound as a
            // dependent endpoint. Sides touched by possible roots need overlap.
            let retained = Interval::new(
                Interval::point(cell[axis][0])
                    .add(Interval::point(low))
                    .mul(Interval::point(0.5))
                    .lo
                    .max(cell[axis][0]),
                Interval::point(high)
                    .add(Interval::point(cell[axis][1]))
                    .mul(Interval::point(0.5))
                    .hi
                    .min(cell[axis][1]),
            )?;
            candidates[index] = Some(Candidate {
                axis,
                dependent_sign,
                necessary,
                retained,
            });
        }
    }
    if candidates.iter().any(Option::is_some) {
        // Fixed outer/knot sides cannot be moved by further subdivision. A
        // complete side inventory failure is an honest refusal, never emptiness.
        let sides = register_sides(cell, cuts, admission, work, cancel)?;
        for candidate in candidates.into_iter().flatten() {
            for fraction in MARGIN_FRACTIONS {
                work.charge(1, cancel)?;
                let axis = candidate.axis;
                let width = cell[axis][1] - cell[axis][0];
                if !width.is_finite() {
                    return Err(indeterminate("native graph atlas parameter width overflow"));
                }
                let margin = width * fraction;
                let mut graph_domain = cell;
                graph_domain[axis] = [
                    (candidate.retained.lo - margin).max(native_domain[axis][0]),
                    (candidate.retained.hi + margin).min(native_domain[axis][1]),
                ];
                // An artificial side may not be used as the endpoint of a
                // graph whose necessary root range still touches that side.
                if candidate.necessary.lo <= cell[axis][0]
                    && cell[axis][0] > native_domain[axis][0]
                    && graph_domain[axis][0] >= cell[axis][0]
                    || candidate.necessary.hi >= cell[axis][1]
                        && cell[axis][1] < native_domain[axis][1]
                        && graph_domain[axis][1] <= cell[axis][1]
                    || graph_domain[axis][0] >= graph_domain[axis][1]
                    || !graph_domain[axis][0].is_finite()
                    || !graph_domain[axis][1].is_finite()
                {
                    continue;
                }
                let neighborhood = match first_jet(surface, graph_domain, scale, work, cancel) {
                    Ok(jet) => jet,
                    Err(error) if error.code == GeometryErrorCode::AnalysisIndeterminate => {
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if sign(partial(&neighborhood, axis, plane)) != candidate.dependent_sign {
                    continue;
                }
                if !brackets_cut_roots(
                    surface,
                    cell,
                    graph_domain,
                    axis,
                    candidate.dependent_sign,
                    sides,
                    scale,
                    plane,
                    cuts,
                    work,
                    cancel,
                )? {
                    continue;
                }
                return admission.push(
                    output,
                    GraphCell {
                        cell,
                        cuts: sides,
                        graph_domain,
                        axis,
                        dependent_sign: candidate.dependent_sign,
                    },
                );
            }
        }
    }
    if depth >= MAX_DEPTH {
        return Err(indeterminate(format!(
            "native graph atlas exhausted regularity/margin proof at depth {depth}: cell={cell:?}"
        )));
    }
    // Alternating the first-choice coordinate shrinks independent uncertainty
    // as well as dependent uncertainty. Neither size nor depth grants authority.
    for axis in [usize::from(depth % 2), 1 - usize::from(depth % 2)] {
        for fraction in CUT_FRACTIONS {
            work.charge(1, cancel)?;
            let coordinate = cell[axis][0] * (1.0 - fraction) + cell[axis][1] * fraction;
            if !coordinate.is_finite() || coordinate <= cell[axis][0] || coordinate >= cell[axis][1]
            {
                continue;
            }
            if protected_cut(axis, coordinate, work, cancel)? {
                continue;
            }
            match cuts.ensure_cut(axis, coordinate, admission, work, cancel) {
                Ok(_) => {}
                Err(error) if error.code == GeometryErrorCode::AnalysisIndeterminate => continue,
                Err(error) => return Err(error),
            }
            let mut lower = cell;
            let mut upper = cell;
            lower[axis][1] = coordinate;
            upper[axis][0] = coordinate;
            visit(
                surface,
                lower,
                native_domain,
                scale,
                plane,
                depth + 1,
                protected_cut,
                cuts,
                admission,
                output,
                work,
                cancel,
            )?;
            return visit(
                surface,
                upper,
                native_domain,
                scale,
                plane,
                depth + 1,
                protected_cut,
                cuts,
                admission,
                output,
                work,
                cancel,
            );
        }
    }
    Err(indeterminate(format!(
        "native graph atlas has no qualified finite nonvertex cut proposal: cell={cell:?}"
    )))
}

fn register_sides(
    cell: [[f64; 2]; 2],
    cuts: &mut CutInventory<'_>,
    admission: &mut Admission,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[usize; 2]; 2], GeometryError> {
    let mut sides = [[0; 2]; 2];
    for axis in 0..2 {
        for side in 0..2 {
            let index = cuts.ensure_cut(axis, cell[axis][side], admission, work, cancel)?;
            let range = cell[1 - axis];
            for root in &cuts.cut(index).roots {
                work.charge(1, cancel)?;
                if range
                    .into_iter()
                    .any(|corner| root.parameter.lo <= corner && corner <= root.parameter.hi)
                {
                    return Err(indeterminate(
                        "native graph atlas cut-root interval straddles a partition corner",
                    ));
                }
            }
            sides[axis][side] = index;
        }
    }
    Ok(sides)
}

/// Positive UV overlap alone is insufficient: uncertain root parameters on a
/// dependent-side cut can project just beyond the cell. Check the actual whole
/// independent-coordinate root enclosure against both graph endpoints. Native
/// domain endpoints cannot acquire a guessed extension or periodic identity.
#[allow(clippy::too_many_arguments)]
fn brackets_cut_roots(
    surface: &Surface,
    cell: [[f64; 2]; 2],
    graph_domain: [[f64; 2]; 2],
    axis: usize,
    dependent_sign: i8,
    sides: [[usize; 2]; 2],
    scale: f64,
    plane: PlaneMm,
    cuts: &CutInventory<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for cut_axis in 0..2 {
        for &cut_id in &sides[cut_axis] {
            let cut = cuts.cut(cut_id);
            let other_range = cell[1 - cut_axis];
            for root in &cut.roots {
                work.charge(1, cancel)?;
                if root.parameter.hi < other_range[0] || root.parameter.lo > other_range[1] {
                    continue;
                }
                let independent = if cut_axis == axis {
                    [root.parameter.lo, root.parameter.hi]
                } else {
                    [cut.coordinate; 2]
                };
                for endpoint in 0..2 {
                    let mut uv = graph_domain;
                    uv[1 - axis] = independent;
                    uv[axis] = [graph_domain[axis][endpoint]; 2];
                    let position = match p::surface_position_enclosure(surface, uv, work, cancel) {
                        Ok(position) => position.map(|value| value.mul(Interval::point(scale))),
                        Err(error) if error.code == GeometryErrorCode::AnalysisIndeterminate => {
                            return Ok(false);
                        }
                        Err(error) => return Err(error),
                    };
                    let expected = if endpoint == 0 {
                        -dependent_sign
                    } else {
                        dependent_sign
                    };
                    if sign(height(position, plane)) != expected {
                        return Ok(false);
                    }
                }
            }
        }
    }
    Ok(true)
}
