// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Error-enclosed arithmetic and native bounded parameter adapters. Every basic
//! IEEE-754 operation is padded by one adjacent representable number. Inputs are
//! exact binary64 coefficients; parameter/extraction/transform errors must be
//! represented in their intervals rather than hidden in a sampled epsilon.

use crate::{GeometryError, GeometryWorkBudget, PlaneFrameMm, check_cancel};
use monstertruck_modeling::{Curve, ParametricCurve, Surface};
use spiling_contracts::geometry::{
    GeometryErrorCode, SECTION_PLANE_TOLERANCE_MM, SECTION_SAMPLING_TOLERANCE_MM,
};
use std::sync::atomic::AtomicBool;

pub(crate) fn down(value: f64) -> f64 {
    if value.is_nan() || value == f64::NEG_INFINITY {
        return value;
    }
    if value == 0.0 {
        return -f64::from_bits(1);
    }
    f64::from_bits(if value > 0.0 {
        value.to_bits() - 1
    } else {
        value.to_bits() + 1
    })
}
pub(crate) fn up(value: f64) -> f64 {
    if value.is_nan() || value == f64::INFINITY {
        return value;
    }
    if value == 0.0 {
        return f64::from_bits(1);
    }
    f64::from_bits(if value > 0.0 {
        value.to_bits() + 1
    } else {
        value.to_bits() - 1
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Interval {
    pub lo: f64,
    pub hi: f64,
}
impl Interval {
    pub(crate) fn new(lo: f64, hi: f64) -> Result<Self, GeometryError> {
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "invalid finite parameter interval",
            ));
        }
        Ok(Self { lo, hi })
    }
    pub(crate) fn point(value: f64) -> Self {
        Self {
            lo: value,
            hi: value,
        }
    }
    pub(crate) fn hull(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }
    pub(crate) fn add(self, other: Self) -> Self {
        if self.lo == 0.0 && self.hi == 0.0 {
            return other;
        }
        if other.lo == 0.0 && other.hi == 0.0 {
            return self;
        }
        if self.lo == self.hi && other.lo == other.hi && self.lo == -other.lo {
            return Self::point(0.0);
        }
        if self.lo == self.hi && other.lo == other.hi {
            let sum = self.lo + other.lo;
            if sum.is_finite() {
                // Error-free TwoSum uses only addition/subtraction. Gradual
                // underflow cannot hide a residual below the binary64 input
                // lattice, unlike an unguarded TwoProduct/FMA-zero test.
                let virtual_other = sum - self.lo;
                let residual = (self.lo - (sum - virtual_other)) + (other.lo - virtual_other);
                if residual == 0.0 {
                    return Self::point(sum);
                }
            }
        }
        Self {
            lo: down(self.lo + other.lo),
            hi: up(self.hi + other.hi),
        }
    }
    pub(crate) fn neg(self) -> Self {
        Self {
            lo: -self.hi,
            hi: -self.lo,
        }
    }
    pub(crate) fn sub(self, other: Self) -> Self {
        self.add(other.neg())
    }
    pub(crate) fn mul(self, other: Self) -> Self {
        if (self.lo == 0.0 && self.hi == 0.0) || (other.lo == 0.0 && other.hi == 0.0) {
            return Self::point(0.0);
        }
        if self.lo == 1.0 && self.hi == 1.0 {
            return other;
        }
        if other.lo == 1.0 && other.hi == 1.0 {
            return self;
        }
        if self.lo == -1.0 && self.hi == -1.0 {
            return other.neg();
        }
        if other.lo == -1.0 && other.hi == -1.0 {
            return self.neg();
        }
        let products = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        if products.iter().any(|v| v.is_nan()) {
            return Self {
                lo: f64::NEG_INFINITY,
                hi: f64::INFINITY,
            };
        }
        Self {
            lo: down(products.iter().copied().fold(f64::INFINITY, f64::min)),
            hi: up(products.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        }
    }
    pub(crate) fn div(self, other: Self) -> Result<Self, GeometryError> {
        if other.lo <= 0.0 && other.hi >= 0.0 {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "whole-interval rational denominator cannot be separated from zero",
            ));
        }
        if other.lo == 1.0 && other.hi == 1.0 {
            return Ok(self);
        }
        if other.lo == -1.0 && other.hi == -1.0 {
            return Ok(self.neg());
        }
        if self.lo == 0.0 && self.hi == 0.0 {
            return Ok(Self::point(0.0));
        }
        if self.lo == self.hi && other.lo == other.hi && self.lo == other.lo {
            return Ok(Self::point(1.0));
        }
        Ok(self.mul(Self {
            lo: down(1.0 / other.hi),
            hi: up(1.0 / other.lo),
        }))
    }
    pub(crate) fn sqrt(self) -> Result<Self, GeometryError> {
        if self.lo < 0.0 || !self.hi.is_finite() {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "invalid square-root enclosure",
            ));
        }
        if self.lo == self.hi && (self.lo == 0.0 || self.lo == 1.0) {
            return Ok(self);
        }
        Ok(Self {
            lo: down(self.lo.sqrt()).max(0.0),
            hi: up(self.hi.sqrt()),
        })
    }
    pub(crate) fn square(self) -> Self {
        if self.lo == self.hi {
            if self.lo == 0.0 {
                return Self::point(0.0);
            }
            if self.lo == 1.0 || self.lo == -1.0 {
                return Self::point(1.0);
            }
        }
        if self.lo <= 0.0 && self.hi >= 0.0 {
            Self {
                lo: 0.0,
                hi: up(self.abs_upper() * self.abs_upper()),
            }
        } else {
            Self {
                lo: down((self.lo * self.lo).min(self.hi * self.hi)).max(0.0),
                hi: up((self.lo * self.lo).max(self.hi * self.hi)),
            }
        }
    }
    pub(crate) fn width(self) -> f64 {
        up(self.hi - self.lo)
    }
    pub(crate) fn mid(self) -> f64 {
        self.lo * 0.5 + self.hi * 0.5
    }
    pub(crate) fn abs_upper(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }
    pub(crate) fn contains_zero(self) -> bool {
        self.lo <= 0.0 && self.hi >= 0.0
    }
}

