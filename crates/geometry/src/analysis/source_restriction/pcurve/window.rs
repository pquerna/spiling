// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Finite candidate covers in the literal PCURVE gauge. A cover is not endpoint
//! incidence or permission to extend an authored trim; the owner proves those.
use crate::analysis::source_restriction::selection;
use crate::kernel::source::{OriginalCoedgeSource, OriginalFaceSource, OriginalPcurveSource};
use crate::kernel::source_operand::{PreparedOriginalCurve, PreparedOriginalSurface};
use crate::parametric::{Interval, down, up};
use crate::{GeometryError, GeometryWorkBudget};
use solidcraft_step::geom::{Curve, Surface};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

pub(super) struct PcurveWindow {
    pub(super) raw: [f64; 2],
    pub(super) search: [f64; 2],
    pub(super) surface_domain: [[f64; 2]; 2],
    pub(super) closed_domain: Option<selection::ClosedEndpointDomain>,
    pub(super) period: Option<Interval>,
    pub(super) declared_endpoints: Option<[f64; 2]>,
    pub(super) declared_turns: [i32; 2],
}
fn unavailable(message: &'static str) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
fn capacity() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "original PCURVE window exceeds its live byte allowance",
    )
}
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |s, i| s.add(a[i].mul(b[i])))
}
fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    std::array::from_fn(|i| {
        a[(i + 1) % 3]
            .mul(b[(i + 2) % 3])
            .sub(a[(i + 2) % 3].mul(b[(i + 1) % 3]))
    })
}
fn knot_domain(
    knots: &[f64],
    multiplicities: &[usize],
    degree: usize,
    count: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[f64; 2], GeometryError> {
    if count <= degree || knots.len() != multiplicities.len() {
        return Err(unavailable("original carrier knot census is invalid"));
    }
    let expected = count
        .checked_add(degree)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(capacity)?;
    let mut at = 0usize;
    let mut ends = [None; 2];
    let mut previous = None;
    for (&knot, &multiplicity) in knots.iter().zip(multiplicities) {
        work.charge(1, cancel)?;
        Interval::new(knot, knot)?;
        if multiplicity == 0 || previous.is_some_and(|old| old >= knot) {
            return Err(unavailable(
                "original carrier knots are not an ordered positive census",
            ));
        }
        let next = at.checked_add(multiplicity).ok_or_else(capacity)?;
        for (slot, index) in [degree, count].into_iter().enumerate() {
            if at <= index && index < next {
                ends[slot] = Some(knot);
            }
        }
        at = next;
        previous = Some(knot);
    }
    let [Some(a), Some(b)] = ends else {
        return Err(unavailable("original carrier natural domain is absent"));
    };
    if at != expected || a >= b {
        return Err(unavailable("original carrier natural domain is invalid"));
    }
    Ok([a, b])
}
fn carrier_domain<'a>(
    mut surface: &'a Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(&'a Surface, [[f64; 2]; 2]), GeometryError> {
    let mut domain = [[f64::NEG_INFINITY, f64::INFINITY]; 2];
    for depth in 0..16 {
        work.charge(1, cancel)?;
        match surface {
            Surface::RectangularTrimmed {
                basis,
                u1,
                u2,
                v1,
                v2,
                ..
            } => {
                for (axis, a, b) in [(0, *u1, *u2), (1, *v1, *v2)] {
                    Interval::new(a.min(b), a.max(b))?;
                    domain[axis][0] = domain[axis][0].max(a.min(b));
                    domain[axis][1] = domain[axis][1].min(a.max(b));
                }
                surface = basis;
            }
            Surface::BSpline(s) => {
                let columns = s
                    .points
                    .first()
                    .ok_or_else(|| unavailable("empty original carrier coefficient net"))?
                    .len();
                let natural = [
                    knot_domain(
                        &s.u_knots,
                        &s.u_multiplicities,
                        s.u_degree,
                        s.points.len(),
                        work,
                        cancel,
                    )?,
                    knot_domain(
                        &s.v_knots,
                        &s.v_multiplicities,
                        s.v_degree,
                        columns,
                        work,
                        cancel,
                    )?,
                ];
                for axis in 0..2 {
                    domain[axis][0] = domain[axis][0].max(natural[axis][0]);
                    domain[axis][1] = domain[axis][1].min(natural[axis][1]);
                }
                return Ok((surface, domain));
            }
            _ => return Ok((surface, domain)),
        }
        if depth == 15 {
            return Err(capacity());
        }
    }
    Err(capacity())
}
fn angular_period(
    surface: &Surface,
    direction: [f64; 3],
    magnitude: f64,
) -> Result<Option<Interval>, GeometryError> {
    let axis = match surface {
        Surface::Cylinder { .. }
        | Surface::Cone { .. }
        | Surface::Sphere { .. }
        | Surface::Torus { .. }
        | Surface::DegenerateTorus { .. }
            if direction[1] == 0.0 && direction[0] != 0.0 =>
        {
            0
        }
        Surface::Torus { .. } | Surface::DegenerateTorus { .. }
            if direction[0] == 0.0 && direction[1] != 0.0 =>
        {
            1
        }
        _ => return Ok(None),
    };
    let velocity = Interval::point(direction[axis]).mul(Interval::point(magnitude));
    if velocity.contains_zero() {
        return Err(unavailable(
            "original PCURVE angular slope is not separated from zero",
        ));
    }
    let speed = if velocity.hi < 0.0 {
        velocity.neg()
    } else {
        velocity
    };
    Ok(Some(
        Interval::new(down(std::f64::consts::TAU), up(std::f64::consts::TAU))?.div(speed)?,
    ))
}

