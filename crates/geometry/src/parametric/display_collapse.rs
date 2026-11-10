// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Display-only natural-v quotient rows of the original stored coefficient law.
//! Exact homogeneous proportionality and a clamped endpoint prove S_u = 0.
//! The first inward normal coefficient is sign * (S_uv cross S_v); closed
//! one-sided jets and bounded subdivision separate it from zero on the WHOLE
//! requested u interval. Higher-order vanishing remains a refusal. The owning
//! singular inventory separately checks the complete unit enclosure against
//! its unchanged normal tolerance and binds original native endpoints.
//! On a complete adjacent strip, C=S_u cross S_v has C(v0)=0, and
//! C/(v-v0) is the integral average of C_v=S_uv cross S_v + S_u cross S_vv.
//! Separating that complete coefficient enclosure from zero qualifies the
//! physical quotient at the collapsed row without asserting a nonzero UV rank.

use super::{Interval, analytic_eval, nurbs_eval};
use crate::{GeometryError, GeometryWorkBudget, exact_coefficient::ExactCoefficient};
use solidcore::{
    nurbs::math::{NOW, NOZ},
    surface::surface::Surface,
};
use spiling_contracts::geometry::{GeometryErrorCode, NORMAL_NORM_TOLERANCE};
use std::{mem::size_of, sync::atomic::AtomicBool};

const MAX_DEPTH: u8 = 24;
const MAX_CELLS: usize = 16_384;
type Pending = [([f64; 2], u8); MAX_DEPTH as usize + 1];
pub(crate) const SCRATCH_BYTES: usize = super::SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
    + 12 * size_of::<ExactCoefficient>()
    + size_of::<Pending>()
    + 12 * size_of::<[Interval; 3]>();

fn proportional(
    coefficient: f64,
    weight: f64,
    reference: f64,
    reference_weight: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let Some(mut left) = ExactCoefficient::point(coefficient).product(
        &ExactCoefficient::point(reference_weight),
        work,
        cancel,
    )?
    else {
        return Ok(false);
    };
    let Some(right) = ExactCoefficient::point(reference).product(
        &ExactCoefficient::point(weight),
        work,
        cancel,
    )?
    else {
        return Ok(false);
    };
    Ok(left.subtract(&right).is_some() && left.sign() == 0)
}

pub(crate) fn collapsed_row_position(
    surface: &Surface,
    v: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    if let Some(position) = analytic_eval::collapsed_step_sphere_position(surface, v, work, cancel)?
    {
        return Ok(Some(position));
    }
    let Some(native) = analytic_eval::native_surface_nurbs(surface)? else {
        return Ok(None);
    };
    let counts = [native.n() + 1, native.m() + 1];
    let degrees = [native.p(), native.q()];
    let knots = [native.u_knots(), native.v_knots()];
    let natural = surface.natural_uv_domain_if_built();
    if v != natural.v_min() && v != natural.v_max() {
        return Ok(None);
    }
    let row = if v == knots[1][degrees[1]] && knots[1][..=degrees[1]].iter().all(|&knot| knot == v)
    {
        0
    } else if v == knots[1][counts[1]] && knots[1][counts[1]..].iter().all(|&knot| knot == v) {
        counts[1] - 1
    } else {
        return Ok(None);
    };
    nurbs_eval::validate_knots(
        knots[0],
        degrees[0],
        counts[0],
        [knots[0][degrees[0]], knots[0][counts[0]]],
        work,
        cancel,
    )?;
    nurbs_eval::validate_knots(knots[1], degrees[1], counts[1], [v, v], work, cancel)?;
    let at = |column| -> Result<[f64; 4], GeometryError> {
        let point = native
            .cpt(column, row)
            .map_err(|_| nurbs_eval::conditioning("missing collapsed display row coefficient"))?;
        if (point.w != NOW) != native.is_rational() || point.z == NOZ {
            return Err(nurbs_eval::conditioning(
                "inconsistent collapsed display row coefficients",
            ));
        }
        let coefficient = [
            point.x,
            point.y,
            point.z,
            if native.is_rational() { point.w } else { 1.0 },
        ];
        for value in coefficient {
            Interval::new(value, value)?;
        }
        Ok(coefficient)
    };
    let reference = at(0)?;
    if reference[3] == 0.0 {
        return Ok(None);
    }
    for column in 0..counts[0] {
        work.charge(1, cancel)?;
        let coefficient = at(column)?;
        if coefficient[3] == 0.0
            || coefficient[3].is_sign_positive() != reference[3].is_sign_positive()
        {
            return Ok(None);
        }
        for axis in 0..3 {
            if !proportional(
                coefficient[axis],
                coefficient[3],
                reference[axis],
                reference[3],
                work,
                cancel,
            )? {
                return Ok(None);
            }
        }
    }
    Ok(Some([
        Interval::point(reference[0]).div(Interval::point(reference[3]))?,
        Interval::point(reference[1]).div(Interval::point(reference[3]))?,
        Interval::point(reference[2]).div(Interval::point(reference[3]))?,
    ]))
}