pub(crate) fn dot_point(point: [f64; 3], direction: [f64; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(Interval::point(point[axis]).mul(Interval::point(direction[axis])))
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SurfaceEnclosure {
    pub position: [Interval; 3],
    pub du: [Interval; 3],
    pub dv: [Interval; 3],
    pub duu: [Interval; 3],
    pub duv: [Interval; 3],
    pub dvv: [Interval; 3],
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct CurveEnclosure {
    pub position: [Interval; 3],
    pub derivative: [Interval; 3],
    pub second_derivative: [Interval; 3],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChartCurveEnclosure {
    pub position: [Interval; 2],
    pub derivative: [Interval; 2],
    pub second_derivative: [Interval; 2],
}

pub(crate) struct CurveIntervalEnclosure {
    pub start_mm: [f64; 3],
    pub end_mm: [f64; 3],
    pub chord_error_mm: f64,
    pub plane_error_mm: f64,
}

pub(crate) fn native_error(error: monstertruck_modeling::ValidatedGeometryError) -> GeometryError {
    use monstertruck_modeling::ValidatedGeometryError as E;
    let code = match error {
        E::Cancelled => GeometryErrorCode::Cancelled,
        E::ResourceLimit => GeometryErrorCode::ResourceLimit,
        E::InvalidInput | E::ParameterOutOfRange => GeometryErrorCode::InvalidGeometry,
        E::Unsupported => GeometryErrorCode::UnsupportedOperation,
        E::DenominatorContainsZero | E::ConditioningFailure => {
            GeometryErrorCode::ConditioningFailure
        }
        E::NonSmooth | E::Singular => GeometryErrorCode::SingularGeometry,
        E::Unresolved
        | E::PlaneCoverageUnresolved { .. }
        | E::AmbiguousBranch
        | E::NonTransverse => GeometryErrorCode::AnalysisIndeterminate,
    };
    GeometryError::new(
        code,
        format!("native bounded parameter analysis: {error:?}"),
    )
}

pub(crate) fn debit_native_work(
    work: &mut GeometryWorkBudget,
    consumed: usize,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    // Cancellation must not erase computation already charged by the native
    // operation, including a failed operation. Preserve cancellation priority
    // after retaining that cumulative debit.
    work.charge(consumed as u64, &AtomicBool::new(false))?;
    check_cancel(cancel)
}

fn adapt(value: monstertruck_modeling::ValidatedInterval) -> Interval {
    Interval {
        lo: value.lower,
        hi: value.upper,
    }
}

/// The native concrete evaluator partitions at knots and encloses extraction,
/// affine transforms and rational division. Its work is debited even on error.
pub(crate) fn surface_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    check_cancel(cancel)?;
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result =
        monstertruck_modeling::validated_surface_jet(surface, uv, &mut native_work, cancel);
    debit_native_work(work, native_work.consumed(), cancel)?;
    let jet = result.map_err(native_error)?;
    let mut enclosure = adapt_surface_jet(jet);
    tighten_revolution(surface, uv, &mut enclosure, work, cancel)?;
    Ok(enclosure)
}

/// Source-geometric orientation, including a proved regular geometric carrier
/// at a singular UV chart. This does not use the kernel's sampled/nearby-normal
/// fallbacks and does not apply the owning topology face's orientation.
pub(crate) fn surface_geometric_normal_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    check_cancel(cancel)?;
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = monstertruck_modeling::validated_surface_geometric_normal(
        surface,
        uv,
        &mut native_work,
        cancel,
    );
    debit_native_work(work, native_work.consumed(), cancel)?;
    Ok(result.map_err(native_error)?.map(adapt))
}

/// Continuous-source images and one-sided derivative ranges, not a C1 or
/// normal certificate. Hessian consumers must separately qualify the span.
/// Do not apply strict profile-based tightening to an unqualified C0 range.
pub(crate) fn surface_piecewise_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    check_cancel(cancel)?;
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = monstertruck_modeling::validated_surface_piecewise_jet(
        surface,
        uv,
        &mut native_work,
        cancel,
    );
    debit_native_work(work, native_work.consumed(), cancel)?;
    let mut enclosure = adapt_surface_jet(result.map_err(native_error)?);
    if matches!(surface, Surface::RevolutionSurface(_))
        && surface_parameter_span_smooth(surface, uv, work, cancel)?
    {
        tighten_revolution(surface, uv, &mut enclosure, work, cancel)?;
    }
    Ok(enclosure)
}

fn adapt_surface_jet(jet: monstertruck_modeling::ValidatedSurfaceJet) -> SurfaceEnclosure {
    SurfaceEnclosure {
        position: jet.position.map(adapt),
        du: jet.du.map(adapt),
        dv: jet.dv.map(adapt),
        duu: jet.duu.map(adapt),
        duv: jet.duv.map(adapt),
        dvv: jet.dvv.map(adapt),
    }
}

/// Collect the stored Rodrigues coefficients BEFORE interval evaluation. The
/// native expression `d*cos + (a dot d)*a*(1-cos)` loses the correlation in its
/// axial component over a full turn. Expanding its linear coefficient instead
/// gives `aa^T + (I-aa^T)*cos + C*sin`, without assuming the stored axis is
/// mathematically unit length. This is valid for every admitted profile, not
/// just a line, and retains exact axial cancellations through affine poses.
fn tighten_revolution(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    enclosure: &mut SurfaceEnclosure,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let Surface::RevolutionSurface(processor) = surface else {
        return Ok(());
    };
    let matrix = processor.transform();
    // Projective poses still use the complete native denominator-enclosed jet.
    if (0..3).any(|i| matrix[i][3] != 0.0) || matrix[3][3] != 1.0 {
        return Ok(());
    }
    work.charge(256, cancel)?;
    let revolution = processor.entity();
    let profile_axis = usize::from(!processor.orientation());
    let profile_range = uv[profile_axis];
    let angle = uv[1 - profile_axis];
    let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let trig = monstertruck_modeling::validated_sin_cos(
        monstertruck_modeling::ValidatedInterval {
            lower: angle[0],
            upper: angle[1],
        },
        &mut native,
        cancel,
    );
    debit_native_work(work, native.consumed(), cancel)?;
    let (sine, cosine) = trig.map_err(native_error)?;
    let (sine, cosine) = (adapt(sine), adapt(cosine));
    let axis = revolution.axis();
    let axis = [axis.x, axis.y, axis.z].map(Interval::point);
    let origin = revolution.origin();
    let origin_point = [origin.x, origin.y, origin.z];
    let origin = origin_point.map(Interval::point);
    let zero = Interval::point(0.0);
    // C*d is exactly the cross term used by the native revolution evaluator.
    let cross = [
        [zero, axis[2].neg(), axis[1]],
        [axis[2], zero, axis[0].neg()],
        [axis[1].neg(), axis[0], zero],
    ];
    let mut profile_jets: Option<CurveEnclosure> = None;
    for row in 0..3 {
        let axial = (0..3).fold(zero, |sum, i| {
            sum.add(Interval::point(matrix[i][row]).mul(axis[i]))
        });
        let constant: [Interval; 3] = std::array::from_fn(|column| axial.mul(axis[column]));
        let cos = std::array::from_fn(|column| {
            Interval::point(matrix[column][row]).sub(constant[column])
        });
        let sin = std::array::from_fn(|column| {
            (0..3).fold(zero, |sum, i| {
                sum.add(Interval::point(matrix[i][row]).mul(cross[i][column]))
            })
        });
        let mut projected = |coefficients: [Interval; 3]| -> Result<[Interval; 3], GeometryError> {
            if coefficients.iter().all(|v| v.lo == 0.0 && v.hi == 0.0) {
                return Ok([zero; 3]);
            }
            // The public scalar native API accepts exact binary64 coefficients.
            // Project their representatives before separating coordinates, then
            // retain every coefficient-rounding remainder in an interval dot.
            let direction = coefficients.map(Interval::mid);
            let remainder: [Interval; 3] =
                std::array::from_fn(|i| coefficients[i].sub(Interval::point(direction[i])));
            let residual = if remainder.iter().all(|v| v.lo == 0.0 && v.hi == 0.0) {
                [zero; 3]
            } else {
                let profile = match profile_jets {
                    Some(profile) => profile,
                    None => {
                        let profile = curve_enclosure(
                            revolution.entity_curve(),
                            (profile_range[0], profile_range[1]),
                            work,
                            cancel,
                        )?;
                        profile_jets = Some(profile);
                        profile
                    }
                };
                [
                    std::array::from_fn(|i| profile.position[i].sub(origin[i])),
                    profile.derivative,
                    profile.second_derivative,
                ]
                .map(|values| (0..3).fold(zero, |sum, i| sum.add(remainder[i].mul(values[i]))))
            };
            let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
                usize::try_from(work.remaining()).unwrap_or(usize::MAX),
            );
            let result = monstertruck_modeling::validated_curve_projected_jet(
                revolution.entity_curve(),
                profile_range,
                origin_point,
                direction,
                &mut native,
                cancel,
            );
            debit_native_work(work, native.consumed(), cancel)?;
            let jet = result.map_err(native_error)?;
            let derivatives = [jet.position, jet.first, jet.second].map(adapt);
            Ok(std::array::from_fn(|order| {
                derivatives[order].add(residual[order])
            }))
        };
        let constant = projected(constant)?;
        let cosine_coefficient = projected(cos)?;
        let sine_coefficient = projected(sin)?;
        let profile_derivatives: [Interval; 3] = std::array::from_fn(|order| {
            constant[order]
                .add(cosine_coefficient[order].mul(cosine))
                .add(sine_coefficient[order].mul(sine))
        });
        let angular = cosine_coefficient[0]
            .mul(sine)
            .neg()
            .add(sine_coefficient[0].mul(cosine));
        let mixed = cosine_coefficient[1]
            .mul(sine)
            .neg()
            .add(sine_coefficient[1].mul(cosine));
        let angular_second = cosine_coefficient[0]
            .mul(cosine)
            .add(sine_coefficient[0].mul(sine))
            .neg();
        let translated_origin = (0..3).fold(Interval::point(matrix[3][row]), |sum, i| {
            sum.add(Interval::point(matrix[i][row]).mul(origin[i]))
        });
        let position = translated_origin.add(profile_derivatives[0]);
        let intersect = |target: &mut Interval, bound: Interval| -> Result<(), GeometryError> {
            *target = Interval::new(target.lo.max(bound.lo), target.hi.min(bound.hi))?;
            Ok(())
        };
        intersect(&mut enclosure.position[row], position)?;
        let (profile_first, angle_first, profile_second, angle_second) = if profile_axis == 0 {
            (
                &mut enclosure.du[row],
                &mut enclosure.dv[row],
                &mut enclosure.duu[row],
                &mut enclosure.dvv[row],
            )
        } else {
            (
                &mut enclosure.dv[row],
                &mut enclosure.du[row],
                &mut enclosure.dvv[row],
                &mut enclosure.duu[row],
            )
        };
        intersect(profile_first, profile_derivatives[1])?;
        intersect(angle_first, angular)?;
        intersect(profile_second, profile_derivatives[2])?;
        intersect(&mut enclosure.duv[row], mixed)?;
        intersect(angle_second, angular_second)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum KnotSource<'a> {
    Borrowed(&'a [f64]),
    Integer(usize),
}
#[derive(Clone, Copy)]
struct KnotAxis<'a> {
    source: KnotSource<'a>,
    degree: usize,
    sign: f64,
    offset: Interval,
}
impl<'a> KnotAxis<'a> {
    fn new(knots: &'a [f64], degree: usize) -> Self {
        Self {
            source: KnotSource::Borrowed(knots),
            degree,
            sign: 1.0,
            offset: Interval::point(0.0),
        }
    }
    fn len(self) -> usize {
        match self.source {
            KnotSource::Borrowed(knots) => knots.len(),
            KnotSource::Integer(count) => count,
        }
    }
    fn raw(self, index: usize) -> f64 {
        let index = if self.sign > 0.0 {
            index
        } else {
            self.len() - 1 - index
        };
        match self.source {
            KnotSource::Borrowed(knots) => knots[index],
            KnotSource::Integer(_) => index as f64,
        }
    }
    fn value(self, index: usize) -> Interval {
        let value = Interval::point(self.raw(index));
        self.offset
            .add(if self.sign > 0.0 { value } else { value.neg() })
    }
    fn reflect(&mut self, range: (f64, f64)) -> Result<(), GeometryError> {
        Interval::new(range.0, range.1)?;
        let center = Interval::point(range.0).add(Interval::point(range.1));
        self.offset = self.offset.add(if self.sign > 0.0 {
            center
        } else {
            center.neg()
        });
        Interval::new(self.offset.lo, self.offset.hi)?;
        self.sign = -self.sign;
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct AxisSpans<'a> {
    knots: KnotAxis<'a>,
    low: f64,
    high: f64,
    index: usize,
    pending: Option<f64>,
    done: bool,
}
impl<'a> AxisSpans<'a> {
    fn new(knots: KnotAxis<'a>, domain: [f64; 2]) -> Self {
        let (mut low, mut high) = (0, knots.len());
        while low < high {
            let middle = low + (high - low) / 2;
            if knots.value(middle).hi <= domain[0] {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Self {
            knots,
            low: domain[0],
            high: domain[1],
            index: low,
            pending: None,
            done: false,
        }
    }
}
impl Iterator for AxisSpans<'_> {
    type Item = [f64; 2];
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut high = self.high;
        if let Some(value) = self.pending.take() {
            if value > self.low {
                high = value.min(self.high);
            }
        }
        if high == self.high {
            while self.index < self.knots.len() {
                let knot = self.knots.value(self.index);
                self.index += 1;
                if knot.lo > self.low {
                    high = knot.lo.min(self.high);
                    if knot.hi > knot.lo {
                        self.pending = Some(knot.hi);
                    }
                    break;
                }
                if knot.hi > self.low {
                    high = knot.hi.min(self.high);
                    break;
                }
            }
        }
        let result = [self.low, high];
        self.done = high >= self.high;
        self.low = high;
        Some(result)
    }
}
/// Borrowed span product: no per-query knot/control-net copies or patch vector.
pub(crate) struct SurfaceParameterBoxes<'a> {
    u: AxisSpans<'a>,
    v_template: AxisSpans<'a>,
    v: AxisSpans<'a>,
    current_u: Option<[f64; 2]>,
}
impl Iterator for SurfaceParameterBoxes<'_> {
    type Item = [[f64; 2]; 2];
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current_u.is_none() {
                self.current_u = self.u.next();
            }
            let u = self.current_u?;
            if let Some(v) = self.v.next() {
                return Some([u, v]);
            }
            self.current_u = self.u.next();
            self.v = self.v_template.clone();
        }
    }
}
fn profile_knots<'a>(
    mut curve: &'a Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<KnotAxis<'a>, GeometryError> {
    let mut axis = KnotAxis::new(&[], 0);
    for depth in 0..=32 {
        work.charge(1, cancel)?;
        match curve {
            Curve::TrimmedCurve(c) => {
                if depth == 32 {
                    break;
                }
                if !c.orientation() {
                    axis.reflect(c.try_range_tuple().ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ConditioningFailure,
                            "unbounded retained profile trim",
                        )
                    })?)?;
                }
                curve = c.entity().curve().as_ref();
            }
            Curve::BsplineCurve(c) => {
                axis.source = KnotSource::Borrowed(c.knot_vector().as_ref());
                axis.degree = c.degree();
                return Ok(axis);
            }
            Curve::NurbsCurve(c) => {
                axis.source = KnotSource::Borrowed(c.knot_vector().as_ref());
                axis.degree = c.degree();
                return Ok(axis);
            }
            Curve::ParameterCurve(c) => return chart_knots(c.curve(), axis, depth, work, cancel),
            _ => return Ok(axis),
        }
    }
    Err(GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "retained profile basis-wrapper nesting exceeds cap",
    ))
}
/// Borrowed native curve/chart knot partition for zeroth-order bounds. Nested
/// profile-carrier C0 preimages still require actual interval subdivision;
/// these computational cuts are not a global smoothness certificate.
pub(crate) fn curve_parameter_spans<'a>(
    curve: &'a Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<AxisSpans<'a>, GeometryError> {
    Interval::new(range[0], range[1])?;
    let knots = profile_knots(curve, work, cancel)?;
    work.charge(knots.len() as u64, cancel)?;
    Ok(AxisSpans::new(knots, range))
}
fn chart_knots<'a>(
    mut chart: &'a monstertruck_modeling::Curve2D,
    mut axis: KnotAxis<'a>,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<KnotAxis<'a>, GeometryError> {
    for level in depth..=32 {
        work.charge(1, cancel)?;
        match chart {
            monstertruck_modeling::Curve2D::TrimmedCurve(c) => {
                if level == 32 {
                    break;
                }
                if !c.orientation() {
                    axis.reflect(c.try_range_tuple().ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ConditioningFailure,
                            "unbounded retained native chart trim",
                        )
                    })?)?;
                }
                chart = c.entity().curve().as_ref();
            }
            monstertruck_modeling::Curve2D::BsplineCurve(c) => {
                axis.source = KnotSource::Borrowed(c.knot_vector().as_ref());
                axis.degree = c.degree();
                return Ok(axis);
            }
            monstertruck_modeling::Curve2D::NurbsCurve(c) => {
                axis.source = KnotSource::Borrowed(c.knot_vector().as_ref());
                axis.degree = c.degree();
                return Ok(axis);
            }
            monstertruck_modeling::Curve2D::Polyline(c) => {
                axis.source = KnotSource::Integer(c.as_slice().len());
                axis.degree = 1;
                return Ok(axis);
            }
            _ => return Ok(axis),
        }
    }
    Err(GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "retained native chart nesting exceeds cap",
    ))
}
pub(crate) fn curve2d_parameter_spans<'a>(
    curve: &'a monstertruck_modeling::Curve2D,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<AxisSpans<'a>, GeometryError> {
    Interval::new(range[0], range[1])?;
    let knots = chart_knots(curve, KnotAxis::new(&[], 0), 0, work, cancel)?;
    work.charge(knots.len() as u64, cancel)?;
    Ok(AxisSpans::new(knots, range))
}

