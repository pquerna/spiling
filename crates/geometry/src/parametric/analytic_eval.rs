// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Literal native parameter charts. Analytic STEP angles must never be passed to
//! a stored NURBS chart: the polar conversion is generally nonlinear.

use super::nurbs_eval::{SurfaceTensor, conditioning, evaluate_nurbs_surface_tensor};
use super::{Interval, SurfaceEnclosure};
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::nurbs::srf_basic;
use solidcore::surface::{surface::Surface, surface::p2::SurfaceKind};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

pub(crate) fn native_surface_nurbs(
    surface: &Surface,
) -> Result<Option<&srf_basic::Surface>, GeometryError> {
    let mut source = surface;
    let mut step_chart = false;
    for _ in 0..32 {
        let base = match source.kind() {
            SurfaceKind::Plane(_) => return Ok(None),
            SurfaceKind::Cone(_) | SurfaceKind::Sphere(_) | SurfaceKind::Torus(_) if step_chart => {
                return Ok(None);
            }
            SurfaceKind::SurfOfRevolution(_) | SurfaceKind::SurfOfExtrusion(_) if step_chart => {
                return Err(GeometryError::new(
                    GeometryErrorCode::UnsupportedOperation,
                    "STEP sweep chart requires original nonlinear profile correspondence",
                ));
            }
            SurfaceKind::Step(s) => {
                source = s
                    .surface()
                    .ok_or_else(|| conditioning("empty native STEP surface wrapper"))?;
                step_chart = true;
                continue;
            }
            SurfaceKind::BSpline(b) => b,
            SurfaceKind::Cone(c) => c.rev().base(),
            SurfaceKind::Sphere(s) => s.base().base(),
            SurfaceKind::Torus(t) => t.base().base(),
            SurfaceKind::SurfOfRevolution(r) => r.base(),
            SurfaceKind::SurfOfExtrusion(e) => e.base(),
            SurfaceKind::CurveBounded(b) => {
                source = b.basis();
                continue;
            }
            _ => {
                return Err(GeometryError::new(
                    GeometryErrorCode::UnsupportedOperation,
                    "surface expression has no bounded immutable coefficient evaluator",
                ));
            }
        };
        return base
            .gw_nurb()
            .map(Some)
            .ok_or_else(|| conditioning("surface has no immutable native NURBS representation"));
    }
    Err(GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "nested native surface depth exceeded",
    ))
}

/// Evaluate the stored native NURBS representation (plane: literal affine chart).
/// This is not the analytic STEP chart and does not certify an analytic/NURBS
/// correspondence. In particular Cone::evaluate's corrected STEP point must not
/// be paired with these derivatives; source consumers retain this one law.
pub fn evaluate_analytic_surface_jet(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    surface_order(surface, uv, 2, work, cancel)
}

pub(super) fn surface_order(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    let j = surface_tensor(surface, uv, order, work, cancel)?;
    Ok(SurfaceEnclosure {
        position: j[0][0],
        du: j[1][0],
        dv: j[0][1],
        duu: j[2][0],
        duv: j[1][1],
        dvv: j[0][2],
    })
}

