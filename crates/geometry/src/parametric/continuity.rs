// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Exact rational C1 joins on original source knots, including closed endpoints.
use crate::{GeometryError, GeometryWorkBudget, exact_coefficient::ExactCoefficient};
use solidcore::nurbs::crv_basic;
use std::sync::atomic::AtomicBool;
pub(super) const SCRATCH_BYTES: usize = 20 * std::mem::size_of::<ExactCoefficient>();
pub(super) fn curve_is_c1(
    nurb: &crv_basic::Curve,
    dim: u64,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let knots = nurb.knots();
    let degree = nurb.degree();
    let count = nurb.cpts().len();
    if degree == 0 {
        return Ok(false);
    }
    let control = |index: usize| {
        let p = nurb.cpts()[index];
        [
            p.x,
            p.y,
            if dim == 2 { 0.0 } else { p.z },
            if nurb.is_rational() { p.w } else { 1.0 },
        ]
    };
    let mut start = knots.partition_point(|k| *k < range[0]);
    while start < knots.len() && knots[start] <= range[1] {
        work.charge(1, cancel)?;
        let value = knots[start];
        let mut end = start + 1;
        while end < knots.len() && knots[end] == value {
            work.charge(1, cancel)?;
            end += 1;
        }
        if knots[degree] < value && value < knots[count] && end - start >= degree {
            if end - start != degree || start == 0 || end >= knots.len() {
                return Ok(false);
            }
            let Some(shared) = end.checked_sub(degree + 1) else {
                return Ok(false);
            };
            if shared == 0
                || shared + 1 >= count
                || !exact_join(
                    control(shared - 1),
                    control(shared),
                    control(shared + 1),
                    [knots[start - 1], value, knots[end]],
                    work,
                    cancel,
                )?
            {
                return Ok(false);
            }
        }
        start = end;
    }
    Ok(true)
}
fn determinant(
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ExactCoefficient>, GeometryError> {
    let Some(mut left) =
        ExactCoefficient::point(a).product(&ExactCoefficient::point(b), work, cancel)?
    else {
        return Ok(None);
    };
    let Some(right) =
        ExactCoefficient::point(c).product(&ExactCoefficient::point(d), work, cancel)?
    else {
        return Ok(None);
    };
    if left.subtract(&right).is_none() {
        return Ok(None);
    }
    Ok(Some(left))
}
/// At multiplicity p the same center homogeneous coefficient supplies C0. The
/// degree and center weight squared cancel; cross-multiply exact knot lengths.
fn exact_join(
    left: [f64; 4],
    center: [f64; 4],
    right: [f64; 4],
    knots: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if left
        .into_iter()
        .chain(center)
        .chain(right)
        .chain(knots)
        .any(|v| !v.is_finite())
    {
        return Ok(false);
    }
    let Some(left_span) = ExactCoefficient::difference(knots[1], knots[0]) else {
        return Ok(false);
    };
    let Some(right_span) = ExactCoefficient::difference(knots[2], knots[1]) else {
        return Ok(false);
    };
    if left_span.sign() <= 0 || right_span.sign() <= 0 || center[3] == 0.0 {
        return Ok(false);
    }
    for axis in 0..3 {
        let Some(l) = determinant(center[axis], left[3], left[axis], center[3], work, cancel)?
        else {
            return Ok(false);
        };
        let Some(r) = determinant(right[axis], center[3], center[axis], right[3], work, cancel)?
        else {
            return Ok(false);
        };
        let Some(mut l) = l.product(&right_span, work, cancel)? else {
            return Ok(false);
        };
        let Some(r) = r.product(&left_span, work, cancel)? else {
            return Ok(false);
        };
        if l.subtract(&r).is_none() || l.sign() != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Sufficient whole-interface rational identity: the homogeneous derivative
/// jump is one common scalar multiple of the shared homogeneous trace. This
/// proves Cartesian C1 for every other-axis parameter, not merely each control
/// row in isolation. Failure to prove this sufficient identity grants nothing.
pub(super) fn surface_is_c1(
    nurb: &solidcore::nurbs::srf_basic::Surface,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let degrees = [nurb.p(), nurb.q()];
    let counts = [nurb.n() + 1, nurb.m() + 1];
    let axes = [nurb.u_knots(), nurb.v_knots()];
    for axis in 0..2 {
        let degree = degrees[axis];
        let count = counts[axis];
        let knots = axes[axis];
        if degree == 0 {
            return Ok(false);
        }
        let mut start = knots.partition_point(|v| *v < domain[axis][0]);
        while start < knots.len() && knots[start] <= domain[axis][1] {
            work.charge(1, cancel)?;
            let value = knots[start];
            let mut end = start + 1;
            while end < knots.len() && knots[end] == value {
                work.charge(1, cancel)?;
                end += 1;
            }
            if knots[degree] < value && value < knots[count] && end - start >= degree {
                if end - start != degree || start == 0 || end >= knots.len() {
                    return Ok(false);
                }
                let Some(shared) = end.checked_sub(degree + 1) else {
                    return Ok(false);
                };
                if shared == 0 || shared + 1 >= count {
                    return Ok(false);
                }
                let Some(left_span) = ExactCoefficient::difference(value, knots[start - 1]) else {
                    return Ok(false);
                };
                let Some(right_span) = ExactCoefficient::difference(knots[end], value) else {
                    return Ok(false);
                };
                if left_span.sign() <= 0 || right_span.sign() <= 0 {
                    return Ok(false);
                }
                let control = |at: usize, other: usize| -> Result<[f64; 4], GeometryError> {
                    let c = if axis == 0 {
                        nurb.cpt(at, other)
                    } else {
                        nurb.cpt(other, at)
                    }
                    .map_err(|_| {
                        super::nurbs_eval::conditioning("missing native continuity coefficient")
                    })?;
                    Ok([c.x, c.y, c.z, if nurb.is_rational() { c.w } else { 1.0 }])
                };
                let mut reference: Option<(f64, ExactCoefficient)> = None;
                for other in 0..counts[1 - axis] {
                    work.charge(1, cancel)?;
                    let center = control(shared, other)?;
                    if center[3] == 0.0 {
                        continue;
                    }
                    let Some(jump) = homogeneous_jump(
                        control(shared - 1, other)?,
                        center,
                        control(shared + 1, other)?,
                        &left_span,
                        &right_span,
                        work,
                        cancel,
                    )?
                    else {
                        return Ok(false);
                    };
                    reference = Some((center[3], jump[3].clone()));
                    break;
                }
                let Some((weight, scalar)) = reference else {
                    return Ok(false);
                };
                for other in 0..counts[1 - axis] {
                    work.charge(1, cancel)?;
                    let center = control(shared, other)?;
                    let Some(jump) = homogeneous_jump(
                        control(shared - 1, other)?,
                        center,
                        control(shared + 1, other)?,
                        &left_span,
                        &right_span,
                        work,
                        cancel,
                    )?
                    else {
                        return Ok(false);
                    };
                    for k in 0..4 {
                        let Some(mut lhs) =
                            jump[k].product(&ExactCoefficient::point(weight), work, cancel)?
                        else {
                            return Ok(false);
                        };
                        let Some(rhs) =
                            scalar.product(&ExactCoefficient::point(center[k]), work, cancel)?
                        else {
                            return Ok(false);
                        };
                        if lhs.subtract(&rhs).is_none() || lhs.sign() != 0 {
                            return Ok(false);
                        }
                    }
                }
            }
            start = end;
        }
    }
    Ok(true)
}
fn homogeneous_jump(
    left: [f64; 4],
    center: [f64; 4],
    right: [f64; 4],
    left_span: &ExactCoefficient,
    right_span: &ExactCoefficient,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 4]>, GeometryError> {
    if left
        .into_iter()
        .chain(center)
        .chain(right)
        .any(|v| !v.is_finite())
    {
        return Ok(None);
    }
    let mut jump = std::array::from_fn(|_| ExactCoefficient::point(0.0));
    for k in 0..4 {
        let Some(a) = ExactCoefficient::difference(center[k], left[k]) else {
            return Ok(None);
        };
        let Some(b) = ExactCoefficient::difference(right[k], center[k]) else {
            return Ok(None);
        };
        let Some(mut a) = a.product(right_span, work, cancel)? else {
            return Ok(None);
        };
        let Some(b) = b.product(left_span, work, cancel)? else {
            return Ok(None);
        };
        if a.subtract(&b).is_none() {
            return Ok(None);
        }
        jump[k] = a;
    }
    Ok(Some(jump))
}

pub(super) fn curve_is_c0(
    nurb: &crv_basic::Curve,
    dim: u64,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let degree = nurb.degree();
    let knots = nurb.knots();
    let count = nurb.cpts().len();
    let mut start = knots.partition_point(|k| *k < range[0]);
    while start < knots.len() && knots[start] <= range[1] {
        work.charge(1, cancel)?;
        let value = knots[start];
        let mut end = start + 1;
        while end < knots.len() && knots[end] == value {
            work.charge(1, cancel)?;
            end += 1;
        }
        if knots[degree] < value && value < knots[count] && end - start > degree {
            if end - start != degree + 1 {
                return Ok(false);
            }
            let Some(left) = end.checked_sub(degree + 2) else {
                return Ok(false);
            };
            if left + 1 >= count {
                return Ok(false);
            }
            let a = nurb.cpts()[left];
            let b = nurb.cpts()[left + 1];
            let aw = if nurb.is_rational() { a.w } else { 1.0 };
            let bw = if nurb.is_rational() { b.w } else { 1.0 };
            if aw == 0.0 || bw == 0.0 {
                return Ok(false);
            }
            let a = [a.x, a.y, if dim == 2 { 0.0 } else { a.z }];
            let b = [b.x, b.y, if dim == 2 { 0.0 } else { b.z }];
            for k in 0..3 {
                let Some(value) = determinant(a[k], bw, b[k], aw, work, cancel)? else {
                    return Ok(false);
                };
                if value.sign() != 0 {
                    return Ok(false);
                }
            }
        }
        start = end;
    }
    Ok(true)
}

pub(super) fn surface_is_c0(
    nurb: &solidcore::nurbs::srf_basic::Surface,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let degrees = [nurb.p(), nurb.q()];
    let counts = [nurb.n() + 1, nurb.m() + 1];
    let axes = [nurb.u_knots(), nurb.v_knots()];
    for axis in 0..2 {
        let degree = degrees[axis];
        let knots = axes[axis];
        let count = counts[axis];
        let mut start = knots.partition_point(|k| *k < domain[axis][0]);
        while start < knots.len() && knots[start] <= domain[axis][1] {
            work.charge(1, cancel)?;
            let value = knots[start];
            let mut end = start + 1;
            while end < knots.len() && knots[end] == value {
                work.charge(1, cancel)?;
                end += 1;
            }
            if knots[degree] < value && value < knots[count] && end - start > degree {
                if end - start != degree + 1 {
                    return Ok(false);
                }
                let Some(left) = end.checked_sub(degree + 2) else {
                    return Ok(false);
                };
                if left + 1 >= count {
                    return Ok(false);
                }
                let control = |at: usize, other: usize| -> Result<[f64; 4], GeometryError> {
                    let c = if axis == 0 {
                        nurb.cpt(at, other)
                    } else {
                        nurb.cpt(other, at)
                    }
                    .map_err(|_| {
                        super::nurbs_eval::conditioning("missing native C0 trace coefficient")
                    })?;
                    Ok([c.x, c.y, c.z, if nurb.is_rational() { c.w } else { 1.0 }])
                };
                let mut weights = None;
                for other in 0..counts[1 - axis] {
                    work.charge(1, cancel)?;
                    let a = control(left, other)?;
                    let b = control(left + 1, other)?;
                    if a[3] != 0.0 && b[3] != 0.0 {
                        weights = Some((a[3], b[3]));
                        break;
                    }
                }
                let Some((aw, bw)) = weights else {
                    return Ok(false);
                };
                for other in 0..counts[1 - axis] {
                    work.charge(1, cancel)?;
                    let a = control(left, other)?;
                    let b = control(left + 1, other)?;
                    for k in 0..4 {
                        let Some(value) = determinant(a[k], bw, b[k], aw, work, cancel)? else {
                            return Ok(false);
                        };
                        if value.sign() != 0 {
                            return Ok(false);
                        }
                    }
                }
            }
            start = end;
        }
    }
    Ok(true)
}