/// Only literal LINEs need a new finite proposal. Finite trim, spline, polyline
/// and periodic selectors continue to use the existing original selector law.
pub(super) fn select(
    face: &OriginalFaceSource,
    coedge: &OriginalCoedgeSource,
    pcurve: &OriginalPcurveSource,
    allowance: f64,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PcurveWindow, GeometryError> {
    work.charge(1, cancel)?;
    if face.carrier_id != pcurve.carrier_id || coedge.source_face != face.source_face {
        return Err(unavailable(
            "original PCURVE does not name this original face carrier",
        ));
    }
    select_operand(coedge, &pcurve.operands, allowance, remaining, work, cancel)
}

/// The selected spatial master can belong to another declared face. Its finite
/// cover uses that actual operand and the edge's captured source vertices;
/// face-specific PCurve ownership remains checked by `select`.
pub(super) fn select_operand(
    coedge: &OriginalCoedgeSource,
    pcurve: &solidcraft_step::geom::Pcurve,
    allowance: f64,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PcurveWindow, GeometryError> {
    work.charge(1, cancel)?;
    if !allowance.is_finite() || allowance < 0.0 {
        return Err(unavailable(
            "original PCURVE allowance is not finite and nonnegative",
        ));
    }
    let fixed = std::mem::size_of::<PcurveWindow>();
    let available = remaining.checked_sub(fixed).ok_or_else(capacity)?;
    let curve = pcurve.curve.as_ref();
    let (carrier, natural) = carrier_domain(&pcurve.surface, work, cancel)?;
    let (raw, search, closed_domain, period, declared_endpoints, declared_turns) =
        if let Curve::Line {
            origin,
            direction,
            magnitude,
        } = curve
        {
            for value in origin.iter().chain(direction).copied().chain([*magnitude]) {
                Interval::new(value, value)?;
            }
            let slope = direction.map(|v| Interval::point(v).mul(Interval::point(*magnitude)));
            if slope[0].contains_zero() && slope[1].contains_zero() {
                return Err(unavailable(
                    "original PCURVE LINE has no nonzero chart velocity",
                ));
            }
            let period = angular_period(carrier, *direction, *magnitude)?;
            let finite_axis = (0..2).any(|i| {
                natural[i][0].is_finite() && natural[i][1].is_finite() && !slope[i].contains_zero()
            });
            let raw = if finite_axis {
                // The division gives an enclosure, not an exact natural-boundary
                // incidence. Choose inward FLOAT proposals, then prove their whole
                // coefficient image below. Root/forward owners retain authority.
                let mut bounds = [f64::NEG_INFINITY, f64::INFINITY];
                for axis in 0..2 {
                    if slope[axis].contains_zero() {
                        if origin[axis] < natural[axis][0] || origin[axis] > natural[axis][1] {
                            return Err(unavailable(
                                "constant original PCURVE coordinate is outside its carrier",
                            ));
                        }
                        continue;
                    }
                    if natural[axis][0].is_finite() && natural[axis][1].is_finite() {
                        let a = Interval::point(natural[axis][0])
                            .sub(Interval::point(origin[axis]))
                            .div(slope[axis])?;
                        let b = Interval::point(natural[axis][1])
                            .sub(Interval::point(origin[axis]))
                            .div(slope[axis])?;
                        let (lo, hi) = if slope[axis].lo > 0.0 {
                            (a.hi, b.lo)
                        } else {
                            (b.hi, a.lo)
                        };
                        bounds[0] = bounds[0].max(up(lo));
                        bounds[1] = bounds[1].min(down(hi));
                    }
                }
                bounds
            } else if let Some(period) = period {
                let margin = period.hi / 16.0;
                [down(-margin), up(period.hi + margin)]
            } else if let Surface::Cylinder { position, .. } | Surface::Cone { position, .. } =
                carrier
            {
                // V is the actual source placement's axial coordinate, not a UV
                // alias for the spatial parameter or a normalized native chart.
                if slope[1].contains_zero() {
                    return Err(unavailable(
                        "nonperiodic source angular LINE lacks an axial seed law",
                    ));
                }
                let x = position.ref_direction.map(Interval::point);
                let z = position.axis.map(Interval::point);
                let y = cross(z, x);
                let cofactor = cross(x, y);
                let determinant = dot(z, cofactor);
                if determinant.contains_zero() {
                    return Err(unavailable("original carrier placement is singular"));
                }
                let inverse = [
                    cofactor[0].div(determinant)?,
                    cofactor[1].div(determinant)?,
                    cofactor[2].div(determinant)?,
                ];
                let uncertainty = dot(inverse, inverse)
                    .sqrt()?
                    .mul(Interval::point(allowance));
                let mut range: Option<Interval> = None;
                for point in coedge.vertex_points_mm {
                    let displacement = std::array::from_fn(|i| {
                        Interval::point(point[i]).sub(Interval::point(position.origin[i]))
                    });
                    let v = dot(inverse, displacement)
                        .add(Interval::new(-uncertainty.hi, uncertainty.hi)?);
                    let t = v.sub(Interval::point(origin[1])).div(slope[1])?;
                    range = Some(range.map_or(t, |old| old.hull(t)));
                }
                let range =
                    range.ok_or_else(|| unavailable("original PCURVE axial seed is absent"))?;
                [down(range.lo), up(range.hi)]
            } else if matches!(carrier, Surface::Plane { .. }) {
                // Evaluate the actual affine S(gamma) law at raw zero. This is a
                // coefficient reference point, not an arbitrary search interval.
                let uv = [[origin[0]; 2], [origin[1]; 2]];
                let surface =
                    PreparedOriginalSurface::new(&pcurve.surface, uv, available, work, cancel)?;
                let jet = surface.evaluate_order(uv, 1, work, cancel)?;
                let velocity = std::array::from_fn(|i| {
                    jet[1][0][i].mul(slope[0]).add(jet[0][1][i].mul(slope[1]))
                });
                let norm = dot(velocity, velocity);
                if norm.lo <= 0.0 {
                    return Err(unavailable("original lifted LINE velocity is singular"));
                }
                let error = Interval::point(allowance).div(norm.sqrt()?)?;
                let mut bounds: Option<Interval> = None;
                for point in coedge.vertex_points_mm {
                    let delta =
                        std::array::from_fn(|i| Interval::point(point[i]).sub(jet[0][0][i]));
                    let t = dot(delta, velocity)
                        .div(norm)?
                        .add(Interval::new(-error.hi, error.hi)?);
                    bounds = Some(bounds.map_or(t, |old| old.hull(t)));
                }
                let bounds =
                    bounds.ok_or_else(|| unavailable("original lifted LINE seed is absent"))?;
                [down(bounds.lo), up(bounds.hi)]
            } else {
                return Err(unavailable(
                    "unbounded nonperiodic PCURVE LINE carrier has no supported finite coefficient seed law",
                ));
            };
            // Carrier-derived inward/expanded bounds remain numerical proposals,
            // never natural coefficient fences or authored parameter incidences.
            (raw, raw, None, period, None, [0; 2])
        } else {
            let selected = selection::select_curve(curve, coedge.same_sense, work, cancel)?;
            (
                selected.active,
                selected.search,
                selected.closed_domain,
                selected.periodic,
                selected.declared_endpoints,
                selected.declared_turns,
            )
        };
    Interval::new(raw[0], raw[1])?;
    if !raw[0].is_finite() || !raw[1].is_finite() || raw[0] >= raw[1] {
        return Err(unavailable(
            "original PCURVE candidate window is not finite with positive span",
        ));
    }
    let chart = PreparedOriginalCurve::new(curve, raw, available, work, cancel)?;
    let image = chart.position_bounds(raw, work, cancel)?;
    let surface_domain = [[image[0].lo, image[0].hi], [image[1].lo, image[1].hi]];
    for axis in 0..2 {
        if surface_domain[axis][0] < natural[axis][0] || surface_domain[axis][1] > natural[axis][1]
        {
            return Err(unavailable(
                "original PCURVE coefficient enclosure exceeds the finite carrier domain; no containment theorem is available",
            ));
        }
    }
    let surface_budget = available
        .checked_sub(chart.retained_bytes())
        .ok_or_else(capacity)?;
    PreparedOriginalSurface::new(
        &pcurve.surface,
        surface_domain,
        surface_budget,
        work,
        cancel,
    )?;
    Ok(PcurveWindow {
        raw,
        search,
        surface_domain,
        closed_domain,
        period,
        declared_endpoints,
        declared_turns,
    })
}