pub(super) fn surface_tensor(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceTensor, GeometryError> {
    if order > 3 {
        return Err(conditioning("surface derivative order exceeds three"));
    }
    work.charge(1, cancel)?;
    for range in uv {
        Interval::new(range[0], range[1])?;
    }
    let domain = surface.natural_uv_domain();
    let min = domain.min();
    let max = domain.max();
    if uv[0][0] < min.x || uv[0][1] > max.x || uv[1][0] < min.y || uv[1][1] > max.y {
        return Err(conditioning(
            "surface query exceeds its immutable native active chart",
        ));
    }
    if let Some(nurb) = native_surface_nurbs(surface)? {
        return evaluate_nurbs_surface_tensor(nurb, uv, order, work, cancel);
    }
    let mut source = surface;
    for _ in 0..32 {
        match source.kind() {
            SurfaceKind::CurveBounded(b) => source = b.basis(),
            SurfaceKind::Step(s) => {
                return step_surface_tensor(
                    s.surface()
                        .ok_or_else(|| conditioning("empty native STEP surface wrapper"))?,
                    uv,
                    order,
                    work,
                    cancel,
                );
            }
            SurfaceKind::Plane(plane) => {
                let pos = plane.position();
                let o = pos.origin();
                let x = pos.x_axis();
                let y = pos.y_axis();
                let scale = plane.uv_scale();
                let ox = [o.x, o.y, o.z];
                let xx = [x.x, x.y, x.z];
                let yy = [y.x, y.y, y.z];
                let du = xx.map(|a| Interval::point(a).mul(Interval::point(scale.x)));
                let dv = yy.map(|a| Interval::point(a).mul(Interval::point(scale.y)));
                let u = Interval::new(uv[0][0], uv[0][1])?;
                let v = Interval::new(uv[1][0], uv[1][1])?;
                let position = std::array::from_fn(|i| {
                    Interval::point(ox[i]).add(u.mul(du[i])).add(v.mul(dv[i]))
                });
                for a in position.into_iter().chain(du).chain(dv) {
                    Interval::new(a.lo, a.hi)?;
                }
                let mut tensor = [[[Interval::point(0.0); 3]; 4]; 4];
                tensor[0][0] = position;
                if order >= 1 {
                    tensor[1][0] = du;
                    tensor[0][1] = dv;
                }
                return Ok(tensor);
            }
            _ => {
                return Err(conditioning(
                    "inconsistent native affine surface representation",
                ));
            }
        }
    }
    Err(GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "nested native surface depth exceeded",
    ))
}

/// Parametric orientation only; the owning face-use orientation is separate.
/// Singular charts require a genuine same-law carrier field, never a nearby
/// sampled normal. An explicit STEP sphere has such an algebraic field.
pub fn evaluate_analytic_normal_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    let jet = surface_order(surface, uv, 1, work, cancel)?;
    if let Some(normal) = step_sphere_normal(surface, uv, work, cancel)? {
        return Ok(normal);
    }
    let normal = super::cross(jet.du, jet.dv);
    let squared = normal
        .into_iter()
        .fold(Interval::point(0.0), |s, v| s.add(v.square()));
    if squared.lo <= 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::SingularGeometry,
            "whole native surface chart normal cannot be separated from zero",
        ));
    }
    let length = squared.sqrt()?;
    Ok([
        normal[0].div(length)?,
        normal[1].div(length)?,
        normal[2].div(length)?,
    ])
}

