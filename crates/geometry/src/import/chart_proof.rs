// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Outward source-chart tubes. Rounded phase values are never exact witnesses.
//! Complete conic coefficient identities bound angular derivatives; endpoint
//! phase isolation plus those derivatives bounds the entire oriented interval.
use super::{error, invalid};
use crate::{
    GeometryWorkBudget,
    parametric::{Interval, native_error, up},
};
use monstertruck_io::step::load::step_geometry::{
    Curve3D, ElementarySurface, Surface, SurfaceParameterMap, SweepSurface,
};
use monstertruck_modeling::{
    Curve2D, Line, ValidatedGeometryBudget, ValidatedInterval, base::*,
    bounding::validated_inverse_affine_point, validated_atan2, validated_sin_cos,
};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use std::sync::atomic::AtomicBool;

/// A complete borrowed native LINE remains affine under its original finite
/// trim/pose/reflection wrappers. This identifies an expression class only;
/// native enclosures and whole physical/tangent correspondence still own proof.
pub(super) fn native_affine_line_basis<'a>(
    mut curve: &'a monstertruck_modeling::Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<&'a Line<Point3>>, GeometryError> {
    use monstertruck_modeling::Curve;
    for depth in 0..=32 {
        work.charge(1, cancel)?;
        match curve {
            Curve::Line(line) => return Ok(Some(line)),
            Curve::TrimmedCurve(wrapper) if depth < 32 => {
                work.charge(16, cancel)?;
                let matrix = wrapper.transform();
                if (0..4).any(|col| (0..4).any(|row| !matrix[col][row].is_finite()))
                    || (0..3).any(|col| matrix[col][3] != 0.0)
                    || matrix[3][3] != 1.0
                {
                    return Ok(None);
                }
                curve = wrapper.entity().curve().as_ref();
            }
            Curve::TrimmedCurve(_) => {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "native affine LINE wrapper nesting exceeds source limit",
                ));
            }
            _ => return Ok(None),
        }
    }
    Ok(None)
}

pub(super) struct ChartWitness {
    pub curve: Curve2D,
    pub margin_uv: [f64; 2],
    pub error_mm: f64,
}

/// A one-sheet meridian graph, NOT a two-coordinate spherical inverse.
/// Its transverse/radial world tube stays separate from latitude uncertainty.
#[derive(Clone, Copy)]
pub(super) struct MeridianGraph {
    pub latitude_axis: usize,
    pub pole_endpoints: [bool; 2],
}

pub(super) struct MeridianWitness {
    pub chart: ChartWitness,
    pub graph: MeridianGraph,
}
fn p(x: f64) -> Interval {
    Interval::point(x)
}
fn adapt(x: ValidatedInterval) -> Interval {
    Interval {
        lo: x.lower,
        hi: x.upper,
    }
}
fn native(x: Interval) -> ValidatedInterval {
    ValidatedInterval {
        lower: x.lo,
        upper: x.hi,
    }
}
fn conic_phase_sin_cos(
    span: Interval,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<(Interval, Interval), GeometryError> {
    let mut enclose = |range| {
        validated_sin_cos(native(range), budget, cancel)
            .map(|(sine, cosine)| (adapt(sine), adapt(cosine)))
            .map_err(native_error)
    };
    let width = span.hi - span.lo;
    if !span.lo.is_finite() || !span.hi.is_finite() || width <= 0.5 {
        return enclose(span);
    }
    if width >= std::f64::consts::TAU.next_up() {
        budget.charge(1, cancel).map_err(native_error)?;
        let unit = Interval { lo: -1., hi: 1. };
        return Ok((unit, unit));
    }
    // The Native evaluator deliberately returns unit bounds for wide inputs.
    // Enclose a complete adjacent partition, never a set of sampled phases.
    // At most thirteen pieces are needed below one full period.
    let pieces = (width / 0.5).ceil() as usize;
    let cut = |index| {
        if index == pieces {
            span.hi
        } else {
            span.lo + width * (index as f64 / pieces as f64)
        }
    };
    let mut lower = cut(1);
    let (mut sine, mut cosine) = enclose(Interval {
        lo: span.lo,
        hi: lower,
    })?;
    for index in 1..pieces {
        let upper = cut(index + 1);
        let bounds = enclose(Interval {
            lo: lower,
            hi: upper,
        })?;
        sine = sine.hull(bounds.0);
        cosine = cosine.hull(bounds.1);
        lower = upper;
    }
    Ok((sine, cosine))
}
fn zero(x: Interval) -> bool {
    x.lo == 0. && x.hi == 0.
}
fn exact_add(a: Interval, b: Interval) -> Interval {
    if zero(a) {
        b
    } else if zero(b) {
        a
    } else {
        a.add(b)
    }
}
fn exact_mul(a: Interval, b: Interval) -> Interval {
    if zero(a) || zero(b) { p(0.) } else { a.mul(b) }
}
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(p(0.), |s, i| exact_add(s, exact_mul(a[i], b[i])))
}
fn sqrt_nonnegative(value: Interval) -> Result<Interval, GeometryError> {
    Interval {
        lo: value.lo.max(0.),
        hi: value.hi,
    }
    .sqrt()
}
fn norm(a: [Interval; 3]) -> Result<Interval, GeometryError> {
    sqrt_nonnegative(a.into_iter().fold(p(0.), |s, x| s.add(x.square())))
}
fn vector(v: Vector3) -> [Interval; 3] {
    [p(v.x), p(v.y), p(v.z)]
}
fn sub(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    std::array::from_fn(|i| if zero(b[i]) { a[i] } else { a[i].sub(b[i]) })
}
fn phase(
    y: Interval,
    x: Interval,
    reference: f64,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    budget.charge(1, cancel).map_err(native_error)?;
    if reference.is_finite()
        && reference.abs() <= 1_099_511_627_776.0
        && x.lo.is_finite()
        && x.hi.is_finite()
        && y.lo.is_finite()
        && y.hi.is_finite()
    {
        let (sine, cosine) =
            validated_sin_cos(native(p(reference)), budget, cancel).map_err(native_error)?;
        let (sine, cosine) = (adapt(sine), adapt(cosine));
        let forward = x.mul(cosine).add(y.mul(sine));
        if forward.lo > 0.0 {
            let transverse = y.mul(cosine).sub(x.mul(sine));
            let radius = transverse.div(forward)?.abs_upper();
            // Rotate the WHOLE vector rectangle, not sampled endpoints. Strict
            // positive X isolates the lift around reference in (-pi/2, pi/2).
            // For every enclosed vector, |atan(Y/X)| <= |Y/X|. The half-radian
            // threshold only selects this inexpensive enclosure; wider inputs
            // retain the existing corner-isolation proof and branch checks.
            if radius <= 0.5 {
                return Ok(p(reference).add(Interval {
                    lo: -radius,
                    hi: radius,
                }));
            }
        }
    }
    validated_atan2(native(y), native(x), reference, 1e-12, budget, cancel)
        .map(adapt)
        .map_err(native_error)
}
fn inv(
    matrix: Matrix4,
    value: [Interval; 3],
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    validated_inverse_affine_point(matrix, value.map(native), budget, cancel)
        .map(|v| v.map(adapt))
        .map_err(native_error)
}
fn linear_matrix(mut matrix: Matrix4) -> Matrix4 {
    matrix.w = Vector4::new(0., 0., 0., 1.);
    matrix
}