fn surface_knots<'a>(
    surface: &'a Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[KnotAxis<'a>; 2], GeometryError> {
    let empty = KnotAxis::new(&[], 0);
    Ok(match surface {
        Surface::BsplineSurface(s) => [
            KnotAxis::new(s.knot_vector_u().as_ref(), s.udegree()),
            KnotAxis::new(s.knot_vector_v().as_ref(), s.vdegree()),
        ],
        Surface::NurbsSurface(s) => [
            KnotAxis::new(s.knot_vector_u().as_ref(), s.udegree()),
            KnotAxis::new(s.knot_vector_v().as_ref(), s.vdegree()),
        ],
        Surface::RevolutionSurface(s) => {
            let profile = profile_knots(s.entity().entity_curve(), work, cancel)?;
            if s.orientation() {
                [profile, empty]
            } else {
                [empty, profile]
            }
        }
        Surface::ExtrusionSurface(s) => [profile_knots(s.entity_curve(), work, cancel)?, empty],
        _ => [empty, empty],
    })
}
/// Qualify a Taylor interval, including uncertain reflected knot bands and
/// nested native profile-on-surface expressions. A single extracted patch box
/// is not sufficient: it may itself be an unresolved knot band.
pub(crate) fn surface_parameter_span_smooth(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    surface_span_smooth_at(surface, domain, 0, work, cancel)
}
fn axis_span_smooth(
    knots: KnotAxis<'_>,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    Interval::new(range[0], range[1])?;
    if range[0] == range[1] {
        return Ok(true);
    }
    let (mut low, mut high) = (0, knots.len());
    while low < high {
        work.charge(1, cancel)?;
        let middle = low + (high - low) / 2;
        if knots.value(middle).hi <= range[0] {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    while low < knots.len() {
        work.charge(1, cancel)?;
        let knot = knots.value(low);
        if knot.lo >= range[1] {
            break;
        }
        let value = knots.raw(low);
        let mut end = low + 1;
        while end < knots.len() && knots.raw(end) == value {
            work.charge(1, cancel)?;
            end += 1;
        }
        // C1 with bounded piecewise second derivatives suffices for Taylor.
        // Structural C0 knots require an independent continuity proof; merely
        // small one-sided differences or a single extracted band cannot do it.
        if knot.lo < range[1] && range[0] < knot.hi && end - low >= knots.degree {
            return Ok(false);
        }
        low = end;
    }
    Ok(true)
}
fn surface_span_smooth_at(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if depth > 32 {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "native Taylor expression nesting exceeds bound",
        ));
    }
    for (axis, knots) in surface_knots(surface, work, cancel)?
        .into_iter()
        .enumerate()
    {
        if !axis_span_smooth(knots, domain[axis], work, cancel)? {
            return Ok(false);
        }
    }
    let profile = match surface {
        Surface::RevolutionSurface(s) => {
            Some((s.entity().entity_curve(), usize::from(!s.orientation())))
        }
        Surface::ExtrusionSurface(s) => Some((s.entity_curve(), 0)),
        _ => None,
    };
    if let Some((curve, axis)) = profile {
        if domain[axis][0] != domain[axis][1] {
            return curve_span_smooth_at(curve, domain[axis], depth + 1, work, cancel);
        }
    }
    Ok(true)
}
fn curve_span_smooth_at(
    mut curve: &Curve,
    range: [f64; 2],
    mut depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if !axis_span_smooth(profile_knots(curve, work, cancel)?, range, work, cancel)? {
        return Ok(false);
    }
    let mut parameter = Interval::new(range[0], range[1])?;
    loop {
        work.charge(1, cancel)?;
        if depth > 32 {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "native Taylor profile nesting exceeds bound",
            ));
        }
        match curve {
            Curve::TrimmedCurve(wrapped) => {
                if !wrapped.orientation() {
                    let (a, b) = wrapped.try_range_tuple().ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ConditioningFailure,
                            "unbounded native Taylor profile",
                        )
                    })?;
                    parameter = Interval::point(a).add(Interval::point(b)).sub(parameter);
                }
                curve = wrapped.entity().curve().as_ref();
                depth += 1;
            }
            Curve::ParameterCurve(composed) => {
                let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
                    usize::try_from(work.remaining()).unwrap_or(usize::MAX),
                );
                let result = monstertruck_modeling::validated_curve2d_jet(
                    composed.curve(),
                    [parameter.lo, parameter.hi],
                    &mut native,
                    cancel,
                );
                debit_native_work(work, native.consumed(), cancel)?;
                let jet = result.map_err(native_error)?;
                let domain = std::array::from_fn(|axis| {
                    let p = jet.position[axis];
                    if jet.first[axis].lower == 0.0
                        && jet.first[axis].upper == 0.0
                        && jet.second[axis].lower == 0.0
                        && jet.second[axis].upper == 0.0
                    {
                        [Interval {
                            lo: p.lower,
                            hi: p.upper,
                        }
                        .mid(); 2]
                    } else {
                        [p.lower, p.upper]
                    }
                });
                return surface_span_smooth_at(
                    composed.surface().as_ref(),
                    domain,
                    depth + 1,
                    work,
                    cancel,
                );
            }
            Curve::IntersectionCurve(_) => return Ok(false),
            _ => return Ok(true),
        }
    }
}

pub(crate) fn surface_parameter_boxes<'a>(
    surface: &'a Surface,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceParameterBoxes<'a>, GeometryError> {
    for span in domain {
        Interval::new(span[0], span[1])?;
    }
    let knots = surface_knots(surface, work, cancel)?;
    let u = AxisSpans::new(knots[0], domain[0]);
    let v = AxisSpans::new(knots[1], domain[1]);
    let count = u
        .clone()
        .count()
        .checked_mul(v.clone().count())
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "surface knot-span product overflow",
            )
        })?;
    if count > 1_000_000 {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "surface knot-span product exceeds limit",
        ));
    }
    work.charge(count as u64, cancel)?;
    Ok(SurfaceParameterBoxes {
        u,
        v_template: v.clone(),
        v,
        current_u: None,
    })
}

#[derive(Clone)]
pub(crate) struct SurfaceCreaseInterfaces<'a> {
    axes: [KnotAxis<'a>; 2],
    domain: [[f64; 2]; 2],
    axis: usize,
    index: usize,
    active: Option<(usize, Interval, AxisSpans<'a>)>,
}
impl Iterator for SurfaceCreaseInterfaces<'_> {
    type Item = [[f64; 2]; 2];
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((axis, value, spans)) = &mut self.active {
                if let Some(span) = spans.next() {
                    let mut result = self.domain;
                    result[*axis] = [value.lo, value.hi];
                    result[1 - *axis] = span;
                    return Some(result);
                }
                self.active = None;
            }
            if self.axis >= 2 {
                return None;
            }
            let knots = self.axes[self.axis];
            if self.index == knots.len() {
                self.axis += 1;
                self.index = 0;
                continue;
            }
            let start = self.index;
            let raw = knots.raw(start);
            let value = knots.value(start);
            while self.index < knots.len() && knots.raw(self.index) == raw {
                self.index += 1;
            }
            if value.hi > self.domain[self.axis][0]
                && value.lo < self.domain[self.axis][1]
                && self.index - start >= knots.degree
            {
                let other = 1 - self.axis;
                self.active = Some((
                    self.axis,
                    value,
                    AxisSpans::new(self.axes[other], self.domain[other]),
                ));
            }
        }
    }
}
/// Potential C0-or-worse source interfaces need lower-dimensional analysis.
/// Knot multiplicity alone never proves an actual geometric crease or event.
pub(crate) fn surface_crease_interfaces<'a>(
    surface: &'a Surface,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceCreaseInterfaces<'a>, GeometryError> {
    for span in domain {
        Interval::new(span[0], span[1])?;
    }
    let interfaces = SurfaceCreaseInterfaces {
        axes: surface_knots(surface, work, cancel)?,
        domain,
        axis: 0,
        index: 0,
        active: None,
    };
    let count = interfaces.clone().take(1_000_001).count();
    if count > 1_000_000 {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "surface crease-interface product exceeds limit",
        ));
    }
    work.charge(count as u64, cancel)?;
    Ok(interfaces)
}

pub(crate) fn curve_enclosure(
    curve: &Curve,
    range: (f64, f64),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CurveEnclosure, GeometryError> {
    check_cancel(cancel)?;
    // Implicit curves require an original-range capture, including under native
    // trim/pose operators. Static jets must never choose fresh projected roots.
    let mut basis = curve;
    let mut depth = 0;
    while let Curve::TrimmedCurve(wrapper) = basis {
        work.charge(1, cancel)?;
        if depth == 32 {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "native curve basis nesting exceeds retained source limit",
            ));
        }
        depth += 1;
        basis = wrapper.entity().curve().as_ref();
    }
    if matches!(basis, Curve::IntersectionCurve(_)) {
        return Err(GeometryError::new(
            GeometryErrorCode::UnsupportedOperation,
            "implicit curves require a retained prepared branch",
        ));
    }
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = monstertruck_modeling::validated_curve_jet(
        curve,
        [range.0, range.1],
        &mut native_work,
        cancel,
    );
    debit_native_work(work, native_work.consumed(), cancel)?;
    let jet = result.map_err(native_error)?;
    Ok(CurveEnclosure {
        position: jet.position.map(adapt),
        derivative: jet.first.map(adapt),
        second_derivative: jet.second.map(adapt),
    })
}

fn norm_upper(vector: [Interval; 3]) -> Result<f64, GeometryError> {
    let squared = vector
        .into_iter()
        .fold(Interval::point(0.0), |sum, x| sum.add(x.square()));
    // Sums of squares are nonnegative analytically; outward addition at exact
    // zero can introduce a negative subnormal lower endpoint.
    Ok(Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi,
    }
    .sqrt()?
    .hi)
}

/// One original-range curve capture. Implicit branch selection is performed
/// once, and all adaptive children contract the same native continuation tube.
pub(crate) struct PreparedCurveEnclosure<'a> {
    native: PreparedNative<'a>,
    scale: f64,
    active: Option<(f64, f64)>,
}
enum PreparedNative<'a> {
    Owned(monstertruck_modeling::PreparedValidatedCurve<'a>),
    Borrowed(&'a monstertruck_modeling::PreparedValidatedCurve<'static>),
}

/// One exact native parameter on one immutable prepared binding and unit view.
#[derive(Clone, Copy)]
pub(crate) struct CurveSamplingEndpoint<'a, 'source> {
    binding: &'a PreparedCurveEnclosure<'source>,
    parameter: f64,
    point: monstertruck_modeling::ValidatedCurvePoint,
}
impl CurveSamplingEndpoint<'_, '_> {
    pub(crate) fn parameter(&self) -> f64 {
        self.parameter
    }
}

/// Chord and Green queries share one certified whole-query curvature bound.
pub(crate) struct CurveSamplingInterval<'a, 'source> {
    binding: &'a PreparedCurveEnclosure<'source>,
    range: (f64, f64),
    enclosure: CurveIntervalEnclosure,
    jet: Option<CurveEnclosure>,
}
impl CurveSamplingInterval<'_, '_> {
    pub(crate) fn enclosure(&self) -> &CurveIntervalEnclosure {
        &self.enclosure
    }

