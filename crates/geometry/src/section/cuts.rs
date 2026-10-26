// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{Admission, indeterminate, roots};
use crate::{
    GeometryError, GeometryWorkBudget, PlaneMm,
    parametric::{self as p, Interval},
};
use solidcore::surface::surface::Surface;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CutRootId {
    pub(super) cut: usize,
    pub(super) root: usize,
}

pub(super) struct CutRoot {
    pub(super) parameter: Interval,
    pub(super) derivative: Interval,
    pub(super) uv: [Interval; 2],
    pub(super) point: [f64; 3],
    pub(super) endpoint_error_mm: f64,
}

pub(super) struct Cut {
    pub(super) axis: usize,
    pub(super) coordinate: f64,
    pub(super) roots: Vec<CutRoot>,
}

/// A cut belongs to the entire original face chart, not an incident atlas cell.
/// Publication is transactional: all roots, physical bounds and perpendicular
/// incidences qualify before the cut receives a stable catalog index.
pub(super) struct CutInventory<'a> {
    surface: &'a Surface,
    domain: [[f64; 2]; 2],
    scale: f64,
    plane: PlaneMm,
    accuracy: f64,
    cuts: Vec<Cut>,
}

impl<'a> CutInventory<'a> {
    pub(super) fn new(
        surface: &'a Surface,
        domain: [[f64; 2]; 2],
        scale: f64,
        plane: PlaneMm,
        accuracy: f64,
    ) -> Self {
        Self {
            surface,
            domain,
            scale,
            plane,
            accuracy,
            cuts: Vec::new(),
        }
    }

