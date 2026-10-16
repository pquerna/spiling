// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Coefficient-correlated forward comparison of native harmonic expressions.
//! This is an arithmetic optimization, not a carrier or source-family permission:
//! both original operands, their complete domains and relative tangents are proved.

use crate::{
    GeometryError, GeometryWorkBudget,
    parametric::{self, Interval},
};
use monstertruck_modeling::{self as model, base::*};
use std::sync::atomic::AtomicBool;

// Bounded coefficient captures and their at-most-32 wrapper traversal coexist
// with the certificate's other scratch. This reservation narrows its 8 MiB cap.
pub(super) const SCRATCH_BYTES: usize = 64 * 1024;

type Vector<const N: usize> = [Interval; N];
fn p(value: f64) -> Interval {
    Interval::point(value)
}
fn dot<const N: usize>(a: Vector<N>, b: Vector<N>) -> Interval {
    (0..N).fold(p(0.0), |sum, i| sum.add(a[i].mul(b[i])))
}
fn norm<const N: usize>(value: Vector<N>) -> Result<Interval, GeometryError> {
    let square = dot(value, value);
    Interval {
        lo: square.lo.max(0.0),
        hi: square.hi,
    }
    .sqrt()
}
fn affine<const N: usize>(
    matrix: impl Fn(usize, usize) -> f64,
    value: Vector<N>,
    point: bool,
) -> Vector<N> {
    std::array::from_fn(|row| {
        (0..N).fold(
            if point { p(matrix(N, row)) } else { p(0.0) },
            |sum, col| sum.add(p(matrix(col, row)).mul(value[col])),
        )
    })
}
fn affine_matrix<const N: usize>(matrix: impl Fn(usize, usize) -> f64) -> bool {
    (0..=N).all(|col| (0..=N).all(|row| matrix(col, row).is_finite()))
        && (0..N).all(|col| matrix(col, N) == 0.0)
        && matrix(N, N) == 1.0
}