    /// Integrates this exact interval from its original native endpoint receipts.
    pub(crate) fn signed_area(
        &self,
        endpoints: &[CurveSamplingEndpoint<'_, '_>; 2],
        frame: &PlaneFrameMm,
        reference_mm: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[f64; 2], GeometryError> {
        work.charge(1, cancel)?;
        frame.validate()?;
        if !std::ptr::eq(endpoints[0].binding, self.binding)
            || !std::ptr::eq(endpoints[1].binding, self.binding)
            || (endpoints[0].parameter, endpoints[1].parameter) != self.range
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "native Green endpoints do not own this prepared interval",
            ));
        }
        if self.range.0 == self.range.1 {
            return Ok([0.0; 2]);
        }
        if let Some(jet) = &self.jet {
            let endpoint_bounds = endpoints.each_ref().map(|endpoint| {
                endpoint
                    .point
                    .error
                    .map(|value| adapt(value).mul(Interval::point(self.binding.scale)))
            });
            return green::signed_area_from_curvature(
                frame,
                reference_mm,
                &endpoint_bounds,
                &jet.second_derivative,
                self.range,
                work,
                cancel,
            );
        }
        let enclosure = self.binding.jet(self.range, work, cancel)?;
        let projection = |axis: [f64; 3], derivative: bool| {
            (0..3).fold(Interval::point(0.0), |sum, index| {
                let value = if derivative {
                    enclosure.derivative[index]
                } else {
                    enclosure.position[index].sub(Interval::point(reference_mm[index]))
                };
                sum.add(value.mul(Interval::point(axis[index])))
            })
        };
        let area = projection(frame.x_axis, false)
            .mul(projection(frame.y_axis, true))
            .sub(projection(frame.y_axis, false).mul(projection(frame.x_axis, true)))
            .mul(Interval::point(self.range.1).sub(Interval::point(self.range.0)))
            .mul(Interval::point(0.5));
        if !area.lo.is_finite() || !area.hi.is_finite() {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "native section signed-area enclosure is nonfinite",
            ));
        }
        Ok([area.lo, area.hi])
    }
}
impl<'a> std::ops::Deref for PreparedNative<'a> {
    type Target = monstertruck_modeling::PreparedValidatedCurve<'a>;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(value) => value,
            Self::Borrowed(value) => value,
        }
    }
}
pub(crate) fn prepare_curve_enclosure<'a>(
    curve: &'a Curve,
    range: (f64, f64),
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedCurveEnclosure<'a>, GeometryError> {
    check_cancel(cancel)?;
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let options = monstertruck_modeling::ValidatedCurveOptions {
        max_retained_bytes: remaining_bytes,
        ..monstertruck_modeling::ValidatedCurveOptions::default()
    };
    let result = monstertruck_modeling::prepared_validated_curve(
        curve,
        [range.0, range.1],
        &options,
        &mut native_work,
        cancel,
    );
    debit_native_work(work, native_work.consumed(), cancel)?;
    Ok(PreparedCurveEnclosure {
        native: PreparedNative::Owned(result.map_err(native_error)?),
        scale: 1.0,
        active: Some(range),
    })
}
impl crate::Definition {
    /// Borrow the retained original branch with the definition's lifetime,
    /// independently of any temporary native boundary-curve mutex guard.
    pub(crate) fn original_boundary_curve(
        &self,
        edge: monstertruck_topology::EdgeId<Curve>,
        range: (f64, f64),
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<PreparedCurveEnclosure<'_>>, GeometryError> {
        check_cancel(cancel)?;
        if self.original_continuations.is_empty() {
            return Ok(None);
        }
        work.charge(1, cancel)?;
        let index = self
            .boundary_curve_indices
            .get(&edge)
            .copied()
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::KernelFailure,
                    "native boundary query lost original edge identity",
                )
            })?;
        let continuation = self.boundary_continuations.get(index).ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::KernelFailure,
                "native boundary query lost its continuation slot",
            )
        })?;
        if let Some(continuation) = continuation {
            if continuation.edge != Some(edge)
                || range.0 < continuation.source_range.0
                || range.1 > continuation.source_range.1
                || range.0 > range.1
            {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "original implicit query exceeds its retained restricted interval",
                ));
            }
            let prepared = self
                .original_continuations
                .get(continuation.prepared)
                .ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::KernelFailure,
                        "original implicit continuation owner is missing",
                    )
                })?;
            return Ok(Some(PreparedCurveEnclosure::borrowed_restricted(
                prepared,
                continuation.scale_to_mm,
                continuation.source_range,
            )));
        }
        if self
            .boundary_parameters
            .get(index)
            .and_then(Option::as_ref)
            .is_none()
        {
            return Err(GeometryError::new(
                GeometryErrorCode::KernelFailure,
                "original implicit source cannot be recaptured as an explicit native curve",
            ));
        }
        Ok(None)
    }
}

impl<'a> PreparedCurveEnclosure<'a> {
    /// Borrow the already selected native continuation without preparing a branch.
    pub(crate) fn native_receipt(&self) -> &monstertruck_modeling::PreparedValidatedCurve<'a> {
        &self.native
    }

    /// Preserve the immutable definition's original proof lifetime across a
    /// temporary boundary-carrier borrow. Query-owned explicit curves return none.
    pub(crate) fn original_receipt(
        &self,
    ) -> Option<&'a monstertruck_modeling::PreparedValidatedCurve<'static>> {
        match &self.native {
            PreparedNative::Borrowed(receipt) => Some(*receipt),
            PreparedNative::Owned(_) => None,
        }
    }

    pub(crate) fn scale_to_mm(&self) -> f64 {
        self.scale
    }
}