/// Coefficient-proved affine tensor, still the ORIGINAL finite native carrier.
/// The outward frame is not a rounded inverse and supplies no trim authority.
pub(super) struct AffineTensorFrame {
    pub origin: [Interval; 3],
    pub axes: [[Interval; 3]; 2],
    pub domain: [[f64; 2]; 2],
    pub arithmetic_scale: f64,
}

/// TwoSum is an error-free sum of finite binary64 operands, including gradual
/// underflow. Nonfinite intermediates refuse rather than assert an identity.
fn exact_pair_sum(a: f64, b: f64) -> Option<(f64, f64)> {
    let sum = a + b;
    let b_virtual = sum - a;
    let a_virtual = sum - b_virtual;
    let b_error = b - b_virtual;
    let a_error = a - a_virtual;
    let error = a_error + b_error;
    [sum, b_virtual, a_virtual, b_error, a_error, error]
        .into_iter()
        .all(f64::is_finite)
        .then_some((sum, error))
}

/// Exact equality of two binary64 products, without evaluating either rounded
/// product. Two 53-bit significands fit in u128 even when their real product
/// overflows/underflows binary64. Signed zeros denote the same exact zero.
pub(super) fn exact_products_equal(a: f64, b: f64, c: f64, d: f64) -> bool {
    fn factor(value: f64) -> Option<(bool, u64, i32)> {
        if !value.is_finite() {
            return None;
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32;
        let mantissa = bits & ((1_u64 << 52) - 1);
        Some((
            bits >> 63 != 0,
            if exponent == 0 {
                mantissa
            } else {
                mantissa | (1_u64 << 52)
            },
            if exponent == 0 {
                -1074
            } else {
                exponent - 1023 - 52
            },
        ))
    }
    fn product(a: f64, b: f64) -> Option<(bool, u128, i32)> {
        let (negative_a, a, exponent_a) = factor(a)?;
        let (negative_b, b, exponent_b) = factor(b)?;
        let product = u128::from(a) * u128::from(b);
        if product == 0 {
            return Some((false, 0, 0));
        }
        let zeros = product.trailing_zeros();
        Some((
            negative_a != negative_b,
            product >> zeros,
            exponent_a + exponent_b + zeros as i32,
        ))
    }
    match (product(a, b), product(c, d)) {
        (Some(first), Some(second)) => first == second,
        _ => false,
    }
}

/// A single clamped bilinear patch is affine exactly when all homogeneous
/// weights are the same nonzero value and every coordinate parallelogram
/// identity holds. All four original controls and both knot vectors participate.
/// Higher-degree/nonconstant-weight carriers retain their existing proof lanes.
pub(super) fn affine_tensor_frame(
    surface: &monstertruck_modeling::Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<AffineTensorFrame>, GeometryError> {
    work.charge(128, cancel)?;
    let (knots, controls) = match surface {
        monstertruck_modeling::Surface::BsplineSurface(surface) => {
            let controls = surface.control_points();
            if controls.len() != 2 || controls.iter().any(|row| row.len() != 2) {
                return Ok(None);
            }
            let points = std::array::from_fn::<_, 2, _>(|i| {
                std::array::from_fn::<_, 2, _>(|j| {
                    let point = controls[i][j];
                    [point.x, point.y, point.z, 1.0]
                })
            });
            (surface.knot_vectors(), points)
        }
        monstertruck_modeling::Surface::NurbsSurface(surface) => {
            let controls = surface.control_points();
            if controls.len() != 2 || controls.iter().any(|row| row.len() != 2) {
                return Ok(None);
            }
            let points = std::array::from_fn::<_, 2, _>(|i| {
                std::array::from_fn::<_, 2, _>(|j| {
                    let point = controls[i][j];
                    [point.x, point.y, point.z, point.w]
                })
            });
            (surface.knot_vectors(), points)
        }
        _ => return Ok(None),
    };
    let mut domain = [[0.0; 2]; 2];
    for (axis, knots) in [&knots.0, &knots.1].into_iter().enumerate() {
        if knots.len() != 4
            || !knots.iter().all(|value| value.is_finite())
            || knots[0] != knots[1]
            || knots[2] != knots[3]
            || knots[0] >= knots[3]
        {
            return Ok(None);
        }
        domain[axis] = [knots[0], knots[3]];
    }
    let weight = controls[0][0][3];
    if !weight.is_finite()
        || weight == 0.0
        || controls.iter().flatten().any(|point| {
            point[3] != weight || !point.iter().all(|coordinate| coordinate.is_finite())
        })
    {
        return Ok(None);
    }
    for (axis, coordinate) in controls[0][0].iter().take(3).enumerate() {
        let Some(first) = exact_pair_sum(*coordinate, controls[1][1][axis]) else {
            return Ok(None);
        };
        if Some(first) != exact_pair_sum(controls[1][0][axis], controls[0][1][axis]) {
            return Ok(None);
        }
    }
    let corner: [Interval; 3] = std::array::from_fn(|axis| p(controls[0][0][axis]));
    let corner = [
        corner[0].div(p(weight))?,
        corner[1].div(p(weight))?,
        corner[2].div(p(weight))?,
    ];
    let mut axes = [[p(0.0); 3]; 2];
    let mut condition = p(1.0);
    for axis in 0..2 {
        let span = p(domain[axis][1]).sub(p(domain[axis][0]));
        let last = if axis == 0 {
            controls[1][0]
        } else {
            controls[0][1]
        };
        for coordinate in 0..3 {
            axes[axis][coordinate] = p(last[coordinate])
                .sub(p(controls[0][0][coordinate]))
                .div(p(weight))?
                .div(span)?;
        }
        condition = condition.mul(
            p(1.0)
                .add(p(domain[axis][0].abs()))
                .add(p(domain[axis][1].abs()))
                .div(span)?,
        );
    }
    let origin = std::array::from_fn(|coordinate| {
        corner[coordinate]
            .sub(axes[0][coordinate].mul(p(domain[0][0])))
            .sub(axes[1][coordinate].mul(p(domain[1][0])))
    });
    let maximum = controls
        .iter()
        .flatten()
        .flat_map(|point| point[..3].iter())
        .map(|coordinate| coordinate.abs())
        .fold(0.0_f64, f64::max);
    let arithmetic_scale = p(maximum).div(p(weight.abs()))?.mul(condition).hi;
    if !arithmetic_scale.is_finite() {
        return Ok(None);
    }
    Ok(Some(AffineTensorFrame {
        origin,
        axes,
        domain,
        arithmetic_scale,
    }))
}

/// Full-ellipse support from the ACTUAL stored affine conic coefficients, not
/// angular samples. The source parameter interval and every phase stay intact.
pub(super) fn conic_support(matrix: &Matrix3) -> Result<[Interval; 2], GeometryError> {
    let mut bounds = [p(0.0); 2];
    for axis in 0..2 {
        let radius =
            sqrt_nonnegative(p(matrix[0][axis]).square().add(p(matrix[1][axis]).square()))?;
        bounds[axis] = p(matrix[2][axis]).add(Interval {
            lo: -radius.hi,
            hi: radius.hi,
        });
    }
    Ok(bounds)
}

struct Coefficients {
    c: [Interval; 3],
    u: [Interval; 3],
    v: [Interval; 3],
    start: [Interval; 3],
    end: [Interval; 3],
    position: [Interval; 3],
    range: (f64, f64),
    circle: bool,
    orientation: f64,
    harmonics: [(Interval, Interval); 2],
}
fn coefficients(
    curve: &Curve3D,
    matrix: Matrix4,
    origin: Point3,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Option<Coefficients>, GeometryError> {
    let Some(source) = super::source_conic::capture(curve, budget, cancel)? else {
        return Ok(None);
    };
    let origin = vector(origin.to_vec());
    let c = sub(inv(matrix, source.c, budget, cancel)?, origin);
    let u = inv(linear_matrix(matrix), source.u, budget, cancel)?;
    let v = inv(linear_matrix(matrix), source.v, budget, cancel)?;
    let position =
        |range, budget: &mut ValidatedGeometryBudget| -> Result<[Interval; 3], GeometryError> {
            Ok(sub(
                inv(
                    matrix,
                    source.jet(range, budget, cancel)?.position,
                    budget,
                    cancel,
                )?,
                origin,
            ))
        };
    let range = source.range;
    let harmonics = if source.circle {
        let parameter = Interval::new(range.0, range.1)?;
        let phase = source.phase_offset.add(if source.orientation > 0.0 {
            parameter
        } else {
            parameter.neg()
        });
        [
            conic_phase_sin_cos(phase, budget, cancel)?,
            conic_phase_sin_cos(phase.mul(p(2.)), budget, cancel)?,
        ]
    } else {
        let unit = Interval { lo: -1., hi: 1. };
        [(unit, unit); 2]
    };
    let start = position([range.0, range.0], budget)?;
    let end = position([range.1, range.1], budget)?;
    let position = position([range.0, range.1], budget)?;
    Ok(Some(Coefficients {
        c,
        u,
        v,
        start,
        end,
        position,
        range,
        circle: source.circle,
        orientation: source.orientation,
        harmonics,
    }))
}
fn projected(c: &Coefficients, x: [Interval; 3], y: [Interval; 3]) -> [[Interval; 2]; 3] {
    [c.c, c.u, c.v].map(|a| [dot(a, x), dot(a, y)])
}
fn squared_radius(coeff: [[Interval; 2]; 3], harmonics: &[(Interval, Interval); 2]) -> Interval {
    let [c, u, v] = coeff;
    let [(sine, cosine), (double_sine, double_cosine)] = *harmonics;
    let dot2 = |a: [Interval; 2], b: [Interval; 2]| a[0].mul(b[0]).add(a[1].mul(b[1]));
    // Preserve the actual retained phase interval in the correlated quadratic.
    // A complete-circle hull can touch the axis even when this original arc
    // remains separated from it; independent Cartesian boxes lose that proof.
    dot2(c, c)
        .add(dot2(u, u).add(dot2(v, v)).mul(p(0.5)))
        .add(cosine.mul(dot2(c, u).mul(p(2.))))
        .add(sine.mul(dot2(c, v).mul(p(2.))))
        .add(double_cosine.mul(dot2(u, u).sub(dot2(v, v)).mul(p(0.5))))
        .add(double_sine.mul(dot2(u, v)))
}
fn squared_norm(c: &Coefficients) -> Interval {
    let unit = Interval { lo: -1., hi: 1. };
    dot(c.c, c.c)
        .add(dot(c.u, c.u).add(dot(c.v, c.v)).mul(p(0.5)))
        .add(unit.mul(dot(c.c, c.u).mul(p(2.))))
        .add(unit.mul(dot(c.c, c.v).mul(p(2.))))
        .add(unit.mul(dot(c.u, c.u).sub(dot(c.v, c.v)).mul(p(0.5))))
        .add(unit.mul(dot(c.u, c.v)))
}
fn angular_derivative(
    c: &Coefficients,
    x: [Interval; 3],
    y: [Interval; 3],
) -> Result<Interval, GeometryError> {
    let [a, u, v] = projected(c, x, y);
    let det = |a: [Interval; 2], b: [Interval; 2]| {
        exact_add(exact_mul(a[0], b[1]), exact_mul(a[1], b[0]).neg())
    };
    let numerator = if c.circle {
        let unit = Interval { lo: -1., hi: 1. };
        exact_add(
            exact_add(det(u, v), exact_mul(unit, det(a, v))),
            exact_mul(unit, det(a, u)).neg(),
        )
        .mul(p(c.orientation))
    } else {
        det(a, u).div(p(c.range.1).sub(p(c.range.0)))?
    };
    // A true axial endpoint may carry a regular limiting generator chart.
    if numerator.lo == 0. && numerator.hi == 0. {
        return Ok(p(0.));
    }
    let px = dot(c.position, x);
    let py = dot(c.position, y);
    let mut denominator = px.square().add(py.square());
    if c.circle {
        let gram = squared_radius([a, u, v], &c.harmonics);
        if gram.lo > denominator.lo {
            denominator.lo = gram.lo;
        }
        if gram.hi < denominator.hi {
            denominator.hi = gram.hi;
        }
    }
    numerator.div(denominator).map_err(|mut error| {
        error.message = format!(
            "analytic inverse angular derivative over {:?}, circle {}, squared projected radius {:?}, projected coefficients {:?}, harmonics {:?}: {}",
            c.range, c.circle, denominator, [a, u, v], c.harmonics, error.message
        );
        error
    })
}
fn angular_margin(
    c: &Coefficients,
    x: [Interval; 3],
    y: [Interval; 3],
    start: f64,
    end: f64,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let at_end = dot(c.start, x).contains_zero() && dot(c.start, y).contains_zero();
    let endpoint = if at_end { c.end } else { c.start };
    let reference = if at_end { end } else { start };
    let angle = phase(
        dot(endpoint, y),
        dot(endpoint, x),
        reference,
        budget,
        cancel,
    )?;
    let width = p(c.range.1).sub(p(c.range.0));
    let expected = p(end).sub(p(start)).div(width)?;
    let derivative = angular_derivative(c, x, y)?;
    Ok(p(angle.sub(p(reference)).abs_upper())
        .add(p(derivative.sub(expected).abs_upper()).mul(width))
        .hi)
}
fn direction(
    angle: f64,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<([Interval; 3], f64), GeometryError> {
    let raw = vector(Vector3::new(angle.cos(), angle.sin(), 0.));
    let n = norm(raw)?;
    let normalized = [raw[0].div(n)?, raw[1].div(n)?, raw[2].div(n)?];
    let actual = phase(raw[1], raw[0], angle, budget, cancel)?;
    Ok((normalized, actual.sub(p(angle)).abs_upper()))
}
fn constant_angle(
    c: &Coefficients,
    x: [Interval; 3],
    y: [Interval; 3],
    reference: f64,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let coeff = projected(c, x, y);
    let rho2 = if c.circle {
        squared_radius(coeff, &c.harmonics)
    } else {
        dot(c.position, x).square().add(dot(c.position, y).square())
    };
    let rho = sqrt_nonnegative(rho2)?;
    let axis = vector(Vector3::unit_z());
    let z = if c.circle {
        let cz = dot(c.c, axis);
        let uz = dot(c.u, axis).abs_upper();
        let vz = dot(c.v, axis).abs_upper();
        cz.add(Interval {
            lo: -up(uz + vz),
            hi: up(uz + vz),
        })
    } else {
        dot(c.position, axis)
    };
    Ok(phase(z, rho, reference, budget, cancel)?
        .sub(p(reference))
        .abs_upper())
}
fn carrier_transform_error(matrix: Matrix4, residual: Interval, scale: f64) -> f64 {
    let rows = [
        [matrix.x.x, matrix.y.x, matrix.z.x],
        [matrix.x.y, matrix.y.y, matrix.z.y],
        [matrix.x.z, matrix.y.z, matrix.z.z],
    ];
    let n = sqrt_nonnegative(rows.into_iter().fold(p(0.), |sum, row| {
        sum.add(row.into_iter().fold(p(0.), |s, v| s.add(p(v).square())))
    }));
    n.map(|n| n.mul(residual).mul(p(scale)).hi)
        .unwrap_or(f64::INFINITY)
}

fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    [
        a[1].mul(b[2]).sub(a[2].mul(b[1])),
        a[2].mul(b[0]).sub(a[0].mul(b[2])),
        a[0].mul(b[1]).sub(a[1].mul(b[0])),
    ]
}
fn normalized(value: [Interval; 3]) -> Result<[Interval; 3], GeometryError> {
    let length = norm(value)?;
    Ok([
        value[0].div(length)?,
        value[1].div(length)?,
        value[2].div(length)?,
    ])
}
fn scalar_coefficients(c: &Coefficients, axis: [Interval; 3]) -> [Interval; 3] {
    [dot(c.c, axis), dot(c.u, axis), dot(c.v, axis)]
}
fn scalar_range(c: &Coefficients, coeff: [Interval; 3]) -> Interval {
    if c.circle {
        let reserve = up(coeff[1].abs_upper() + coeff[2].abs_upper());
        coeff[0].add(Interval {
            lo: -reserve,
            hi: reserve,
        })
    } else {
        coeff[0].hull(coeff[0].add(coeff[1]))
    }
}
fn profile_margin(c: &Coefficients, coordinate: [Interval; 3], start: f64, end: f64) -> f64 {
    if c.circle {
        scalar_range(c, coordinate).sub(p(start)).abs_upper()
    } else {
        coordinate[0]
            .sub(p(start))
            .abs_upper()
            .max(coordinate[0].add(coordinate[1]).sub(p(end)).abs_upper())
    }
}

/// Fold the *difference* of squared radial/profile coefficients before the
/// harmonic or polynomial enclosure. Independent radius hull subtraction would
/// lose the common varying cone-generator radius over its complete interval.
fn radial_squared_residual(c: &Coefficients, q: [[Interval; 2]; 3], r: [Interval; 3]) -> Interval {
    let dot2 = |a: [Interval; 2], b: [Interval; 2]| {
        exact_add(exact_mul(a[0], b[0]), exact_mul(a[1], b[1]))
    };
    let pair = |i: usize, j: usize| dot2(q[i], q[j]).sub(r[i].mul(r[j]));
    if c.circle {
        let unit = Interval { lo: -1., hi: 1. };
        pair(0, 0)
            .add(pair(1, 1).add(pair(2, 2)).mul(p(0.5)))
            .add(unit.mul(pair(0, 1).mul(p(2.))))
            .add(unit.mul(pair(0, 2).mul(p(2.))))
            .add(unit.mul(pair(1, 1).sub(pair(2, 2)).mul(p(0.5))))
            .add(unit.mul(pair(1, 2)))
    } else {
        let parameter = Interval { lo: 0., hi: 1. };
        pair(0, 0)
            .add(parameter.mul(pair(0, 1).mul(p(2.))))
            .add(parameter.square().mul(pair(1, 1)))
    }
}

/// Concrete rings/generators on a line-profile source revolution, including
/// cylindrical and conical STEP carriers. The frame is an outward enclosure of
/// an ideal orthonormal frame; the stored almost-unit Rodrigues axis and any
/// transverse profile component retain their own physical residual reserve.
struct RevolutionFrame<'a> {
    profile: &'a Line<Point3>,
    origin: Point3,
    axis: Vector3,
}

fn revolution_witness(
    c: &Coefficients,
    frame: RevolutionFrame<'_>,
    a: [f64; 2],
    b: [f64; 2],
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Option<([f64; 2], Interval)>, GeometryError> {
    let RevolutionFrame {
        profile,
        origin,
        axis,
    } = frame;
    budget.charge(1, cancel).map_err(native_error)?;
    if (c.circle && a[0] != b[0]) || (!c.circle && a[1] != b[1]) {
        return Ok(None);
    }
    let raw_axis = vector(axis);
    let k = normalized(raw_axis)?;
    let base = sub(vector(profile.0.to_vec()), vector(origin.to_vec()));
    let delta = sub(vector(profile.1.to_vec()), vector(profile.0.to_vec()));
    let z0 = dot(base, k);
    let radial = sub(base, std::array::from_fn(|i| z0.mul(k[i])));
    let r0 = norm(radial)?;
    let x = normalized(radial)?;
    let y = cross(k, x);
    let dz = dot(delta, k);
    let dr = dot(delta, x);
    let transverse = dot(delta, y);
    let q = projected(c, x, y);
    let rho2 = if c.circle {
        squared_radius(q, &c.harmonics)
    } else {
        dot(c.position, x).square().add(dot(c.position, y).square())
    };
    let rho = sqrt_nonnegative(rho2)?;
    if rho.lo <= 0. {
        return Err(error(
            GeometryErrorCode::SingularGeometry,
            "source revolution trim reaches an unresolved axis chart",
        ));
    }
    let z = scalar_coefficients(c, k);
    let angular = angular_margin(c, x, y, a[1], b[1], budget, cancel)?;

    // Pick a nonzero profile-coordinate derivative, not a family-specific
    // formula or a near-equality assertion. All source coefficients participate.
    let axial = !dz.contains_zero() && (dr.contains_zero() || dz.abs_upper() >= dr.abs_upper());
    let (parameter_range, axial_error, radial_error, margin) = if axial {
        let parameter = [z[0].sub(z0).div(dz)?, z[1].div(dz)?, z[2].div(dz)?];
        let range = scalar_range(c, parameter);
        let radius = [
            r0.add(dr.mul(parameter[0])),
            dr.mul(parameter[1]),
            dr.mul(parameter[2]),
        ];
        let expected = scalar_range(c, radius);
        if expected.lo <= 0. {
            return Err(error(
                GeometryErrorCode::SingularGeometry,
                "source revolution profile radius crosses its axis",
            ));
        }
        let residual = radial_squared_residual(c, q, radius).div(rho.add(expected))?;
        (
            range,
            p(0.),
            residual,
            profile_margin(c, parameter, a[0], b[0]),
        )
    } else {
        if dr.contains_zero() {
            return Err(error(
                GeometryErrorCode::SingularGeometry,
                "source revolution line profile is collapsed",
            ));
        }
        if c.circle {
            let range = rho.sub(r0).div(dr)?;
            let axial_error = scalar_range(c, z).sub(z0).sub(dz.mul(range));
            // Here radius equality is the defining algebraic coordinate, not
            // cancellation of independent interval evaluations.
            (range, axial_error, p(0.), range.sub(p(a[0])).abs_upper())
        } else {
            let (direction, _) = direction(a[1], budget, cancel)?;
            let radial_axis =
                std::array::from_fn(|i| direction[0].mul(x[i]).add(direction[1].mul(y[i])));
            let projection = scalar_coefficients(c, radial_axis);
            let parameter = [
                projection[0].sub(r0).div(dr)?,
                projection[1].div(dr)?,
                p(0.),
            ];
            let range = scalar_range(c, parameter);
            let radius = [r0.add(dr.mul(parameter[0])), dr.mul(parameter[1]), p(0.)];
            let expected = scalar_range(c, radius);
            if expected.lo <= 0. {
                return Err(error(
                    GeometryErrorCode::SingularGeometry,
                    "source revolution generator crosses its axis",
                ));
            }
            let residual = radial_squared_residual(c, q, radius).div(rho.add(expected))?;
            let axial_coeff = [
                z[0].sub(z0).sub(dz.mul(parameter[0])),
                z[1].sub(dz.mul(parameter[1])),
                p(0.),
            ];
            (
                range,
                scalar_range(c, axial_coeff),
                residual,
                profile_margin(c, parameter, a[0], b[0]),
            )
        }
    };
    // Relate the ideal meridian carrier to the actual stored source profile.
    // For n = stored axis and k = n/|n|, Rodrigues gives
    // ||R_n-R_k|| <= ||n-k|| (1 + 2 (|n|+1)), uniformly in the angle.
    let axis_defect = norm(sub(raw_axis, k))?;
    let profile_norm = norm(base)?.add(norm(delta)?.mul(p(parameter_range.abs_upper())));
    let rotation_error = axis_defect
        .mul(p(1.).add(norm(raw_axis)?.add(p(1.)).mul(p(2.))))
        .mul(profile_norm);
    let profile_error = p(transverse.abs_upper()).mul(p(parameter_range.abs_upper()));
    let residual = norm([radial_error, axial_error, p(0.)])?
        .add(profile_error)
        .add(rotation_error);
    Ok(Some(([margin, angular], residual)))
}

struct AffineChart {
    start: Point2,
    end: Point2,
    error: [f64; 2],
}

/// A retained LINE or single clamped degree-one polynomial span is affine on
/// its actual own range. Multiple degree-one spans need not be collinear or
/// equally parameterized and deliberately remain on the general proof path.
/// Native jets check complete support through every original pose/reflection.
/// Endpoint boxes and the whole-interval second derivative enclose the chord.
fn affine_chart(
    chart: &Curve2D,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Option<AffineChart>, GeometryError> {
    let mut basis = chart;
    for depth in 0..=32 {
        budget.charge(1, cancel).map_err(native_error)?;
        match basis {
            Curve2D::TrimmedCurve(source) if depth < 32 => {
                let matrix = source.transform();
                if (0..3).any(|col| (0..3).any(|row| !matrix[col][row].is_finite()))
                    || matrix.x.z != 0.0
                    || matrix.y.z != 0.0
                    || matrix.z.z != 1.0
                {
                    return Ok(None);
                }
                basis = source.entity().curve().as_ref();
            }
            Curve2D::TrimmedCurve(_) => {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "source affine UV wrapper nesting exceeds cap",
                ));
            }
            Curve2D::Line(_) => break,
            Curve2D::BsplineCurve(source)
                if source.control_points().len() == 2
                    && source.knot_vector().len() == 4
                    && source.knot_vector()[0] == source.knot_vector()[1]
                    && source.knot_vector()[2] == source.knot_vector()[3]
                    && source.knot_vector()[1] < source.knot_vector()[2] =>
            {
                break;
            }
            _ => return Ok(None),
        }
    }
    let Some((a, b)) = chart.try_range_tuple() else {
        return Ok(None);
    };
    let domain = Interval::new(a, b)?;
    if a == b {
        return Ok(None);
    }
    let start = chart.subs(a);
    let end = chart.subs(b);
    if [start.x, start.y, end.x, end.y]
        .into_iter()
        .any(|value| !value.is_finite())
    {
        return Ok(None);
    }
    let whole = monstertruck_modeling::validated_curve2d_jet(chart, [a, b], budget, cancel)
        .map_err(native_error)?;
    let first = monstertruck_modeling::validated_curve2d_jet(chart, [a, a], budget, cancel)
        .map_err(native_error)?;
    let last = monstertruck_modeling::validated_curve2d_jet(chart, [b, b], budget, cancel)
        .map_err(native_error)?;
    let reserve = std::array::from_fn(|axis| {
        let endpoint = adapt(first.position[axis])
            .sub(p(start[axis]))
            .abs_upper()
            .max(adapt(last.position[axis]).sub(p(end[axis])).abs_upper());
        p(endpoint)
            .add(
                p(adapt(whole.second[axis]).abs_upper())
                    .mul(p(domain.width()).square())
                    .mul(p(0.125)),
            )
            .hi
    });
    if reserve.into_iter().any(|bound| !bound.is_finite()) {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "source affine UV interval remainder is not finite",
        ));
    }
    Ok(Some(AffineChart {
        start,
        end,
        error: reserve,
    }))
}