fn unit_enclosure(vector: [Interval; 3]) -> Result<Option<[Interval; 3]>, GeometryError> {
    let pivot = (0..3).max_by(|&left, &right| {
        vector[left]
            .abs_lower()
            .total_cmp(&vector[right].abs_lower())
    });
    let Some(pivot) = pivot.filter(|&axis| !vector[axis].contains_zero()) else {
        return Ok(None);
    };
    let mut ratios = [Interval::point(1.0); 3];
    for axis in 0..3 {
        if axis != pivot {
            ratios[axis] = vector[axis].div(vector[pivot])?;
        }
    }
    let length = ratios
        .into_iter()
        .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
        .sqrt()?;
    let sign = Interval::point(if vector[pivot].lo > 0.0 { 1.0 } else { -1.0 });
    Ok(Some([
        ratios[0].div(length)?.mul(sign),
        ratios[1].div(length)?.mul(sign),
        ratios[2].div(length)?.mul(sign),
    ]))
}

pub(crate) fn strip_regular(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let natural = surface.natural_uv_domain_if_built();
    let endpoint = if domain[1][0] == natural.v_min() {
        natural.v_min()
    } else if domain[1][1] == natural.v_max() {
        natural.v_max()
    } else {
        return Ok(false);
    };
    if collapsed_row_position(surface, endpoint, work, cancel)?.is_none()
        || !super::surface_parameter_span_smooth(surface, domain, work, cancel)?
    {
        return Ok(false);
    }
    let jet = super::surface_piecewise_enclosure(surface, domain, work, cancel)?;
    let mixed = super::cross(jet.duv, jet.dv);
    let second = super::cross(jet.du, jet.dvv);
    Ok((0..3).any(|axis| !mixed[axis].add(second[axis]).contains_zero()))
}

pub(crate) fn collapsed_row_normal(
    surface: &Surface,
    u: [f64; 2],
    v: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    let Some(position) = collapsed_row_position(surface, v, work, cancel)? else {
        return Err(nurbs_eval::conditioning(
            "display normal lacks an exact collapsed natural row",
        ));
    };
    let natural = surface.natural_uv_domain_if_built();
    let inward = if v == natural.v_min() {
        super::up(v)
    } else {
        super::down(v)
    };
    let strip = [u, [v.min(inward), v.max(inward)]];
    if analytic_eval::collapsed_step_sphere_position(surface, v, work, cancel)?.is_some() {
        return super::surface_geometric_normal_enclosure(surface, strip, work, cancel);
    }
    if !super::surface_parameter_span_smooth(surface, strip, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "collapsed display row lacks whole-span one-sided C1 authority",
        ));
    }
    let sign = Interval::point(if v == natural.v_min() { 1.0 } else { -1.0 });
    let native = analytic_eval::native_surface_nurbs(surface)?.ok_or_else(|| {
        nurbs_eval::conditioning("collapsed display row lacks native coefficients")
    })?;
    let origin = position.map(|coordinate| coordinate.lo * 0.5 + coordinate.hi * 0.5);
    let mut pending: Pending = [([0.0; 2], 0); MAX_DEPTH as usize + 1];
    pending[0] = (u, 0);
    let mut live = 1;
    let mut cells = 0;
    let mut result: Option<[Interval; 3]> = None;
    while live != 0 {
        work.charge(1, cancel)?;
        cells += 1;
        if cells > MAX_CELLS {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "collapsed display normal exceeded bounded subdivision",
            ));
        }
        live -= 1;
        let (span, depth) = pending[live];
        let jet = nurbs_eval::evaluate_nurbs_surface_tensor_about(
            native,
            [span, [v, v]],
            2,
            Some(origin),
            work,
            cancel,
        )?;
        let coefficient = super::cross(jet[1][1], jet[0][1]).map(|value| value.mul(sign));
        if let Some(normal) = unit_enclosure(coefficient)?
            && super::norm_upper(normal.map(|value| Interval::point(value.width())))?
                <= NORMAL_NORM_TOLERANCE * 0.25
        {
            result = Some(result.map_or(normal, |previous| {
                std::array::from_fn(|axis| previous[axis].hull(normal[axis]))
            }));
            continue;
        }
        if depth == MAX_DEPTH {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "collapsed display row has no separated first normal coefficient",
            ));
        }
        let middle = span[0] * 0.5 + span[1] * 0.5;
        if middle <= span[0] || middle >= span[1] {
            return Err(nurbs_eval::conditioning(
                "collapsed display normal exhausted representable subdivision",
            ));
        }
        pending[live] = ([middle, span[1]], depth + 1);
        pending[live + 1] = ([span[0], middle], depth + 1);
        live += 2;
    }
    result.ok_or_else(|| nurbs_eval::conditioning("collapsed display normal has no complete span"))
}

#[cfg(test)]
mod tests;