/// For the literal affine sphere P=O+r*A*q, q on the unit sphere,
/// du×dv = r²*c²*cos(v)*cofactor(A)*q. Remove only the proved scalar chart
/// factor on [-90,90]; invertible A gives the unique physical pole normal.
/// No orthonormality assumption is made about stored placement coefficients.
fn step_sphere_normal(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    let mut source = surface;
    let mut step_chart = false;
    for _ in 0..32 {
        work.charge(1, cancel)?;
        match source.kind() {
            SurfaceKind::Step(s) => {
                source = s
                    .surface()
                    .ok_or_else(|| conditioning("empty STEP normal source"))?;
                step_chart = true;
            }
            SurfaceKind::CurveBounded(s) => source = s.basis(),
            SurfaceKind::Sphere(s) if step_chart => {
                if uv[1][0] < -90.0
                    || uv[1][1] > 90.0
                    || !s.radius().is_finite()
                    || s.radius() == 0.0
                {
                    return Ok(None);
                }
                let placement = s.base().position();
                let x = placement.x_axis();
                let y = placement.y_axis();
                let z = placement.z_axis();
                let x = [x.x, x.y, x.z].map(Interval::point);
                let y = [y.x, y.y, y.z].map(Interval::point);
                let z = [z.x, z.y, z.z].map(Interval::point);
                for c in x.into_iter().chain(y).chain(z) {
                    Interval::new(c.lo, c.hi)?;
                }
                let cofactors = [super::cross(y, z), super::cross(z, x), super::cross(x, y)];
                let determinant = (0..3).fold(Interval::point(0.0), |v, k| {
                    v.add(x[k].mul(cofactors[0][k]))
                });
                if determinant.contains_zero() {
                    return Ok(None);
                }
                let degree = Interval {
                    lo: super::down(std::f64::consts::PI),
                    hi: super::up(std::f64::consts::PI),
                }
                .div(Interval::point(180.0))?;
                let (su, cu) =
                    super::sin_cos_interval(Interval::new(uv[0][0], uv[0][1])?.mul(degree));
                let (sv, cv) =
                    super::sin_cos_interval(Interval::new(uv[1][0], uv[1][1])?.mul(degree));
                let q = [cv.mul(cu), cv.mul(su), sv];
                let normal: [Interval; 3] = std::array::from_fn(|k| {
                    (0..3).fold(Interval::point(0.0), |v, j| {
                        v.add(cofactors[j][k].mul(q[j]))
                    })
                });
                let frobenius = x
                    .into_iter()
                    .chain(y)
                    .chain(z)
                    .fold(Interval::point(0.0), |v, c| v.add(c.square()))
                    .sqrt()?;
                // A^T*N = det(A)*q and |q|=1, so |N| >= |det(A)|/||A||_F.
                let lower = Interval::point(determinant.abs_lower())
                    .div(Interval::point(frobenius.hi))?
                    .lo;
                let direct_length = normal
                    .into_iter()
                    .fold(Interval::point(0.0), |v, c| v.add(c.square()))
                    .sqrt()?;
                if lower <= 0.0 {
                    return Ok(None);
                }
                let length = Interval::new(lower.max(direct_length.lo), direct_length.hi)?;
                let mut result = [Interval::point(0.0); 3];
                for k in 0..3 {
                    result[k] = normal[k]
                        .div(length)?
                        .intersect(Interval { lo: -1.0, hi: 1.0 })?;
                }
                return Ok(Some(result));
            }
            _ => return Ok(None),
        }
    }
    Err(GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "native STEP normal nesting limit exceeded",
    ))
}