/// Endpoint proposals of a structurally affine original declaration. These
/// rounded values are not identities: callers must prove each endpoint
/// coefficient exactly before using them to bind an independently gauged row.
pub(super) fn affine_endpoint_proposals(
    chart: &Curve2D,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Point2; 2]>, GeometryError> {
    let mut budget =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = affine_chart(chart, &mut budget, cancel)
        .map(|proposal| proposal.map(|chart| [chart.start, chart.end]));
    crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
    match result {
        Err(cause)
            if !matches!(
                cause.code,
                GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
            ) =>
        {
            Ok(None)
        }
        result => result,
    }
}

/// Certifies a structural candidate, rather than granting it authority merely
/// because its endpoints appear near the source BREP.
pub(super) fn certify(
    curve: &Curve3D,
    surface: &Surface,
    mut chart: Curve2D,
    map: SurfaceParameterMap,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ChartWitness>, GeometryError> {
    Ok(
        certify_in_place(curve, surface, &mut chart, map, scale, work, cancel)?.map(
            |(margin_uv, error_mm)| ChartWitness {
                curve: chart,
                margin_uv,
                error_mm,
            },
        ),
    )
}

/// The identity-map retention path needs only the outward tube, not a copied
/// UV expression. Nonidentity callers also retain the mapped expression here.
pub(super) fn certify_in_place(
    curve: &Curve3D,
    surface: &Surface,
    chart: &mut Curve2D,
    map: SurfaceParameterMap,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<([f64; 2], f64)>, GeometryError> {
    let mut budget =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = (|| {
        let Some(candidate) = affine_chart(chart, &mut budget, cancel)? else {
            return Ok(None);
        };
        let old = [candidate.start, candidate.end];
        let (mut a, mut b) = ([old[0].x, old[0].y], [old[1].x, old[1].y]);
        let (matrix, origin, swapped) = match surface {
            Surface::ElementarySurface(ElementarySurface::Sphere(s)) => {
                (*s.transform(), s.entity().0.center(), !s.orientation())
            }
            Surface::ElementarySurface(ElementarySurface::ToroidalSurface(s)) => {
                (*s.transform(), s.entity().center(), !s.orientation())
            }
            Surface::ElementarySurface(
                ElementarySurface::CylindricalSurface(s) | ElementarySurface::ConicalSurface(s),
            ) => (*s.transform(), s.entity().origin(), !s.orientation()),
            Surface::SweepSurface(SweepSurface::RevolutionSurface(s)) => {
                (*s.transform(), s.entity().origin(), !s.orientation())
            }
            _ => return Ok(None),
        };
        if swapped {
            a.swap(0, 1);
            b.swap(0, 1);
        }
        let Some(c) = coefficients(curve, matrix, origin, &mut budget, cancel)? else {
            return Ok(None);
        };
        let x = vector(Vector3::unit_x());
        let y = vector(Vector3::unit_y());
        let z = vector(Vector3::unit_z());
        let mut margin = [0.; 2];
        let residual = match surface {
            Surface::ElementarySurface(ElementarySurface::Sphere(s)) => {
                margin[0] = angular_margin(&c, x, y, a[0], b[0], &mut budget, cancel)?;
                if a[1] == b[1] {
                    margin[1] = constant_angle(&c, x, y, a[1], &mut budget, cancel)?;
                } else {
                    let (radial, dir_error) = direction(a[0], &mut budget, cancel)?;
                    margin[1] = angular_margin(&c, radial, z, a[1], b[1], &mut budget, cancel)?;
                    let longitude_error = up(margin[0] + dir_error);
                    if longitude_error >= 0.5 {
                        return Err(error(
                            GeometryErrorCode::ConditioningFailure,
                            "sphere meridian inverse chart is unresolved",
                        ));
                    }
                    margin[1] = up(margin[1] + longitude_error);
                }
                // The source latitude wrapper uses binary64 pi/2. Its ideal
                // coordinate differs from mathematical latitude by this enclosure.
                margin[1] = up(margin[1] + 2.0 * f64::EPSILON * std::f64::consts::FRAC_PI_2);
                sqrt_nonnegative(squared_norm(&c))?.sub(p(s.entity().0.radius()))
            }
            Surface::ElementarySurface(ElementarySurface::ToroidalSurface(s)) => {
                margin[0] = angular_margin(&c, x, y, a[0], b[0], &mut budget, cancel)?;
                let major = s.entity().large_radius();
                if a[1] == b[1] {
                    let rho = sqrt_nonnegative(squared_radius(projected(&c, x, y), &c.harmonics))?;
                    let zz = dot(c.c, z).add(Interval {
                        lo: -up(dot(c.u, z).abs_upper() + dot(c.v, z).abs_upper()),
                        hi: up(dot(c.u, z).abs_upper() + dot(c.v, z).abs_upper()),
                    });
                    margin[1] = phase(zz, rho.sub(p(major)), a[1], &mut budget, cancel)?
                        .sub(p(a[1]))
                        .abs_upper();
                    sqrt_nonnegative(rho.sub(p(major)).square().add(zz.square()))?
                        .sub(p(s.entity().small_radius()))
                } else {
                    let (radial, dir_error) = direction(a[0], &mut budget, cancel)?;
                    let mut shifted = c;
                    let shift = std::array::from_fn(|i| radial[i].mul(p(major)));
                    shifted.c = sub(shifted.c, shift);
                    shifted.start = sub(shifted.start, shift);
                    shifted.end = sub(shifted.end, shift);
                    shifted.position = sub(shifted.position, shift);
                    margin[1] =
                        angular_margin(&shifted, radial, z, a[1], b[1], &mut budget, cancel)?;
                    let radial_error = up(margin[0] + dir_error);
                    if radial_error >= 0.5 {
                        return Err(error(
                            GeometryErrorCode::ConditioningFailure,
                            "torus meridian inverse chart is unresolved",
                        ));
                    }
                    let displacement = p(major)
                        .add(p(s.entity().small_radius()))
                        .mul(p(radial_error).square())
                        .mul(p(0.5));
                    let tube = sqrt_nonnegative(squared_radius(
                        projected(&shifted, radial, z),
                        &shifted.harmonics,
                    ))?;
                    margin[1] = p(margin[1])
                        .add(
                            displacement
                                .div(p(tube.lo).sub(p(displacement.hi)).hull(p(tube.hi)))?,
                        )
                        .hi;
                    tube.sub(p(s.entity().small_radius())).add(Interval {
                        lo: -displacement.hi,
                        hi: displacement.hi,
                    })
                }
            }
            Surface::ElementarySurface(
                ElementarySurface::CylindricalSurface(s) | ElementarySurface::ConicalSurface(s),
            ) => {
                let Some((tube, residual)) = revolution_witness(
                    &c,
                    RevolutionFrame {
                        profile: s.entity().entity_curve(),
                        origin: s.entity().origin(),
                        axis: s.entity().axis(),
                    },
                    a,
                    b,
                    &mut budget,
                    cancel,
                )?
                else {
                    return Ok(None);
                };
                margin = tube;
                residual
            }
            Surface::SweepSurface(SweepSurface::RevolutionSurface(s)) => {
                let Curve3D::Line(profile) = s.entity().entity_curve().as_ref() else {
                    return Ok(None);
                };
                let Some((tube, residual)) = revolution_witness(
                    &c,
                    RevolutionFrame {
                        profile,
                        origin: s.entity().origin(),
                        axis: s.entity().axis(),
                    },
                    a,
                    b,
                    &mut budget,
                    cancel,
                )?
                else {
                    return Ok(None);
                };
                margin = tube;
                residual
            }
            _ => return Ok(None),
        };
        let error_mm = carrier_transform_error(matrix, p(residual.abs_upper()), scale);
        if !error_mm.is_finite() || error_mm > 1e-7 {
            return Err(invalid(
                "source analytic trim coefficients disagree with the complete native carrier",
            ));
        }
        if swapped {
            margin.swap(0, 1);
        }
        Ok(
            apply_chart_map(chart, old, map, margin, &mut budget, cancel)?
                .map(|margin| (margin, error_mm)),
        )
    })();
    crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
    result
}

fn apply_chart_map(
    chart: &mut Curve2D,
    old: [Point2; 2],
    map: SurfaceParameterMap,
    margin: [f64; 2],
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Option<[f64; 2]>, GeometryError> {
    let affine = map.affine_transform().ok_or_else(|| {
        error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "source chart requires an unavailable nonlinear composition",
        )
    })?;
    let ideal = old.map(|q| {
        [
            p(affine.x.x)
                .mul(p(q.x))
                .add(p(affine.y.x).mul(p(q.y)))
                .add(p(affine.z.x)),
            p(affine.x.y)
                .mul(p(q.x))
                .add(p(affine.y.y).mul(p(q.y)))
                .add(p(affine.z.y)),
        ]
    });
    if !matches!(map, SurfaceParameterMap::Identity) {
        chart.transform_by(affine);
    }
    let Some(native) = affine_chart(chart, budget, cancel)? else {
        return Ok(None);
    };
    let endpoints = [native.start, native.end];
    let margin = std::array::from_fn(|axis| {
        let coefficients = [affine[0][axis], affine[1][axis]];
        p(coefficients[0].abs())
            .mul(p(margin[0]))
            .add(p(coefficients[1].abs()).mul(p(margin[1])))
            .add(p((0..2)
                .map(|i| ideal[i][axis].sub(p(endpoints[i][axis])).abs_upper())
                .fold(0., f64::max)))
            .add(p(native.error[axis]))
            .hi
    });
    if margin
        .into_iter()
        .any(|bound| !bound.is_finite() || bound < 0.)
    {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "source analytic chart tube is not finite",
        ));
    }
    Ok(Some(margin))
}