impl<'source> PreparedCurveEnclosure<'source> {
    /// Query-owned bytes only: the immutable definition's shared source was
    /// charged once at admission, not again for each use or occurrence.
    pub(crate) fn retained_bytes(&self) -> usize {
        match &self.native {
            PreparedNative::Owned(value) => value.retained_bytes(),
            PreparedNative::Borrowed(_) => 0,
        }
    }
    #[cfg(test)]
    pub(crate) fn borrowed<'a>(
        native: &'a monstertruck_modeling::PreparedValidatedCurve<'static>,
        scale: f64,
    ) -> PreparedCurveEnclosure<'a> {
        PreparedCurveEnclosure {
            native: PreparedNative::Borrowed(native),
            scale,
            active: None,
        }
    }
    pub(crate) fn borrowed_restricted<'a>(
        native: &'a monstertruck_modeling::PreparedValidatedCurve<'static>,
        scale: f64,
        range: (f64, f64),
    ) -> PreparedCurveEnclosure<'a> {
        PreparedCurveEnclosure {
            native: PreparedNative::Borrowed(native),
            scale,
            active: Some(range),
        }
    }
    fn check_range(&self, range: (f64, f64)) -> Result<(), GeometryError> {
        if !range.0.is_finite()
            || !range.1.is_finite()
            || range.0 > range.1
            || self
                .active
                .is_some_and(|active| range.0 < active.0 || range.1 > active.1)
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "retained original curve query leaves its active source restriction",
            ));
        }
        Ok(())
    }
    /// Original explicit knots and retained continuation chart partitions.
    /// These seed adaptive queries; they alone do not qualify a Taylor bound
    /// across an underlying source-surface C0 stratum. Use `is_smooth_on`.
    pub(crate) fn parameter_intervals(&self) -> impl DoubleEndedIterator<Item = (f64, f64)> + '_ {
        self.native.pieces().iter().filter_map(move |piece| {
            let (a, b) = self
                .active
                .map_or((piece.parameter[0], piece.parameter[1]), |active| {
                    (
                        piece.parameter[0].max(active.0),
                        piece.parameter[1].min(active.1),
                    )
                });
            (a < b).then_some((a, b))
        })
    }
    pub(crate) fn is_smooth_on(
        &self,
        range: (f64, f64),
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        check_cancel(cancel)?;
        self.check_range(range)?;
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = self
            .native
            .is_smooth_on([range.0, range.1], &mut native, cancel);
        debit_native_work(work, native.consumed(), cancel)?;
        result.map_err(native_error)
    }
    /// Independent whole native image enclosure. No differentiability/Taylor
    /// premise is attached to this bound across retained coordinate joints.
    pub(crate) fn position_bounds(
        &self,
        range: (f64, f64),
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        check_cancel(cancel)?;
        self.check_range(range)?;
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = self
            .native
            .position_bounds([range.0, range.1], &mut native, cancel);
        debit_native_work(work, native.consumed(), cancel)?;
        Ok(result
            .map_err(native_error)?
            .map(|value| adapt(value).mul(Interval::point(self.scale))))
    }
    pub(crate) fn endpoint_position(
        &self,
        parameter: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        check_cancel(cancel)?;
        self.check_range((parameter, parameter))?;
        let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = self.native.evaluate(parameter, &mut native_work, cancel);
        debit_native_work(work, native_work.consumed(), cancel)?;
        Ok(result
            .map_err(native_error)?
            .error
            .map(|value| adapt(value).mul(Interval::point(self.scale))))
    }

    /// Actual (unnormalized) scalar functional, folded into explicit native
    /// coefficients before interval evaluation so geometric correlations survive.
    pub(crate) fn projected_position_enclosure(
        &self,
        range: (f64, f64),
        origin: [f64; 3],
        direction: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        check_cancel(cancel)?;
        self.check_range(range)?;
        let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result =
            self.projected_with_native_budget(range, origin, direction, &mut native_work, cancel);
        debit_native_work(work, native_work.consumed(), cancel)?;
        result
    }

    fn projected_with_native_budget(
        &self,
        range: (f64, f64),
        origin: [f64; 3],
        direction: [f64; 3],
        native_work: &mut monstertruck_modeling::ValidatedGeometryBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        // Evaluate in the original source frame before the actual unit map.
        let native_origin = if self.scale == 1.0 { origin } else { [0.0; 3] };
        let jet = self
            .native
            .projected_jet(
                [range.0, range.1],
                native_origin,
                direction,
                native_work,
                cancel,
            )
            .map_err(native_error)?;
        if self.scale == 1.0 {
            Ok(adapt(jet.position))
        } else {
            Ok(adapt(jet.position)
                .mul(Interval::point(self.scale))
                .sub(dot_point(origin, direction)))
        }
    }

    pub(crate) fn jet(
        &self,
        range: (f64, f64),
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveEnclosure, GeometryError> {
        check_cancel(cancel)?;
        self.check_range(range)?;
        let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = self.refined_with_native_budget(range, &mut native_work, cancel);
        debit_native_work(work, native_work.consumed(), cancel)?;
        result
    }

    fn refined_with_native_budget(
        &self,
        range: (f64, f64),
        native_work: &mut monstertruck_modeling::ValidatedGeometryBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveEnclosure, GeometryError> {
        let jet = self
            .native
            .jet([range.0, range.1], native_work, cancel)
            .map_err(native_error)?;
        let scaled = |value| adapt(value).mul(Interval::point(self.scale));
        Ok(CurveEnclosure {
            position: jet.position.map(scaled),
            derivative: jet.first.map(scaled),
            second_derivative: jet.second.map(scaled),
        })
    }

    /// Borrow a finite whole-piece certificate on a contained smooth query.
    /// The caller separately qualifies C1; a retained Hessian is not that proof.
    fn retained_with_native_budget(
        &self,
        range: (f64, f64),
        native_work: &mut monstertruck_modeling::ValidatedGeometryBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<CurveEnclosure>, GeometryError> {
        if range.0 == range.1 {
            native_work.charge(1, cancel).map_err(native_error)?;
            return Ok(None);
        }
        let pieces = self.native.pieces();
        let (mut low, mut high) = (0, pieces.len());
        while low < high {
            native_work.charge(1, cancel).map_err(native_error)?;
            let middle = low + (high - low) / 2;
            if pieces[middle].parameter[1] <= range.0 {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut covered = range.0;
        let mut result: Option<CurveEnclosure> = None;
        let scaled = |value| adapt(value).mul(Interval::point(self.scale));
        for piece in &pieces[low..] {
            native_work.charge(1, cancel).map_err(native_error)?;
            if piece.parameter[0] >= range.1 {
                break;
            }
            let lower = piece.parameter[0].max(range.0);
            let upper = piece.parameter[1].min(range.1);
            if lower >= upper {
                continue;
            }
            if lower > covered {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "retained native curve partition has an uncovered query interval",
                ));
            }
            let jet = CurveEnclosure {
                position: piece.jet.position.map(scaled),
                derivative: piece.jet.first.map(scaled),
                second_derivative: piece.jet.second.map(scaled),
            };
            if jet
                .position
                .into_iter()
                .chain(jet.derivative)
                .chain(jet.second_derivative)
                .any(|bound| !bound.lo.is_finite() || !bound.hi.is_finite())
            {
                return Ok(None);
            }
            result = Some(match result {
                None => jet,
                Some(previous) => CurveEnclosure {
                    position: std::array::from_fn(|axis| {
                        previous.position[axis].hull(jet.position[axis])
                    }),
                    derivative: std::array::from_fn(|axis| {
                        previous.derivative[axis].hull(jet.derivative[axis])
                    }),
                    second_derivative: std::array::from_fn(|axis| {
                        previous.second_derivative[axis].hull(jet.second_derivative[axis])
                    }),
                },
            });
            covered = upper;
            if covered == range.1 {
                return Ok(result);
            }
        }
        Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "retained native curve partition does not cover the requested range",
        ))
    }

    /// Compose the actual retained native branch with a pole-free projective
    /// chart. Frame vectors need not be perfectly orthonormal: independence and
    /// a strictly positive hemisphere denominator are proved outwardly.
    pub(crate) fn gnomonic_jet(
        &self,
        range: (f64, f64),
        center_mm: [f64; 3],
        x_axis: [f64; 3],
        y_axis: [f64; 3],
        normal: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ChartCurveEnclosure, GeometryError> {
        work.charge(1, cancel)?;
        if center_mm
            .into_iter()
            .chain(x_axis)
            .chain(y_axis)
            .chain(normal)
            .any(|v| !v.is_finite())
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "nonfinite projective pole chart frame",
            ));
        }
        let dot = |a: [Interval; 3], b: [f64; 3]| {
            (0..3).fold(Interval::point(0.0), |sum, i| {
                sum.add(a[i].mul(Interval::point(b[i])))
            })
        };
        let determinant = dot(
            cross(x_axis.map(Interval::point), y_axis.map(Interval::point)),
            normal,
        );
        if determinant.contains_zero() || !determinant.lo.is_finite() || !determinant.hi.is_finite()
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "projective pole chart frame independence is unresolved",
            ));
        }
        let native = self.jet(range, work, cancel)?;
        let p = std::array::from_fn(|i| native.position[i].sub(Interval::point(center_mm[i])));
        let s = dot(p, normal);
        if s.lo <= 0.0 || !s.lo.is_finite() || !s.hi.is_finite() {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "native branch cannot be enclosed in a positive projective hemisphere",
            ));
        }
        let ds = dot(native.derivative, normal);
        let dds = dot(native.second_derivative, normal);
        let s2 = s.square();
        let s3 = s2.mul(s);
        let chart_axis = |axis: [f64; 3]| -> Result<[Interval; 3], GeometryError> {
            let n = dot(p, axis);
            let dn = dot(native.derivative, axis);
            let ddn = dot(native.second_derivative, axis);
            let value = n.div(s)?;
            let first = dn.mul(s).sub(n.mul(ds)).div(s2)?;
            let second = ddn
                .div(s)?
                .sub(n.mul(dds).div(s2)?)
                .sub(Interval::point(2.0).mul(dn).mul(ds).div(s2)?)
                .add(Interval::point(2.0).mul(n).mul(ds.square()).div(s3)?);
            for value in [value, first, second] {
                Interval::new(value.lo, value.hi)?;
            }
            Ok([value, first, second])
        };
        let x = chart_axis(x_axis)?;
        let y = chart_axis(y_axis)?;
        Ok(ChartCurveEnclosure {
            position: [x[0], y[0]],
            derivative: [x[1], y[1]],
            second_derivative: [x[2], y[2]],
        })
    }

    /// Retains an original endpoint enclosure for exact-parameter reuse by this sampler.
    pub(crate) fn sampling_endpoint(
        &self,
        parameter: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveSamplingEndpoint<'_, 'source>, GeometryError> {
        check_cancel(cancel)?;
        self.check_range((parameter, parameter))?;
        let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let point = self.native.evaluate(parameter, &mut native_work, cancel);
        debit_native_work(work, native_work.consumed(), cancel)?;
        Ok(CurveSamplingEndpoint {
            binding: self,
            parameter,
            point: point.map_err(native_error)?,
        })
    }

    /// Whole-curve Hausdorff bound to the actual returned endpoint chord.
    /// Taylor requires a native C1 receipt; otherwise use a complete image and
    /// an actual native endpoint anchor, never a piecewise Hessian union.
    pub(crate) fn sampling_interval(
        &self,
        endpoints: &[CurveSamplingEndpoint<'_, 'source>; 2],
        frame: &PlaneFrameMm,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveSamplingInterval<'_, 'source>, GeometryError> {
        frame.validate()?;
        work.charge(1, cancel)?;
        let [start, end] = endpoints;
        if !std::ptr::eq(start.binding, self) || !std::ptr::eq(end.binding, self) {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "native sampler endpoint belongs to another prepared source binding",
            ));
        }
        let range = (start.parameter, end.parameter);
        self.check_range(range)?;
        let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = (|| -> Result<CurveSamplingInterval<'_, 'source>, GeometryError> {
            let smooth = self
                .native
                .is_smooth_on([range.0, range.1], &mut native_work, cancel)
                .map_err(native_error)?;
            let start = &start.point;
            let end = &end.point;
            let start_mm = [start.position.x, start.position.y, start.position.z]
                .map(|value| value * self.scale);
            let end_mm =
                [end.position.x, end.position.y, end.position.z].map(|value| value * self.scale);
            let scaled = |value| adapt(value).mul(Interval::point(self.scale));
            let endpoint_error =
                |point: [f64; 3], enclosure: [monstertruck_modeling::ValidatedInterval; 3]| {
                    norm_upper(std::array::from_fn(|axis| {
                        scaled(enclosure[axis]).sub(Interval::point(point[axis]))
                    }))
                };
            let reserve =
                endpoint_error(start_mm, start.error)?.max(endpoint_error(end_mm, end.error)?);
            let mut retained_jet = None;
            let (position, chord_error_mm) = if smooth {
                let width = Interval::point(range.1).sub(Interval::point(range.0));
                let chord_error = |jet: &CurveEnclosure| -> Result<f64, GeometryError> {
                    let remainder = Interval::point(norm_upper(jet.second_derivative)?)
                        .mul(width.square())
                        .mul(Interval::point(0.125))
                        .hi;
                    Ok(up(remainder + reserve))
                };
                let coarse = self.retained_with_native_budget(range, &mut native_work, cancel)?;
                let jet = match coarse {
                    Some(jet) => {
                        if chord_error(&jet)? <= SECTION_SAMPLING_TOLERANCE_MM {
                            jet
                        } else {
                            self.refined_with_native_budget(range, &mut native_work, cancel)?
                        }
                    }
                    None => self.refined_with_native_budget(range, &mut native_work, cancel)?,
                };
                let error = chord_error(&jet)?;
                retained_jet = Some(jet);
                (jet.position, error)
            } else {
                // Curve -> chord uses its first actual chord endpoint. Chord
                // -> curve uses the true native start point enclosed by E0;
                // convexity of norm bounds every chord point by its endpoints.
                let position = self
                    .native
                    .position_bounds([range.0, range.1], &mut native_work, cancel)
                    .map_err(native_error)?
                    .map(scaled);
                let image_to_chord = norm_upper(std::array::from_fn(|axis| {
                    position[axis].sub(Interval::point(start_mm[axis]))
                }))?;
                let chord_to_anchor = reserve.max(norm_upper(std::array::from_fn(|axis| {
                    scaled(start.error[axis]).sub(Interval::point(end_mm[axis]))
                }))?);
                (position, image_to_chord.max(chord_to_anchor))
            };
            let projection = (0..3).fold(Interval::point(0.0), |sum, axis| {
                sum.add(
                    position[axis]
                        .sub(Interval::point(frame.origin_mm[axis]))
                        .mul(Interval::point(frame.z_axis[axis])),
                )
            });
            let mut plane_error_mm = projection.abs_upper();
            if smooth && plane_error_mm > SECTION_PLANE_TOLERANCE_MM {
                plane_error_mm = self
                    .projected_with_native_budget(
                        range,
                        frame.origin_mm,
                        frame.z_axis,
                        &mut native_work,
                        cancel,
                    )?
                    .abs_upper();
            }
            Ok(CurveSamplingInterval {
                binding: self,
                range,
                enclosure: CurveIntervalEnclosure {
                    start_mm,
                    end_mm,
                    chord_error_mm,
                    plane_error_mm,
                },
                jet: retained_jet,
            })
        })();
        debit_native_work(work, native_work.consumed(), cancel)?;
        result
    }
}
pub(crate) fn clone_oriented_curve_with_byte_limit(
    edge: &monstertruck_modeling::Edge,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Curve, usize), GeometryError> {
    check_cancel(cancel)?;
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let size = edge.with_curve(|curve| {
        monstertruck_modeling::curve_owned_bytes(curve, remaining_bytes, &mut native_work, cancel)
    });
    debit_native_work(work, native_work.consumed(), cancel)?;
    let size = size.map_err(native_error)?;
    work.charge(1, cancel)?;
    Ok((edge.oriented_curve(), size))
}

pub(crate) fn same_surface_coefficients(
    lhs: &Surface,
    rhs: &Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    check_cancel(cancel)?;
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = monstertruck_modeling::validated_same_surface_coefficients(
        lhs,
        rhs,
        &mut native_work,
        cancel,
    );
    debit_native_work(work, native_work.consumed(), cancel)?;
    result.map_err(native_error)
}

pub(crate) fn prepare_section_curve_enclosure(
    guide: &Curve,
    source: monstertruck_modeling::ValidatedFacePlaneSource<'_>,
    plane: &monstertruck_modeling::Plane,
    range: (f64, f64),
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedCurveEnclosure<'static>, GeometryError> {
    check_cancel(cancel)?;
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let options = monstertruck_modeling::ValidatedCurveOptions {
        max_retained_bytes: remaining_bytes,
        ..monstertruck_modeling::ValidatedCurveOptions::default()
    };
    let result = monstertruck_modeling::prepared_face_plane_curve(
        guide,
        source,
        plane,
        [range.0, range.1],
        &options,
        &mut native_work,
        cancel,
    );
    debit_native_work(work, native_work.consumed(), cancel)?;
    Ok(PreparedCurveEnclosure {
        native: PreparedNative::Owned(result.map_err(native_error)?),
        scale: 1.0,
        active: Some(range),
    })
}

mod green;
mod section_coverage;
pub(crate) use section_coverage::{
    PreparedSectionOriginals, PreparedSectionPlaneRoots, SectionPlaneCoverage,
    SectionPlaneEndpointReceipt, certify_section_plane_coverage, prepare_section_originals,
    prepare_section_plane_roots,
};

fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    std::array::from_fn(|axis| {
        let i = (axis + 1) % 3;
        let j = (axis + 2) % 3;
        a[i].mul(b[j]).sub(a[j].mul(b[i]))
    })
}
fn regular_jet(jet: &SurfaceEnclosure) -> bool {
    cross(jet.du, jet.dv).into_iter().any(|component| {
        component.lo.is_finite() && component.hi.is_finite() && !component.contains_zero()
    })
}
fn certify_surface_regular(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    root: &SurfaceEnclosure,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if regular_jet(root) {
        return Ok(());
    }
    // Geometric regularity need not imply a regular choice of UV chart.
    // A declined whole-cell normal proof still permits the existing strict
    // local Jacobian subdivision; invalid input and exhausted work do not.
    match surface_geometric_normal_enclosure(surface, domain, work, cancel) {
        Ok(_) => return Ok(()),
        Err(error)
            if matches!(
                error.code,
                GeometryErrorCode::SingularGeometry
                    | GeometryErrorCode::AnalysisIndeterminate
                    | GeometryErrorCode::UnsupportedOperation
            ) => {}
        Err(error) => return Err(error),
    }
    let mut stack = [([[0.0; 2]; 2], 0_u8); 50];
    stack[0] = (domain, 0);
    let mut live = 1;
    while live != 0 {
        live -= 1;
        let (cell, depth) = stack[live];
        work.charge(1, cancel)?;
        let jet = surface_enclosure(surface, cell, work, cancel)?;
        if regular_jet(&jet) {
            continue;
        }
        if depth == 24 {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "native display patch Jacobian cannot be separated from zero",
            ));
        }
        let axis = if cell[0][1] - cell[0][0] >= cell[1][1] - cell[1][0] {
            0
        } else {
            1
        };
        let middle = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
        if middle <= cell[axis][0] || middle >= cell[axis][1] {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "native display chart exhausted representable regularity subdivision",
            ));
        }
        let mut left = cell;
        let mut right = cell;
        left[axis][1] = middle;
        right[axis][0] = middle;
        stack[live] = (right, depth + 1);
        stack[live + 1] = (left, depth + 1);
        live += 2;
    }
    Ok(())
}

/// Linear interpolation on a UV triangle, with an interval Taylor remainder
/// over its complete bounding rectangle. Native vertex evaluation error is
/// separately enclosed. The same barycentric weights pair every mesh point
/// with a point of the complete native UV-triangle image and conversely, so
/// the bound controls both metric directions. Trim coverage is a separate proof.
pub(crate) fn triangle_error_bound(
    surface: &Surface,
    uv: &[[f64; 2]; 3],
    points: &[[f64; 3]; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let domain = std::array::from_fn(|axis| {
        [
            uv.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min),
            uv.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max),
        ]
    });
    let bound = surface_piecewise_enclosure(surface, domain, work, cancel)?;
    certify_surface_regular(surface, domain, &bound, work, cancel)?;
    triangle_interpolation_error(surface, uv, points, domain, &bound, work, cancel)
}

fn triangle_interpolation_error(
    surface: &Surface,
    uv: &[[f64; 2]; 3],
    points: &[[f64; 3]; 3],
    domain: [[f64; 2]; 2],
    bound: &SurfaceEnclosure,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    // A second-derivative Taylor theorem cannot cross an unqualified C0 knot.
    // For a multi-span triangle use a complete image/vertex enclosure instead;
    // it is coarser but needs neither continuity nor sampled derivative claims.
    if !surface_parameter_span_smooth(surface, domain, work, cancel)? {
        let mut error = norm_upper(std::array::from_fn(|axis| {
            bound.position[axis].sub(Interval::point(points[0][axis]))
        }))?;
        let native = surface_enclosure(surface, [[uv[0][0]; 2], [uv[0][1]; 2]], work, cancel)?;
        for point in points {
            error = error.max(norm_upper(std::array::from_fn(|axis| {
                native.position[axis].sub(Interval::point(point[axis]))
            }))?);
        }
        return Ok(error);
    }
    let du = Interval::point(domain[0][1])
        .sub(Interval::point(domain[0][0]))
        .abs_upper();
    let dv = Interval::point(domain[1][1])
        .sub(Interval::point(domain[1][0]))
        .abs_upper();
    // For any barycentric point, the weighted coordinate variances are at
    // most width^2/4 (Popoviciu); the absolute mixed covariance is at most
    // du*dv/4 by Cauchy--Schwarz. First derivatives cancel in the barycentric
    // Taylor sum, leaving one half of these variances, not half of width^2.
    let remainder: [Interval; 3] = std::array::from_fn(|axis| {
        Interval::point(bound.duu[axis].abs_upper())
            .mul(Interval::point(du).square())
            .add(
                Interval::point(bound.duv[axis].abs_upper())
                    .mul(Interval::point(du))
                    .mul(Interval::point(dv))
                    .mul(Interval::point(2.0)),
            )
            .add(Interval::point(bound.dvv[axis].abs_upper()).mul(Interval::point(dv).square()))
            .mul(Interval::point(0.125))
    });
    let mut vertex_error = 0.0_f64;
    for vertex in 0..3 {
        let enclosure = surface_enclosure(
            surface,
            [[uv[vertex][0]; 2], [uv[vertex][1]; 2]],
            work,
            cancel,
        )?;
        vertex_error = vertex_error.max(norm_upper(std::array::from_fn(|axis| {
            enclosure.position[axis].sub(Interval::point(points[vertex][axis]))
        }))?);
    }
    Ok(up(norm_upper(remainder)? + vertex_error))
}

/// The native source boundary joint supplies quotient topology. This proof
/// supplies only complete-image display error and actual one-sided native
/// normals; it never asserts a unique limiting normal at a chart singularity.
pub(crate) enum BoundaryTriangleProof {
    NativeTriangle {
        error_mm: f64,
        normals: [[f64; 3]; 3],
        boundary_normal: [bool; 3],
    },
    VertexTube {
        error_mm: f64,
    },
}

fn native_unit_normal(jet: &SurfaceEnclosure) -> Result<[f64; 3], GeometryError> {
    let normal = cross(jet.du, jet.dv);
    if !regular_jet(jet) {
        return Err(GeometryError::new(
            GeometryErrorCode::SingularGeometry,
            "native display normal has no proved regular representative",
        ));
    }
    let center = normal.map(Interval::mid);
    let scale = center.into_iter().map(f64::abs).fold(0.0_f64, f64::max);
    if !scale.is_finite() || scale == 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "native display normal representative is ill-conditioned",
        ));
    }
    let scaled = center.map(|x| x / scale);
    let length = scaled[0].hypot(scaled[1]).hypot(scaled[2]);
    let unit = scaled.map(|x| x / length);
    let orientation = (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(normal[axis].mul(Interval::point(unit[axis])))
    });
    if unit.into_iter().any(|x| !x.is_finite()) || orientation.lo <= 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "native one-sided normal orientation is unresolved",
        ));
    }
    Ok(unit)
}

fn certify_boundary_patch(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    fixed_axis: usize,
    fixed: f64,
    supported: [[f64; 2]; 2],
    vertex: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut stack = [([[0.0; 2]; 2], 0_u8); 50];
    stack[0] = (domain, 0);
    let mut live = 1;
    while live != 0 {
        live -= 1;
        let (cell, depth) = stack[live];
        work.charge(1, cancel)?;
        let jet = surface_enclosure(surface, cell, work, cancel)?;
        if regular_jet(&jet) {
            continue;
        }
        let variable = 1 - fixed_axis;
        // Only a connected collar of this ORIGINAL natural-boundary connector
        // may belong to the explicit source-vertex stratum. The WHOLE strip to
        // the native boundary must fit its display tube. An unrelated interior
        // rank failure, a merely small row, or endpoint nearness is insufficient.
        if cell[variable][0] >= supported[variable][0]
            && cell[variable][1] <= supported[variable][1]
        {
            let mut strip = cell;
            strip[fixed_axis][0] = strip[fixed_axis][0].min(fixed);
            strip[fixed_axis][1] = strip[fixed_axis][1].max(fixed);
            let collar = surface_enclosure(surface, strip, work, cancel)?;
            let distance = norm_upper(std::array::from_fn(|axis| {
                collar.position[axis].sub(Interval::point(vertex[axis]))
            }))?;
            if distance <= 0.00625 {
                continue;
            }
        }
        if depth == 24 {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "native display rank outside the original boundary vertex tube is unresolved",
            ));
        }
        let axis = if cell[0][1] - cell[0][0] >= cell[1][1] - cell[1][0] {
            0
        } else {
            1
        };
        let middle = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
        if middle <= cell[axis][0] || middle >= cell[axis][1] {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "native boundary display chart exhausted representable subdivision",
            ));
        }
        let mut left = cell;
        let mut right = cell;
        left[axis][1] = middle;
        right[axis][0] = middle;
        stack[live] = (right, depth + 1);
        stack[live + 1] = (left, depth + 1);
        live += 2;
    }
    Ok(())
}

