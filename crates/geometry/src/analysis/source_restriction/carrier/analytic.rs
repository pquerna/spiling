// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Coefficient theorems for the laws selected by `SurfaceEvaluator`.
//!
//! A STEP wrapper, not an analytic label on a NURBS proxy, selects the angular
//! law. Proposed gauges are checked by affine or harmonic coefficient deltas;
//! neither a small residual nor a placement's nominal orthogonality is a proof.

use super::CarrierRecord;
use crate::exact_coefficient::ExactCoefficient as E;
use crate::parametric::{Interval, down, sin_cos_interval, up};
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::surface::surface::{Surface as NativeSurface, p2::SurfaceKind};
use solidcraft_step::geom::{Placement, Surface};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

type Vector = [Interval; 3];
const ZERO: Interval = Interval::point(0.0);
const ONE: Interval = Interval::point(1.0);

fn unavailable() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ConditioningFailure,
        "actual carrier coefficients exceed finite exact arithmetic conditioning",
    )
}
fn finite(value: Interval) -> Result<Interval, GeometryError> {
    Interval::new(value.lo, value.hi)
}
fn enclose(value: &E) -> Result<Interval, GeometryError> {
    finite(
        value.terms[..value.len]
            .iter()
            .fold(ZERO, |s, &v| s.add(Interval::point(v))),
    )
}
fn add_exact(a: &mut E, b: &E) -> Result<(), GeometryError> {
    for &term in &b.terms[..b.len] {
        a.push(term).ok_or_else(unavailable)?;
    }
    Ok(())
}
fn product(
    a: &E,
    b: &E,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<E, GeometryError> {
    a.product(b, work, cancel)?.ok_or_else(unavailable)
}
fn dot(a: Vector, b: Vector) -> Interval {
    (0..3).fold(ZERO, |s, k| s.add(a[k].mul(b[k])))
}
fn subtract(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|k| a[k].sub(b[k]))
}
fn scale(a: Vector, b: Interval) -> Vector {
    a.map(|x| x.mul(b))
}
fn add(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|k| a[k].add(b[k]))
}
// L1 dominates Euclidean distance, with no unproved floating-point norm.
fn norm(a: Vector) -> Result<Interval, GeometryError> {
    finite(
        a.into_iter()
            .fold(ZERO, |s, v| s.add(Interval::point(v.abs_upper()))),
    )
}
/// Convert a native-coordinate linear bound to the ORIGINAL/source UV gauge.
/// If s=o+M*n then n=-M^-1*o+M^-1*s. Interval inversion encloses the
/// actual real map (including the exact degree/radian coefficient).
fn source_errors(
    map: [[Interval; 3]; 2],
    native: [f64; 4],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[f64; 4], GeometryError> {
    work.charge(1, cancel)?;
    if native[3] != 0.0 {
        return Err(unavailable());
    }
    if native == [0.0; 4] {
        return Ok(native);
    }
    let determinant = finite(map[0][1].mul(map[1][2]).sub(map[0][2].mul(map[1][1])))?;
    if determinant.contains_zero() {
        return Err(unavailable());
    }
    let inverse = [
        [
            finite(map[1][2].div(determinant)?)?,
            finite(map[0][2].neg().div(determinant)?)?,
        ],
        [
            finite(map[1][1].neg().div(determinant)?)?,
            finite(map[0][1].div(determinant)?)?,
        ],
    ];
    let mut offsets = [ZERO; 2];
    for row in 0..2 {
        work.charge(1, cancel)?;
        offsets[row] = finite(
            inverse[row][0]
                .mul(map[0][0])
                .add(inverse[row][1].mul(map[1][0]))
                .neg(),
        )?;
    }
    let [constant, u, v, _] = native.map(Interval::point);
    let constant = constant
        .add(u.mul(Interval::point(offsets[0].abs_upper())))
        .add(v.mul(Interval::point(offsets[1].abs_upper())));
    let source_u = u
        .mul(Interval::point(inverse[0][0].abs_upper()))
        .add(v.mul(Interval::point(inverse[1][0].abs_upper())));
    let source_v = u
        .mul(Interval::point(inverse[0][1].abs_upper()))
        .add(v.mul(Interval::point(inverse[1][1].abs_upper())));
    Ok([
        finite(constant)?.hi,
        finite(source_u)?.hi,
        finite(source_v)?.hi,
        0.0,
    ])
}

fn degree() -> Result<Interval, GeometryError> {
    Interval {
        lo: down(std::f64::consts::PI),
        hi: up(std::f64::consts::PI),
    }
    .div(Interval::point(180.0))
    .and_then(finite)
}
fn slope(angle: Interval) -> Result<Interval, GeometryError> {
    if angle == ZERO {
        return Ok(ZERO);
    }
    let (s, c) = sin_cos_interval(angle);
    finite(s.div(c)?)
}

fn exact_source_frame(
    p: &Placement,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[E; 3]; 4], GeometryError> {
    for value in p.origin.into_iter().chain(p.axis).chain(p.ref_direction) {
        work.charge(1, cancel)?;
        Interval::new(value, value)?;
    }
    let mut result = std::array::from_fn(|_| std::array::from_fn(|_| E::point(0.0)));
    for (k, &origin) in p.origin.iter().enumerate() {
        result[0][k] = E::point(origin);
        result[1][k] = E::point(p.ref_direction[k]);
        result[3][k] = E::point(p.axis[k]);
        let mut y = product(
            &E::point(p.axis[(k + 1) % 3]),
            &E::point(p.ref_direction[(k + 2) % 3]),
            work,
            cancel,
        )?;
        let other = product(
            &E::point(p.axis[(k + 2) % 3]),
            &E::point(p.ref_direction[(k + 1) % 3]),
            work,
            cancel,
        )?;
        y.subtract(&other).ok_or_else(unavailable)?;
        result[2][k] = y;
    }
    Ok(result)
}
fn source_frame(
    p: &Placement,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Vector; 4], GeometryError> {
    let exact = exact_source_frame(p, work, cancel)?;
    let mut frame = [[ZERO; 3]; 4];
    for a in 0..4 {
        for k in 0..3 {
            work.charge(1, cancel)?;
            frame[a][k] = enclose(&exact[a][k])?;
        }
    }
    Ok(frame)
}

/// Numerators of O + U*u + V*v with one exact common denominator.
struct Affine {
    coefficients: [[E; 3]; 3],
    denominator: E,
}
fn native_affine(
    surface: &NativeSurface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Affine>, GeometryError> {
    let mut base = surface;
    for depth in 0..32 {
        work.charge(1, cancel)?;
        match base.kind() {
            SurfaceKind::CurveBounded(b) => base = b.basis(),
            SurfaceKind::Step(s) => {
                let Some(inner) = s.surface() else {
                    return Err(unavailable());
                };
                base = inner;
            }
            SurfaceKind::Plane(p) => {
                let frame = p.position();
                let o = frame.origin();
                let x = frame.x_axis();
                let y = frame.y_axis();
                let s = p.uv_scale();
                let raw = [[o.x, o.y, o.z], [x.x, x.y, x.z], [y.x, y.y, y.z]];
                let mut coefficients =
                    std::array::from_fn(|_| std::array::from_fn(|_| E::point(0.0)));
                for a in 0..3 {
                    for k in 0..3 {
                        work.charge(1, cancel)?;
                        Interval::new(raw[a][k], raw[a][k])?;
                        let factor = match a {
                            1 => s.x,
                            2 => s.y,
                            _ => 1.0,
                        };
                        Interval::new(factor, factor)?;
                        coefficients[a][k] =
                            product(&E::point(raw[a][k]), &E::point(factor), work, cancel)?;
                    }
                }
                return Ok(Some(Affine {
                    coefficients,
                    denominator: E::point(1.0),
                }));
            }
            _ => break,
        }
        if depth == 31 {
            return Err(unavailable());
        }
    }
    // Only the actual immutable tensor is considered here. An analytic label
    // cannot promote a fitted patch to an affine plane.
    let n = match base.kind() {
        SurfaceKind::BSpline(b) => b.gw_nurb(),
        _ => return Ok(None),
    };
    let Some(n) = n else {
        return Ok(None);
    };
    if n.degrees() != (1, 1) || n.n() != 1 || n.m() != 1 {
        return Ok(None);
    }
    let uk = n.u_knots();
    let vk = n.v_knots();
    if uk.len() != 4 || vk.len() != 4 {
        return Ok(None);
    }
    for value in uk.iter().chain(vk) {
        work.charge(1, cancel)?;
        Interval::new(*value, *value)?;
    }
    if uk[0] != uk[1]
        || uk[2] != uk[3]
        || uk[1] >= uk[2]
        || vk[0] != vk[1]
        || vk[2] != vk[3]
        || vk[1] >= vk[2]
    {
        return Ok(None);
    }
    let mut points = [[[0.0; 3]; 2]; 2];
    let mut weight = None;
    for i in 0..2 {
        for j in 0..2 {
            work.charge(1, cancel)?;
            let p = n.cpt(i, j).map_err(|_| unavailable())?;
            if (p.w != solidcore::nurbs::math::NOW) != n.is_rational()
                || p.z == solidcore::nurbs::math::NOZ
            {
                return Err(unavailable());
            }
            let w = if n.is_rational() { p.w } else { 1.0 };
            Interval::new(w, w)?;
            if w <= 0.0 || weight.is_some_and(|old| old != w) {
                return Ok(None);
            }
            weight = Some(w);
            points[i][j] = [p.x, p.y, p.z];
            for c in points[i][j] {
                Interval::new(c, c)?;
            }
        }
    }
    let du = E::difference(uk[2], uk[1]).ok_or_else(unavailable)?;
    let dv = E::difference(vk[2], vk[1]).ok_or_else(unavailable)?;
    let area = product(&du, &dv, work, cancel)?;
    let denominator = product(
        &E::point(weight.ok_or_else(unavailable)?),
        &area,
        work,
        cancel,
    )?;
    let mut coefficients = std::array::from_fn(|_| std::array::from_fn(|_| E::point(0.0)));
    for k in 0..3 {
        work.charge(1, cancel)?;
        let mut mixed = E::difference(points[1][1][k], points[1][0][k]).ok_or_else(unavailable)?;
        mixed.push(-points[0][1][k]).ok_or_else(unavailable)?;
        mixed.push(points[0][0][k]).ok_or_else(unavailable)?;
        if mixed.sign() != 0 {
            return Ok(None);
        }
        let u = E::difference(points[1][0][k], points[0][0][k]).ok_or_else(unavailable)?;
        let v = E::difference(points[0][1][k], points[0][0][k]).ok_or_else(unavailable)?;
        coefficients[1][k] = product(&u, &dv, work, cancel)?;
        coefficients[2][k] = product(&v, &du, work, cancel)?;
        let mut constant = product(&E::point(points[0][0][k]), &area, work, cancel)?;
        constant
            .subtract(&product(
                &coefficients[1][k],
                &E::point(uk[1]),
                work,
                cancel,
            )?)
            .ok_or_else(unavailable)?;
        constant
            .subtract(&product(
                &coefficients[2][k],
                &E::point(vk[1]),
                work,
                cancel,
            )?)
            .ok_or_else(unavailable)?;
        coefficients[0][k] = constant;
    }
    Ok(Some(Affine {
        coefficients,
        denominator,
    }))
}

fn plane(
    position: &Placement,
    native: &NativeSurface,
    domain: Option<[[f64; 2]; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CarrierRecord>, GeometryError> {
    affine_source(
        exact_source_frame(position, work, cancel)?,
        native,
        domain,
        work,
        cancel,
    )
}

fn affine_source(
    source: [[E; 3]; 4],
    native: &NativeSurface,
    domain: Option<[[f64; 2]; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CarrierRecord>, GeometryError> {
    let Some(native) = native_affine(native, work, cancel)? else {
        return Ok(None);
    };
    let denominator = enclose(&native.denominator)?;
    if denominator.contains_zero() {
        return Ok(None);
    }
    let mut a = [[ZERO; 3]; 2];
    for row in 0..2 {
        for k in 0..3 {
            a[row][k] = enclose(&source[row + 1][k])?;
        }
    }
    // The complete A^T A inverse is essential: source Y is the exact cross
    // product, and neither it nor the stored source X is assumed unit length.
    let xx = dot(a[0], a[0]);
    let xy = dot(a[0], a[1]);
    let yy = dot(a[1], a[1]);
    let determinant = finite(xx.mul(yy).sub(xy.square()))?;
    if determinant.lo <= 0.0 {
        return Ok(None);
    }
    let mut map = [[ZERO; 3]; 2];
    for coefficient in 0..3 {
        work.charge(1, cancel)?;
        let mut vector = [ZERO; 3];
        for k in 0..3 {
            vector[k] = finite(enclose(&native.coefficients[coefficient][k])?.div(denominator)?)?;
            if coefficient == 0 {
                vector[k] = vector[k].sub(enclose(&source[0][k])?);
            }
        }
        let x = dot(a[0], vector);
        let y = dot(a[1], vector);
        let u = finite(yy.mul(x).sub(xy.mul(y)).div(determinant)?)?.mid();
        let v = finite(xx.mul(y).sub(xy.mul(x)).div(determinant)?)?.mid();
        Interval::new(u, u)?;
        Interval::new(v, v)?;
        map[0][coefficient] = Interval::point(u);
        map[1][coefficient] = Interval::point(v);
    }
    let gauge_det = finite(map[0][1].mul(map[1][2]).sub(map[0][2].mul(map[1][1])))?;
    if gauge_det.contains_zero() {
        return Ok(None);
    }
    // Bound the actual coefficients AFTER the proposed inverse gauge. Each
    // residual is formed as an error-free expansion over the common native
    // denominator, so cancellation is exact rather than a nearness test.
    let mut errors = [0.0; 4];
    let mut literal = true;
    for coefficient in 0..3 {
        let mut residual = [ZERO; 3];
        for k in 0..3 {
            work.charge(1, cancel)?;
            let mut original = if coefficient == 0 {
                source[0][k].clone()
            } else {
                E::point(0.0)
            };
            for row in 0..2 {
                add_exact(
                    &mut original,
                    &product(
                        &source[row + 1][k],
                        &E::point(map[row][coefficient].lo),
                        work,
                        cancel,
                    )?,
                )?;
            }
            let mut delta = native.coefficients[coefficient][k].clone();
            delta
                .subtract(&product(&original, &native.denominator, work, cancel)?)
                .ok_or_else(unavailable)?;
            literal &= delta.sign() == 0;
            residual[k] = finite(enclose(&delta)?.div(denominator)?)?;
        }
        errors[coefficient] = norm(residual)?.hi;
    }
    let errors = source_errors(map, errors, work, cancel)?;
    Ok(Some(CarrierRecord {
        from_native: map,
        original_domain: domain.map_or([None; 2], |d| d.map(Some)),
        error_coefficients: errors,
        literal,
    }))
}

#[derive(Clone, Copy)]
struct NativeAnalytic {
    frame: [[f64; 3]; 4],
    radius: f64,
    minor: f64,
    angle_degrees: f64,
    kind: AnalyticKind,
}
#[derive(Clone, Copy, PartialEq)]
enum AnalyticKind {
    Cone,
    Sphere,
    Torus,
}
fn native_analytic(
    surface: &NativeSurface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeAnalytic>, GeometryError> {
    let mut surface = surface;
    let mut step = false;
    for _ in 0..32 {
        work.charge(1, cancel)?;
        match surface.kind() {
            SurfaceKind::CurveBounded(b) => surface = b.basis(),
            SurfaceKind::Step(s) => {
                surface = s.surface().ok_or_else(unavailable)?;
                step = true;
            }
            _ if !step => return Ok(None),
            _ => {
                let (placement, radius, minor, angle_degrees, kind) = match surface.kind() {
                    SurfaceKind::Cone(c) => {
                        let c = c.canonical();
                        (
                            c.origin,
                            c.radius,
                            0.0,
                            c.semi_angle_deg,
                            AnalyticKind::Cone,
                        )
                    }
                    SurfaceKind::Sphere(s) => (
                        *s.base().position(),
                        s.radius(),
                        s.radius(),
                        0.0,
                        AnalyticKind::Sphere,
                    ),
                    SurfaceKind::Torus(t) => (
                        *t.base().position(),
                        t.major_radius(),
                        t.minor_radius(),
                        0.0,
                        AnalyticKind::Torus,
                    ),
                    _ => return Ok(None),
                };
                let o = placement.origin();
                let x = placement.x_axis();
                let y = placement.y_axis();
                let z = placement.z_axis();
                let frame = [
                    [o.x, o.y, o.z],
                    [x.x, x.y, x.z],
                    [y.x, y.y, y.z],
                    [z.x, z.y, z.z],
                ];
                for v in frame
                    .into_iter()
                    .flatten()
                    .chain([radius, minor, angle_degrees])
                {
                    work.charge(1, cancel)?;
                    Interval::new(v, v)?;
                }
                return Ok(Some(NativeAnalytic {
                    frame,
                    radius,
                    minor,
                    angle_degrees,
                    kind,
                }));
            }
        }
    }
    Err(unavailable())
}

/// A numerical phase is merely a proposal. Its complete sine/cosine addition
/// coefficients are enclosed below, globally for every native angular value.
fn angular_proposal(source: [Vector; 4], native: [Vector; 4]) -> Option<(f64, f64)> {
    let xx = dot(source[1], source[1]);
    let xy = dot(source[1], source[2]);
    let yy = dot(source[2], source[2]);
    let determinant = xx.mul(yy).sub(xy.square());
    if !determinant.lo.is_finite() || determinant.lo <= 0.0 {
        return None;
    }
    let project = |v| -> Option<[f64; 2]> {
        let x = dot(source[1], v);
        let y = dot(source[2], v);
        Some([
            yy.mul(x).sub(xy.mul(y)).div(determinant).ok()?.mid(),
            xx.mul(y).sub(xy.mul(x)).div(determinant).ok()?.mid(),
        ])
    };
    let x = project(native[1])?;
    let y = project(native[2])?;
    let det = x[0] * y[1] - x[1] * y[0];
    if !det.is_finite()
        || det == 0.0
        || !x.into_iter().all(f64::is_finite)
        || (x[0] == 0.0 && x[1] == 0.0)
    {
        return None;
    }
    // Preserve the exact zero phase when the projected X is on the positive
    // original axis; atan2 itself supplies no coefficient identity authority.
    let phase = if x[1] == 0.0 && x[0] > 0.0 {
        0.0
    } else {
        x[1].atan2(x[0])
    };
    Some((phase, if det > 0.0 { 1.0 } else { -1.0 }))
}

fn coefficient_equal(
    native: [f64; 3],
    native_scale: f64,
    source: &[E; 3],
    source_scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut equal = true;
    for k in 0..3 {
        work.charge(1, cancel)?;
        let mut delta = product(&E::point(native[k]), &E::point(native_scale), work, cancel)?;
        delta
            .subtract(&product(&source[k], &E::point(source_scale), work, cancel)?)
            .ok_or_else(unavailable)?;
        equal &= delta.sign() == 0;
    }
    Ok(equal)
}
fn literal_analytic(
    position: &Placement,
    native: NativeAnalytic,
    radius: f64,
    minor: f64,
    phase: f64,
    sign_u: f64,
    offset_v: f64,
    scale_v: f64,
    original_slope_zero: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if phase != 0.0 {
        return Ok(false);
    }
    let source = exact_source_frame(position, work, cancel)?;
    let mut same_origin = true;
    for (k, &origin) in position.origin.iter().enumerate() {
        work.charge(1, cancel)?;
        let mut delta = E::difference(native.frame[0][k], origin).ok_or_else(unavailable)?;
        delta
            .subtract(&product(&source[3][k], &E::point(offset_v), work, cancel)?)
            .ok_or_else(unavailable)?;
        same_origin &= delta.sign() == 0;
    }
    if !same_origin {
        return Ok(false);
    }
    match native.kind {
        AnalyticKind::Cone => {
            if native.angle_degrees != 0.0 || !original_slope_zero {
                return Ok(false);
            }
            Ok(coefficient_equal(
                native.frame[1],
                native.radius,
                &source[1],
                radius,
                work,
                cancel,
            )? && coefficient_equal(
                native.frame[2],
                native.radius,
                &source[2],
                radius * sign_u,
                work,
                cancel,
            )? && coefficient_equal(native.frame[3], 1.0, &source[3], scale_v, work, cancel)?)
        }
        AnalyticKind::Sphere | AnalyticKind::Torus => {
            // ±1 factors are exact. No multiply-then-compare radius identity.
            let mut equal = coefficient_equal(
                native.frame[1],
                native.minor,
                &source[1],
                minor,
                work,
                cancel,
            )? && coefficient_equal(
                native.frame[2],
                native.minor,
                &source[2],
                minor * sign_u,
                work,
                cancel,
            )? && coefficient_equal(
                native.frame[3],
                native.minor,
                &source[3],
                minor * scale_v,
                work,
                cancel,
            )?;
            if native.kind == AnalyticKind::Torus {
                equal &= coefficient_equal(
                    native.frame[1],
                    native.radius,
                    &source[1],
                    radius,
                    work,
                    cancel,
                )? && coefficient_equal(
                    native.frame[2],
                    native.radius,
                    &source[2],
                    radius * sign_u,
                    work,
                    cancel,
                )?;
            }
            Ok(equal)
        }
    }
}

fn analytic(
    original: &Surface,
    native: &NativeSurface,
    domain: Option<[[f64; 2]; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CarrierRecord>, GeometryError> {
    let (position, radius, minor, original_angle, expected) = match original {
        Surface::Cylinder { position, radius } => (position, *radius, 0.0, 0.0, AnalyticKind::Cone),
        Surface::Cone {
            position,
            radius,
            semi_angle,
        } => (position, *radius, 0.0, *semi_angle, AnalyticKind::Cone),
        Surface::Sphere { position, radius } => {
            (position, *radius, *radius, 0.0, AnalyticKind::Sphere)
        }
        Surface::Torus {
            position,
            major_radius,
            minor_radius,
        }
        | Surface::DegenerateTorus {
            position,
            major_radius,
            minor_radius,
            ..
        } => (
            position,
            *major_radius,
            *minor_radius,
            0.0,
            AnalyticKind::Torus,
        ),
        _ => return Ok(None),
    };
    let Some(native) = native_analytic(native, work, cancel)? else {
        return Ok(None);
    };
    if native.kind != expected {
        return Ok(None);
    }
    for v in [radius, minor, original_angle] {
        Interval::new(v, v)?;
    }
    let source = source_frame(position, work, cancel)?;
    let nf = native.frame.map(|v| v.map(Interval::point));
    let Some((phase, sign_u)) = angular_proposal(source, nf) else {
        return Ok(None);
    };
    let radians = degree()?;
    let (sin_phase, cos_phase) = if phase == 0.0 {
        (ZERO, ONE)
    } else {
        sin_cos_interval(Interval::point(phase))
    };
    let cosine_frame = add(scale(source[1], cos_phase), scale(source[2], sin_phase));
    let sine_frame = scale(
        add(
            scale(source[1], sin_phase.neg()),
            scale(source[2], cos_phase),
        ),
        Interval::point(sign_u),
    );
    let zz = dot(source[3], source[3]);
    if !zz.lo.is_finite() || zz.lo <= 0.0 {
        return Ok(None);
    }
    let (offset_v, scale_v) = if native.kind == AnalyticKind::Cone {
        let offset = finite(dot(subtract(nf[0], source[0]), source[3]).div(zz)?)?.mid();
        let scale = finite(dot(nf[3], source[3]).div(zz)?)?.mid();
        if !offset.is_finite() || !scale.is_finite() || scale == 0.0 {
            return Ok(None);
        }
        (offset, scale)
    } else {
        // Flipping the physical axis flips latitude/minor angle, independently
        // of the azimuth handedness. Remaining frame differences stay in the
        // global harmonic coefficient bound, not an assumed isometry.
        let z = dot(nf[3], source[3]).mid();
        if !z.is_finite() || z == 0.0 {
            return Ok(None);
        }
        (0.0, if z > 0.0 { 1.0 } else { -1.0 })
    };
    let mut map = [[ZERO; 3]; 2];
    map[0] = [
        Interval::point(phase),
        radians.mul(Interval::point(sign_u)),
        ZERO,
    ];
    map[1] = [
        Interval::point(offset_v),
        ZERO,
        if native.kind == AnalyticKind::Cone {
            Interval::point(scale_v)
        } else {
            radians.mul(Interval::point(scale_v))
        },
    ];
    let mut c0 = norm(subtract(
        nf[0],
        add(source[0], scale(source[3], Interval::point(offset_v))),
    ))?;
    let mut cv = ZERO;
    if native.kind == AnalyticKind::Cone {
        // M2's selected native law is radius + height*tan(angle), axial height;
        // no historical Cone::evaluate value/derivative mixture is consulted.
        let ns = slope(Interval::point(native.angle_degrees).mul(radians))?;
        let os = slope(Interval::point(original_angle))?;
        let source_radius = Interval::point(radius).add(os.mul(Interval::point(offset_v)));
        let source_slope = os.mul(Interval::point(scale_v));
        for (n, s) in [(nf[1], cosine_frame), (nf[2], sine_frame)] {
            work.charge(1, cancel)?;
            c0 = c0.add(norm(subtract(
                scale(n, Interval::point(native.radius)),
                scale(s, source_radius),
            ))?);
            cv = cv.add(norm(subtract(scale(n, ns), scale(s, source_slope)))?);
        }
        cv = cv.add(norm(subtract(
            nf[3],
            scale(source[3], Interval::point(scale_v)),
        ))?);
    } else {
        // |cos(u)cos(v)|, |sin(u)cos(v)|, |sin(v)| <= 1 globally.
        // Torus additionally has the two major-circle harmonics. Consequently
        // no finite UV box, sampled root, or nominal native tolerance is needed.
        for (n, s) in [
            (nf[1], cosine_frame),
            (nf[2], sine_frame),
            (nf[3], scale(source[3], Interval::point(scale_v))),
        ] {
            work.charge(1, cancel)?;
            c0 = c0.add(norm(subtract(
                scale(n, Interval::point(native.minor)),
                scale(s, Interval::point(minor)),
            ))?);
        }
        if native.kind == AnalyticKind::Torus {
            for (n, s) in [(nf[1], cosine_frame), (nf[2], sine_frame)] {
                work.charge(1, cancel)?;
                c0 = c0.add(norm(subtract(
                    scale(n, Interval::point(native.radius)),
                    scale(s, Interval::point(radius)),
                ))?);
            }
        }
    }
    let literal = literal_analytic(
        position,
        native,
        radius,
        minor,
        phase,
        sign_u,
        offset_v,
        scale_v,
        original_angle == 0.0,
        work,
        cancel,
    )?;
    let errors = if literal {
        [0.0; 4]
    } else {
        [finite(c0)?.hi, 0.0, finite(cv)?.hi, 0.0]
    };
    let errors = source_errors(map, errors, work, cancel)?;
    for row in map {
        for value in row {
            finite(value)?;
        }
    }
    Ok(Some(CarrierRecord {
        from_native: map,
        original_domain: domain.map_or([None; 2], |d| d.map(Some)),
        error_coefficients: errors,
        literal,
    }))
}

pub(super) fn try_associate(
    original: &Surface,
    native: &NativeSurface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CarrierRecord>, GeometryError> {
    let mut original = original;
    let mut domain: Option<[[f64; 2]; 2]> = None;
    for _ in 0..32 {
        work.charge(1, cancel)?;
        match original {
            Surface::RectangularTrimmed {
                basis,
                u1,
                u2,
                v1,
                v2,
                ..
            } => {
                for value in [*u1, *u2, *v1, *v2] {
                    work.charge(1, cancel)?;
                    Interval::new(value, value)?;
                }
                Interval::new(u1.min(*u2), u1.max(*u2))?;
                Interval::new(v1.min(*v2), v1.max(*v2))?;
                let next = [[u1.min(*u2), u1.max(*u2)], [v1.min(*v2), v1.max(*v2)]];
                domain = Some(if let Some(previous) = domain {
                    let mut intersection = [[0.0; 2]; 2];
                    for axis in 0..2 {
                        work.charge(1, cancel)?;
                        intersection[axis] = [
                            previous[axis][0].max(next[axis][0]),
                            previous[axis][1].min(next[axis][1]),
                        ];
                        Interval::new(intersection[axis][0], intersection[axis][1])?;
                    }
                    intersection
                } else {
                    next
                });
                original = basis;
            }
            Surface::Extrusion {
                profile,
                direction,
                magnitude,
            } => {
                if let Some(circle) =
                    crate::kernel::source_operand::original_circular_extrusion(original)
                {
                    let cylinder = Surface::Cylinder {
                        position: *circle.position,
                        radius: circle.radius_mm,
                    };
                    let Some(mut record) = analytic(&cylinder, native, None, work, cancel)? else {
                        return Ok(None);
                    };
                    let scale = Interval::point(circle.axial_scale_mm);
                    for coefficient in &mut record.from_native[1] {
                        *coefficient = coefficient.div(scale)?;
                    }
                    for index in [2, 3] {
                        record.error_coefficients[index] =
                            Interval::point(record.error_coefficients[index])
                                .mul(scale)
                                .hi;
                    }
                    record.original_domain = domain.map_or([None; 2], |d| d.map(Some));
                    return Ok(Some(record));
                }
                if let solidcraft_step::geom::Curve::Line {
                    origin,
                    direction: profile_direction,
                    magnitude: profile_scale,
                } = profile.as_ref()
                {
                    let mut frame: [[E; 3]; 4] =
                        std::array::from_fn(|_| std::array::from_fn(|_| E::point(0.0)));
                    for k in 0..3 {
                        Interval::new(origin[k], origin[k])?;
                        Interval::new(profile_direction[k], profile_direction[k])?;
                        Interval::new(direction[k], direction[k])?;
                        Interval::new(*profile_scale, *profile_scale)?;
                        Interval::new(*magnitude, *magnitude)?;
                        frame[0][k] = E::point(origin[k]);
                        frame[1][k] = product(
                            &E::point(profile_direction[k]),
                            &E::point(*profile_scale),
                            work,
                            cancel,
                        )?;
                        frame[2][k] =
                            product(&E::point(direction[k]), &E::point(*magnitude), work, cancel)?;
                    }
                    return affine_source(frame, native, domain, work, cancel);
                }
                return Ok(None);
            }
            Surface::Plane { position } => return plane(position, native, domain, work, cancel),
            _ => return analytic(original, native, domain, work, cancel),
        }
    }
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcore::nurbs::{
        b_spline_surface::BSplineSurface,
        math::{CPoint, NOW},
        srf_basic,
    };
    use solidcore::surface::{cone::Cone, surface::p1::StepSurface};
    use solidcore::{Point3, Vec3};

    fn source_placement() -> Placement {
        Placement {
            origin: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            ref_direction: [1.0, 0.0, 0.0],
        }
    }
    #[test]
    fn circular_extrusion_requires_literal_positive_axis_and_consumes_axial_scale() {
        let cancel = AtomicBool::new(false);
        let source = Surface::Extrusion {
            profile: std::sync::Arc::new(solidcraft_step::geom::Curve::Circle {
                position: source_placement(),
                radius: 2.0,
            }),
            direction: [0.0, 0.0, 1.0],
            magnitude: 8.0,
        };
        let native = cylinder([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 2.0, true);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let record = try_associate(&source, &native, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(record.from_native[1][2].contains(0.125));
        assert!(record.literal);
        for (direction, magnitude) in [
            ([0.0, 0.0, -1.0], 8.0),
            ([0.0, 0.0, 1.0], -8.0),
            ([0.0, 0.0, 1.0], 0.0),
            ([0.0, 0.0, 1.0], f64::INFINITY),
            ([0.0, f64::EPSILON, 1.0], 8.0),
        ] {
            let mut changed = source.clone();
            if let Surface::Extrusion {
                direction: d,
                magnitude: m,
                ..
            } = &mut changed
            {
                *d = direction;
                *m = magnitude;
            }
            assert!(
                try_associate(&changed, &native, &mut work, &cancel)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn line_extrusion_proves_nonunit_source_parameter_coefficients() {
        let source = Surface::Extrusion {
            profile: std::sync::Arc::new(solidcraft_step::geom::Curve::Line {
                origin: [1.0, 2.0, 3.0],
                direction: [0.0, 1.0, 0.0],
                magnitude: 2.0,
            }),
            direction: [0.0, 0.0, 1.0],
            magnitude: 8.0,
        };
        let native = patch(
            [
                [[1.0, 2.0, 3.0], [1.0, 2.0, 11.0]],
                [[1.0, 4.0, 3.0], [1.0, 4.0, 11.0]],
            ],
            [[0.0, 0.0, 1.0, 1.0]; 2],
            1.0,
        );
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let record = try_associate(&source, &native, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(record.literal);
        assert_eq!(record.error_coefficients, [0.0; 4]);
    }

    #[test]
    fn spline_extrusion_preserves_knots_and_bounds_changed_coefficients() {
        use solidcraft_step::geom::{BSplineCurve, Curve, CurveForm, KnotSpec, Logical};
        let mut source = Surface::Extrusion {
            profile: std::sync::Arc::new(Curve::BSpline(Box::new(BSplineCurve {
                degree: 1,
                points: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
                weights: None,
                knots: vec![2.0, 4.0],
                multiplicities: vec![2, 2],
                form: CurveForm::PolylineForm,
                closed: Logical::False,
                self_intersect: Logical::False,
                knot_spec: KnotSpec::QuasiUniformKnots,
            }))),
            direction: [0.0, 0.0, 1.0],
            magnitude: 8.0,
        };
        let native = patch(
            [
                [[0.0, 0.0, -8.0], [0.0, 0.0, 16.0]],
                [[2.0, 0.0, -8.0], [2.0, 0.0, 16.0]],
            ],
            [[2.0, 2.0, 4.0, 4.0], [-1.0, -1.0, 2.0, 2.0]],
            1.0,
        );
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let exact = super::super::spline::try_associate(&source, &native, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(exact.literal);
        assert_eq!(exact.error_coefficients, [0.0; 4]);
        assert!(exact.from_native[1][2].contains(1.0));
        let association =
            super::super::SourceCarrierAssociation::new(&source, &native, &mut work, &cancel)
                .unwrap();
        assert_eq!(
            association.original_natural_domain(),
            [Some([2.0, 4.0]), None]
        );
        // Even literal equality cannot license profile-basis extrapolation.
        assert!(
            association
                .position_error_mm([[1.0, 4.0], [-100.0, 100.0]], &mut work, &cancel,)
                .is_err()
        );
        assert_eq!(
            association
                .position_error_mm([[2.0, 4.0], [-100.0, 100.0]], &mut work, &cancel,)
                .unwrap(),
            0.0
        );
        if let Surface::Extrusion { magnitude, .. } = &mut source {
            *magnitude = 9.0;
        }
        let changed = super::super::spline::try_associate(&source, &native, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(!changed.literal);
        assert!(changed.error_coefficients[2] >= 1.0);
        if let Surface::Extrusion { profile, .. } = &mut source
            && let Curve::BSpline(profile) = std::sync::Arc::make_mut(profile)
        {
            profile.multiplicities[0] = 1;
        }
        assert!(
            super::super::spline::try_associate(&source, &native, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }
    fn patch(points: [[[f64; 3]; 2]; 2], knots: [[f64; 4]; 2], weight: f64) -> NativeSurface {
        let net = points
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|p| {
                        CPoint::new(
                            p[0] * weight,
                            p[1] * weight,
                            p[2] * weight,
                            if weight == 1.0 { NOW } else { weight },
                        )
                    })
                    .collect()
            })
            .collect();
        let raw = srf_basic::Surface::new(net, 1, 1, knots[0].to_vec(), knots[1].to_vec()).unwrap();
        NativeSurface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)))
    }
    fn cylinder(x: [f64; 3], y: [f64; 3], radius: f64, step: bool) -> NativeSurface {
        let cone = Cone::new_cylinder_bounded(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(x[0], x[1], x[2]),
            Vec3::new(y[0], y[1], y[2]),
            radius,
            0.0,
            180.0,
            10.0,
            true,
            true,
            false,
        );
        let native = NativeSurface::new(SurfaceKind::Cone(Box::new(cone)));
        if step {
            NativeSurface::new(SurfaceKind::Step(StepSurface::new(native, true).unwrap()))
        } else {
            native
        }
    }
    #[test]
    fn actual_affine_tensor_uses_source_gram_and_changed_native_axes() {
        let original = Surface::Plane {
            position: Placement {
                origin: [1.0, 2.0, 3.0],
                axis: [0.0, 0.0, 3.0],
                ref_direction: [2.0, 0.0, 0.0],
            },
        };
        // P_native = (9,14,3) + u*(0,3,0) + v*(-4,0,0).
        let native = patch(
            [
                [[21.0, 8.0, 3.0], [5.0, 8.0, 3.0]],
                [[21.0, 20.0, 3.0], [5.0, 20.0, 3.0]],
            ],
            [[-2.0, -2.0, 2.0, 2.0], [-3.0, -3.0, 1.0, 1.0]],
            2.0,
        );
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let association = try_associate(&original, &native, &mut work, &cancel)
            .unwrap()
            .unwrap();
        for (row, expected) in association
            .from_native
            .into_iter()
            .zip([[4.0, 0.0, -2.0], [2.0, 0.5, 0.0]])
        {
            for (value, expected) in row.into_iter().zip(expected) {
                assert!((value.mid() - expected).abs() < 1.0e-12);
            }
        }
        assert!(
            association
                .error_coefficients
                .into_iter()
                .all(|e| e.is_finite() && (0.0..1.0e-10).contains(&e))
        );
    }
    #[test]
    fn exact_plane_tensor_is_literal_but_nonzero_residual_is_not() {
        let original = Surface::Plane {
            position: source_placement(),
        };
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let exact = patch(
            [
                [[0.0; 3], [0.0, 1.0, 0.0]],
                [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            ],
            [[0.0, 0.0, 1.0, 1.0]; 2],
            1.0,
        );
        let a = try_associate(&original, &exact, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(a.literal);
        assert_eq!(a.error_coefficients, [0.0; 4]);
        let gap = 2.0_f64.powi(-40);
        let displaced = patch(
            [
                [[0.0, 0.0, gap], [0.0, 1.0, gap]],
                [[1.0, 0.0, gap], [1.0, 1.0, gap]],
            ],
            [[0.0, 0.0, 1.0, 1.0]; 2],
            1.0,
        );
        let a = try_associate(&original, &displaced, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(!a.literal);
        assert!(a.error_coefficients[0] >= gap);
    }
    #[test]
    fn bilinear_planar_label_cannot_replace_an_affine_coefficient_theorem() {
        let original = Surface::Plane {
            position: source_placement(),
        };
        let native = patch(
            [
                [[0.0; 3], [0.0, 1.0, 0.0]],
                [[1.0, 0.0, 0.0], [2.0, 1.0, 0.0]],
            ],
            [[0.0, 0.0, 1.0, 1.0]; 2],
            1.0,
        );
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(
            try_associate(&original, &native, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn step_cylinder_phase_and_axis_flip_have_global_coefficient_bounds() {
        let original = Surface::Cylinder {
            position: source_placement(),
            radius: 2.0,
        };
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        // Native X=source Y, native Y=source X, hence axis and azimuth reverse.
        let native = cylinder([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], 2.125, true);
        let a = try_associate(&original, &native, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(a.from_native[0][1].hi < 0.0);
        assert!(a.from_native[1][2].hi < 0.0);
        assert!(!a.literal);
        // The radius delta is physical, not a native-tolerance declaration.
        assert!(a.error_coefficients[0] >= 0.125);
        assert!(
            a.error_coefficients
                .into_iter()
                .all(|e| e.is_finite() && e >= 0.0)
        );
    }
    #[test]
    fn analytic_label_without_step_does_not_authorize_angular_gauge() {
        let original = Surface::Cylinder {
            position: source_placement(),
            radius: 2.0,
        };
        let native = cylinder([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 2.0, false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(
            try_associate(&original, &native, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn preserved_step_coefficients_are_literal_not_merely_small_residuals() {
        let original = Surface::Cylinder {
            position: source_placement(),
            radius: 2.0,
        };
        let native = cylinder([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 2.0, true);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let a = try_associate(&original, &native, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(a.literal);
        assert_eq!(a.error_coefficients, [0.0; 4]);
        let d = degree().unwrap();
        assert_eq!(a.from_native[0][1], d);
    }
    #[test]
    fn cone_bounds_follow_selected_height_slope_not_historical_slant_value() {
        use crate::kernel::source_operand::PreparedOriginalSurface;
        use crate::parametric::SurfaceEvaluator;
        let original = Surface::Cone {
            position: source_placement(),
            radius: 2.0,
            semi_angle: 0.5_f64.atan(),
        };
        let cone = Cone::new_bounded(
            Point3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            3.0,
            6.0,
            0.0,
            180.0,
            4.0,
            true,
            true,
            false,
        );
        let native = NativeSurface::new(SurfaceKind::Step(
            StepSurface::new(NativeSurface::new(SurfaceKind::Cone(Box::new(cone))), true).unwrap(),
        ));
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let a = try_associate(&original, &native, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(!a.literal);
        assert!(a.error_coefficients[0] >= 0.5);
        assert!(a.error_coefficients[2] >= 0.25);
        assert_eq!(a.from_native[1][0], ONE);
        assert_eq!(a.from_native[1][2], ONE);
        let source = PreparedOriginalSurface::new(
            &original,
            [[0.0, 4.0], [0.0, 6.0]],
            usize::MAX,
            &mut work,
            &cancel,
        )
        .unwrap();
        let n = native
            .tensor([[0.0; 2], [4.0; 2]], 0, &mut work, &cancel)
            .unwrap();
        let s = source
            .tensor([[0.0; 2], [5.0; 2]], 0, &mut work, &cancel)
            .unwrap();
        let error = norm(subtract(n[0][0], s[0][0])).unwrap().hi;
        let bound = Interval::point(a.error_coefficients[0])
            .add(Interval::point(5.0).mul(Interval::point(a.error_coefficients[2])))
            .hi;
        assert!(error <= bound);
        // Native V=4 maps to original/source axial height V=5.
        assert!(n[0][0][2].contains(5.0));
    }
    #[test]
    fn residual_coefficients_are_transported_to_original_uv() {
        // source U=10+2*native U, source V=-6+3*native V.
        let map = [
            [Interval::point(10.0), Interval::point(2.0), ZERO],
            [Interval::point(-6.0), ZERO, Interval::point(3.0)],
        ];
        let mut work = GeometryWorkBudget::new(10_000);
        let cancel = AtomicBool::new(false);
        let source = source_errors(map, [1.0, 4.0, 9.0, 0.0], &mut work, &cancel).unwrap();
        // 1+4|nU|+9|nV| <= 39+2|source U|+3|source V|.
        assert!(source[0] >= 39.0 && source[0] < 40.0);
        assert!(source[1] >= 2.0 && source[1] < 2.01);
        assert!(source[2] >= 3.0 && source[2] < 3.01);
        assert_eq!(source[3], 0.0);
    }
}