/// Original C(t) is a graph over alpha=atan2(C.z,C.r), with strictly positive
/// projected radius and signed phase derivative on its COMPLETE original span.
/// There is deliberately no division by the vanishing XY radius. The analytic
/// sphere ray (cos(lambda),sin(lambda),0) is unit by the sphere formula, not by
/// assuming any stored processor/frame is orthonormal or changing coefficients.
/// A pending caller must still prove original/native relative tangents and both
/// actual source-vertex joint images, requiring Native pole authority at poles.
pub(super) fn certify_pole_meridian(
    curve: &Curve3D,
    surface: &Surface,
    mut chart: Curve2D,
    map: SurfaceParameterMap,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<MeridianWitness>, GeometryError> {
    let mut budget =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = (|| {
        let Surface::ElementarySurface(ElementarySurface::Sphere(sphere)) = surface else {
            return Ok(None);
        };
        let Some(candidate) = affine_chart(&chart, &mut budget, cancel)? else {
            return Ok(None);
        };
        let old = [candidate.start, candidate.end];
        let source_latitude = usize::from(sphere.orientation());
        let longitude = 1 - source_latitude;
        let a = old[0][source_latitude];
        let b = old[1][source_latitude];
        let half_pi = std::f64::consts::FRAC_PI_2;
        let poles = [a, b].map(|latitude| latitude == -half_pi || latitude == half_pi);
        if old[0][longitude] != old[1][longitude]
            || a == b
            || a.min(b) < -half_pi
            || a.max(b) > half_pi
            || !poles.into_iter().any(|pole| pole)
        {
            return Ok(None);
        }
        let Some(c) = coefficients(
            curve,
            *sphere.transform(),
            sphere.entity().0.center(),
            &mut budget,
            cancel,
        )?
        else {
            return Ok(None);
        };
        if !c.circle {
            return Ok(None);
        }
        let (sine, cosine) = validated_sin_cos(native(p(old[0][longitude])), &mut budget, cancel)
            .map_err(native_error)?;
        let radial = [adapt(cosine), adapt(sine), p(0.)];
        let transverse = [adapt(sine).neg(), adapt(cosine), p(0.)];
        let z = vector(Vector3::unit_z());
        let square = squared_radius(projected(&c, radial, z), &c.harmonics);
        if square.lo <= 0. {
            return Ok(None);
        }
        let derivative = angular_derivative(&c, radial, z)?;
        if (a < b && derivative.lo <= 0.) || (b < a && derivative.hi >= 0.) {
            return Ok(None);
        }
        let latitude = angular_margin(&c, radial, z, a, b, &mut budget, cancel)?;
        // Isolate one signed-radial sheet. By monotonicity, any pole overrun
        // admitted by the endpoint phase tube lies at that designated endpoint,
        // not at an interior pole hidden by another longitude or wrapped phase.
        if latitude >= 0.5 {
            return Ok(None);
        }
        let radius = sphere.entity().0.radius();
        if !radius.is_finite() || radius <= 0. {
            return Ok(None);
        }
        let radial_error = square.sqrt()?.sub(p(radius)).abs_upper();
        let (phase_sine, phase_cosine) = c.harmonics[0];
        let normal_error = dot(c.c, transverse)
            .add(dot(c.u, transverse).mul(phase_cosine))
            .add(dot(c.v, transverse).mul(phase_sine))
            .abs_upper();
        let local_error = p(radial_error)
            .square()
            .add(p(normal_error).square())
            .sqrt()?;
        // The ORIGINAL affine processor may shear/scale/reflect: its full
        // outward operator bound transports the true inverse-frame residual.
        let error_mm = carrier_transform_error(*sphere.transform(), local_error, scale);
        if !error_mm.is_finite() || error_mm > 1e-7 {
            return Ok(None);
        }
        let mut margin = [0.; 2];
        margin[source_latitude] = up(latitude + 2. * f64::EPSILON * half_pi);
        let affine = map.affine_transform().ok_or_else(|| {
            error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "meridian chart map is nonlinear",
            )
        })?;
        let Some(latitude_axis) = (0..2)
            .find(|&axis| affine[source_latitude][axis] != 0. && affine[longitude][axis] == 0.)
        else {
            return Ok(None);
        };
        let Some(margin_uv) = apply_chart_map(&mut chart, old, map, margin, &mut budget, cancel)?
        else {
            return Ok(None);
        };
        Ok(Some(MeridianWitness {
            chart: ChartWitness {
                curve: chart,
                margin_uv,
                error_mm,
            },
            graph: MeridianGraph {
                latitude_axis,
                pole_endpoints: poles,
            },
        }))
    })();
    crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_io::step::load::step_geometry::Conic3D;
    use monstertruck_modeling::{Processor, RevolutionSurface, TrimmedCurve, UnitCircle};

    #[test]
    fn affine_tensor_frame_proves_all_original_coefficients_and_denominator() {
        use monstertruck_modeling::{
            BsplineSurface, KnotVector, NurbsSurface, Surface as NativeSurface,
        };
        let surface = |controls| {
            let knots = KnotVector::from(vec![0., 0., 1., 1.]);
            NativeSurface::NurbsSurface(NurbsSurface::new(BsplineSurface::new(
                (knots.clone(), knots),
                controls,
            )))
        };
        let controls = vec![
            vec![
                Vector4::new(12., 12., 8., 2.),
                Vector4::new(12., 28., 8., 2.),
            ],
            vec![
                Vector4::new(28., 12., 8., 2.),
                Vector4::new(28., 28., 8., 2.),
            ],
        ];
        let valid = surface(controls.clone());
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let frame = affine_tensor_frame(&valid, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(frame.origin[0].lo <= 6. && frame.origin[0].hi >= 6.);
        assert!(frame.axes[0][0].lo <= 8. && frame.axes[0][0].hi >= 8.);
        for weight in [0., 1.] {
            let mut changed = controls.clone();
            changed[1][1].w = weight;
            assert!(
                affine_tensor_frame(&surface(changed), &mut work, &cancel)
                    .unwrap()
                    .is_none()
            );
        }
        let mut all_zero = controls.clone();
        for point in all_zero.iter_mut().flatten() {
            point.w = 0.;
        }
        assert!(
            affine_tensor_frame(&surface(all_zero), &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let mut rounded_identity = controls;
        rounded_identity[0][0].x = 1e16;
        rounded_identity[1][1].x = 1.;
        rounded_identity[1][0].x = 1e16;
        rounded_identity[0][1].x = 0.;
        assert!(
            affine_tensor_frame(&surface(rounded_identity), &mut work, &cancel)
                .unwrap()
                .is_none(),
            "equal rounded sums are not a homogeneous parallelogram identity",
        );
        assert_eq!(
            affine_tensor_frame(&valid, &mut GeometryWorkBudget::new(0), &cancel)
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(
            affine_tensor_frame(&valid, &mut work, &AtomicBool::new(true))
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::Cancelled,
        );
    }

    fn ring(radius: f64, height: f64) -> Curve3D {
        let matrix = Matrix4::from_translation(Vector3::new(0., 0., height))
            * Matrix4::from_nonuniform_scale(radius, radius, 1.);
        Curve3D::Conic(Conic3D::Ellipse(Processor::with_transform(
            TrimmedCurve::new(UnitCircle::<Point3>::new(), (0., std::f64::consts::TAU)),
            matrix,
        )))
    }
    fn witness(curve: &Curve3D, surface: &Surface) -> ChartWitness {
        let chart = super::super::charts::source_chart(curve, surface)
            .unwrap()
            .unwrap();
        certify(
            curve,
            surface,
            chart,
            SurfaceParameterMap::Identity,
            1.,
            &mut GeometryWorkBudget::new(100_000),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn reference_phase_tube_encloses_complete_rectangle_and_lift() {
        let cancel = AtomicBool::new(false);
        // A native quadrant-wide bisection cannot fit this allowance. The
        // rectangle includes an interior continuum, not just a nominal vector.
        let tube = phase(
            Interval { lo: -0.1, hi: 0.2 },
            Interval { lo: 2.0, hi: 3.0 },
            0.0,
            &mut ValidatedGeometryBudget::new(64),
            &cancel,
        )
        .unwrap();
        assert!(tube.lo <= -0.05 && tube.hi >= 0.1);
        assert!(tube.lo > -0.101 && tube.hi < 0.101);
        let lifted = phase(
            p(0.0),
            p(-1.0),
            std::f64::consts::PI,
            &mut ValidatedGeometryBudget::new(64),
            &cancel,
        )
        .unwrap();
        assert!(lifted.lo <= std::f64::consts::PI.next_down());
        assert!(lifted.hi >= std::f64::consts::PI.next_up());
    }

    #[test]
    fn reference_phase_tube_preserves_quadrant_fallback_and_origin_refusal() {
        let cancel = AtomicBool::new(false);
        let opposite = phase(
            p(0.0),
            p(-1.0),
            0.0,
            &mut ValidatedGeometryBudget::new(100_000),
            &cancel,
        )
        .unwrap();
        assert!(opposite.abs_upper() >= std::f64::consts::PI);
        assert!(opposite.hi - opposite.lo < 1e-12);
        let failure = phase(
            Interval { lo: -1.0, hi: 1.0 },
            Interval { lo: -1.0, hi: 1.0 },
            0.0,
            &mut ValidatedGeometryBudget::new(100_000),
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::SingularGeometry);
    }

    #[test]
    fn legacy_cylinder_and_cone_rings_and_generators_have_coefficient_tubes() {
        for (first, last, cone) in [(2., 2., false), (1., 3., true)] {
            let profile = Line(Point3::new(first, 0., 0.), Point3::new(last, 0., 4.));
            let processor = Processor::new(RevolutionSurface::by_revolution(
                profile,
                Point3::origin(),
                Vector3::unit_z(),
            ));
            let surface = Surface::ElementarySurface(if cone {
                ElementarySurface::ConicalSurface(processor)
            } else {
                ElementarySurface::CylindricalSurface(processor)
            });
            for curve in [ring((first + last) * 0.5, 2.), Curve3D::Line(profile)] {
                let proof = witness(&curve, &surface);
                assert!(proof.error_mm.is_finite() && proof.error_mm < 1e-7);
                assert!(
                    proof
                        .margin_uv
                        .into_iter()
                        .all(|margin| margin.is_finite() && margin < 1e-8)
                );
            }
        }
    }

    #[test]
    fn straight_sweep_revolution_and_planar_radial_profile_use_the_same_proof() {
        for (first, last, height) in [(1., 3., 4.), (1., 3., 0.)] {
            let profile = Line(Point3::new(first, 0., 0.), Point3::new(last, 0., height));
            let surface = Surface::SweepSurface(SweepSurface::RevolutionSurface(Processor::new(
                RevolutionSurface::by_revolution(
                    Box::new(Curve3D::Line(profile)),
                    Point3::origin(),
                    Vector3::unit_z(),
                ),
            )));
            for curve in [ring(2., height * 0.5), Curve3D::Line(profile)] {
                assert!(witness(&curve, &surface).error_mm < 1e-7);
            }
        }
    }

    #[test]
    fn misplaced_ring_is_not_authorized_by_its_nominal_chart() {
        let surface = Surface::ElementarySurface(ElementarySurface::CylindricalSurface(
            Processor::new(RevolutionSurface::by_revolution(
                Line(Point3::new(2., 0., 0.), Point3::new(2., 0., 4.)),
                Point3::origin(),
                Vector3::unit_z(),
            )),
        ));
        let candidate = Curve2D::Line(Line(
            Point2::new(0.5, 0.),
            Point2::new(0.5, std::f64::consts::TAU),
        ));
        let error = certify(
            &ring(2.01, 2.),
            &surface,
            candidate,
            SurfaceParameterMap::Identity,
            1.,
            &mut GeometryWorkBudget::new(100_000),
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
    }

    #[test]
    fn squared_norm_zero_keeps_its_nonnegative_analytic_lower_bound() {
        let value = norm([p(0.); 3]).unwrap();
        assert_eq!(value.lo, 0.);
        assert!(value.hi.is_finite());
    }

    #[test]
    fn nested_restricted_conics_and_original_angular_uv_lines_keep_basis_anchor() {
        let tau = std::f64::consts::TAU;
        let cylinder = Surface::ElementarySurface(ElementarySurface::CylindricalSurface(
            Processor::new(RevolutionSurface::by_revolution(
                Line(Point3::new(2., 0., 0.), Point3::new(2., 0., 4.)),
                Point3::origin(),
                Vector3::unit_z(),
            )),
        ));
        let sphere = Surface::ElementarySurface(ElementarySurface::Sphere(Processor::new(
            monstertruck_io::step::load::step_geometry::Sphere(monstertruck_modeling::Sphere::new(
                Point3::origin(),
                2.,
            )),
        )));
        let torus = Surface::ElementarySurface(ElementarySurface::ToroidalSurface(Processor::new(
            monstertruck_modeling::Torus::new(Point3::origin(), 5., 2.),
        )));
        for (surface, radius, height, first, last) in [
            (cylinder, 2., 2., Point2::new(0.5, 0.), Point2::new(0.5, 1.)),
            (sphere, 2., 0., Point2::new(0., 0.), Point2::new(1., 0.)),
            (torus, 7., 0., Point2::new(0., 0.), Point2::new(1., 0.)),
        ] {
            let mut original = Processor::<_, Matrix4>::new(TrimmedCurve::new(
                Box::new(ring(radius, height)),
                (0., tau),
            ));
            original.invert();
            let curve=Curve3D::TrimmedCurve(
                monstertruck_io::step::load::step_geometry::SourceTrimmedCurve::internal(
                    Processor::<_,Matrix4>::new(TrimmedCurve::new(
                        Box::new(Curve3D::TrimmedCurve(
                            monstertruck_io::step::load::step_geometry::SourceTrimmedCurve::internal(original))),
                        (0.25,1.25)))));
            let mut original_uv = Processor::<_, Matrix3>::new(TrimmedCurve::new(
                Box::new(Curve2D::Line(Line(first, last))),
                (0., tau),
            ));
            original_uv.invert();
            let chart = Curve2D::TrimmedCurve(Processor::<_, Matrix3>::new(TrimmedCurve::new(
                Box::new(Curve2D::TrimmedCurve(original_uv)),
                (0.25, 1.25),
            )));
            let proof = certify(
                &curve,
                &surface,
                chart,
                SurfaceParameterMap::Identity,
                1.,
                &mut GeometryWorkBudget::new(500_000),
                &AtomicBool::new(false),
            )
            .unwrap()
            .unwrap();
            assert_eq!(proof.curve.range_tuple(), (0.25, 1.25));
            assert!(matches!(proof.curve, Curve2D::TrimmedCurve(_)));
            assert!(proof.error_mm < 1e-7);
            assert!(proof.margin_uv.into_iter().all(|bound| bound < 1e-8));
        }
    }
}