pub(crate) fn triangle_boundary_error_bound(
    surface: &Surface,
    uv: &[[f64; 2]; 3],
    points: &[[f64; 3]; 3],
    original_vertex_mm: [f64; 3],
    joint: &monstertruck_modeling::TrimJointKind,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<BoundaryTriangleProof, GeometryError> {
    work.charge(1, cancel)?;
    let monstertruck_modeling::TrimJointKind::NaturalBoundary {
        fixed_axis,
        fixed_parameter,
        supported_parameter_box,
        ..
    } = joint
    else {
        return Err(GeometryError::new(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "native boundary triangle lacks original natural-boundary joint authority",
        ));
    };
    if *fixed_axis >= 2
        || uv
            .iter()
            .flatten()
            .chain(points.iter().flatten())
            .chain(original_vertex_mm.iter())
            .any(|x| !x.is_finite())
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "invalid native boundary triangle parameters",
        ));
    }
    let domain = std::array::from_fn(|axis| {
        [
            uv.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min),
            uv.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max),
        ]
    });
    let bound = surface_piecewise_enclosure(surface, domain, work, cancel)?;
    if points[0] == points[1] || points[1] == points[2] || points[2] == points[0] {
        // A collapsed ROW does not bound its incident UV wedge. Enclose the
        // ENTIRE wedge image (its rectangle is a conservative superset).
        let error_mm = norm_upper(std::array::from_fn(|axis| {
            bound.position[axis].sub(Interval::point(original_vertex_mm[axis]))
        }))?;
        return Ok(BoundaryTriangleProof::VertexTube { error_mm });
    }
    certify_boundary_patch(
        surface,
        domain,
        *fixed_axis,
        *fixed_parameter,
        *supported_parameter_box,
        original_vertex_mm,
        work,
        cancel,
    )?;
    let error_mm = triangle_interpolation_error(surface, uv, points, domain, &bound, work, cancel)?;
    let boundary_normal = uv.map(|point| {
        (0..2).all(|axis| {
            supported_parameter_box[axis][0] <= point[axis]
                && point[axis] <= supported_parameter_box[axis][1]
        })
    });
    let representative = std::array::from_fn::<_, 2, _>(|axis| {
        uv[0][axis] + ((uv[1][axis] - uv[0][axis]) + (uv[2][axis] - uv[0][axis])) / 3.0
    });
    let mut normals = [[0.0; 3]; 3];
    let mut interior_normal = None;
    for slot in 0..3 {
        if boundary_normal[slot] {
            if interior_normal.is_none() {
                let jet = surface_enclosure(
                    surface,
                    [[representative[0]; 2], [representative[1]; 2]],
                    work,
                    cancel,
                )?;
                interior_normal = Some(native_unit_normal(&jet)?);
            }
            normals[slot] = interior_normal.ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::SingularGeometry,
                    "native one-sided display normal is unavailable",
                )
            })?;
        } else {
            let jet =
                surface_enclosure(surface, [[uv[slot][0]; 2], [uv[slot][1]; 2]], work, cancel)?;
            normals[slot] = native_unit_normal(&jet)?;
        }
    }
    Ok(BoundaryTriangleProof::NativeTriangle {
        error_mm,
        normals,
        boundary_normal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rodrigues_derivatives_preserve_axial_cancellation_and_surface_sense() {
        use monstertruck_modeling::{Line, Point3, Processor, RevolutionSurface, Vector3, base::*};
        for inverted in [false, true] {
            let mut processor = Processor::new(RevolutionSurface::by_revolution(
                Curve::Line(Line(
                    Point3::new(6.0, 0.0, 0.0),
                    Point3::new(4.0, 0.0, 12.0),
                )),
                Point3::origin(),
                Vector3::unit_z(),
            ));
            if inverted {
                processor.invert();
            }
            let surface = Surface::RevolutionSurface(processor);
            let domain = if inverted {
                [[0.0, std::f64::consts::TAU], [0.0, 1.0]]
            } else {
                [[0.0, 1.0], [0.0, std::f64::consts::TAU]]
            };
            let enclosure = surface_enclosure(
                &surface,
                domain,
                &mut GeometryWorkBudget::new(1_000_000),
                &AtomicBool::new(false),
            )
            .unwrap();
            let (profile, angle) = if inverted {
                (enclosure.dv[2], enclosure.du[2])
            } else {
                (enclosure.du[2], enclosure.dv[2])
            };
            assert!(profile.lo <= 12.0 && profile.hi >= 12.0);
            assert!(profile.width() <= 1e-10);
            assert!(angle.abs_upper() <= 1e-10);
            assert!(enclosure.duv[2].abs_upper() <= 1e-10);
            assert!(enclosure.duu[2].abs_upper() <= 1e-10);
            assert!(enclosure.dvv[2].abs_upper() <= 1e-10);
        }
    }

    #[test]
    fn whole_triangle_interpolation_encloses_bilinear_saddle_material() {
        use monstertruck_modeling::{BsplineSurface, KnotVector, Point3};
        let surface = Surface::BsplineSurface(BsplineSurface::new(
            (KnotVector::bezier_knot(1), KnotVector::bezier_knot(1)),
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0)],
            ],
        ));
        let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let triangle = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let bound = triangle_error_bound(
            &surface,
            &uv,
            &triangle,
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        // The exact saddle z=u*v reaches 1/4 on this triangle's sloped edge,
        // although every mesh vertex lies on z=0.
        assert!(bound >= 0.25 && bound <= 0.25000001);
        for i in 0..=20 {
            for j in 0..=20 - i {
                let u = f64::from(i) / 20.0;
                let v = f64::from(j) / 20.0;
                assert!(u * v <= bound);
            }
        }
    }

    fn collapsed_row_surface() -> Surface {
        use monstertruck_modeling::{BsplineSurface, KnotVector, Point3};
        Surface::BsplineSurface(BsplineSurface::new(
            (
                KnotVector::from(vec![0.0, 0.0, 1.0, 1.0]),
                KnotVector::from(vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
            ),
            vec![
                vec![Point3::new(0.0, 0.0, 0.0); 3],
                vec![
                    Point3::new(1.0, 0.0, 0.0),
                    Point3::new(1.0, 1.0, 0.0),
                    Point3::new(0.0, 1.0, 0.0),
                ],
            ],
        ))
    }
    fn exact_collapsed_row_joint() -> monstertruck_modeling::TrimJointKind {
        use monstertruck_modeling::{
            Line, NativeSurfaceBoundary, Point2, TrimJointKind, ValidatedInterval,
        };
        TrimJointKind::NaturalBoundary {
            boundary: NativeSurfaceBoundary::UFront,
            fixed_axis: 0,
            fixed_parameter: 0.0,
            variable_range: [0.0, 1.0],
            connector: Line(Point2::new(0.0, 1.0), Point2::new(0.0, 0.0)),
            supported_parameter_box: [[0.0, 0.0], [0.0, 1.0]],
            vertex_residual: [ValidatedInterval {
                lower: 0.0,
                upper: 0.0,
            }; 3],
            world_enclosure: [ValidatedInterval {
                lower: 0.0,
                upper: 0.0,
            }; 3],
            max_vertex_deviation: [0.0; 3],
        }
    }
    #[test]
    fn collapsed_row_does_not_authorize_omitting_a_large_uv_wedge() {
        let surface = collapsed_row_surface();
        let mut work = GeometryWorkBudget::new(1_000_000);
        let proof = triangle_boundary_error_bound(
            &surface,
            &[[0.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
            &[[0.0; 3], [0.0; 3], [0.0, 1.0, 0.0]],
            [0.0; 3],
            &exact_collapsed_row_joint(),
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap();
        let BoundaryTriangleProof::VertexTube { error_mm } = proof else {
            panic!("physically collapsed triangle needs whole-image vertex tube");
        };
        // S(1/2,1/2)=(3/8,3/8,0), despite the exactly collapsed u=0 row.
        assert!(error_mm >= 0.375_f64.hypot(0.375));
        assert!(error_mm > 0.025);
    }
    #[test]
    fn boundary_triangle_uses_real_interior_native_normal_not_zero_pole_cross() {
        let surface = collapsed_row_surface();
        let uv = [[0.0, 0.0], [0.001, 0.0], [0.001, 0.001]];
        let points = [[0.0; 3], [0.001, 0.0, 0.0], [0.000999999, 0.000001999, 0.0]];
        let mut work = GeometryWorkBudget::new(1_000_000);
        let proof = triangle_boundary_error_bound(
            &surface,
            &uv,
            &points,
            [0.0; 3],
            &exact_collapsed_row_joint(),
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap();
        let BoundaryTriangleProof::NativeTriangle {
            error_mm,
            normals,
            boundary_normal,
        } = proof
        else {
            panic!("nondegenerate native triangle must not become an omitted tube");
        };
        assert!(error_mm < 0.025);
        assert_eq!(boundary_normal, [true, false, false]);
        assert!(
            normals
                .into_iter()
                .all(|n| n[2] > 0.999999 && n[0].abs() < 1e-12 && n[1].abs() < 1e-12)
        );
    }
    #[test]
    fn arithmetic_encloses_cancellation_and_signed_denominators() {
        let sum = Interval::point(1e16)
            .add(Interval::point(1.0))
            .sub(Interval::point(1e16));
        assert!(sum.lo <= 1.0 && sum.hi >= 1.0);
        let negative = Interval::new(-4.0, -2.0).unwrap();
        let quotient = Interval::point(2.0).div(negative).unwrap();
        assert!(quotient.lo <= -1.0 && quotient.hi >= -0.5);
        assert!(
            Interval::point(1.0)
                .div(Interval::new(-1.0, 1.0).unwrap())
                .is_err()
        );
    }
    #[test]
    fn adjacent_padding_includes_subnormal_zero_cases() {
        assert!(down(0.0) < 0.0 && up(-0.0) > 0.0);
        let square = Interval::new(-2.0, 1.0).unwrap().square();
        assert_eq!(square.lo, 0.0);
        assert!(square.hi >= 4.0);
    }
    #[test]
    fn cubic_aliasing_is_enclosed_between_zero_samples() {
        use monstertruck_modeling::{BsplineCurve, KnotVector, Point3};
        let curve = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0 / 3.0, 1.0, 0.0),
                Point3::new(2.0 / 3.0, -1.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
        ));
        let mut work = GeometryWorkBudget::new(100_000);
        let enclosure =
            curve_enclosure(&curve, (0.0, 1.0), &mut work, &AtomicBool::new(false)).unwrap();
        // Endpoints and midpoint all have y=0; the complete curve does not.
        assert!(enclosure.position[1].lo < -0.28);
        assert!(enclosure.position[1].hi > 0.28);
        assert!(work.consumed() > 0);
    }
    #[test]
    fn rational_sign_normalization_and_mixed_denominator_exclusion() {
        use monstertruck_modeling::{BsplineCurve, KnotVector, NurbsCurve, Vector4};
        let rational = |weights: [f64; 3]| {
            Curve::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
                KnotVector::from(vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
                weights
                    .into_iter()
                    .enumerate()
                    .map(|(i, w)| Vector4::new(i as f64 * w, 0.0, 0.0, w))
                    .collect(),
            )))
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let negative =
            curve_enclosure(&rational([-1.0; 3]), (0.25, 0.75), &mut work, &cancel).unwrap();
        assert!(negative.position[0].lo <= 0.5 && negative.position[0].hi >= 1.5);
        let mixed = curve_enclosure(
            &rational([1.0, -0.1, 1.0]),
            (0.45, 0.55),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(mixed.position[0].lo.is_finite() && mixed.position[0].hi.is_finite());
        assert_eq!(
            curve_enclosure(&rational([1.0, -1.0, 1.0]), (0.5, 0.5), &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ConditioningFailure
        );
    }
    #[test]
    fn cancellation_and_cumulative_work_are_terminal() {
        use monstertruck_modeling::{Line, Point3};
        let curve = Curve::Line(Line(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)));
        let mut work = GeometryWorkBudget::new(0);
        assert_eq!(
            curve_enclosure(&curve, (0.0, 1.0), &mut work, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        let mut work = GeometryWorkBudget::new(100);
        assert_eq!(
            curve_enclosure(&curve, (0.0, 1.0), &mut work, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
    }
    #[test]
    fn nonbinary_knot_partition_preserves_closed_interfaces_without_patch_copies() {
        use monstertruck_modeling::{BsplineSurface, KnotVector, Point3};
        let surface = Surface::BsplineSurface(BsplineSurface::new(
            (
                KnotVector::from(vec![0.0, 0.0, 0.37, 1.0, 1.0]),
                KnotVector::from(vec![0.0, 0.0, 1.0, 1.0]),
            ),
            [0.0, 0.37, 1.0]
                .into_iter()
                .map(|u| vec![Point3::new(u, 0.0, 0.0), Point3::new(u, 1.0, 0.0)])
                .collect(),
        ));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100);
        let spans: Vec<_> =
            surface_parameter_boxes(&surface, [[0.0, 1.0], [0.0, 1.0]], &mut work, &cancel)
                .unwrap()
                .collect();
        assert_eq!(
            spans,
            vec![[[0.0, 0.37], [0.0, 1.0]], [[0.37, 1.0], [0.0, 1.0]]]
        );
        let interfaces: Vec<_> =
            surface_crease_interfaces(&surface, [[0.0, 1.0], [0.0, 1.0]], &mut work, &cancel)
                .unwrap()
                .collect();
        assert_eq!(interfaces, vec![[[0.37, 0.37], [0.0, 1.0]]]);
        // The chart interface is merely a candidate: this particular surface
        // remains geometrically planar and regular across the inserted knot.
        assert_eq!(work.consumed(), 3);
    }
    #[test]
    fn projective_pole_chart_encloses_native_curve_and_rejects_horizon() {
        use monstertruck_modeling::{Line, Point3};
        let curve = Curve::Line(Line(Point3::new(0.0, 0.0, 1.0), Point3::new(1.0, 0.0, 2.0)));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let prepared =
            prepare_curve_enclosure(&curve, (0.0, 1.0), 1024 * 1024, &mut work, &cancel).unwrap();
        let x = [1.0, 0.0, 0.0];
        let y = [0.0, 1.0, 0.0];
        let normal = [0.0, 0.0, 1.0];
        let chart = prepared
            .gnomonic_jet((0.0, 1.0), [0.0; 3], x, y, normal, &mut work, &cancel)
            .unwrap();
        for t in [0.0_f64, 0.3, 1.0] {
            let values = [
                t / (1.0 + t),
                1.0 / (1.0 + t).powi(2),
                -2.0 / (1.0 + t).powi(3),
            ];
            for (bound, value) in [
                chart.position[0],
                chart.derivative[0],
                chart.second_derivative[0],
            ]
            .into_iter()
            .zip(values)
            {
                assert!(bound.lo <= value && value <= bound.hi);
            }
        }
        assert_eq!(
            prepared
                .gnomonic_jet(
                    (0.0, 1.0),
                    [0.0, 0.0, 1.5],
                    x,
                    y,
                    normal,
                    &mut work,
                    &cancel
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        assert_eq!(
            prepared
                .gnomonic_jet((0.0, 1.0), [0.0; 3], x, x, normal, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ConditioningFailure
        );
    }
    #[test]
    fn exact_extrusion_partitions_borrowed_profile_knots_without_v_clipping() {
        use monstertruck_modeling::{BsplineCurve, ExtrusionSurface, KnotVector, Point3, Vector3};
        let profile = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.37, 1.0, 1.0]),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.37, 1.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
        ));
        let surface =
            Surface::ExtrusionSurface(ExtrusionSurface::by_extrusion(profile, Vector3::unit_z()));
        let mut work = GeometryWorkBudget::new(100);
        let spans: Vec<_> = surface_parameter_boxes(
            &surface,
            [[0.0, 1.0], [-2.0, 3.0]],
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap()
        .collect();
        assert_eq!(
            spans,
            vec![[[0.0, 0.37], [-2.0, 3.0]], [[0.37, 1.0], [-2.0, 3.0]]]
        );
    }

    #[test]
    fn cancelled_native_return_keeps_completed_work_debit() {
        let mut work = GeometryWorkBudget::new(10);
        let cancel = AtomicBool::new(true);
        assert_eq!(
            debit_native_work(&mut work, 4, &cancel).unwrap_err().code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(work.remaining(), 6);
    }

    #[test]
    fn c0_curve_chord_uses_native_image_not_zero_piecewise_curvature() {
        use monstertruck_modeling::{BsplineCurve, KnotVector, Point3};
        let curve = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.5, 1.0, 1.0]),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.5, 0.0, 1.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
        ));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let prepared =
            prepare_curve_enclosure(&curve, (0.0, 1.0), 1_000_000, &mut work, &cancel).unwrap();
        assert!(
            !prepared
                .is_smooth_on((0.0, 1.0), &mut work, &cancel)
                .unwrap()
        );
        let frame = PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        };
        let endpoints = [0.0, 1.0].map(|parameter| {
            prepared
                .sampling_endpoint(parameter, &mut work, &cancel)
                .unwrap()
        });
        let interval = prepared
            .sampling_interval(&endpoints, &frame, &mut work, &cancel)
            .unwrap();
        let bound = interval.enclosure();
        // Both endpoint tangents are finite and both pieces have C''=0, but
        // the actual native midpoint is 1 mm from the returned straight chord.
        assert!(bound.chord_error_mm >= 1.0);
        assert!(bound.plane_error_mm >= 1.0);
    }

    #[test]
    fn a_single_uncertain_knot_band_is_not_a_smooth_taylor_span() {
        let values = [0.0, 0.5, 1.0];
        let knots = KnotAxis {
            source: KnotSource::Borrowed(&values),
            degree: 1,
            sign: 1.0,
            offset: Interval {
                lo: -0.125,
                hi: 0.125,
            },
        };
        let range = [0.49, 0.51];
        assert_eq!(AxisSpans::new(knots, range).count(), 1);
        assert!(
            !axis_span_smooth(
                knots,
                range,
                &mut GeometryWorkBudget::new(100),
                &AtomicBool::new(false)
            )
            .unwrap()
        );
    }

    #[test]
    fn structural_c1_knots_admit_bounded_piecewise_second_derivatives() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100);
        for (degree, multiplicity, expected) in
            [(2, 1, true), (2, 2, false), (3, 2, true), (3, 3, false)]
        {
            let mut values = vec![0.0; degree + 1];
            values.extend(std::iter::repeat_n(0.5, multiplicity));
            values.extend(std::iter::repeat_n(1.0, degree + 1));
            assert_eq!(
                axis_span_smooth(
                    KnotAxis::new(&values, degree),
                    [0.0, 1.0],
                    &mut work,
                    &cancel
                )
                .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn sweep_smoothness_checks_the_actual_nested_profile_carrier() {
        use monstertruck_modeling::{
            BsplineSurface, Curve2D, ExtrusionSurface, KnotVector, Line, ParameterCurve, Point2,
            Point3, Vector3,
        };
        let carrier = Surface::BsplineSurface(BsplineSurface::new(
            (
                KnotVector::from(vec![0.0, 0.0, 0.5, 1.0, 1.0]),
                KnotVector::from(vec![0.0, 0.0, 1.0, 1.0]),
            ),
            [0.0, 0.5, 1.0]
                .into_iter()
                .enumerate()
                .map(|(i, u)| {
                    let y = if i == 1 { 0.25 } else { 0.0 };
                    vec![Point3::new(u, y, 0.0), Point3::new(u, y + 1.0, 0.0)]
                })
                .collect(),
        ));
        let profile = Curve::ParameterCurve(ParameterCurve::new(
            Curve2D::Line(Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
            Box::new(carrier),
        ));
        let surface =
            Surface::ExtrusionSurface(ExtrusionSurface::by_extrusion(profile, Vector3::unit_z()));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let domain = [[0.0, 1.0], [0.0, 1.0]];
        // The exposed profile chart is one LINE, so its own knots alone miss
        // the genuine interior C0 carrier stratum.
        assert_eq!(
            surface_parameter_boxes(&surface, domain, &mut work, &cancel)
                .unwrap()
                .count(),
            1
        );
        assert!(!surface_parameter_span_smooth(&surface, domain, &mut work, &cancel).unwrap());
        assert!(
            surface_parameter_span_smooth(&surface, [[0.1, 0.2], [0.0, 1.0]], &mut work, &cancel)
                .unwrap()
        );
    }

    #[test]
    fn borrowed_original_continuation_keeps_one_branch_across_unit_views() {
        use monstertruck_modeling::{
            Line, Plane, Point3, SurfaceCurve, ValidatedCurveOptions, ValidatedGeometryBudget,
        };
        let xy = Surface::Plane(Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ));
        let yz = Surface::Plane(Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ));
        let guide = Curve::Line(Line(
            Point3::new(0.05, 0.0, 0.02),
            Point3::new(0.05, 1.0, 0.02),
        ));
        let curve = Curve::IntersectionCurve(SurfaceCurve::with_boundaries(
            Box::new(xy),
            Box::new(yz),
            Box::new(guide),
            None,
            None,
        ));
        let cancel = AtomicBool::new(false);
        let prepared = monstertruck_modeling::prepared_validated_curve_owned(
            curve,
            [0.0, 1.0],
            &ValidatedCurveOptions::default(),
            &mut ValidatedGeometryBudget::new(1_000_000),
            &cancel,
        )
        .unwrap();
        let original = PreparedCurveEnclosure::borrowed(&prepared, 1.0);
        let mm = PreparedCurveEnclosure::borrowed(&prepared, 25.4);
        let mut work = GeometryWorkBudget::new(1_000_000);
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let source = original.endpoint_position(t, &mut work, &cancel).unwrap();
            let normalized = mm.endpoint_position(t, &mut work, &cancel).unwrap();
            for axis in [0, 2] {
                assert!(source[axis].contains_zero());
                assert!(normalized[axis].contains_zero());
                assert!(normalized[axis].abs_upper() < 1.0e-7);
            }
            assert!(source[1].lo <= t && source[1].hi >= t);
            assert!(normalized[1].lo <= 25.4 * t && normalized[1].hi >= 25.4 * t);
        }
        let child = mm.jet((0.25, 0.75), &mut work, &cancel).unwrap();
        assert!(child.derivative[1].lo <= 25.4 && child.derivative[1].hi >= 25.4);
        assert!(child.position[0].abs_upper() < 1.0e-7);
        let first = PreparedCurveEnclosure::borrowed_restricted(&prepared, 25.4, (0.0, 0.5));
        let second = PreparedCurveEnclosure::borrowed_restricted(&prepared, 25.4, (0.5, 1.0));
        assert!(
            first
                .parameter_intervals()
                .all(|(a, b)| a >= 0.0 && b <= 0.5)
        );
        assert!(
            second
                .parameter_intervals()
                .all(|(a, b)| a >= 0.5 && b <= 1.0)
        );
        let left = first.endpoint_position(0.5, &mut work, &cancel).unwrap();
        let right = second.endpoint_position(0.5, &mut work, &cancel).unwrap();
        for axis in 0..3 {
            assert_eq!(
                (left[axis].lo, left[axis].hi),
                (right[axis].lo, right[axis].hi)
            );
        }
        assert_eq!(
            first
                .jet((0.25, 0.75), &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
    }

    fn quadratic_sampling_native() -> monstertruck_modeling::PreparedValidatedCurve<'static> {
        use monstertruck_modeling::{
            BsplineCurve, KnotVector, Point3, ValidatedCurveOptions, ValidatedGeometryBudget,
        };
        // x=s, y=s^2, s=(t-10)/4. The native gauge is deliberately not [0,1].
        let curve = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![10.0, 10.0, 10.0, 14.0, 14.0, 14.0]),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.5, 0.0, 0.0),
                Point3::new(1.0, 1.0, 0.0),
            ],
        ));
        monstertruck_modeling::prepared_validated_curve_owned(
            curve,
            [10.0, 14.0],
            &ValidatedCurveOptions::default(),
            &mut ValidatedGeometryBudget::new(100_000),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn sampling_receipts_preserve_native_gauge_chord_and_analytic_green_area() {
        use spiling_contracts::geometry::{
            SECTION_PLANE_TOLERANCE_MM, SECTION_SAMPLING_TOLERANCE_MM,
        };
        let native = quadratic_sampling_native();
        let prepared = PreparedCurveEnclosure::borrowed(&native, 1.0);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let frame = PlaneFrameMm::from_plane(crate::PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let mut start = prepared
            .sampling_endpoint(10.0, &mut work, &cancel)
            .unwrap();
        let mut area = Interval::point(0.0);
        for index in 0..16 {
            let end = prepared
                .sampling_endpoint(10.0 + f64::from(index + 1) / 4.0, &mut work, &cancel)
                .unwrap();
            let interval = prepared
                .sampling_interval(&[start, end], &frame, &mut work, &cancel)
                .unwrap();
            let enclosure = interval.enclosure();
            let x0 = f64::from(index) / 16.0;
            let x1 = f64::from(index + 1) / 16.0;
            assert!((enclosure.start_mm[0] - x0).abs() < 1e-10);
            assert!((enclosure.end_mm[1] - x1 * x1).abs() < 1e-10);
            let true_sagitta = (x1 - x0).powi(2) / (4.0 * (1.0 + (x0 + x1).powi(2)).sqrt());
            assert!(enclosure.chord_error_mm >= true_sagitta);
            assert!(enclosure.chord_error_mm <= SECTION_SAMPLING_TOLERANCE_MM);
            assert!(enclosure.plane_error_mm <= SECTION_PLANE_TOLERANCE_MM);
            let part = interval
                .signed_area(&[start, end], &frame, [0.0; 3], &mut work, &cancel)
                .unwrap();
            area = area.add(Interval {
                lo: part[0],
                hi: part[1],
            });
            start = end;
        }
        // The closing diagonal has zero Green integral. This is an independent
        // exact parabola area, not an equality between two implementation paths.
        assert!(area.lo > 0.0);
        assert!(area.lo <= 1.0 / 6.0 && 1.0 / 6.0 <= area.hi);
    }

    #[test]
    fn sampling_receipts_reject_other_bindings_gauges_and_cancelled_reuse() {
        let native = quadratic_sampling_native();
        let prepared = PreparedCurveEnclosure::borrowed(&native, 1.0);
        let other_units = PreparedCurveEnclosure::borrowed(&native, 25.4);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let frame = PlaneFrameMm::from_plane(crate::PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let start = prepared
            .sampling_endpoint(10.0, &mut work, &cancel)
            .unwrap();
        let end = prepared
            .sampling_endpoint(10.25, &mut work, &cancel)
            .unwrap();
        let foreign = other_units
            .sampling_endpoint(10.25, &mut work, &cancel)
            .unwrap();
        for endpoints in [[start, foreign], [end, start]] {
            let error = prepared
                .sampling_interval(&endpoints, &frame, &mut work, &cancel)
                .err()
                .expect("foreign binding or reversed native gauge must refuse");
            assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
        }
        assert_eq!(
            prepared
                .sampling_endpoint(0.0, &mut work, &cancel)
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        let interval = prepared
            .sampling_interval(&[start, end], &frame, &mut work, &cancel)
            .unwrap();
        for endpoints in [[start, foreign], [end, start]] {
            let failure = interval
                .signed_area(&endpoints, &frame, [0.0; 3], &mut work, &cancel)
                .unwrap_err();
            assert_eq!(failure.code, GeometryErrorCode::InvalidGeometry);
        }
        assert_eq!(
            interval
                .signed_area(
                    &[start, end],
                    &frame,
                    [0.0; 3],
                    &mut GeometryWorkBudget::new(0),
                    &cancel
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            interval
                .signed_area(&[start, end], &frame, [0.0; 3], &mut work, &cancelled)
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(
            prepared
                .sampling_interval(&[start, end], &frame, &mut work, &cancelled)
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::Cancelled
        );
    }
}