struct Harmonic<const N: usize> {
    center: Vector<N>,
    cosine: Vector<N>,
    sine: Vector<N>,
    offset: Interval,
    sense: f64,
    // Absolute-value majorant of the ORIGINAL evaluation tree, including
    // intermediate poses/phase arithmetic, not only its cancelled result.
    arithmetic_scale: Interval,
}
impl<const N: usize> Harmonic<N> {
    fn reflect(&mut self, range: (f64, f64)) {
        self.offset = self
            .offset
            .add(p(self.sense).mul(p(range.0).add(p(range.1))));
        self.sense = -self.sense;
        self.arithmetic_scale = self
            .arithmetic_scale
            .mul(p(1.0).add(p(range.0.abs())).add(p(range.1.abs())));
    }
    fn transform(&mut self, matrix: impl Fn(usize, usize) -> f64) {
        self.center = affine(&matrix, self.center, true);
        self.cosine = affine(&matrix, self.cosine, false);
        self.sine = affine(&matrix, self.sine, false);
        let linear = (0..N)
            .map(|row| {
                (0..N)
                    .fold(p(0.0), |sum, col| sum.add(p(matrix(col, row).abs())))
                    .hi
            })
            .fold(0.0_f64, f64::max);
        let translation = (0..N)
            .map(|row| matrix(N, row).abs())
            .fold(0.0_f64, f64::max);
        self.arithmetic_scale = p(self.arithmetic_scale.hi.max(f64::MIN_POSITIVE))
            .mul(p(linear))
            .add(p(translation));
    }
    fn positive_sense(&mut self) {
        if self.sense < 0.0 {
            self.offset = self.offset.neg();
            self.sine = self.sine.map(Interval::neg);
            self.sense = 1.0;
        }
    }
}
fn uv_harmonic(
    curve: &model::Curve2D,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Harmonic<2>>, GeometryError> {
    work.charge(32, cancel)?;
    if depth > 32 {
        return Ok(None);
    }
    match curve {
        model::Curve2D::Conic(model::Conic2D::Ellipse(ellipse)) => {
            let matrix = ellipse.transform();
            if !affine_matrix::<2>(|col, row| matrix[col][row]) {
                return Ok(None);
            }
            let range = ellipse.range_tuple();
            let mut result = Harmonic {
                center: [p(0.0); 2],
                cosine: [p(1.0), p(0.0)],
                sine: [p(0.0), p(1.0)],
                offset: p(0.0),
                sense: 1.0,
                arithmetic_scale: p(1.0).add(p(range.0.abs())).add(p(range.1.abs())),
            };
            if !ellipse.orientation() {
                result.reflect(ellipse.range_tuple());
            }
            result.transform(|col, row| matrix[col][row]);
            Ok(Some(result))
        }
        model::Curve2D::TrimmedCurve(trim) => {
            let matrix = trim.transform();
            if !affine_matrix::<2>(|col, row| matrix[col][row]) {
                return Ok(None);
            }
            let Some(mut result) = uv_harmonic(trim.entity().curve(), depth + 1, work, cancel)?
            else {
                return Ok(None);
            };
            if !trim.orientation() {
                result.reflect(trim.range_tuple());
            }
            result.transform(|col, row| matrix[col][row]);
            Ok(Some(result))
        }
        _ => Ok(None),
    }
}
fn plane_harmonic(plane: &model::Plane, mut uv: Harmonic<2>, margin: [f64; 2]) -> Harmonic<3> {
    let origin = plane.origin();
    let origin = [origin.x, origin.y, origin.z].map(p);
    let axes = if let Some(axes) = plane.stored_axes() {
        axes.map(|axis| [axis.x, axis.y, axis.z].map(p))
    } else {
        let points = plane.defining_points();
        [points[1], points[2]].map(|point| {
            let point = [point.x, point.y, point.z].map(p);
            std::array::from_fn(|axis| point[axis].sub(origin[axis]))
        })
    };
    uv.center = std::array::from_fn(|axis| {
        uv.center[axis].add(Interval {
            lo: -margin[axis],
            hi: margin[axis],
        })
    });
    let map = |value: Vector<2>| -> Vector<3> {
        std::array::from_fn(|row| axes[0][row].mul(value[0]).add(axes[1][row].mul(value[1])))
    };
    let center = map(uv.center);
    let linear = (0..3)
        .map(|row| axes[0][row].abs_upper() + axes[1][row].abs_upper())
        .fold(0.0_f64, f64::max);
    let translation = origin
        .into_iter()
        .map(Interval::abs_upper)
        .fold(0.0_f64, f64::max);
    Harmonic {
        center: std::array::from_fn(|row| origin[row].add(center[row])),
        cosine: map(uv.cosine),
        sine: map(uv.sine),
        offset: uv.offset,
        sense: uv.sense,
        arithmetic_scale: p(uv.arithmetic_scale.hi.max(f64::MIN_POSITIVE))
            .mul(p(parametric::up(linear)))
            .add(p(translation)),
    }
}

/// Compose a harmonic chart with the ORIGINAL coefficient-proved finite tensor.
/// This does not replace the native surface by a plane. Whole actual stored UV
/// coefficients must lie in source support before any physical receipt is made.
fn tensor_harmonic(
    frame: &super::chart_proof::AffineTensorFrame,
    chart: &model::Curve2D,
    mut uv: Harmonic<2>,
    margin: [f64; 2],
) -> Result<Option<Harmonic<3>>, GeometryError> {
    let bounds = if let model::Curve2D::Conic(model::Conic2D::Ellipse(conic)) = chart {
        super::chart_proof::conic_support(conic.transform())?
    } else {
        let mut bounds = [p(0.0); 2];
        for axis in 0..2 {
            let square = uv.cosine[axis].square().add(uv.sine[axis].square());
            let radius = Interval {
                lo: square.lo.max(0.0),
                hi: square.hi,
            }
            .sqrt()?
            .hi;
            bounds[axis] = uv.center[axis].add(Interval {
                lo: -radius,
                hi: radius,
            });
        }
        bounds
    };
    for axis in 0..2 {
        let bounds = bounds[axis].add(Interval {
            lo: -margin[axis],
            hi: margin[axis],
        });
        if bounds.lo < frame.domain[axis][0] || bounds.hi > frame.domain[axis][1] {
            return Ok(None);
        }
        uv.center[axis] = uv.center[axis].add(Interval {
            lo: -margin[axis],
            hi: margin[axis],
        });
    }
    let map = |value: Vector<2>| -> Vector<3> {
        std::array::from_fn(|axis| {
            frame.axes[0][axis]
                .mul(value[0])
                .add(frame.axes[1][axis].mul(value[1]))
        })
    };
    let center = map(uv.center);
    let linear = (0..3)
        .map(|axis| {
            p(frame.axes[0][axis].abs_upper())
                .add(p(frame.axes[1][axis].abs_upper()))
                .hi
        })
        .fold(0.0_f64, f64::max);
    Ok(Some(Harmonic {
        center: std::array::from_fn(|axis| frame.origin[axis].add(center[axis])),
        cosine: map(uv.cosine),
        sine: map(uv.sine),
        offset: uv.offset,
        sense: uv.sense,
        arithmetic_scale: p(uv.arithmetic_scale.hi.max(f64::MIN_POSITIVE))
            .mul(p(linear))
            .add(p(frame.arithmetic_scale.max(f64::MIN_POSITIVE))),
    }))
}

/// Majorize the original Rodrigues evaluation, not its cancelled harmonic
/// coefficients. Native evaluation is o + R(axis,phi)*(profile(u)-o).
/// Its literal matrix entries are (1-c)*ai*ai+c on the diagonal and
/// (1-c)*ai*aj +/- s*ak elsewhere, with |1-c|<=2 and |s|,|c|<=1.
/// Keep each actual row/axis dependency: replacing all axis components by
/// their maximum would invent operations that amplify unrelated world axes.
fn revolution_arithmetic_scale(profile: Vector<3>, origin: Vector<3>, axis: Vector<3>) -> Interval {
    let delta_scale: Vector<3> =
        std::array::from_fn(|i| p(profile[i].abs_upper()).add(p(origin[i].abs_upper())));
    let rows: Vector<3> = std::array::from_fn(|row| {
        (0..3).fold(p(origin[row].abs_upper()), |sum, col| {
            let product = p(axis[row].abs_upper()).mul(p(axis[col].abs_upper()));
            let coefficient = p(2.0).mul(product).add(if row == col {
                p(1.0)
            } else {
                p(axis[3 - row - col].abs_upper())
            });
            sum.add(coefficient.mul(delta_scale[col]))
        })
    });
    // Profile enclosures already retain the native original-tree arithmetic
    // in the harmonic coefficient intervals. Keep both profile and origin
    // operands here too: a small final delta cannot erase spatial roundoff.
    p(rows
        .into_iter()
        .map(|value| value.hi)
        .fold(0.0_f64, f64::max))
}

fn spatial_harmonic(
    curve: &model::Curve,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Harmonic<3>>, GeometryError> {
    work.charge(64, cancel)?;
    if depth > 32 {
        return Ok(None);
    }
    match curve {
        model::Curve::TrimmedCurve(trim) => {
            let matrix = trim.transform();
            if !affine_matrix::<3>(|col, row| matrix[col][row]) {
                return Ok(None);
            }
            let Some(mut result) =
                spatial_harmonic(trim.entity().curve(), depth + 1, work, cancel)?
            else {
                return Ok(None);
            };
            if !trim.orientation() {
                result.reflect(trim.range_tuple());
            }
            result.transform(|col, row| matrix[col][row]);
            Ok(Some(result))
        }
        model::Curve::ParameterCurve(parameter) => {
            let model::Surface::Plane(plane) = parameter.surface().as_ref() else {
                return Ok(None);
            };
            let Some(uv) = uv_harmonic(parameter.curve(), depth + 1, work, cancel)? else {
                return Ok(None);
            };
            // Native ParameterCurve has only these two operands: the public
            // STEP converter has already composed its original input chart and
            // decoded-to-native map into this exact UV expression. Unlike a
            // StepParameterCurve, no hidden raw chart remains to be applied.
            Ok(Some(plane_harmonic(plane, uv, [0.0; 2])))
        }
        _ => Ok(None),
    }
}

/// Original source chart domain, kept separate from the native expression's
/// evaluable interval (a nominal conic window is not an original source trim).
#[derive(Clone, Copy)]
pub(super) enum ProfileParameterDomain {
    Bounded([f64; 2]),
    Unbounded,
}

/// Propose the affine phase relation of two complete borrowed expressions.
/// Numerical coefficient collection selects search coordinates only; the
/// ordinary whole coefficient/position/tangent certificate must still pass.
pub(super) fn extrusion_profile_chart_proposal(
    curve: &model::Curve,
    surface: &model::Surface,
    original_domain: ProfileParameterDomain,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<model::Curve2D>, GeometryError> {
    let model::Surface::ExtrusionSurface(extrusion) = surface else {
        return Ok(None);
    };
    let Some(source) = spatial_harmonic(curve, 0, work, cancel)? else {
        return Ok(None);
    };
    let Some(profile) = spatial_harmonic(extrusion.entity_curve(), 0, work, cancel)? else {
        return Ok(None);
    };
    work.charge(128, cancel)?;
    let mids = |values: Vector<3>| values.map(Interval::mid);
    let (a, b, x, y) = (
        mids(profile.cosine),
        mids(profile.sine),
        mids(source.cosine),
        mids(source.sine),
    );
    let dot = |x: [f64; 3], y: [f64; 3]| (0..3).map(|i| x[i] * y[i]).sum::<f64>();
    let (aa, ab, bb) = (dot(a, a), dot(a, b), dot(b, b));
    let determinant = aa * bb - ab * ab;
    if !determinant.is_finite() || determinant <= 0.0 {
        return Ok(None);
    }
    let (ax, bx) = (dot(a, x), dot(b, x));
    let cosine = (ax * bb - bx * ab) / determinant;
    let sine = (bx * aa - ax * ab) / determinant;
    let phase = sine.atan2(cosine);
    let quadrature = std::array::from_fn(|i| -a[i] * sine + b[i] * cosine);
    let orientation = dot(y, quadrature);
    if !orientation.is_finite() || orientation == 0.0 {
        return Ok(None);
    }
    let orientation = if orientation > 0.0 { 1.0 } else { -1.0 };
    let (start, end) = curve.range_tuple();
    let u = |q| {
        (orientation * (source.sense * q + source.offset.mid()) + phase - profile.offset.mid())
            / profile.sense
    };
    let (mut front, mut back) = (u(start), u(end));
    let native_range = extrusion.entity_curve().range_tuple();
    if matches!(original_domain, ProfileParameterDomain::Unbounded) {
        // A harmonic expression's unit phase identifies this numerical branch
        // proposal. Neither TAU nor rounding grants a periodic identity: the
        // complete original sin/cos phase-error certificate must still pass.
        let center = 0.5 * front + 0.5 * back;
        let native_center = 0.5 * native_range.0 + 0.5 * native_range.1;
        let turns = ((native_center - center) * profile.sense / std::f64::consts::TAU).round();
        let shift = turns * std::f64::consts::TAU / profile.sense;
        front += shift;
        back += shift;
    }
    let direction = extrusion.extruding_vector();
    let direction = [direction.x, direction.y, direction.z];
    let delta = std::array::from_fn(|i| source.center[i].mid() - profile.center[i].mid());
    let v = dot(delta, direction) / dot(direction, direction);
    if ![start, end, front, back, v].into_iter().all(f64::is_finite)
        || start >= end
        || front == back
    {
        return Ok(None);
    }
    if front.min(back) < native_range.0 || front.max(back) > native_range.1 {
        return Ok(None);
    }
    if let ProfileParameterDomain::Bounded(domain) = original_domain {
        if front.min(back) < domain[0] || front.max(back) > domain[1] {
            return Ok(None);
        }
    }
    work.charge(
        (4 * std::mem::size_of::<f64>() + 2 * std::mem::size_of::<model::Point2>()) as u64,
        cancel,
    )?;
    Ok(Some(model::Curve2D::BsplineCurve(
        model::BsplineCurve::new(
            model::KnotVector::from(vec![start, start, end, end]),
            vec![model::Point2::new(front, v), model::Point2::new(back, v)],
        ),
    )))
}
struct AffineUv {
    offset: Vector<2>,
    slope: Vector<2>,
    // Bound the ORIGINAL evaluation tree by B0 + B1*|t| componentwise.
    // Offsets/outer translations do not depend on the input parameter.
    offset_scale: Vector<2>,
    slope_scale: Vector<2>,
}
fn affine_uv(
    curve: &model::Curve2D,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<AffineUv>, GeometryError> {
    work.charge(32, cancel)?;
    if depth > 32 {
        return Ok(None);
    }
    match curve {
        model::Curve2D::Line(line) => Ok(Some(AffineUv {
            offset: [p(line.0.x), p(line.0.y)],
            slope: [p(line.1.x).sub(p(line.0.x)), p(line.1.y).sub(p(line.0.y))],
            offset_scale: [p(line.0.x.abs()), p(line.0.y.abs())],
            slope_scale: [
                p(p(line.1.x).sub(p(line.0.x)).abs_upper()),
                p(p(line.1.y).sub(p(line.0.y)).abs_upper()),
            ],
        })),
        model::Curve2D::BsplineCurve(line)
            if line.degree() == 1
                && line.control_points().len() == 2
                && line.knot_vector().len() == 4 =>
        {
            let knots = line.knot_vector();
            if knots[0] != knots[1] || knots[2] != knots[3] || knots[1] >= knots[2] {
                return Ok(None);
            }
            let points = line.control_points();
            let width = p(knots[2]).sub(p(knots[1]));
            let slope = [
                p(points[1].x).sub(p(points[0].x)).div(width)?,
                p(points[1].y).sub(p(points[0].y)).div(width)?,
            ];
            Ok(Some(AffineUv {
                offset: std::array::from_fn(|axis| {
                    p(points[0][axis]).sub(slope[axis].mul(p(knots[1])))
                }),
                slope,
                offset_scale: std::array::from_fn(|axis| {
                    p(points[0][axis].abs()).add(p(slope[axis].abs_upper()).mul(p(knots[1].abs())))
                }),
                slope_scale: slope.map(|value| p(value.abs_upper())),
            }))
        }
        model::Curve2D::TrimmedCurve(trim) => {
            let matrix = trim.transform();
            if !affine_matrix::<2>(|col, row| matrix[col][row]) {
                return Ok(None);
            }
            let Some(mut result) = affine_uv(trim.entity().curve(), depth + 1, work, cancel)?
            else {
                return Ok(None);
            };
            if !trim.orientation() {
                let range = trim.range_tuple();
                result.offset = std::array::from_fn(|axis| {
                    result.offset[axis].add(result.slope[axis].mul(p(range.0).add(p(range.1))))
                });
                result.slope = result.slope.map(Interval::neg);
                // F(a+b-t) has majorant B0+B1*(|a|+|b|)+B1*|t|.
                // Reflecting the parameter cannot amplify an existing UV
                // offset: that offset is evaluated outside the parameter.
                let reflection = p(range.0.abs()).add(p(range.1.abs()));
                result.offset_scale = std::array::from_fn(|axis| {
                    result.offset_scale[axis].add(result.slope_scale[axis].mul(reflection))
                });
            }
            result.offset = affine(|col, row| matrix[col][row], result.offset, true);
            result.slope = affine(|col, row| matrix[col][row], result.slope, false);
            result.offset_scale = std::array::from_fn(|row| {
                (0..2).fold(p(matrix[2][row].abs()), |sum, col| {
                    sum.add(p(matrix[col][row].abs()).mul(result.offset_scale[col]))
                })
            });
            result.slope_scale = std::array::from_fn(|row| {
                (0..2).fold(p(0.0), |sum, col| {
                    sum.add(p(matrix[col][row].abs()).mul(result.slope_scale[col]))
                })
            });
            Ok(Some(result))
        }
        _ => Ok(None),
    }
}

/// Bounded stack receipt, formatted only on a terminal correspondence failure.
#[derive(Clone, Copy)]
pub(super) struct HarmonicDiagnostic {
    stage: &'static str,
    position_bound: Option<f64>,
    tolerance: f64,
    center_error: Option<f64>,
    coefficient_error: Option<f64>,
    roundoff: Option<f64>,
    phase_error: Option<[f64; 2]>,
    amplitude_scale: Option<f64>,
    phase_distance: Option<f64>,
    source_arithmetic_scale: Option<f64>,
    native_arithmetic_scale: Option<f64>,
    tangent_error: Option<f64>,
    tangent_limit: Option<f64>,
    speed_lower: Option<f64>,
}

impl HarmonicDiagnostic {
    pub(super) fn new(tolerance: f64) -> Self {
        Self {
            stage: "not-attempted",
            position_bound: None,
            tolerance,
            center_error: None,
            coefficient_error: None,
            roundoff: None,
            phase_error: None,
            amplitude_scale: None,
            phase_distance: None,
            source_arithmetic_scale: None,
            native_arithmetic_scale: None,
            tangent_error: None,
            tangent_limit: None,
            speed_lower: None,
        }
    }
}

impl std::fmt::Display for HarmonicDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.stage)?;
        if let Some(position) = self.position_bound {
            write!(f, " pos={position:.6e}/tol={:.6e}", self.tolerance)?;
        }
        for (name, value) in [
            ("roundoff", self.roundoff),
            ("srcA", self.source_arithmetic_scale),
            ("nativeA", self.native_arithmetic_scale),
            ("tangent", self.tangent_error),
            ("limit", self.tangent_limit),
            ("speed", self.speed_lower),
        ] {
            if let Some(value) = value {
                write!(f, " {name}={value:.6e}")?;
            }
        }
        if let Some([lo, hi]) = self.phase_error {
            write!(f, " phase=[{lo:.6e},{hi:.6e}]")?;
        }
        if f.alternate() {
            for (name, value) in [
                ("normalized_center", self.center_error),
                ("normalized_coefficients", self.coefficient_error),
                ("amplitude", self.amplitude_scale),
                ("phase_distance", self.phase_distance),
            ] {
                if let Some(value) = value {
                    write!(f, " {name}={value:.6e}")?;
                }
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn certify(
    curve: &model::Curve,
    surface: &model::Surface,
    chart: &model::Curve2D,
    range: [f64; 2],
    target: [f64; 2],
    margin: [f64; 2],
    tolerance: f64,
    require_tangent: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: &mut HarmonicDiagnostic,
) -> Result<Option<f64>, GeometryError> {
    *diagnostic = HarmonicDiagnostic::new(tolerance);
    diagnostic.stage = "source-coefficients";
    work.charge(512, cancel)?;
    let Some(mut source) = spatial_harmonic(curve, 0, work, cancel)? else {
        return Ok(None);
    };
    diagnostic.source_arithmetic_scale = Some(source.arithmetic_scale.hi);
    let map_slope = p(target[1])
        .sub(p(target[0]))
        .div(p(range[1]).sub(p(range[0])))?;
    let mut tangent_arithmetic_scale = None;
    let (mut native, mut front, mut slope, phase_margin) = match surface {
        model::Surface::Plane(plane) => {
            diagnostic.stage = "planar-chart-coefficients";
            let Some(uv) = uv_harmonic(chart, 0, work, cancel)? else {
                return Ok(None);
            };
            let front = uv.offset.add(p(uv.sense).mul(p(target[0])));
            let slope = p(uv.sense).mul(map_slope);
            (plane_harmonic(plane, uv, margin), front, slope, 0.0)
        }
        model::Surface::BsplineSurface(_) | model::Surface::NurbsSurface(_) => {
            diagnostic.stage = "affine-tensor-chart-coefficients";
            let Some(frame) = super::chart_proof::affine_tensor_frame(surface, work, cancel)?
            else {
                return Ok(None);
            };
            let Some(uv) = uv_harmonic(chart, 0, work, cancel)? else {
                return Ok(None);
            };
            let front = uv.offset.add(p(uv.sense).mul(p(target[0])));
            let slope = p(uv.sense).mul(map_slope);
            let Some(native) = tensor_harmonic(&frame, chart, uv, margin)? else {
                diagnostic.stage = "affine-tensor-finite-source-support";
                return Ok(None);
            };
            (native, front, slope, 0.0)
        }
        model::Surface::RevolutionSurface(processor) => {
            diagnostic.stage = "revolution-chart-coefficients";
            let matrix = processor.transform();
            if !affine_matrix::<3>(|col, row| matrix[col][row]) {
                return Ok(None);
            }
            let Some(uv) = affine_uv(chart, 0, work, cancel)? else {
                return Ok(None);
            };
            let profile_axis = usize::from(!processor.orientation());
            let angle_axis = 1 - profile_axis;
            if uv.slope[profile_axis].lo != 0.0 || uv.slope[profile_axis].hi != 0.0 {
                diagnostic.stage = "varying-revolution-profile";
                return Ok(None);
            }
            let profile_range = uv.offset[profile_axis].add(Interval {
                lo: -margin[profile_axis],
                hi: margin[profile_axis],
            });
            let revolution = processor.entity();
            // The native profile evaluator supplies its original denominator
            // and smooth-span qualification over the complete uncertain input.
            let profile = parametric::curve_enclosure(
                revolution.entity_curve(),
                (profile_range.lo, profile_range.hi),
                work,
                cancel,
            )?;
            let origin = revolution.origin();
            let origin = [origin.x, origin.y, origin.z].map(p);
            let axis = revolution.axis();
            let axis = [axis.x, axis.y, axis.z].map(p);
            let delta = std::array::from_fn(|i| profile.position[i].sub(origin[i]));
            let axial = dot(axis, delta);
            let axial = axis.map(|component| component.mul(axial));
            let mut native = Harmonic {
                center: std::array::from_fn(|i| origin[i].add(axial[i])),
                cosine: std::array::from_fn(|i| delta[i].sub(axial[i])),
                sine: std::array::from_fn(|i| {
                    let j = (i + 1) % 3;
                    let k = (i + 2) % 3;
                    axis[j].mul(delta[k]).sub(axis[k].mul(delta[j]))
                }),
                offset: p(0.0),
                sense: 1.0,
                arithmetic_scale: revolution_arithmetic_scale(profile.position, origin, axis),
            };
            native.transform(|col, row| matrix[col][row]);
            let front = uv.offset[angle_axis].add(uv.slope[angle_axis].mul(p(target[0])));
            let slope = uv.slope[angle_axis].mul(map_slope);
            // Evaluate B0+B1*|t| over the actual complete target interval.
            // Multiplying (B0+B1) by (1+|t|) would count the UV offset as
            // parameter-dependent, even when a period translation cancels it.
            let phase_scale = uv.offset_scale[angle_axis]
                .add(uv.slope_scale[angle_axis].mul(p(target[0].abs().max(target[1].abs()))));
            // For H(phi) = c + u*cos(phi) + v*sin(phi), perturbing the
            // computed phase by e changes position by at most
            // (||u|| + ||v||)*|e|, since sin/cos are 1-Lipschitz. The
            // coefficient enclosures already include the actual spatial map.
            // In particular its world translation is in c, NOT in dH/dphi.
            //
            // Keep the original spatial evaluation majorant independently:
            // at any computed phase, |sin|, |cos| <= 1 still bounds that
            // evaluation's arithmetic. The unchanged gamma_n below bounds
            // both this spatial error and the phase-tree error, so their SUM
            // is gamma_n*(spatial_scale + amplitude*phase_scale). Multiplying
            // spatial_scale by phase_scale would spuriously amplify absolute
            // world translation, while dropping spatial_scale would lose
            // original floating evaluation error despite coefficient equality.
            // The L1 coefficient sum bounds both Euclidean amplitude norms
            // without squaring tiny coefficients or overflowing large squares.
            let phase_amplitude = native
                .cosine
                .into_iter()
                .chain(native.sine)
                .fold(p(0.0), |sum, component| sum.add(p(component.abs_upper())));
            // Differentiating also evaluates the affine phase slope. Unlike
            // position, that reserve must retain B1 even when |target| < 1.
            // For dH/ds = H'(phi)*phi', phase error is amplified by |phi'|,
            // while slope error is amplified by the same harmonic amplitude.
            let phase_slope = p(slope.abs_upper());
            let slope_scale = uv.slope_scale[angle_axis].mul(p(map_slope.abs_upper()));
            tangent_arithmetic_scale = Some(
                native
                    .arithmetic_scale
                    .mul(phase_slope)
                    .add(phase_amplitude.mul(phase_scale.mul(phase_slope).add(slope_scale))),
            );
            native.arithmetic_scale = native
                .arithmetic_scale
                .add(phase_amplitude.mul(phase_scale));
            (native, front, slope, margin[angle_axis])
        }
        model::Surface::ExtrusionSurface(extrusion) => {
            diagnostic.stage = "extrusion-chart-coefficients";
            let Some(uv) = affine_uv(chart, 0, work, cancel)? else {
                return Ok(None);
            };
            if uv.slope[1].lo != 0.0 || uv.slope[1].hi != 0.0 {
                diagnostic.stage = "varying-extrusion-offset";
                return Ok(None);
            }
            let Some(mut native) = spatial_harmonic(extrusion.entity_curve(), 0, work, cancel)?
            else {
                return Ok(None);
            };
            let direction = extrusion.extruding_vector();
            let direction = [direction.x, direction.y, direction.z].map(p);
            let v = uv.offset[1].add(Interval {
                lo: -margin[1],
                hi: margin[1],
            });
            native.center = std::array::from_fn(|i| native.center[i].add(direction[i].mul(v)));
            let target_extent = p(target[0].abs().max(target[1].abs()));
            let v_scale = uv.offset_scale[1].add(uv.slope_scale[1].mul(target_extent));
            native.arithmetic_scale =
                native
                    .arithmetic_scale
                    .add(direction.into_iter().fold(p(0.0), |sum, value| {
                        sum.add(p(value.abs_upper()).mul(v_scale))
                    }));
            let front = native
                .offset
                .add(p(native.sense).mul(uv.offset[0].add(uv.slope[0].mul(p(target[0])))));
            let slope = p(native.sense).mul(uv.slope[0]).mul(map_slope);
            let phase_scale = p(native.offset.abs_upper())
                .add(uv.offset_scale[0].add(uv.slope_scale[0].mul(target_extent)));
            let amplitude = native
                .cosine
                .into_iter()
                .chain(native.sine)
                .fold(p(0.0), |sum, value| sum.add(p(value.abs_upper())));
            let phase_slope = p(slope.abs_upper());
            let slope_scale = uv.slope_scale[0].mul(p(map_slope.abs_upper()));
            tangent_arithmetic_scale = Some(
                native
                    .arithmetic_scale
                    .mul(phase_slope)
                    .add(amplitude.mul(phase_scale.mul(phase_slope).add(slope_scale))),
            );
            native.arithmetic_scale = native.arithmetic_scale.add(amplitude.mul(phase_scale));
            (native, front, slope, margin[0])
        }
        _ => {
            diagnostic.stage = "unsupported-carrier-expression";
            return Ok(None);
        }
    };
    diagnostic.native_arithmetic_scale = Some(native.arithmetic_scale.hi);
    source.positive_sense();
    if slope.hi < 0.0 {
        front = front.neg();
        slope = slope.neg();
        native.sine = native.sine.map(Interval::neg);
    }
    let source_front = source.offset.add(p(range[0]));
    let phase_error = source_front
        .sub(front)
        .add(p(1.0).sub(slope).mul(Interval {
            lo: 0.0,
            hi: p(range[1]).sub(p(range[0])).hi,
        }))
        .add(Interval {
            lo: -phase_margin,
            hi: phase_margin,
        });
    diagnostic.phase_error = Some([phase_error.lo, phase_error.hi]);
    diagnostic.stage = "phase-enclosure";
    let mut budget = model::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let half = phase_error.mul(p(0.5));
    let trig = model::validated_sin_cos(
        model::ValidatedInterval {
            lower: half.lo,
            upper: half.hi,
        },
        &mut budget,
        cancel,
    );
    parametric::debit_native_work(work, budget.consumed(), cancel)?;
    let (sine, _) = trig.map_err(parametric::native_error)?;
    let phase_distance = p(Interval {
        lo: sine.lower,
        hi: sine.upper,
    }
    .abs_upper())
    .mul(p(2.0));
    let scale = source
        .cosine
        .iter()
        .chain(&source.sine)
        .map(|value| value.abs_upper())
        .fold(0.0_f64, f64::max);
    diagnostic.amplitude_scale = Some(scale);
    diagnostic.phase_distance = Some(phase_distance.hi);
    if !scale.is_finite() || scale == 0.0 {
        diagnostic.stage = "nonpositive-source-amplitude";
        return Ok(None);
    }
    let normalize = |value: Vector<3>| -> Result<Vector<3>, GeometryError> {
        Ok([
            value[0].div(p(scale))?,
            value[1].div(p(scale))?,
            value[2].div(p(scale))?,
        ])
    };
    let difference =
        |a: Vector<3>, b: Vector<3>| normalize(std::array::from_fn(|i| a[i].sub(b[i])));
    let coefficient_error = norm(difference(source.cosine, native.cosine)?)?
        .add(norm(difference(source.sine, native.sine)?)?);
    let amplitude = norm(normalize(native.cosine)?)?.add(norm(normalize(native.sine)?)?);
    // Preserve the original floating evaluation order's error as well as
    // outward coefficient collection. At most 32 wrapper levels per operand,
    // each with fewer than 256 scalar operations, plus the base compositions
    // and phase transport fit below 32768 operations. gamma_n bounds their
    // roundoff; the propagated MIN_POSITIVE floors cover gradual underflow.
    // This numerical reserve consumes the existing physical error allowance.
    let operations = p(32768.0);
    let unit = p(f64::EPSILON);
    let product = operations.mul(unit);
    let gamma = product.div(p(1.0).sub(product))?;
    let roundoff = gamma.mul(
        p(source.arithmetic_scale.hi.max(f64::MIN_POSITIVE))
            .add(p(native.arithmetic_scale.hi.max(f64::MIN_POSITIVE))),
    );
    let center_error = norm(difference(source.center, native.center)?)?;
    let position = center_error
        .add(coefficient_error)
        .add(amplitude.mul(phase_distance))
        .mul(p(scale))
        .add(roundoff);
    diagnostic.position_bound = Some(position.hi);
    diagnostic.center_error = Some(center_error.hi);
    diagnostic.coefficient_error = Some(coefficient_error.hi);
    diagnostic.roundoff = Some(roundoff.hi);
    diagnostic.stage = "position-bound";
    if position.hi > tolerance {
        return Ok(None);
    }
    if require_tangent {
        let u = normalize(source.cosine)?;
        let v = normalize(source.sine)?;
        let lower_square = p(dot(u, u).lo.min(dot(v, v).lo)).sub(p(dot(u, v).abs_upper()));
        if lower_square.lo <= 0.0 {
            diagnostic.stage = "nonpositive-tangent-gram";
            diagnostic.speed_lower = Some(lower_square.lo);
            return Ok(None);
        }
        let speed = lower_square.sqrt()?;
        let tangent_roundoff = gamma.mul(
            p(source.arithmetic_scale.hi.max(f64::MIN_POSITIVE)).add(p(tangent_arithmetic_scale
                .unwrap_or(native.arithmetic_scale)
                .hi
                .max(f64::MIN_POSITIVE))),
        );
        let normalized_roundoff = tangent_roundoff.div(p(scale))?;
        let speed_lower = p(speed.lo).sub(normalized_roundoff).lo;
        let tangent_error = coefficient_error
            .add(p(p(1.0).sub(slope).abs_upper()).mul(amplitude))
            .add(p(slope.abs_upper()).mul(amplitude).mul(phase_distance))
            .add(normalized_roundoff);
        let tangent_limit = p(speed_lower).mul(p(0.25)).lo;
        diagnostic.stage = "relative-tangent";
        diagnostic.speed_lower = Some(speed_lower);
        diagnostic.tangent_error = Some(tangent_error.hi);
        diagnostic.tangent_limit = Some(tangent_limit);
        if speed_lower <= 0.0 || tangent_error.hi >= tangent_limit {
            return Ok(None);
        }
    }
    diagnostic.stage = "proved";
    Ok(Some(position.hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn original_ring(radius: f64, center: Point3, pose: Matrix4) -> model::Curve {
        let circle = model::Curve::ParameterCurve(model::ParameterCurve::new(
            model::Curve2D::Conic(model::Conic2D::Ellipse(
                model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(
                    model::UnitCircle::<Point2>::new(),
                    (0.0, std::f64::consts::TAU),
                )),
            )),
            Box::new(model::Surface::Plane(model::Plane::from_origin_vectors(
                center,
                Vector3::new(radius, 0.0, 0.0),
                Vector3::new(0.0, radius, 0.0),
            ))),
        ));
        model::Curve::TrimmedCurve(model::Processor::with_transform(
            model::TrimmedCurve::new(Box::new(circle), (0.0, std::f64::consts::TAU)),
            pose,
        ))
    }

    fn original_carrier(radius: f64, pose: Matrix4) -> model::Surface {
        model::Surface::RevolutionSurface(model::Processor::with_transform(
            model::RevolutionSurface::by_revolution(
                model::Curve::Line(model::Line(
                    Point3::new(radius, 0.0, 0.0),
                    Point3::new(radius, 0.0, 1.0),
                )),
                Point3::origin(),
                Vector3::unit_z(),
            ),
            pose,
        ))
    }

    fn periodic_chart(turn: f64, slope: f64) -> model::Curve2D {
        model::Curve2D::TrimmedCurve(model::Processor::with_transform(
            model::TrimmedCurve::new(
                Box::new(model::Curve2D::Line(model::Line(
                    Point2::new(0.0, 0.0),
                    Point2::new(0.0, slope),
                ))),
                (0.0, std::f64::consts::TAU),
            ),
            Matrix3::from_translation(Vector2::new(0.0, turn)),
        ))
    }

    fn receipt(
        source: &model::Curve,
        carrier: &model::Surface,
        chart: &model::Curve2D,
        tolerance: f64,
    ) -> Option<f64> {
        mapped_receipt(
            source,
            carrier,
            chart,
            [0.0, std::f64::consts::TAU],
            [0.0, std::f64::consts::TAU],
            tolerance,
        )
    }

    fn mapped_receipt(
        source: &model::Curve,
        carrier: &model::Surface,
        chart: &model::Curve2D,
        range: [f64; 2],
        target: [f64; 2],
        tolerance: f64,
    ) -> Option<f64> {
        certify(
            source,
            carrier,
            chart,
            range,
            target,
            [0.0; 2],
            tolerance,
            true,
            &mut GeometryWorkBudget::new(50_000),
            &AtomicBool::new(false),
            &mut HarmonicDiagnostic::new(tolerance),
        )
        .unwrap()
    }

    #[test]
    fn translated_scaled_periodic_harmonics_keep_spatial_and_phase_error_separate() {
        let pose = Matrix4::from_translation(Vector3::new(1595.0, -795.0, 20.0))
            * Matrix4::from_nonuniform_scale(2.0, 0.5, 1.5);
        let source = original_ring(20.0, Point3::origin(), pose);
        let carrier = original_carrier(20.0, pose);
        for turn in [-std::f64::consts::TAU, std::f64::consts::TAU] {
            let chart = periodic_chart(turn, 1.0);
            let bound = receipt(&source, &carrier, &chart, 1e-7)
                .expect("whole original expressions must retain a bounded periodic receipt");
            assert!(bound > 0.0 && bound <= 1e-7);
            // These evaluations only exercise the returned numerical reserve;
            // the receipt itself proves coefficients and the complete domain.
            for index in 0..=32 {
                let t = std::f64::consts::TAU * f64::from(index) / 32.0;
                let uv = chart.subs(t);
                assert!((source.subs(t) - carrier.subs(uv.x, uv.y)).magnitude() <= bound);
            }
            let displaced = original_ring(20.0, Point3::new(0.001, 0.0, 0.0), pose);
            assert!(receipt(&displaced, &carrier, &chart, 1e-7).is_none());
        }
        assert!(receipt(&source, &carrier, &periodic_chart(0.0, 2.0), 1e-7).is_none());
    }

    #[test]
    fn tiny_harmonic_amplitudes_keep_relative_tangent_authority() {
        let source = original_ring(1e-200, Point3::origin(), Matrix4::identity());
        let carrier = original_carrier(1e-200, Matrix4::identity());
        assert!(
            receipt(
                &source,
                &carrier,
                &periodic_chart(std::f64::consts::TAU, 1.0),
                1e-7,
            )
            .is_some()
        );
        // Both windings are absolutely closer than the positional tolerance,
        // but only the original whole-span relative tangent can authorize one.
        assert!(receipt(&source, &carrier, &periodic_chart(0.0, 2.0), 1e-7).is_none());
    }

    #[test]
    fn equal_coefficients_do_not_remove_large_world_spatial_roundoff_reserve() {
        let pose = Matrix4::from_translation(Vector3::new(1_073_741_824.0, -1_073_741_824.0, 0.0));
        let source = original_ring(20.0, Point3::origin(), pose);
        let carrier = original_carrier(20.0, pose);
        let chart = periodic_chart(std::f64::consts::TAU, 1.0);
        assert!(receipt(&source, &carrier, &chart, 1e-7).is_none());
        let bound = receipt(&source, &carrier, &chart, 0.1)
            .expect("a larger requested allowance may contain the retained spatial reserve");
        assert!(bound > 1e-7 && bound <= 0.1);
    }

    // Original native operands retain world coordinates in their plane/profile,
    // rather than hiding that arithmetic behind a final world transform.
    fn world_ring(radius: f64, center: Point3, uv_translation: Vector2) -> model::Curve {
        let range = (std::f64::consts::TAU, 3.0 * std::f64::consts::PI);
        let u = Vector3::new(-radius, 0.0, 0.0);
        let v = Vector3::new(0.0, -radius, 0.0);
        let origin = center - u * uv_translation.x - v * uv_translation.y;
        let original_uv =
            model::Curve2D::Conic(model::Conic2D::Ellipse(model::Processor::with_transform(
                model::TrimmedCurve::new(model::UnitCircle::<Point2>::new(), range),
                Matrix3::from_translation(uv_translation),
            )));
        let original = model::Curve::ParameterCurve(model::ParameterCurve::new(
            original_uv,
            Box::new(model::Surface::Plane(model::Plane::from_origin_vectors(
                origin, u, v,
            ))),
        ));
        model::Curve::TrimmedCurve(model::Processor::<_, Matrix4>::new(
            model::TrimmedCurve::new(Box::new(original), range),
        ))
    }

    fn world_carrier(center: Point3) -> model::Surface {
        let origin = center - Vector3::new(0.0, 0.0, 20.0);
        let profile = origin - Vector3::new(20.0, 0.0, 0.0);
        let mut processor =
            model::Processor::<_, Matrix4>::new(model::RevolutionSurface::by_revolution(
                model::Curve::Line(model::Line(profile, profile + Vector3::unit_z())),
                origin,
                Vector3::unit_z(),
            ));
        // Preserve the original cylinder's (angle, profile) chart ordering.
        processor.invert();
        model::Surface::RevolutionSurface(processor)
    }

    fn reparameterized_world_chart(
        reflected: bool,
        target_end: f64,
        winding: f64,
    ) -> model::Curve2D {
        let tau = std::f64::consts::TAU;
        let travel = std::f64::consts::PI * winding;
        let (offset, slope) = if reflected {
            (tau + travel, -travel / target_end)
        } else {
            (tau, travel / target_end)
        };
        let mut original = model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(
            Box::new(model::Curve2D::Line(model::Line(
                Point2::new(offset, 20.0),
                Point2::new(offset + slope, 20.0),
            ))),
            (0.0, target_end),
        ));
        if reflected {
            original.invert();
        }
        model::Curve2D::TrimmedCurve(model::Processor::with_transform(
            model::TrimmedCurve::new(
                Box::new(model::Curve2D::TrimmedCurve(original)),
                (0.0, target_end),
            ),
            Matrix3::from_translation(Vector2::new(-2.0 * tau, 0.0)),
        ))
    }

    #[test]
    fn whole_span_world_offsets_reflections_and_uv_reparameterizations_keep_physical_authority() {
        let range = [std::f64::consts::TAU, 3.0 * std::f64::consts::PI];
        for center in [
            Point3::new(595.0, 45.0, 20.0),
            Point3::new(1595.0, 795.0, 20.0),
        ] {
            let carrier = world_carrier(center);
            for uv_translation in [Vector2::new(0.0, 0.0), Vector2::new(2.0, -3.0)] {
                let source = world_ring(20.0, center, uv_translation);
                let wrong_radius = world_ring(20.001, center, uv_translation);
                for reflected in [false, true] {
                    for target_end in [0.25, 1.0, 4.0] {
                        let target = [0.0, target_end];
                        let chart = reparameterized_world_chart(reflected, target_end, 1.0);
                        let bound = mapped_receipt(&source, &carrier, &chart, range, target, 1e-7)
                            .expect("the complete original world/UV expressions must certify");
                        assert!(bound > 0.0 && bound <= 1e-7);
                        // The coefficient receipt above, not these observations,
                        // establishes full-span position and relative tangent.
                        for index in 0..=32 {
                            let fraction = f64::from(index) / 32.0;
                            let t = range[0] + (range[1] - range[0]) * fraction;
                            let uv = chart.subs(target_end * fraction);
                            assert!(
                                (source.subs(t) - carrier.subs(uv.x, uv.y)).magnitude() <= bound
                            );
                        }
                        assert!(
                            mapped_receipt(&wrong_radius, &carrier, &chart, range, target, 1e-7,)
                                .is_none()
                        );
                        let wrong_winding = reparameterized_world_chart(reflected, target_end, 2.0);
                        assert!(
                            mapped_receipt(&source, &carrier, &wrong_winding, range, target, 1e-7,)
                                .is_none()
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn original_unbounded_profile_branch_keeps_reflected_rim_active_gauge_and_whole_proof() {
        let tau = std::f64::consts::TAU;
        let pi = std::f64::consts::PI;
        let ring = |radius, z, range, reflected| {
            let mut uv = model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(
                model::UnitCircle::<Point2>::new(),
                range,
            ));
            if reflected {
                uv.invert();
            }
            model::Curve::ParameterCurve(model::ParameterCurve::new(
                model::Curve2D::Conic(model::Conic2D::Ellipse(uv)),
                Box::new(model::Surface::Plane(model::Plane::from_origin_vectors(
                    Point3::new(0.0, 0.0, z),
                    Vector3::new(radius, 0.0, 0.0),
                    Vector3::new(0.0, radius, 0.0),
                ))),
            ))
        };
        let profile = ring(2.0, 0.0, (0.0, tau), false);
        let carrier = model::Surface::ExtrusionSurface(model::ExtrusionSurface::by_extrusion(
            profile,
            Vector3::new(0.0, 0.0, 8.0),
        ));
        let source = ring(2.0, 8.0, (tau, 3.0 * pi), true);
        let chart = extrusion_profile_chart_proposal(
            &source,
            &carrier,
            ProfileParameterDomain::Unbounded,
            &mut GeometryWorkBudget::new(50_000),
            &AtomicBool::new(false),
        )
        .unwrap()
        .expect("original harmonic branch into native evaluable domain");
        assert_eq!(chart.range_tuple(), (tau, 3.0 * pi));
        let model::Curve2D::BsplineCurve(line) = &chart else {
            panic!("affine proposal");
        };
        assert!(
            line.control_points()
                .iter()
                .all(|point| point.x >= 0.0 && point.x <= tau)
        );
        let range = [tau, 3.0 * pi];
        assert!(mapped_receipt(&source, &carrier, &chart, range, range, 1e-7).is_some());
        assert_eq!(source.range_tuple(), (tau, 3.0 * pi));
        assert!(
            extrusion_profile_chart_proposal(
                &source,
                &carrier,
                ProfileParameterDomain::Bounded([0.0, pi]),
                &mut GeometryWorkBudget::new(50_000),
                &AtomicBool::new(false),
            )
            .unwrap()
            .is_none(),
            "an explicit source trim cannot gain an unbounded periodic branch"
        );
        let changed_chart = |back_u, v| {
            model::Curve2D::BsplineCurve(model::BsplineCurve::new(
                model::KnotVector::from(vec![tau, tau, 3.0 * pi, 3.0 * pi]),
                vec![Point2::new(pi, v), Point2::new(back_u, v)],
            ))
        };
        for wrong in [changed_chart(tau, 1.0), changed_chart(0.0, 1.001)] {
            assert!(mapped_receipt(&source, &carrier, &wrong, range, range, 1e-7).is_none());
        }
        let wrong_radius = ring(2.001, 8.0, (tau, 3.0 * pi), true);
        assert!(mapped_receipt(&wrong_radius, &carrier, &chart, range, range, 1e-7).is_none());
        assert!(
            mapped_receipt(
                &source,
                &carrier,
                &chart,
                [tau, tau + pi / 2.0],
                range,
                1e-7
            )
            .is_none()
        );
        let wrong_direction =
            model::Surface::ExtrusionSurface(model::ExtrusionSurface::by_extrusion(
                ring(2.0, 0.0, (0.0, tau), false),
                Vector3::new(0.01, 0.0, 8.0),
            ));
        assert!(mapped_receipt(&source, &wrong_direction, &chart, range, range, 1e-7).is_none());
    }
}