/// The *explicit* original analytic STEP chart. Angles use native degrees and
/// cone V uses height. This function does not convert an original trim, invent
/// a STEP/NURBS map, or apply face-use/inside-out NURBS orientation.
pub fn evaluate_step_analytic_surface_jet(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    let j = step_surface_tensor(surface, uv, 2, work, cancel)?;
    Ok(SurfaceEnclosure {
        position: j[0][0],
        du: j[1][0],
        dv: j[0][1],
        duu: j[2][0],
        duv: j[1][1],
        dvv: j[0][2],
    })
}
fn step_surface_tensor(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceTensor, GeometryError> {
    work.charge(1, cancel)?;
    if order > 3 {
        return Err(conditioning("STEP surface derivative order exceeds three"));
    }
    let mut surface = surface;
    for depth in 0..=32 {
        match surface.kind() {
            SurfaceKind::Step(s) => {
                surface = s
                    .surface()
                    .ok_or_else(|| conditioning("empty native STEP surface wrapper"))?
            }
            SurfaceKind::CurveBounded(s) => surface = s.basis(),
            _ => break,
        }
        work.charge(1, cancel)?;
        if depth == 32 {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "nested STEP surface depth exceeded",
            ));
        }
    }
    let u = Interval::new(uv[0][0], uv[0][1])?;
    let v = Interval::new(uv[1][0], uv[1][1])?;
    let domain = surface.step_uv_domain();
    let lo = domain.min();
    let hi = domain.max();
    if u.lo < lo.x || u.hi > hi.x || v.lo < lo.y || v.hi > hi.y {
        return Err(conditioning(
            "analytic STEP query exceeds its declared active domain",
        ));
    }
    let zero = Interval::point(0.0);
    let one = Interval::point(1.0);
    let degree = Interval {
        lo: super::down(std::f64::consts::PI),
        hi: super::up(std::f64::consts::PI),
    }
    .div(Interval::point(180.0))?;
    let (su, cu) = super::sin_cos_interval(u.mul(degree));
    // Radial and axial functions, with their literal STEP V derivatives.
    let (placement, radial, axial) = match surface.kind() {
        SurfaceKind::Plane(_) | SurfaceKind::BSpline(_) => {
            return surface_tensor(surface, uv, order, work, cancel);
        }
        SurfaceKind::Cone(c) => {
            let native = c.canonical();
            let (sin_a, cos_a) =
                super::sin_cos_interval(Interval::point(native.semi_angle_deg).mul(degree));
            let slope = sin_a.div(cos_a)?;
            (
                native.origin,
                [
                    Interval::point(native.radius).add(v.mul(slope)),
                    slope,
                    zero,
                    zero,
                ],
                [v, one, zero, zero],
            )
        }
        SurfaceKind::Sphere(s) => {
            let (sv, cv) = super::sin_cos_interval(v.mul(degree));
            let r = Interval::point(s.radius());
            (
                *s.base().position(),
                [
                    r.mul(cv),
                    r.mul(sv.neg()).mul(degree),
                    r.mul(cv.neg()).mul(degree.square()),
                    r.mul(sv).mul(degree.square()).mul(degree),
                ],
                [
                    r.mul(sv),
                    r.mul(cv).mul(degree),
                    r.mul(sv.neg()).mul(degree.square()),
                    r.mul(cv.neg()).mul(degree.square()).mul(degree),
                ],
            )
        }
        SurfaceKind::Torus(t) => {
            let (sv, cv) = super::sin_cos_interval(v.mul(degree));
            let r = Interval::point(t.minor_radius());
            (
                *t.base().position(),
                [
                    Interval::point(t.major_radius()).add(r.mul(cv)),
                    r.mul(sv.neg()).mul(degree),
                    r.mul(cv.neg()).mul(degree.square()),
                    r.mul(sv).mul(degree.square()).mul(degree),
                ],
                [
                    r.mul(sv),
                    r.mul(cv).mul(degree),
                    r.mul(sv.neg()).mul(degree.square()),
                    r.mul(cv.neg()).mul(degree.square()).mul(degree),
                ],
            )
        }
        _ => {
            return Err(GeometryError::new(
                GeometryErrorCode::UnsupportedOperation,
                "surface has no original analytic STEP operand evaluator",
            ));
        }
    };
    let p = placement.origin();
    let x = placement.x_axis();
    let y = placement.y_axis();
    let z = placement.z_axis();
    let p = [p.x, p.y, p.z];
    let x = [x.x, x.y, x.z];
    let y = [y.x, y.y, y.z];
    let z = [z.x, z.y, z.z];
    for c in p.into_iter().chain(x).chain(y).chain(z) {
        Interval::new(c, c)?;
    }
    let h: [Interval; 3] = std::array::from_fn(|k| {
        cu.mul(Interval::point(x[k]))
            .add(su.mul(Interval::point(y[k])))
    });
    let tangent: [Interval; 3] = std::array::from_fn(|k| {
        su.neg()
            .mul(Interval::point(x[k]))
            .add(cu.mul(Interval::point(y[k])))
    });
    let directions = [h, tangent, h.map(Interval::neg), tangent.map(Interval::neg)];
    let mut result = [[[zero; 3]; 4]; 4];
    let mut scale_u = one;
    for a in 0..=order {
        for b in 0..=order - a {
            for k in 0..3 {
                let mut value = radial[b].mul(directions[a][k]).mul(scale_u);
                if a == 0 {
                    value = value.add(axial[b].mul(Interval::point(z[k])));
                }
                if a == 0 && b == 0 {
                    value = value.add(Interval::point(p[k]));
                }
                Interval::new(value.lo, value.hi)?;
                result[a][b][k] = value;
            }
        }
        scale_u = scale_u.mul(degree);
    }
    Ok(result)
}