    pub(super) fn ensure_cut(
        &mut self,
        axis: usize,
        coordinate: f64,
        admission: &mut Admission,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<usize, GeometryError> {
        work.charge(1, cancel)?;
        if axis >= 2
            || !coordinate.is_finite()
            || !self.scale.is_finite()
            || self.scale <= 0.0
            || !self.accuracy.is_finite()
            || self.accuracy <= 0.0
            || self
                .domain
                .iter()
                .any(|r| !r[0].is_finite() || !r[1].is_finite() || r[0] >= r[1])
        {
            return Err(indeterminate(
                "native atlas cut has invalid chart or physical accuracy",
            ));
        }
        let coordinate = if coordinate == 0.0 { 0.0 } else { coordinate };
        if coordinate < self.domain[axis][0] || coordinate > self.domain[axis][1] {
            return Err(indeterminate(
                "native atlas cut exceeds its original face domain",
            ));
        }
        for (index, cut) in self.cuts.iter().enumerate() {
            work.charge(1, cancel)?;
            if cut.axis == axis && cut.coordinate == coordinate {
                return Ok(index);
            }
        }
        let other = 1 - axis;
        let mut line = self.domain;
        line[axis] = [coordinate; 2];

        // This whole-line first enclosure also proves original C0 continuity
        // across every knot, including a fixed coordinate lying on a knot.
        // Piecewise derivative bounds suffice for the physical Lipschitz bound;
        // no global C1 assertion or native parameter-unit guess is needed.
        let whole = p::surface_piecewise_first_enclosure(self.surface, line, work, cancel)?;
        let velocity = if other == 0 { whole.du } else { whole.dv }
            .map(|v| v.mul(Interval::point(self.scale)));
        let speed = norm(velocity)?;
        if !speed.is_finite() {
            return Err(indeterminate(
                "native atlas cut has no finite whole-line speed bound",
            ));
        }
        let width = Interval::new(line[other][0], line[other][1])?.width();
        let mut parameter_accuracy = if speed > 0.0 {
            Interval::point(self.accuracy)
                .div(Interval::point(speed))?
                .mul(Interval::point(0.25))
                .lo
                .min(width)
        } else {
            width
        };
        let mut previous_width = f64::INFINITY;
        loop {
            if !parameter_accuracy.is_finite() || parameter_accuracy <= 0.0 {
                return Err(indeterminate(
                    "native atlas endpoint accuracy exhausted chart precision",
                ));
            }
            // At a fixed native knot, the tensor partition can emit the same
            // other-axis spans twice (one for each incident fixed-axis span).
            // Its traversal is ordered within each such copy. Keep each span
            // once, without allocating a second knot inventory.
            let mut end = None;
            let spans = p::surface_parameter_boxes(self.surface, line, work, cancel)?
                .map(|piece| piece[other])
                .filter(move |span| {
                    if span[0] >= span[1] || end.is_some_and(|last| span[0] < last) {
                        return false;
                    }
                    end = Some(span[1]);
                    true
                });
            let isolated = roots::isolate(
                spans,
                &mut |range, work, cancel| {
                    let mut uv = line;
                    uv[other] = range;
                    let jet = p::surface_piecewise_first_enclosure(self.surface, uv, work, cancel)?;
                    let position = jet.position.map(|v| v.mul(Interval::point(self.scale)));
                    let derivative = if other == 0 { jet.du } else { jet.dv }
                        .map(|v| v.mul(Interval::point(self.scale)));
                    Ok(roots::ScalarJet {
                        value: dot(
                            std::array::from_fn(|i| {
                                position[i].sub(Interval::point(self.plane.origin_mm[i]))
                            }),
                            self.plane.normal,
                        ),
                        derivative: dot(derivative, self.plane.normal),
                    })
                },
                parameter_accuracy,
                admission,
                work,
                cancel,
            )?;
            let mut qualified = Vec::new();
            let mut maximum_error: f64 = 0.0;
            let mut maximum_width: f64 = 0.0;
            for root in &isolated {
                work.charge(1, cancel)?;
                for cut in &self.cuts {
                    work.charge(1, cancel)?;
                    if cut.axis == other && root.parameter.contains(cut.coordinate) {
                        return Err(indeterminate(
                            "native atlas cut root meets an unresolved grid vertex",
                        ));
                    }
                }
                let mut uv = [Interval::point(coordinate); 2];
                uv[other] = root.parameter;
                let position = p::surface_piecewise_first_enclosure(
                    self.surface,
                    uv.map(|v| [v.lo, v.hi]),
                    work,
                    cancel,
                )?
                .position
                .map(|v| v.mul(Interval::point(self.scale)));
                let point = position.map(|v| v.mid());
                let endpoint_error_mm = norm(std::array::from_fn(|i| {
                    position[i].sub(Interval::point(point[i]))
                }))?;
                if !endpoint_error_mm.is_finite() || point.iter().any(|v| !v.is_finite()) {
                    return Err(indeterminate(
                        "native atlas root lacks a finite physical enclosure",
                    ));
                }
                maximum_error = maximum_error.max(endpoint_error_mm);
                maximum_width = maximum_width.max(root.parameter.width());
                admission.push(
                    &mut qualified,
                    CutRoot {
                        parameter: root.parameter,
                        derivative: root.derivative,
                        uv,
                        point,
                        endpoint_error_mm,
                    },
                )?;
            }
            // Apply the uncertain-vertex rule in both directions: an older
            // perpendicular root must not acquire a new grid coordinate through
            // its already published uncertainty interval either.
            for cut in &self.cuts {
                work.charge(1, cancel)?;
                if cut.axis == other {
                    for root in &cut.roots {
                        work.charge(1, cancel)?;
                        if root.parameter.contains(coordinate) {
                            return Err(indeterminate(
                                "native atlas cut crosses an existing uncertain grid vertex",
                            ));
                        }
                    }
                }
            }
            if maximum_error <= self.accuracy {
                let index = self.cuts.len();
                admission.push(
                    &mut self.cuts,
                    Cut {
                        axis,
                        coordinate,
                        roots: qualified,
                    },
                )?;
                return Ok(index);
            }
            if maximum_width >= previous_width {
                return Err(indeterminate(
                    "native atlas root physical enclosure cannot refine to accuracy",
                ));
            }
            previous_width = maximum_width;
            // Re-isolate with a stricter physically derived request, rather
            // than shrinking a UV bracket without a root existence proof.
            // All failed-attempt work and capacity admission remain charged;
            // isolated and qualified coexist and both were admitted above.
            parameter_accuracy = Interval::point(parameter_accuracy)
                .mul(Interval::point(self.accuracy).div(Interval::point(maximum_error))?)
                .mul(Interval::point(0.25))
                .lo;
        }
    }

    pub(super) fn cut(&self, index: usize) -> &Cut {
        &self.cuts[index]
    }

    pub(super) fn root(&self, id: CutRootId) -> &CutRoot {
        &self.cuts[id.cut].roots[id.root]
    }

    pub(super) fn cuts(&self) -> &[Cut] {
        &self.cuts
    }
}

fn dot(values: [Interval; 3], direction: [f64; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, i| {
        sum.add(values[i].mul(Interval::point(direction[i])))
    })
}

fn norm(values: [Interval; 3]) -> Result<f64, GeometryError> {
    Ok(values
        .into_iter()
        .fold(Interval::point(0.0), |sum, v| sum.add(v.square()))
        .sqrt()?
        .hi)
}
